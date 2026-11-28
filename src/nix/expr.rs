//! A reader for the Nix language, for `nami import`: parses a file and
//! evaluates what it says statically (lists of packages, attribute sets,
//! strings), without nixpkgs and without Nix. It knows the common shapes
//! (`with pkgs; [...]`, `pkgs.mkShell`, flake-utils, `lib.optionals`,
//! `let ... in`, interpolation); anything it can't know (a derivation, an
//! option's value, a function it doesn't model) becomes `Val::Unknown`
//! carrying its source text, so callers can say what was left out.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use anyhow::{anyhow, Context, Result};

/// The platform nami's catalog is for; flakes are read for it.
pub const SYSTEM: &str = crate::index::SYSTEM;

// ---------- syntax ----------

pub struct Expr {
    pub k: K,
    file: u32,
    at: u32,
    end: u32,
}

pub enum K {
    Int(i64),
    Float(f64),
    Str(Vec<Part>),
    Path(Vec<Part>),
    SearchPath(String),
    Uri(String),
    Ident(String),
    List(Vec<Rc<Expr>>),
    Attrs(bool, Vec<Bind>),
    Let(Vec<Bind>, Rc<Expr>),
    With(Rc<Expr>, Rc<Expr>),
    Assert(Rc<Expr>),
    If(Rc<Expr>, Rc<Expr>, Rc<Expr>),
    Lambda(Rc<Param>, Rc<Expr>),
    Apply(Rc<Expr>, Rc<Expr>),
    Select(Rc<Expr>, Vec<Key>, Option<Rc<Expr>>),
    Has(Rc<Expr>, Vec<Key>),
    Bin(Op, Rc<Expr>, Rc<Expr>),
    Not(Rc<Expr>),
    Neg(Rc<Expr>),
}

pub enum Part {
    Lit(String),
    Expr(Rc<Expr>),
}

pub enum Key {
    Name(String),
    Dyn(Rc<Expr>),
}

pub enum Bind {
    Attr(Vec<Key>, Rc<Expr>),
    /// `inherit (from) names;`: each name with its own expression (an
    /// identifier, or a select from `from`).
    Inherit(Vec<(String, Rc<Expr>)>),
}

/// A function's `{ a, b ? default, ... }`: each name, with its default.
pub type Fields = Vec<(String, Option<Rc<Expr>>)>;

pub enum Param {
    Ident(String),
    Pat { fields: Fields, at: Option<String> },
}

#[derive(Clone, Copy, PartialEq)]
pub enum Op {
    Impl,
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    Update,
    Add,
    Sub,
    Mul,
    Div,
    Concat,
}

const KEYWORDS: &[&str] = &["if", "then", "else", "assert", "with", "let", "in", "rec", "inherit", "or"];

fn ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}
fn ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'\'' || b == b'-'
}
fn path_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"._-+".contains(&b)
}
fn uri_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"%/?:@&=+$,-_.!~*'".contains(&b)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    file: u32,
}

impl<'a> Parser<'a> {
    fn err<T>(&self, msg: impl std::fmt::Display) -> Result<T> {
        let line = self.s[..self.i.min(self.s.len())].iter().filter(|&&b| b == b'\n').count() + 1;
        Err(anyhow!("line {line}: {msg}"))
    }

    fn node(&self, k: K, at: usize) -> Rc<Expr> {
        Rc::new(Expr { k, file: self.file, at: at as u32, end: self.i as u32 })
    }

    fn peek(&self) -> u8 {
        self.s.get(self.i).copied().unwrap_or(0)
    }
    fn peek_at(&self, n: usize) -> u8 {
        self.s.get(self.i + n).copied().unwrap_or(0)
    }
    fn rest(&self) -> &[u8] {
        &self.s[self.i.min(self.s.len())..]
    }

    /// Skip whitespace and comments.
    fn ws(&mut self) {
        loop {
            match self.peek() {
                b' ' | b'\t' | b'\n' | b'\r' => self.i += 1,
                b'#' => {
                    while self.i < self.s.len() && self.s[self.i] != b'\n' {
                        self.i += 1;
                    }
                }
                b'/' if self.peek_at(1) == b'*' => {
                    self.i += 2;
                    while self.i < self.s.len() && !self.rest().starts_with(b"*/") {
                        self.i += 1;
                    }
                    self.i = (self.i + 2).min(self.s.len());
                }
                _ => return,
            }
        }
    }

    fn eat(&mut self, tok: &str) -> bool {
        self.ws();
        if self.rest().starts_with(tok.as_bytes()) {
            self.i += tok.len();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, tok: &str) -> Result<()> {
        if self.eat(tok) {
            Ok(())
        } else {
            let got = String::from_utf8_lossy(&self.rest()[..self.rest().len().min(20)]).into_owned();
            self.err(format!("expected `{tok}`, found `{}`", got.lines().next().unwrap_or("end of file")))
        }
    }

    /// The identifier at the cursor (no whitespace skipped), not consumed.
    fn ident_here(&self) -> Option<&'a str> {
        if !ident_start(self.peek()) {
            return None;
        }
        let mut j = self.i;
        while j < self.s.len() && ident_char(self.s[j]) {
            j += 1;
        }
        std::str::from_utf8(&self.s[self.i..j]).ok()
    }

    fn kw(&mut self, word: &str) -> bool {
        self.ws();
        if self.ident_here() == Some(word) {
            self.i += word.len();
            true
        } else {
            false
        }
    }

    fn at_kw(&mut self, word: &str) -> bool {
        self.ws();
        self.ident_here() == Some(word)
    }

    fn expr(&mut self) -> Result<Rc<Expr>> {
        self.ws();
        let at = self.i;
        if let Some(e) = self.lambda()? {
            return Ok(e);
        }
        if self.kw("let") {
            let binds = self.binds(true)?;
            self.ws();
            if !self.kw("in") {
                return self.err("expected `in`");
            }
            let body = self.expr()?;
            return Ok(self.node(K::Let(binds, body), at));
        }
        if self.kw("with") {
            let scope = self.expr()?;
            self.expect(";")?;
            let body = self.expr()?;
            return Ok(self.node(K::With(scope, body), at));
        }
        if self.kw("assert") {
            let cond = self.expr()?;
            self.expect(";")?;
            let body = self.expr()?;
            let _ = cond;
            return Ok(self.node(K::Assert(body), at));
        }
        if self.kw("if") {
            let c = self.expr()?;
            if !self.kw("then") {
                return self.err("expected `then`");
            }
            let t = self.expr()?;
            if !self.kw("else") {
                return self.err("expected `else`");
            }
            let e = self.expr()?;
            return Ok(self.node(K::If(c, t, e), at));
        }
        self.binary(0)
    }

    /// `x: body`, `{ a, b ? 1, ... }: body`, `args@{ ... }: body`, or None.
    fn lambda(&mut self) -> Result<Option<Rc<Expr>>> {
        let at = self.i;
        if let Some(id) = self.ident_here().filter(|id| !KEYWORDS.contains(id)) {
            self.i += id.len();
            // `x:y` with no space is a URI.
            if self.peek() == b':' && !uri_char(self.peek_at(1)) {
                self.i += 1;
                let body = self.expr()?;
                return Ok(Some(self.node(K::Lambda(Rc::new(Param::Ident(id.into())), body), at)));
            }
            self.ws();
            if self.peek() == b':' && !uri_char(self.peek_at(1)) {
                self.i += 1;
                let body = self.expr()?;
                return Ok(Some(self.node(K::Lambda(Rc::new(Param::Ident(id.into())), body), at)));
            }
            if self.peek() == b'@' {
                self.i += 1;
                self.ws();
                if let Some(fields) = self.pattern()? {
                    self.expect(":")?;
                    let body = self.expr()?;
                    return Ok(Some(self.node(K::Lambda(Rc::new(Param::Pat { fields, at: Some(id.into()) }), body), at)));
                }
            }
            self.i = at;
            return Ok(None);
        }
        if self.peek() == b'{' {
            if let Some(fields) = self.pattern()? {
                let mut name = None;
                if self.eat("@") {
                    self.ws();
                    let Some(id) = self.ident_here() else { return self.err("expected a name after `@`") };
                    self.i += id.len();
                    name = Some(id.to_string());
                }
                if self.eat(":") {
                    let body = self.expr()?;
                    return Ok(Some(self.node(K::Lambda(Rc::new(Param::Pat { fields, at: name }), body), at)));
                }
            }
            self.i = at;
        }
        Ok(None)
    }

    /// `{ a, b ? default, ... }` when that's what follows (then `:` or `@`).
    fn pattern(&mut self) -> Result<Option<Fields>> {
        let at = self.i;
        if !self.eat("{") {
            return Ok(None);
        }
        let mut fields = Vec::new();
        loop {
            self.ws();
            if self.eat("}") {
                break;
            }
            if self.eat("...") {
                self.ws();
                if self.eat("}") {
                    break;
                }
                self.i = at;
                return Ok(None);
            }
            let Some(id) = self.ident_here() else {
                self.i = at;
                return Ok(None);
            };
            self.i += id.len();
            let mut default = None;
            if self.eat("?") {
                default = Some(self.expr()?);
            }
            fields.push((id.to_string(), default.clone()));
            self.ws();
            if self.eat(",") {
                continue;
            }
            if self.peek() == b'}' {
                continue;
            }
            self.i = at;
            return Ok(None);
        }
        self.ws();
        if self.peek() == b':' || self.peek() == b'@' {
            Ok(Some(fields))
        } else {
            self.i = at;
            Ok(None)
        }
    }

    fn binary(&mut self, lvl: u8) -> Result<Rc<Expr>> {
        self.ws();
        let at = self.i;
        match lvl {
            6 => {
                if self.peek() == b'!' && self.peek_at(1) != b'=' {
                    self.i += 1;
                    let e = self.binary(6)?;
                    return Ok(self.node(K::Not(e), at));
                }
                self.binary(7)
            }
            10 => {
                let mut e = self.binary(11)?;
                while self.eat("?") {
                    let path = self.attrpath()?;
                    e = self.node(K::Has(e, path), at);
                }
                Ok(e)
            }
            11 => {
                if self.peek() == b'-' && self.peek_at(1) != b'>' {
                    self.i += 1;
                    let e = self.binary(11)?;
                    return Ok(self.node(K::Neg(e), at));
                }
                self.apply()
            }
            _ => {
                let mut l = self.binary(lvl + 1)?;
                while let Some((op, len)) = self.op_at(lvl) {
                    self.i += len;
                    let right = matches!(op, Op::Impl | Op::Update | Op::Concat);
                    let r = self.binary(if right { lvl } else { lvl + 1 })?;
                    l = self.node(K::Bin(op, l, r), at);
                    if matches!(op, Op::Eq | Op::Ne | Op::Lt | Op::Gt | Op::Le | Op::Ge) {
                        break;
                    }
                }
                Ok(l)
            }
        }
    }

    fn op_at(&mut self, lvl: u8) -> Option<(Op, usize)> {
        self.ws();
        let r = self.rest();
        let s = |t: &str| r.starts_with(t.as_bytes());
        match lvl {
            0 if s("->") => Some((Op::Impl, 2)),
            1 if s("||") => Some((Op::Or, 2)),
            2 if s("&&") => Some((Op::And, 2)),
            3 if s("==") => Some((Op::Eq, 2)),
            3 if s("!=") => Some((Op::Ne, 2)),
            4 if s("<=") => Some((Op::Le, 2)),
            4 if s(">=") => Some((Op::Ge, 2)),
            4 if s("<") => Some((Op::Lt, 1)),
            4 if s(">") => Some((Op::Gt, 1)),
            5 if s("//") => Some((Op::Update, 2)),
            7 if s("+") && !s("++") => Some((Op::Add, 1)),
            7 if s("-") && !s("->") => Some((Op::Sub, 1)),
            8 if s("*") => Some((Op::Mul, 1)),
            8 if s("/") && !s("//") && !s("/*") => Some((Op::Div, 1)),
            9 if s("++") => Some((Op::Concat, 2)),
            _ => None,
        }
    }

    fn apply(&mut self) -> Result<Rc<Expr>> {
        self.ws();
        let at = self.i;
        let mut f = self.select()?;
        loop {
            self.ws();
            if !self.starts_operand() {
                break;
            }
            let a = self.select()?;
            f = self.node(K::Apply(f, a), at);
        }
        Ok(f)
    }

    fn starts_operand(&self) -> bool {
        let b = self.peek();
        if let Some(id) = self.ident_here() {
            return !KEYWORDS.contains(&id) || id == "rec";
        }
        b.is_ascii_digit()
            || b == b'"'
            || b == b'('
            || b == b'['
            || b == b'{'
            || self.rest().starts_with(b"''")
            || self.path_len().is_some()
            || self.search_path_len().is_some()
    }

    /// Length of a path literal at the cursor (`./a`, `../a`, `/a`, `~/a`, `a/b`).
    fn path_len(&self) -> Option<usize> {
        let r = self.rest();
        let mut j = 0;
        if r.starts_with(b"~/") {
            j = 1;
        } else {
            while j < r.len() && path_char(r[j]) {
                j += 1;
            }
        }
        if r.get(j) != Some(&b'/') || !r.get(j + 1).is_some_and(|&b| path_char(b) || b == b'$') {
            return None;
        }
        // `a/b` as division is written with spaces; `a/b` without is a path.
        while j < r.len() && (path_char(r[j]) || r[j] == b'/' || r[j] == b'$' || r[j] == b'{' || r[j] == b'}') {
            if r[j] == b'$' && r.get(j + 1) == Some(&b'{') {
                let mut depth = 0;
                while j < r.len() {
                    match r[j] {
                        b'{' => depth += 1,
                        b'}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
            } else if r[j] == b'{' || r[j] == b'}' {
                break;
            }
            j += 1;
        }
        Some(j)
    }

    fn search_path_len(&self) -> Option<usize> {
        let r = self.rest();
        if r.first() != Some(&b'<') {
            return None;
        }
        let end = r.iter().position(|&b| b == b'>')?;
        (end > 1 && r[1..end].iter().all(|&b| path_char(b) || b == b'/')).then_some(end + 1)
    }

    fn select(&mut self) -> Result<Rc<Expr>> {
        self.ws();
        let at = self.i;
        let mut e = self.primary()?;
        loop {
            let save = self.i;
            self.ws();
            if self.peek() == b'.' && self.peek_at(1) != b'.' && self.peek_at(1) != b'/' {
                self.i += 1;
                let path = self.attrpath()?;
                let mut default = None;
                if self.kw("or") {
                    default = Some(self.select()?);
                }
                e = self.node(K::Select(e, path, default), at);
            } else {
                self.i = save;
                break;
            }
        }
        Ok(e)
    }

    fn attrpath(&mut self) -> Result<Vec<Key>> {
        let mut keys = vec![self.attr_key()?];
        loop {
            let save = self.i;
            self.ws();
            if self.peek() == b'.' && self.peek_at(1) != b'.' && self.peek_at(1) != b'/' {
                self.i += 1;
                keys.push(self.attr_key()?);
            } else {
                self.i = save;
                return Ok(keys);
            }
        }
    }

    fn attr_key(&mut self) -> Result<Key> {
        self.ws();
        let at = self.i;
        if let Some(id) = self.ident_here() {
            self.i += id.len();
            return Ok(Key::Name(id.into()));
        }
        if self.peek() == b'"' {
            self.i += 1;
            let parts = self.string()?;
            if let [Part::Lit(s)] = parts.as_slice() {
                return Ok(Key::Name(s.clone()));
            }
            if parts.is_empty() {
                return Ok(Key::Name(String::new()));
            }
            return Ok(Key::Dyn(self.node(K::Str(parts), at)));
        }
        if self.rest().starts_with(b"${") {
            self.i += 2;
            let e = self.expr()?;
            self.expect("}")?;
            return Ok(Key::Dyn(e));
        }
        self.err("expected an attribute name")
    }

    fn primary(&mut self) -> Result<Rc<Expr>> {
        self.ws();
        let at = self.i;
        let b = self.peek();
        if b == 0 {
            return self.err("unexpected end of file");
        }
        if let Some(n) = self.path_len() {
            let end = self.i + n;
            let (mut parts, mut lit) = (Vec::new(), Vec::new());
            while self.i < end {
                if self.rest().starts_with(b"${") {
                    if !lit.is_empty() {
                        parts.push(Part::Lit(String::from_utf8_lossy(&std::mem::take(&mut lit)).into_owned()));
                    }
                    self.i += 2;
                    let e = self.expr()?;
                    self.expect("}")?;
                    parts.push(Part::Expr(e));
                } else {
                    lit.push(self.peek());
                    self.i += 1;
                }
            }
            if !lit.is_empty() {
                parts.push(Part::Lit(String::from_utf8_lossy(&lit).into_owned()));
            }
            return Ok(self.node(K::Path(parts), at));
        }
        if b.is_ascii_digit() {
            let mut j = self.i;
            while j < self.s.len() && (self.s[j].is_ascii_digit() || self.s[j] == b'.' || self.s[j] == b'e') {
                j += 1;
            }
            let text = std::str::from_utf8(&self.s[self.i..j]).unwrap_or("0");
            self.i = j;
            return Ok(match text.parse::<i64>() {
                Ok(n) => self.node(K::Int(n), at),
                Err(_) => self.node(K::Float(text.parse().unwrap_or(0.0)), at),
            });
        }
        if b == b'"' {
            self.i += 1;
            let parts = self.string()?;
            return Ok(self.node(K::Str(parts), at));
        }
        if self.rest().starts_with(b"''") {
            self.i += 2;
            let parts = self.indented()?;
            return Ok(self.node(K::Str(parts), at));
        }
        if let Some(n) = self.search_path_len() {
            let p = String::from_utf8_lossy(&self.s[self.i + 1..self.i + n - 1]).into_owned();
            self.i += n;
            return Ok(self.node(K::SearchPath(p), at));
        }
        if let Some(id) = self.ident_here() {
            if self.s.get(self.i + id.len()) == Some(&b':') && self.s.get(self.i + id.len() + 1).is_some_and(|&b| uri_char(b)) {
                let mut j = self.i + id.len() + 1;
                while j < self.s.len() && uri_char(self.s[j]) {
                    j += 1;
                }
                let uri = String::from_utf8_lossy(&self.s[self.i..j]).into_owned();
                self.i = j;
                return Ok(self.node(K::Uri(uri), at));
            }
            if id == "rec" {
                self.i += 3;
                self.expect("{")?;
                let binds = self.binds(false)?;
                self.expect("}")?;
                return Ok(self.node(K::Attrs(true, binds), at));
            }
            if KEYWORDS.contains(&id) {
                return self.err(format!("unexpected `{id}`"));
            }
            self.i += id.len();
            return Ok(self.node(K::Ident(id.into()), at));
        }
        match b {
            b'(' => {
                self.i += 1;
                let e = self.expr()?;
                self.expect(")")?;
                Ok(e)
            }
            b'[' => {
                self.i += 1;
                let mut items = Vec::new();
                loop {
                    self.ws();
                    if self.eat("]") {
                        break;
                    }
                    items.push(self.select()?);
                }
                Ok(self.node(K::List(items), at))
            }
            b'{' => {
                self.i += 1;
                let binds = self.binds(false)?;
                self.expect("}")?;
                Ok(self.node(K::Attrs(false, binds), at))
            }
            _ => self.err(format!("unexpected `{}`", b as char)),
        }
    }

    /// A "string", after its opening quote.
    fn string(&mut self) -> Result<Vec<Part>> {
        let mut parts = Vec::new();
        let mut lit: Vec<u8> = Vec::new();
        loop {
            match self.peek() {
                0 => return self.err("unterminated string"),
                b'"' => {
                    self.i += 1;
                    break;
                }
                b'\\' => {
                    let c = self.peek_at(1);
                    lit.push(match c {
                        b'n' => b'\n',
                        b't' => b'\t',
                        b'r' => b'\r',
                        c => c,
                    });
                    self.i += 2;
                }
                b'$' if self.peek_at(1) == b'{' => {
                    self.i += 2;
                    if !lit.is_empty() {
                        parts.push(Part::Lit(String::from_utf8_lossy(&std::mem::take(&mut lit)).into_owned()));
                    }
                    let e = self.expr()?;
                    self.expect("}")?;
                    parts.push(Part::Expr(e));
                }
                c => {
                    lit.push(c);
                    self.i += 1;
                }
            }
        }
        if !lit.is_empty() {
            parts.push(Part::Lit(String::from_utf8_lossy(&lit).into_owned()));
        }
        Ok(parts)
    }

    /// An ''indented string'', after its opening quotes: common indentation
    /// removed, as Nix does.
    fn indented(&mut self) -> Result<Vec<Part>> {
        enum T {
            Ch(u8, bool),
            E(Rc<Expr>),
        }
        let mut toks = Vec::new();
        loop {
            let r = self.rest();
            if r.is_empty() {
                return self.err("unterminated ''string''");
            }
            if r.starts_with(b"'''") {
                toks.push(T::Ch(b'\'', true));
                toks.push(T::Ch(b'\'', true));
                self.i += 3;
            } else if r.starts_with(b"''$") {
                toks.push(T::Ch(b'$', true));
                self.i += 3;
            } else if r.starts_with(b"''\\") {
                let c = r.get(3).copied().unwrap_or(b'\\');
                toks.push(T::Ch(
                    match c {
                        b'n' => b'\n',
                        b't' => b'\t',
                        b'r' => b'\r',
                        c => c,
                    },
                    true,
                ));
                self.i += 4;
            } else if r.starts_with(b"''") {
                self.i += 2;
                break;
            } else if r.starts_with(b"${") {
                self.i += 2;
                let e = self.expr()?;
                self.expect("}")?;
                toks.push(T::E(e));
            } else {
                toks.push(T::Ch(r[0], false));
                self.i += 1;
            }
        }
        let mut lines: Vec<Vec<T>> = vec![Vec::new()];
        for t in toks {
            let nl = matches!(t, T::Ch(b'\n', false));
            lines.last_mut().unwrap().push(t);
            if nl {
                lines.push(Vec::new());
            }
        }
        let is_space = |t: &T| matches!(t, T::Ch(b' ', false));
        let is_blank = |l: &[T]| l.iter().all(|t| is_space(t) || matches!(t, T::Ch(b'\n', false)));
        let min = lines.iter().filter(|l| !is_blank(l)).map(|l| l.iter().take_while(|t| is_space(t)).count()).min().unwrap_or(0);
        // A first line of only spaces goes; so do the spaces before the closing ''.
        if lines.len() > 1 && is_blank(&lines[0]) {
            lines.remove(0);
        }
        if let Some(last) = lines.last_mut() {
            if last.iter().all(is_space) {
                last.clear();
            }
        }
        let mut parts = Vec::new();
        let mut lit: Vec<u8> = Vec::new();
        for line in lines {
            let skip = line.iter().take_while(|t| is_space(t)).count().min(min);
            for t in line.into_iter().skip(skip) {
                match t {
                    T::Ch(c, _) => lit.push(c),
                    T::E(e) => {
                        if !lit.is_empty() {
                            parts.push(Part::Lit(String::from_utf8_lossy(&std::mem::take(&mut lit)).into_owned()));
                        }
                        parts.push(Part::Expr(e));
                    }
                }
            }
        }
        if !lit.is_empty() {
            parts.push(Part::Lit(String::from_utf8_lossy(&lit).into_owned()));
        }
        Ok(parts)
    }

    /// Bindings up to `}` (or `in`, for `let`).
    fn binds(&mut self, is_let: bool) -> Result<Vec<Bind>> {
        let mut out = Vec::new();
        loop {
            self.ws();
            if self.peek() == b'}' || self.peek() == 0 || (is_let && self.at_kw("in")) {
                return Ok(out);
            }
            if self.kw("inherit") {
                let from = if self.eat("(") {
                    let e = self.expr()?;
                    self.expect(")")?;
                    Some(e)
                } else {
                    None
                };
                let mut names = Vec::new();
                loop {
                    self.ws();
                    if self.eat(";") {
                        break;
                    }
                    let at = self.i;
                    let name = match self.attr_key()? {
                        Key::Name(n) => n,
                        Key::Dyn(_) => return self.err("inherit needs plain names"),
                    };
                    let e = match &from {
                        Some(f) => self.node(K::Select(f.clone(), vec![Key::Name(name.clone())], None), at),
                        None => self.node(K::Ident(name.clone()), at),
                    };
                    names.push((name, e));
                }
                out.push(Bind::Inherit(names));
                continue;
            }
            let path = self.attrpath()?;
            self.expect("=")?;
            let value = self.expr()?;
            self.expect(";")?;
            out.push(Bind::Attr(path, value));
        }
    }
}

// ---------- values ----------

#[derive(Clone)]
pub struct Thunk(Rc<RefCell<TS>>);

enum TS {
    Lazy(Rc<Expr>, Env),
    Busy,
    Done(Val),
}

impl Thunk {
    pub fn val(v: Val) -> Thunk {
        Thunk(Rc::new(RefCell::new(TS::Done(v))))
    }
    fn lazy(e: &Rc<Expr>, env: &Env) -> Thunk {
        Thunk(Rc::new(RefCell::new(TS::Lazy(e.clone(), env.clone()))))
    }
    /// The expression it came from, while still unevaluated.
    fn expr(&self) -> Option<Rc<Expr>> {
        match &*self.0.borrow() {
            TS::Lazy(e, _) => Some(e.clone()),
            _ => None,
        }
    }
}

pub type Env = Rc<Scope>;

pub struct Scope {
    vars: RefCell<HashMap<String, Thunk>>,
    with: Option<Thunk>,
    parent: Option<Env>,
}

fn scope(parent: Option<&Env>, with: Option<Thunk>) -> Env {
    Rc::new(Scope { vars: RefCell::new(HashMap::new()), with, parent: parent.cloned() })
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Prim {
    Optionals,
    Optional,
    MkIf,
    OptionalString,
    OptionalAttrs,
    Id,
    Second,
    MkMerge,
    GenAttrs,
    EachDefaultSystem,
    EachSystem,
    MkFlake,
    Import,
    ImportNixpkgs,
    AttrValues,
    ConcatLists,
    Flatten,
    Map,
    ToString,
    ConcatStringsSep,
    WriteScriptBin,
    WriteShellApplication,
    MkShell,
    HomeManagerConfiguration,
    NixosSystem,
    Throw,
    /// home-manager's ordered entries: lib.hm.dag.entryAfter/Before/Anywhere/Between.
    DagAfter,
    DagBefore,
    DagAnywhere,
    DagBetween,
}

impl Prim {
    fn arity(self) -> usize {
        match self {
            Prim::Optionals | Prim::Optional | Prim::MkIf | Prim::OptionalString | Prim::OptionalAttrs | Prim::Second | Prim::GenAttrs | Prim::EachSystem | Prim::MkFlake | Prim::Map | Prim::ConcatStringsSep | Prim::WriteScriptBin | Prim::DagAfter | Prim::DagBefore => 2,
            Prim::DagBetween => 3,
            _ => 1,
        }
    }
}

#[derive(Clone)]
pub enum Val {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// Text, and whether it's exact (no interpolation of something unknown).
    Str(String, bool),
    Path(PathBuf),
    List(Rc<Vec<Thunk>>),
    Attrs(Rc<BTreeMap<String, Thunk>>),
    Lambda(Rc<Param>, Rc<Expr>, Env),
    Prim(PrimFn),
    /// A nixpkgs attribute ("" is pkgs itself).
    Pkg(String),
    /// Something known by name: `lib`, `builtins`, a flake input, `config`...
    Ns(String),
    /// `pkgs.mkShell { ... }`: its arguments.
    Shell(Rc<BTreeMap<String, Thunk>>),
    /// `writeShellScriptBin name text` / `writeShellApplication { ... }`.
    Script { name: String, text: String, exact: bool, inputs: Vec<Thunk> },
    /// `python3.withPackages (ps: [...])`: the interpreter and the packages.
    WithPkgs(String, Rc<Vec<Thunk>>),
    /// `homeManagerConfiguration` / `nixosSystem`: its modules.
    Config(Rc<Vec<Thunk>>),
    /// Not knowable here; the source text (or what it is).
    Unknown(String),
}

#[derive(Clone)]
pub struct PrimFn(Prim, Rc<Vec<Thunk>>);

impl Val {
    fn prim(p: Prim) -> Val {
        Val::Prim(PrimFn(p, Rc::new(Vec::new())))
    }
    pub fn str(&self) -> Option<&str> {
        match self {
            Val::Str(s, true) => Some(s),
            _ => None,
        }
    }
    pub fn attrs(&self) -> Option<&BTreeMap<String, Thunk>> {
        match self {
            Val::Attrs(m) => Some(m),
            _ => None,
        }
    }
}

// ---------- evaluation ----------

const FUEL: u64 = 2_000_000;
const MAX_DEPTH: u32 = 180;

/// What `inputs.<name>` of a flake is, by its URL or name.
fn input_kind(name: &str, url: &str) -> Option<&'static str> {
    let both = format!("{name} {url}").to_lowercase();
    if both.contains("flake-utils") {
        Some("flake-utils")
    } else if both.contains("flake-parts") {
        Some("flake-parts")
    } else if both.contains("home-manager") {
        Some("home-manager")
    } else if name.starts_with("nixpkgs") || url.to_lowercase().contains("nixos/nixpkgs") || url.starts_with("nixpkgs") {
        Some("nixpkgs")
    } else {
        None
    }
}

pub struct Eval {
    sources: RefCell<Vec<(PathBuf, Rc<str>)>>,
    imported: RefCell<HashMap<PathBuf, Val>>,
    fuel: Cell<u64>,
    depth: Cell<u32>,
    /// Flake inputs: name -> kind (nixpkgs, flake-utils, ...).
    inputs: RefCell<HashMap<String, &'static str>>,
    /// The home directory `config.home.homeDirectory` stands for.
    pub home_dir: Option<String>,
    pub user: Option<String>,
    /// Things worth saying about what was read (an `if` it couldn't decide...).
    pub warnings: RefCell<Vec<String>>,
    /// What was read but has no nami equivalent: (what, why).
    pub skipped: RefCell<Vec<(String, String)>>,
    /// Packages referenced as `${pkgs.x}/bin/...` in strings.
    pub bin_refs: RefCell<Vec<String>>,
    /// Reading shell code (home.shell): `${pkgs.x}/share/<path>` becomes
    /// `"$(nami_share <path>)"`, with x in `share_refs`.
    pub shell_text: Cell<bool>,
    pub share_refs: RefCell<Vec<String>>,
}

impl Default for Eval {
    fn default() -> Self {
        Eval {
            sources: RefCell::new(Vec::new()),
            imported: RefCell::new(HashMap::new()),
            fuel: Cell::new(FUEL),
            depth: Cell::new(0),
            inputs: RefCell::new(HashMap::new()),
            home_dir: std::env::var("HOME").ok(),
            user: std::env::var("USER").ok(),
            warnings: RefCell::new(Vec::new()),
            skipped: RefCell::new(Vec::new()),
            bin_refs: RefCell::new(Vec::new()),
            shell_text: Cell::new(false),
            share_refs: RefCell::new(Vec::new()),
        }
    }
}

/// `a/./b/../c` -> `a/c`, without touching the file system.
pub fn clean(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            c => out.push(c),
        }
    }
    out
}

impl Eval {
    pub fn warn(&self, msg: impl Into<String>) {
        let msg = msg.into();
        if !self.warnings.borrow().contains(&msg) {
            self.warnings.borrow_mut().push(msg);
        }
    }

    pub fn skip(&self, what: impl Into<String>, why: impl Into<String>) {
        let item = (what.into(), why.into());
        if !self.skipped.borrow().contains(&item) {
            self.skipped.borrow_mut().push(item);
        }
    }

    /// Parse a file (kept for messages).
    pub fn parse_file(&self, path: &Path) -> Result<Rc<Expr>> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        self.parse(path, &text)
    }

    pub fn parse(&self, path: &Path, text: &str) -> Result<Rc<Expr>> {
        let file = self.sources.borrow().len() as u32;
        self.sources.borrow_mut().push((path.to_path_buf(), Rc::from(text)));
        let mut p = Parser { s: text.as_bytes(), i: 0, file };
        let e = p.expr().with_context(|| format!("can't read {}", path.display()))?;
        p.ws();
        if p.i < p.s.len() {
            return p.err("unexpected text after the expression").with_context(|| format!("can't read {}", path.display()));
        }
        Ok(e)
    }

    /// Evaluate a file's expression (to a value, maybe a function).
    pub fn eval_file(&self, path: &Path) -> Result<Val> {
        let e = self.parse_file(path)?;
        Ok(self.eval(&e, &scope(None, None)))
    }

    /// Record a flake's inputs (from its `inputs` attribute and the outputs
    /// function's arguments), so `nixpkgs`, `flake-utils`... mean something.
    pub fn set_input(&self, name: &str, url: &str) {
        if let Some(kind) = input_kind(name, url) {
            self.inputs.borrow_mut().insert(name.to_string(), kind);
        }
    }

    pub fn input(&self, name: &str) -> Val {
        match self.inputs.borrow().get(name) {
            Some(kind) => Val::Ns((*kind).to_string()),
            None if name == "self" => Val::Unknown("self (this flake's own outputs)".into()),
            None => Val::Unknown(format!("inputs.{name}")),
        }
    }

    /// The source text of an expression, on one line, shortened.
    pub fn text(&self, e: &Expr) -> String {
        let sources = self.sources.borrow();
        let Some((_, src)) = sources.get(e.file as usize) else { return "…".into() };
        let raw = src.get(e.at as usize..e.end as usize).unwrap_or("…");
        let one: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        if one.chars().count() > 80 {
            format!("{}…", one.chars().take(79).collect::<String>())
        } else {
            one
        }
    }

    fn file_dir(&self, e: &Expr) -> PathBuf {
        self.sources.borrow().get(e.file as usize).and_then(|(p, _)| p.parent().map(Path::to_path_buf)).unwrap_or_default()
    }

    pub fn force(&self, t: &Thunk) -> Val {
        let state = std::mem::replace(&mut *t.0.borrow_mut(), TS::Busy);
        match state {
            TS::Done(v) => {
                *t.0.borrow_mut() = TS::Done(v.clone());
                v
            }
            TS::Busy => {
                // Leave it busy: whoever started it will finish it.
                Val::Unknown("a value defined in terms of itself".into())
            }
            TS::Lazy(e, env) => {
                let v = self.eval(&e, &env);
                *t.0.borrow_mut() = TS::Done(v.clone());
                v
            }
        }
    }

    pub fn eval(&self, e: &Rc<Expr>, env: &Env) -> Val {
        let fuel = self.fuel.get();
        if fuel == 0 || self.depth.get() >= MAX_DEPTH {
            return Val::Unknown(format!("{} (too deeply nested to read)", self.text(e)));
        }
        self.fuel.set(fuel - 1);
        self.depth.set(self.depth.get() + 1);
        let v = self.eval_inner(e, env);
        self.depth.set(self.depth.get() - 1);
        v
    }

    fn eval_inner(&self, e: &Rc<Expr>, env: &Env) -> Val {
        match &e.k {
            K::Int(n) => Val::Int(*n),
            K::Float(f) => Val::Float(*f),
            K::Str(parts) => self.interp(parts, env),
            K::Path(parts) => {
                let Val::Str(raw, true) = self.interp(parts, env) else { return Val::Unknown(self.text(e)) };
                let p = if let Some(rest) = raw.strip_prefix("~/") {
                    match &self.home_dir {
                        Some(h) => Path::new(h).join(rest),
                        None => return Val::Unknown(raw),
                    }
                } else if raw.starts_with('/') {
                    PathBuf::from(&raw)
                } else {
                    self.file_dir(e).join(&raw)
                };
                Val::Path(clean(&p))
            }
            K::SearchPath(p) if p == "nixpkgs" || p.starts_with("nixpkgs/") => Val::Ns("nixpkgs".into()),
            K::SearchPath(p) => Val::Unknown(format!("<{p}>")),
            K::Uri(u) => Val::Str(u.clone(), true),
            K::Ident(n) => self.lookup(env, n, e),
            K::List(items) => Val::List(Rc::new(items.iter().map(|x| Thunk::lazy(x, env)).collect())),
            K::Attrs(rec, binds) => {
                if *rec {
                    let inner = scope(Some(env), None);
                    let m = self.build_attrs(binds, &inner, env);
                    inner.vars.borrow_mut().extend(m.iter().map(|(k, v)| (k.clone(), v.clone())));
                    Val::Attrs(Rc::new(m))
                } else {
                    Val::Attrs(Rc::new(self.build_attrs(binds, env, env)))
                }
            }
            K::Let(binds, body) => {
                let inner = scope(Some(env), None);
                let m = self.build_attrs(binds, &inner, env);
                inner.vars.borrow_mut().extend(m);
                self.eval(body, &inner)
            }
            K::With(s, body) => self.eval(body, &scope(Some(env), Some(Thunk::lazy(s, env)))),
            K::Assert(body) => self.eval(body, env),
            K::If(c, t, f) => match self.eval(c, env) {
                Val::Bool(true) => self.eval(t, env),
                Val::Bool(false) => self.eval(f, env),
                _ => {
                    self.warn(format!("couldn't tell whether `if {}` holds; read its `then` part", self.text(c)));
                    self.eval(t, env)
                }
            },
            K::Lambda(p, body) => Val::Lambda(p.clone(), body.clone(), env.clone()),
            K::Apply(f, a) => {
                let fv = self.eval(f, env);
                self.apply(fv, Thunk::lazy(a, env), Some(e))
            }
            K::Select(base, keys, default) => {
                let mut v = self.eval(base, env);
                for k in keys {
                    let Some(name) = self.key_name(k, env) else { return Val::Unknown(self.text(e)) };
                    match self.select(&v, &name) {
                        Some(Val::Unknown(_)) if default.is_some() => return self.eval(default.as_ref().unwrap(), env),
                        Some(Val::Unknown(_)) => return Val::Unknown(self.text(e)),
                        Some(next) => v = next,
                        None => {
                            return match default {
                                Some(d) => self.eval(d, env),
                                None => Val::Unknown(self.text(e)),
                            }
                        }
                    }
                }
                v
            }
            K::Has(base, keys) => {
                let mut v = self.eval(base, env);
                for k in keys {
                    let Some(name) = self.key_name(k, env) else { return Val::Unknown(self.text(e)) };
                    match &v {
                        Val::Attrs(m) => match m.get(&name) {
                            Some(t) => v = self.force(t),
                            None => return Val::Bool(false),
                        },
                        _ => return Val::Unknown(self.text(e)),
                    }
                }
                Val::Bool(true)
            }
            K::Bin(op, l, r) => self.binop(*op, l, r, env, e),
            K::Not(x) => match self.eval(x, env) {
                Val::Bool(b) => Val::Bool(!b),
                _ => Val::Unknown(self.text(e)),
            },
            K::Neg(x) => match self.eval(x, env) {
                Val::Int(n) => Val::Int(-n),
                Val::Float(f) => Val::Float(-f),
                _ => Val::Unknown(self.text(e)),
            },
        }
    }

    fn key_name(&self, k: &Key, env: &Env) -> Option<String> {
        match k {
            Key::Name(n) => Some(n.clone()),
            Key::Dyn(e) => match self.eval(e, env) {
                Val::Str(s, true) => Some(s),
                _ => None,
            },
        }
    }

    fn lookup(&self, env: &Env, name: &str, e: &Expr) -> Val {
        let mut s = Some(env);
        while let Some(sc) = s {
            let hit = sc.vars.borrow().get(name).cloned();
            if let Some(t) = hit {
                return self.force(&t);
            }
            s = sc.parent.as_ref();
        }
        // Builtins come before `with`.
        match name {
            "true" => return Val::Bool(true),
            "false" => return Val::Bool(false),
            "null" => return Val::Null,
            "import" => return Val::prim(Prim::Import),
            "builtins" => return Val::Ns("builtins".into()),
            "map" => return Val::prim(Prim::Map),
            "toString" => return Val::prim(Prim::ToString),
            "throw" | "abort" => return Val::prim(Prim::Throw),
            _ => {}
        }
        let mut s = Some(env);
        while let Some(sc) = s {
            if let Some(w) = &sc.with {
                let wv = self.force(w);
                if let Some(v) = match &wv {
                    Val::Attrs(m) => m.get(name).map(|t| self.force(t)),
                    Val::Pkg(_) | Val::Ns(_) => self.select(&wv, name),
                    _ => None,
                } {
                    return v;
                }
            }
            s = sc.parent.as_ref();
        }
        match name {
            "pkgs" => Val::Pkg(String::new()),
            "lib" => Val::Ns("lib".into()),
            "config" | "osConfig" => Val::Ns("config".into()),
            "system" => Val::Str(SYSTEM.into(), true),
            "inputs" => Val::Ns("inputs".into()),
            _ if self.inputs.borrow().contains_key(name) => self.input(name),
            _ => Val::Unknown(self.text(e)),
        }
    }

    /// `v.name`: Some(value), or None when `v` surely has no such attribute.
    pub fn select(&self, v: &Val, name: &str) -> Option<Val> {
        match v {
            Val::Attrs(m) | Val::Shell(m) => m.get(name).map(|t| self.force(t)),
            Val::Pkg(p) => Some(self.pkg(&if p.is_empty() { name.to_string() } else { format!("{p}.{name}") })),
            Val::Ns(ns) => Some(self.ns(ns, name)),
            Val::Unknown(t) => Some(Val::Unknown(format!("{t}.{name}"))),
            Val::Lambda(..) | Val::Prim(..) | Val::Config(..) => Some(Val::Unknown(name.into())),
            _ => None,
        }
    }

    /// What `pkgs.<attr>` is: a package, or one of the functions modelled here.
    fn pkg(&self, attr: &str) -> Val {
        let last = attr.rsplit('.').next().unwrap_or(attr);
        match attr {
            "lib" | "stdenv.lib" => return Val::Ns("lib".into()),
            "mkShell" | "mkShellNoCC" => return Val::prim(Prim::MkShell),
            "writeShellScriptBin" | "writeScriptBin" => return Val::prim(Prim::WriteScriptBin),
            "writeShellApplication" => return Val::prim(Prim::WriteShellApplication),
            "system" => return Val::Str(SYSTEM.into(), true),
            _ => {}
        }
        if attr.ends_with("hostPlatform.system") || attr.ends_with("buildPlatform.system") || attr == "stdenv.system" {
            return Val::Str(SYSTEM.into(), true);
        }
        if attr.starts_with("stdenv.") || attr.contains("Platform.") {
            let (darwin, arm) = (SYSTEM.ends_with("-darwin"), SYSTEM.starts_with("aarch64"));
            match last {
                "is64bit" => return Val::Bool(true),
                "isLinux" | "isGnu" => return Val::Bool(!darwin),
                "isDarwin" => return Val::Bool(darwin),
                "isx86_64" | "isx86" => return Val::Bool(!arm),
                "isAarch64" | "isAarch" => return Val::Bool(arm),
                "isAarch32" | "isMusl" | "isFreeBSD" | "isWindows" | "isi686" | "isRiscV" => return Val::Bool(false),
                _ => {}
            }
        }
        Val::Pkg(attr.to_string())
    }

    fn ns(&self, ns: &str, name: &str) -> Val {
        let p = Val::prim;
        match (ns, name) {
            ("lib", "optionals") => p(Prim::Optionals),
            ("lib", "optional") => p(Prim::Optional),
            ("lib", "mkIf") => p(Prim::MkIf),
            ("lib", "optionalString") => p(Prim::OptionalString),
            ("lib", "optionalAttrs") => p(Prim::OptionalAttrs),
            ("lib", "mkDefault" | "mkForce" | "mkBefore" | "mkAfter" | "id" | "mkOptionDefault") => p(Prim::Id),
            ("lib", "mkOverride" | "mkOrder") => p(Prim::Second),
            ("lib", "mkMerge") => p(Prim::MkMerge),
            ("lib", "genAttrs") => p(Prim::GenAttrs),
            ("lib" | "builtins", "attrValues") => p(Prim::AttrValues),
            ("lib" | "builtins", "concatLists") => p(Prim::ConcatLists),
            ("lib", "flatten") => p(Prim::Flatten),
            ("lib" | "builtins", "map") => p(Prim::Map),
            ("lib" | "builtins", "toString") => p(Prim::ToString),
            ("lib" | "builtins", "concatStringsSep") => p(Prim::ConcatStringsSep),
            ("lib", "nixosSystem") => p(Prim::NixosSystem),
            ("lib", "hm") | ("config.lib", "hm") => Val::Ns("lib.hm".into()),
            ("lib.hm", "dag") | ("config.lib", "dag") => Val::Ns("lib.hm.dag".into()),
            ("lib.hm.dag", "entryAfter") => p(Prim::DagAfter),
            ("lib.hm.dag", "entryBefore") => p(Prim::DagBefore),
            ("lib.hm.dag", "entryAnywhere") => p(Prim::DagAnywhere),
            ("lib.hm.dag", "entryBetween") => p(Prim::DagBetween),
            ("lib", "systems") => Val::Ns("lib.systems".into()),
            ("lib.systems", "flakeExposed") | ("flake-utils.lib", "defaultSystems" | "allSystems") => {
                Val::List(Rc::new(vec![Thunk::val(Val::Str(SYSTEM.into(), true))]))
            }
            ("builtins", "currentSystem") => Val::Str(SYSTEM.into(), true),
            ("builtins", "import") => p(Prim::Import),
            ("builtins", "throw" | "abort") => p(Prim::Throw),
            ("nixpkgs", "legacyPackages") => Val::Ns("nixpkgs.legacyPackages".into()),
            ("nixpkgs.legacyPackages", _) => Val::Pkg(String::new()),
            ("nixpkgs", "lib") => Val::Ns("lib".into()),
            ("flake-utils", "lib") => Val::Ns("flake-utils.lib".into()),
            ("flake-utils.lib", "eachDefaultSystem") => p(Prim::EachDefaultSystem),
            ("flake-utils.lib", "eachSystem") => p(Prim::EachSystem),
            ("flake-utils.lib", "system") => Val::Ns("flake-utils.lib.system".into()),
            ("flake-utils.lib.system", s) => Val::Str(s.into(), true),
            ("flake-parts", "lib") => Val::Ns("flake-parts.lib".into()),
            ("flake-parts.lib", "mkFlake") => p(Prim::MkFlake),
            ("home-manager", "lib") => Val::Ns("home-manager.lib".into()),
            ("home-manager.lib", "homeManagerConfiguration") => p(Prim::HomeManagerConfiguration),
            ("inputs", n) => self.input(n),
            ("config.home", "homeDirectory") if self.home_dir.is_some() => Val::Str(self.home_dir.clone().unwrap(), true),
            ("config.home", "username") if self.user.is_some() => Val::Str(self.user.clone().unwrap(), true),
            ("config.xdg", "configHome") if self.home_dir.is_some() => Val::Str(format!("{}/.config", self.home_dir.as_ref().unwrap()), true),
            ("config.xdg", "dataHome") if self.home_dir.is_some() => Val::Str(format!("{}/.local/share", self.home_dir.as_ref().unwrap()), true),
            ("config", _) | ("config.home" | "config.xdg", _) => Val::Ns(format!("{ns}.{name}")),
            _ => Val::Unknown(format!("{ns}.{name}")),
        }
    }

    pub fn apply(&self, f: Val, arg: Thunk, e: Option<&Expr>) -> Val {
        let unknown = || Val::Unknown(e.map(|e| self.text(e)).unwrap_or_else(|| "a function call".into()));
        match f {
            Val::Lambda(param, body, env) => self.call(&param, &body, &env, arg),
            Val::Prim(PrimFn(prim, args)) => {
                let mut args = (*args).clone();
                args.push(arg);
                if args.len() < prim.arity() {
                    Val::Prim(PrimFn(prim, Rc::new(args)))
                } else {
                    self.prim(prim, &args, e)
                }
            }
            Val::Pkg(p) => {
                if let Some(interp) = p.strip_suffix(".withPackages").or_else(|| p.strip_suffix(".ghcWithPackages")) {
                    let (interp, set) = if p.ends_with(".ghcWithPackages") {
                        (format!("{interp}.ghc"), interp.to_string())
                    } else {
                        let base = interp.strip_suffix(".pkgs").unwrap_or(interp);
                        (base.to_string(), format!("{base}Packages"))
                    };
                    let list = self.apply(self.force(&arg), Thunk::val(Val::Pkg(set)), e);
                    let items = self.list(&list);
                    self.warn(format!("{interp}.withPackages: added {interp} and its packages one by one; check they find each other in nami"));
                    return Val::WithPkgs(interp, Rc::new(items.into_iter().map(Thunk::val).collect()));
                }
                for suffix in [".override", ".overrideAttrs", ".overridePythonAttrs", ".overrideDerivation"] {
                    if let Some(base) = p.strip_suffix(suffix) {
                        self.warn(format!("{p}: took {base} as it is; the override isn't imported"));
                        return self.pkg(base);
                    }
                }
                unknown()
            }
            _ => unknown(),
        }
    }

    fn call(&self, param: &Param, body: &Rc<Expr>, env: &Env, arg: Thunk) -> Val {
        let inner = scope(Some(env), None);
        match param {
            Param::Ident(n) => {
                inner.vars.borrow_mut().insert(n.clone(), arg);
            }
            Param::Pat { fields, at } => {
                let given = match self.force(&arg) {
                    Val::Attrs(m) => Some(m),
                    _ => None,
                };
                for (name, default) in fields {
                    let t = given
                        .as_ref()
                        .and_then(|m| m.get(name).cloned())
                        .or_else(|| default.as_ref().map(|d| Thunk::lazy(d, &inner)))
                        .unwrap_or_else(|| Thunk::val(self.free(name)));
                    inner.vars.borrow_mut().insert(name.clone(), t);
                }
                if let Some(a) = at {
                    inner.vars.borrow_mut().insert(a.clone(), arg);
                }
            }
        }
        self.eval(body, &inner)
    }

    /// A function argument nobody passed: what modules and flakes get.
    fn free(&self, name: &str) -> Val {
        match name {
            "pkgs" => Val::Pkg(String::new()),
            "lib" => Val::Ns("lib".into()),
            "config" | "osConfig" => Val::Ns("config".into()),
            "system" => Val::Str(SYSTEM.into(), true),
            "inputs" => Val::Ns("inputs".into()),
            _ => self.input(name),
        }
    }

    /// The items of a list (nested lists flattened); a non-list is one item.
    pub fn list(&self, v: &Val) -> Vec<Val> {
        match v {
            Val::List(items) => items.iter().flat_map(|t| {
                let v = self.force(t);
                if matches!(v, Val::List(_)) { self.list(&v) } else { vec![v] }
            }).collect(),
            Val::Null => Vec::new(),
            other => vec![other.clone()],
        }
    }

    fn cond(&self, t: &Thunk, what: &str) -> Option<bool> {
        let src = t.expr().map(|e| self.text(&e));
        match self.force(t) {
            Val::Bool(b) => Some(b),
            _ => {
                self.warn(format!("couldn't tell whether `{what} {}` applies; included it", src.unwrap_or_default()));
                None
            }
        }
    }

    fn prim(&self, prim: Prim, args: &[Thunk], e: Option<&Expr>) -> Val {
        let unknown = || Val::Unknown(e.map(|e| self.text(e)).unwrap_or_else(|| "a function call".into()));
        let strs = |v: &Val| -> Option<Vec<String>> { self.list(v).iter().map(|v| v.str().map(String::from)).collect() };
        match prim {
            // As home-manager has them: { data, after, before }.
            Prim::DagAfter | Prim::DagBefore | Prim::DagAnywhere | Prim::DagBetween => {
                let none = || Thunk::val(Val::List(Rc::new(Vec::new())));
                let (data, after, before) = match prim {
                    Prim::DagAfter => (args[1].clone(), args[0].clone(), none()),
                    Prim::DagBefore => (args[1].clone(), none(), args[0].clone()),
                    Prim::DagBetween => (args[2].clone(), args[1].clone(), args[0].clone()),
                    _ => (args[0].clone(), none(), none()),
                };
                Val::Attrs(Rc::new(BTreeMap::from([("data".to_string(), data), ("after".to_string(), after), ("before".to_string(), before)])))
            }
            Prim::Optionals => match self.cond(&args[0], "lib.optionals") {
                Some(false) => Val::List(Rc::new(Vec::new())),
                _ => self.force(&args[1]),
            },
            Prim::Optional => match self.cond(&args[0], "lib.optional") {
                Some(false) => Val::List(Rc::new(Vec::new())),
                _ => Val::List(Rc::new(vec![args[1].clone()])),
            },
            Prim::MkIf => match self.cond(&args[0], "lib.mkIf") {
                Some(false) => Val::Null,
                _ => self.force(&args[1]),
            },
            Prim::OptionalString => match self.cond(&args[0], "lib.optionalString") {
                Some(false) => Val::Str(String::new(), true),
                _ => self.force(&args[1]),
            },
            Prim::OptionalAttrs => match self.cond(&args[0], "lib.optionalAttrs") {
                Some(false) => Val::Attrs(Rc::new(BTreeMap::new())),
                _ => self.force(&args[1]),
            },
            Prim::Id => self.force(&args[0]),
            Prim::Second => self.force(&args[1]),
            Prim::MkMerge => {
                let mut out = Val::Attrs(Rc::new(BTreeMap::new()));
                for v in self.list(&self.force(&args[0])) {
                    out = self.merge(&out, &v);
                }
                out
            }
            Prim::GenAttrs => {
                let Some(names) = strs(&self.force(&args[0])) else { return unknown() };
                let f = self.force(&args[1]);
                Val::Attrs(Rc::new(names.into_iter().map(|n| {
                    let v = self.apply(f.clone(), Thunk::val(Val::Str(n.clone(), true)), e);
                    (n, Thunk::val(v))
                }).collect()))
            }
            Prim::EachDefaultSystem | Prim::EachSystem => {
                let f = self.force(args.last().unwrap());
                match self.apply(f, Thunk::val(Val::Str(SYSTEM.into(), true)), e) {
                    Val::Attrs(m) => Val::Attrs(Rc::new(m.iter().map(|(k, v)| {
                        let per = BTreeMap::from([(SYSTEM.to_string(), v.clone())]);
                        (k.clone(), Thunk::val(Val::Attrs(Rc::new(per))))
                    }).collect())),
                    other => other,
                }
            }
            Prim::MkFlake => {
                let mut module = self.force(&args[1]);
                if matches!(module, Val::Lambda(..)) {
                    module = self.apply(module, Thunk::val(Val::Attrs(Rc::new(BTreeMap::new()))), e);
                }
                let Val::Attrs(m) = &module else { return unknown() };
                let mut out: BTreeMap<String, Thunk> = BTreeMap::new();
                if let Some(Val::Attrs(flake)) = m.get("flake").map(|t| self.force(t)) {
                    out.extend(flake.iter().map(|(k, v)| (k.clone(), v.clone())));
                }
                if let Some(per) = m.get("perSystem").map(|t| self.force(t)) {
                    let args = BTreeMap::from([
                        ("pkgs".to_string(), Thunk::val(Val::Pkg(String::new()))),
                        ("system".to_string(), Thunk::val(Val::Str(SYSTEM.into(), true))),
                        ("lib".to_string(), Thunk::val(Val::Ns("lib".into()))),
                    ]);
                    if let Val::Attrs(ps) = self.apply(per, Thunk::val(Val::Attrs(Rc::new(args))), e) {
                        for (k, v) in ps.iter() {
                            let per = BTreeMap::from([(SYSTEM.to_string(), v.clone())]);
                            out.insert(k.clone(), Thunk::val(Val::Attrs(Rc::new(per))));
                        }
                    }
                }
                Val::Attrs(Rc::new(out))
            }
            Prim::Import => match self.force(&args[0]) {
                Val::Path(p) => self.import(&p).unwrap_or_else(|err| Val::Unknown(format!("import {} ({err:#})", p.display()))),
                Val::Ns(n) if n == "nixpkgs" => Val::prim(Prim::ImportNixpkgs),
                _ => unknown(),
            },
            Prim::ImportNixpkgs => {
                if let Val::Attrs(m) = self.force(&args[0]) {
                    if let Some(o) = m.get("overlays") {
                        if !self.list(&self.force(o)).is_empty() {
                            self.skip("nixpkgs overlays", "nami uses nixpkgs as it is; packages an overlay adds or changes aren't in the catalog");
                        }
                    }
                }
                Val::Pkg(String::new())
            }
            Prim::AttrValues => match self.force(&args[0]) {
                Val::Attrs(m) => Val::List(Rc::new(m.values().cloned().collect())),
                _ => unknown(),
            },
            Prim::ConcatLists | Prim::Flatten => Val::List(Rc::new(self.list(&self.force(&args[0])).into_iter().map(Thunk::val).collect())),
            Prim::Map => {
                let f = self.force(&args[0]);
                match self.force(&args[1]) {
                    Val::List(items) => Val::List(Rc::new(items.iter().map(|t| Thunk::val(self.apply(f.clone(), t.clone(), e))).collect())),
                    _ => unknown(),
                }
            }
            Prim::ToString => match self.force(&args[0]) {
                s @ Val::Str(..) => s,
                Val::Int(n) => Val::Str(n.to_string(), true),
                Val::Path(p) => Val::Str(p.display().to_string(), true),
                Val::Bool(b) => Val::Str(if b { "1".into() } else { String::new() }, true),
                Val::Null => Val::Str(String::new(), true),
                _ => unknown(),
            },
            Prim::ConcatStringsSep => {
                let Some(sep) = self.force(&args[0]).str().map(String::from) else { return unknown() };
                let items = self.list(&self.force(&args[1]));
                let mut exact = true;
                let parts: Vec<String> = items.iter().map(|v| match v {
                    Val::Str(s, ex) => {
                        exact &= ex;
                        s.clone()
                    }
                    _ => {
                        exact = false;
                        "${…}".into()
                    }
                }).collect();
                Val::Str(parts.join(&sep), exact)
            }
            Prim::WriteScriptBin => {
                let name = self.force(&args[0]).str().unwrap_or("script").to_string();
                let (text, exact) = match self.force(&args[1]) {
                    Val::Str(s, ex) => (s, ex),
                    _ => (String::new(), false),
                };
                Val::Script { name, text, exact, inputs: Vec::new() }
            }
            Prim::WriteShellApplication => {
                let Val::Attrs(m) = self.force(&args[0]) else { return unknown() };
                let get = |k: &str| m.get(k).map(|t| self.force(t));
                let name = get("name").and_then(|v| v.str().map(String::from)).unwrap_or_else(|| "script".into());
                let (text, exact) = match get("text") {
                    Some(Val::Str(s, ex)) => (s, ex),
                    _ => (String::new(), false),
                };
                let inputs = get("runtimeInputs").map(|v| self.list(&v).into_iter().map(Thunk::val).collect()).unwrap_or_default();
                Val::Script { name, text, exact, inputs }
            }
            Prim::MkShell => {
                let mut a = self.force(&args[0]);
                if matches!(a, Val::Lambda(..)) {
                    a = self.apply(a, Thunk::val(Val::Unknown("finalAttrs".into())), e);
                }
                match a {
                    Val::Attrs(m) => Val::Shell(m),
                    _ => unknown(),
                }
            }
            Prim::HomeManagerConfiguration | Prim::NixosSystem => {
                let Val::Attrs(m) = self.force(&args[0]) else { return unknown() };
                let modules = m.get("modules").map(|t| self.list(&self.force(t))).unwrap_or_default();
                Val::Config(Rc::new(modules.into_iter().map(Thunk::val).collect()))
            }
            Prim::Throw => Val::Unknown("throw".into()),
        }
    }

    /// Read another Nix file, once.
    pub fn import(&self, path: &Path) -> Result<Val> {
        let file = if path.is_dir() { path.join("default.nix") } else { path.to_path_buf() };
        if let Some(v) = self.imported.borrow().get(&file) {
            return Ok(v.clone());
        }
        let v = self.eval_file(&file)?;
        self.imported.borrow_mut().insert(file, v.clone());
        Ok(v)
    }

    /// `a // b` deep enough for modules: attribute sets merge, lists append,
    /// otherwise `b` wins.
    pub fn merge(&self, a: &Val, b: &Val) -> Val {
        match (a, b) {
            (Val::Attrs(x), Val::Attrs(y)) => {
                let mut out = (**x).clone();
                for (k, v) in y.iter() {
                    let merged = match out.get(k) {
                        Some(old) => Thunk::val(self.merge(&self.force(old), &self.force(v))),
                        None => v.clone(),
                    };
                    out.insert(k.clone(), merged);
                }
                Val::Attrs(Rc::new(out))
            }
            (Val::List(x), Val::List(y)) => Val::List(Rc::new(x.iter().chain(y.iter()).cloned().collect())),
            (_, Val::Null) => a.clone(),
            _ => b.clone(),
        }
    }

    fn binop(&self, op: Op, l: &Rc<Expr>, r: &Rc<Expr>, env: &Env, e: &Expr) -> Val {
        let unknown = || Val::Unknown(self.text(e));
        match op {
            Op::And | Op::Or | Op::Impl => {
                let a = self.eval(l, env);
                match (op, &a) {
                    (Op::And, Val::Bool(false)) => return Val::Bool(false),
                    (Op::Or, Val::Bool(true)) => return Val::Bool(true),
                    (Op::Impl, Val::Bool(false)) => return Val::Bool(true),
                    _ => {}
                }
                match (a, self.eval(r, env)) {
                    (Val::Bool(_), Val::Bool(b)) => Val::Bool(b),
                    _ => unknown(),
                }
            }
            Op::Concat => {
                let (a, b) = (self.eval(l, env), self.eval(r, env));
                let items = |v: Val| match v {
                    Val::List(items) => (*items).clone(),
                    other => vec![Thunk::val(other)],
                };
                Val::List(Rc::new(items(a).into_iter().chain(items(b)).collect()))
            }
            Op::Update => match (self.eval(l, env), self.eval(r, env)) {
                (Val::Attrs(a), Val::Attrs(b)) => {
                    let mut m = (*a).clone();
                    m.extend(b.iter().map(|(k, v)| (k.clone(), v.clone())));
                    Val::Attrs(Rc::new(m))
                }
                (Val::Attrs(a), _) | (_, Val::Attrs(a)) => {
                    self.warn(format!("read only the known half of `{}`", self.text(e)));
                    Val::Attrs(a)
                }
                _ => unknown(),
            },
            _ => {
                let (a, b) = (self.eval(l, env), self.eval(r, env));
                match (op, &a, &b) {
                    (Op::Add, Val::Str(x, ex), Val::Str(y, ey)) => Val::Str(format!("{x}{y}"), *ex && *ey),
                    (Op::Add, Val::Path(p), Val::Str(s, true)) => Val::Path(clean(&PathBuf::from(format!("{}{s}", p.display())))),
                    (Op::Add, Val::Str(s, ex), Val::Path(p)) => Val::Str(format!("{s}{}", p.display()), *ex),
                    (Op::Add, Val::Int(x), Val::Int(y)) => Val::Int(x + y),
                    (Op::Sub, Val::Int(x), Val::Int(y)) => Val::Int(x - y),
                    (Op::Mul, Val::Int(x), Val::Int(y)) => Val::Int(x * y),
                    (Op::Div, Val::Int(x), Val::Int(y)) if *y != 0 => Val::Int(x / y),
                    (Op::Lt, Val::Int(x), Val::Int(y)) => Val::Bool(x < y),
                    (Op::Gt, Val::Int(x), Val::Int(y)) => Val::Bool(x > y),
                    (Op::Le, Val::Int(x), Val::Int(y)) => Val::Bool(x <= y),
                    (Op::Ge, Val::Int(x), Val::Int(y)) => Val::Bool(x >= y),
                    (Op::Eq | Op::Ne, _, _) => match (prim_eq(&a, &b), op) {
                        (Some(eq), Op::Eq) => Val::Bool(eq),
                        (Some(eq), _) => Val::Bool(!eq),
                        (None, _) => unknown(),
                    },
                    _ => unknown(),
                }
            }
        }
    }

    /// An attribute set's bindings: nested paths (`a.b = 1; a.c = 2;`) merged.
    /// `env` is where values are evaluated (the set itself, for `rec`/`let`);
    /// `outer` is where plain `inherit x;` looks.
    fn build_attrs(&self, binds: &[Bind], env: &Env, outer: &Env) -> BTreeMap<String, Thunk> {
        enum Node {
            Leaf(Thunk, Option<(Rc<Expr>, Env)>),
            Tree(BTreeMap<String, Node>),
        }
        fn insert(ev: &Eval, tree: &mut BTreeMap<String, Node>, path: &[String], value: Rc<Expr>, env: &Env) {
            let (first, rest) = path.split_first().unwrap();
            if rest.is_empty() {
                match tree.get_mut(first) {
                    Some(Node::Tree(sub)) => {
                        if let K::Attrs(false, binds) = &value.k {
                            expand(ev, sub, binds, env);
                        }
                    }
                    _ => {
                        tree.insert(first.clone(), Node::Leaf(Thunk::lazy(&value, env), Some((value, env.clone()))));
                    }
                }
                return;
            }
            let node = tree.entry(first.clone()).or_insert_with(|| Node::Tree(BTreeMap::new()));
            if let Node::Leaf(_, Some((e, lenv))) = node {
                if let K::Attrs(false, binds) = &e.k {
                    let (binds_env, mut sub) = (lenv.clone(), BTreeMap::new());
                    expand(ev, &mut sub, binds, &binds_env);
                    *node = Node::Tree(sub);
                }
            }
            if let Node::Tree(sub) = node {
                insert(ev, sub, rest, value, env);
            }
        }
        fn expand(ev: &Eval, tree: &mut BTreeMap<String, Node>, binds: &[Bind], env: &Env) {
            for b in binds {
                match b {
                    Bind::Attr(keys, value) => {
                        let Some(path) = keys.iter().map(|k| ev.key_name(k, env)).collect::<Option<Vec<_>>>() else { continue };
                        insert(ev, tree, &path, value.clone(), env);
                    }
                    Bind::Inherit(names) => {
                        for (n, e) in names {
                            tree.insert(n.clone(), Node::Leaf(Thunk::lazy(e, env), None));
                        }
                    }
                }
            }
        }
        fn finish(tree: BTreeMap<String, Node>) -> BTreeMap<String, Thunk> {
            tree.into_iter().map(|(k, n)| (k, match n {
                Node::Leaf(t, _) => t,
                Node::Tree(sub) => Thunk::val(Val::Attrs(Rc::new(finish(sub)))),
            })).collect()
        }
        let mut tree = BTreeMap::new();
        for b in binds {
            match b {
                Bind::Attr(keys, value) => {
                    let Some(path) = keys.iter().map(|k| self.key_name(k, env)).collect::<Option<Vec<_>>>() else {
                        self.warn("skipped an attribute whose name is computed");
                        continue;
                    };
                    insert(self, &mut tree, &path, value.clone(), env);
                }
                Bind::Inherit(names) => {
                    for (n, e) in names {
                        // `inherit x;` reads x from outside; `inherit (s) x;` evaluates s here.
                        let at = if matches!(e.k, K::Ident(_)) { outer } else { env };
                        tree.insert(n.clone(), Node::Leaf(Thunk::lazy(e, at), None));
                    }
                }
            }
        }
        finish(tree)
    }

    fn interp(&self, parts: &[Part], env: &Env) -> Val {
        let mut s = String::new();
        let mut exact = true;
        let mut strip_bin = false;
        let mut share = false;
        for (i, part) in parts.iter().enumerate() {
            match part {
                Part::Lit(t) if share => {
                    let rest = &t["/share/".len()..];
                    let end = rest.find(|c: char| c.is_whitespace() || "\"';)|&<>`$".contains(c)).unwrap_or(rest.len());
                    s += &format!("\"$(nami_share {})\"{}", &rest[..end], &rest[end..]);
                    share = false;
                }
                Part::Lit(t) => {
                    if strip_bin {
                        s += t.strip_prefix("/bin/").unwrap_or(t);
                    } else {
                        s += t;
                    }
                    strip_bin = false;
                }
                Part::Expr(x) => match self.eval(x, env) {
                    Val::Str(t, ex) => {
                        s += &t;
                        exact &= ex;
                    }
                    Val::Int(n) => s += &n.to_string(),
                    Val::Path(p) => {
                        s += &p.display().to_string();
                        exact = false;
                    }
                    // `${pkgs.eza}/bin/eza` is the command `eza`, with eza installed.
                    Val::Pkg(p) if matches!(parts.get(i + 1), Some(Part::Lit(t)) if t.starts_with("/bin/")) => {
                        self.bin_refs.borrow_mut().push(p);
                        strip_bin = true;
                    }
                    // In shell code, `${pkgs.x}/share/a/b.zsh` is that file of x, installed.
                    Val::Pkg(p) if self.shell_text.get() && matches!(parts.get(i + 1), Some(Part::Lit(t)) if t.starts_with("/share/") && t.len() > 7) => {
                        self.share_refs.borrow_mut().push(p);
                        share = true;
                    }
                    _ => {
                        s += &format!("${{{}}}", self.text(x));
                        exact = false;
                    }
                },
            }
        }
        Val::Str(s, exact)
    }

    /// A value as YAML, all the way down; Err says what in it isn't known.
    pub fn to_yaml(&self, v: &Val) -> std::result::Result<serde_yaml::Value, String> {
        use serde_yaml::Value as Y;
        Ok(match v {
            Val::Null => Y::Null,
            Val::Bool(b) => Y::Bool(*b),
            Val::Int(n) => Y::Number((*n).into()),
            Val::Float(f) => Y::Number((*f).into()),
            Val::Str(s, true) => Y::String(s.clone()),
            Val::Str(s, false) => return Err(format!("\"{s}\" refers to values only Nix knows (store paths, options)")),
            Val::Path(p) => Y::String(p.display().to_string()),
            Val::List(items) => Y::Sequence(items.iter().map(|t| self.to_yaml(&self.force(t))).collect::<std::result::Result<_, _>>()?),
            Val::Attrs(m) => {
                let mut out = serde_yaml::Mapping::new();
                for (k, t) in m.iter() {
                    let v = self.force(t);
                    if matches!(v, Val::Null) {
                        continue;
                    }
                    out.insert(Y::String(k.clone()), self.to_yaml(&v)?);
                }
                Y::Mapping(out)
            }
            Val::Pkg(p) => return Err(format!("pkgs.{p} (a store path)")),
            Val::Ns(n) => return Err(n.clone()),
            Val::Unknown(t) => return Err(t.clone()),
            _ => return Err("a function or derivation".into()),
        })
    }
}

fn prim_eq(a: &Val, b: &Val) -> Option<bool> {
    Some(match (a, b) {
        (Val::Null, Val::Null) => true,
        (Val::Bool(x), Val::Bool(y)) => x == y,
        (Val::Int(x), Val::Int(y)) => x == y,
        (Val::Str(x, true), Val::Str(y, true)) => x == y,
        (Val::Null | Val::Bool(_) | Val::Int(_) | Val::Str(_, true), Val::Null | Val::Bool(_) | Val::Int(_) | Val::Str(_, true)) => false,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(src: &str) -> (Eval, Val) {
        let ev = Eval::default();
        let e = ev.parse(Path::new("/x/test.nix"), src).unwrap_or_else(|e| panic!("{e:#}"));
        let v = ev.eval(&e, &scope(None, None));
        (ev, v)
    }

    fn get(ev: &Eval, v: &Val, path: &str) -> Val {
        path.split('.').fold(v.clone(), |v, k| ev.select(&v, k).unwrap_or(Val::Unknown(format!("missing {k}"))))
    }

    fn pkgs(ev: &Eval, v: &Val) -> Vec<String> {
        ev.list(v).into_iter().map(|v| match v {
            Val::Pkg(p) => p,
            Val::Unknown(t) => format!("?{t}"),
            _ => "?".into(),
        }).collect()
    }

    #[test]
    fn parses_comments_strings_and_lets() {
        let (ev, v) = eval(r#"
            # a comment
            let /* block */ name = "world"; n = 2; in {
              greeting = "hello ${name}";
              multi = ''
                line one
                  indented ''${not} ${name}
              '';
              sum = n + 3;
              "quoted.key" = true;
              nested.a.b = 1;
              nested.a.c = "x";
            }"#);
        assert_eq!(get(&ev, &v, "greeting").str(), Some("hello world"));
        assert_eq!(get(&ev, &v, "multi").str(), Some("line one\n  indented ${not} world\n"));
        assert!(matches!(get(&ev, &v, "sum"), Val::Int(5)));
        assert!(matches!(ev.select(&v, "quoted.key"), Some(Val::Bool(true))));
        assert!(matches!(get(&ev, &v, "nested.a.b"), Val::Int(1)));
        assert_eq!(get(&ev, &v, "nested.a.c").str(), Some("x"));
    }

    #[test]
    fn unary_not_and_minus() {
        let (ev, v) = eval("let t = true; n = 3; in { a = !t; b = !(n == 4); c = -n; d = 5 - -2; e = !t || t; f = n != 3; g = t -> !t; }");
        assert!(matches!(get(&ev, &v, "a"), Val::Bool(false)));
        assert!(matches!(get(&ev, &v, "b"), Val::Bool(true)));
        assert!(matches!(get(&ev, &v, "c"), Val::Int(-3)));
        assert!(matches!(get(&ev, &v, "d"), Val::Int(7)));
        assert!(matches!(get(&ev, &v, "e"), Val::Bool(true)));
        assert!(matches!(get(&ev, &v, "f"), Val::Bool(false)));
        assert!(matches!(get(&ev, &v, "g"), Val::Bool(false)));
    }

    #[test]
    fn package_lists_in_their_usual_shapes() {
        let (ev, v) = eval(r#"{ pkgs ? import <nixpkgs> {}, lib ? pkgs.lib }: {
            a = with pkgs; [ ripgrep fd nodejs_22 python3Packages.requests ];
            b = [ pkgs.jq pkgs.gnome.nautilus ] ++ lib.optionals pkgs.stdenv.isLinux [ pkgs.strace ] ++ lib.optional pkgs.stdenv.isDarwin pkgs.darwin.libiconv;
            c = [ (pkgs.python3.withPackages (ps: with ps; [ requests numpy ])) (pkgs.callPackage ./thing.nix {}) ];
        }"#);
        let f = match v { Val::Lambda(..) => ev.apply(v, Thunk::val(Val::Attrs(Rc::new(BTreeMap::new()))), None), v => v };
        assert_eq!(pkgs(&ev, &get(&ev, &f, "a")), ["ripgrep", "fd", "nodejs_22", "python3Packages.requests"]);
        // (For the platform nami runs on: a Mac's list has darwin's instead of Linux's.)
        let os_own = if SYSTEM.ends_with("-darwin") { "darwin.libiconv" } else { "strace" };
        assert_eq!(pkgs(&ev, &get(&ev, &f, "b")), ["jq", "gnome.nautilus", os_own]);
        let c = ev.list(&get(&ev, &f, "c"));
        match &c[0] {
            Val::WithPkgs(i, items) => {
                assert_eq!(i, "python3");
                assert_eq!(pkgs(&ev, &Val::List(items.clone())), ["python3Packages.requests", "python3Packages.numpy"]);
            }
            _ => panic!("withPackages"),
        }
        assert!(matches!(&c[1], Val::Unknown(t) if t.contains("callPackage")));
    }

    #[test]
    fn flake_utils_and_mkshell() {
        let ev = Eval::default();
        ev.set_input("nixpkgs", "github:NixOS/nixpkgs/nixos-unstable");
        ev.set_input("flake-utils", "github:numtide/flake-utils");
        let e = ev.parse(Path::new("/x/flake.nix"), r#"{
          inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
          outputs = { self, nixpkgs, flake-utils }:
            flake-utils.lib.eachDefaultSystem (system:
              let pkgs = nixpkgs.legacyPackages.${system}; in {
                devShells.default = pkgs.mkShell { packages = [ pkgs.go ]; GOFLAGS = "-mod=vendor"; };
              });
        }"#).unwrap_or_else(|e| panic!("{e:#}"));
        let top = ev.eval(&e, &scope(None, None));
        let out = ev.apply(get(&ev, &top, "outputs"), Thunk::val(Val::Attrs(Rc::new(BTreeMap::new()))), None);
        let shell = get(&ev, &out, &format!("devShells.{SYSTEM}.default"));
        let Val::Shell(m) = shell else { panic!("not a shell") };
        assert_eq!(pkgs(&ev, &ev.force(&m["packages"])), ["go"]);
        assert_eq!(ev.force(&m["GOFLAGS"]).str(), Some("-mod=vendor"));
    }

    #[test]
    fn bin_paths_become_commands_and_unknowns_say_what() {
        let (ev, v) = eval(r#"{ pkgs, config, ... }: { ll = "${pkgs.eza}/bin/eza -l"; p = "${config.home.homeDirectory}/bin"; q = "${pkgs.foo}"; }"#);
        let m = ev.apply(v, Thunk::val(Val::Attrs(Rc::new(BTreeMap::new()))), None);
        assert_eq!(get(&ev, &m, "ll").str(), Some("eza -l"));
        assert_eq!(ev.bin_refs.borrow().as_slice(), ["eza"]);
        assert!(get(&ev, &m, "p").str().is_some_and(|s| s.ends_with("/bin")));
        assert!(matches!(get(&ev, &m, "q"), Val::Str(s, false) if s == "${pkgs.foo}"));
    }

    #[test]
    fn syntax_errors_have_lines() {
        let ev = Eval::default();
        let Err(err) = ev.parse(Path::new("/x/bad.nix"), "{\n  a = 1;\n  b = ;\n}") else { panic!("parsed") };
        assert!(format!("{err:#}").contains("line 3"), "{err:#}");
    }
}
