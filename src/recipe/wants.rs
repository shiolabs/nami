//! What a project's build files ask for, read from its source: the
//! libraries in CMake's find_package and pkg_check_modules, Meson's
//! dependency(), configure.ac's PKG_CHECK_MODULES and AC_CHECK_LIB, a
//! Makefile's `pkg-config` and `-l`; the -sys crates in Cargo.lock; a
//! Python project's dependencies and build backend; and its name,
//! version and description. Nothing here asks the catalog: from_source.rs
//! maps these to its packages (names.rs).

use std::collections::BTreeSet;

use crate::archive::Tree;
use crate::build_kinds::Kind;

/// One thing a build asks for, and where it says so.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Want {
    pub name: String,
    pub from: String,
    /// The build goes on without it (no REQUIRED, `required: false`).
    pub optional: bool,
    /// Asked for only on a branch (`if test "$enable_sensors" = yes`, an
    /// `if()` in CMake): needed when that's taken, so never NOT FOUND.
    pub conditional: bool,
}

#[derive(Debug, Default)]
pub struct Wants {
    pub kind: Option<Kind>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub homepage: Option<String>,
    /// Commands the build runs.
    pub tools: Vec<Want>,
    pub pkg_config: Vec<Want>,
    /// CMake's find_package names.
    pub cmake: Vec<Want>,
    /// Libraries to link, by name (`z` for -lz).
    pub libraries: Vec<Want>,
    pub headers: Vec<Want>,
    /// -sys crates in Cargo.lock.
    pub crates: Vec<Want>,
    /// A Python project's dependencies, by their distribution names.
    pub python: Vec<Want>,
    /// `make-flags` it needs: `PREFIX=$(out)` for a Makefile that installs to /usr/local.
    pub make_flags: Vec<String>,
    /// What the recipe's reader should know (`# ...` lines in it).
    pub notes: Vec<String>,
}

/// Read `tree` (see the module's docs).
pub fn read(tree: &Tree) -> Wants {
    let mut w = Wants { kind: crate::build_kinds::detect(tree), ..Default::default() };
    for (path, text) in tree.texts(|p| p.ends_with("CMakeLists.txt") || p.ends_with(".cmake")) {
        cmake(text, path, &mut w);
    }
    for (path, text) in tree.texts(|p| p.ends_with("meson.build")) {
        meson(text, path, &mut w);
    }
    if w.kind == Some(Kind::Autotools) {
        for (path, text) in tree.texts(|p| p == "configure.ac" || p == "configure.in") {
            autoconf(text, path, !tree.has("configure"), &mut w);
        }
    }
    if w.kind == Some(Kind::Make) {
        for f in ["Makefile", "makefile", "GNUmakefile"] {
            if let Some(text) = tree.text(f) {
                makefile(text, f, &mut w);
            }
        }
    }
    if let Some(lock) = tree.text("Cargo.lock") {
        cargo_lock(lock, &mut w);
    }
    match w.kind {
        Some(Kind::Cargo) => cargo_meta(tree, &mut w),
        Some(Kind::Go) => {
            let module = tree.text("go.mod").and_then(|t| t.lines().find_map(|l| l.trim().strip_prefix("module ")).map(|m| m.trim().trim_matches('"').to_string()));
            if let Some(m) = module {
                let mut parts: Vec<&str> = m.split('/').collect();
                if parts.len() > 1 && parts.last().is_some_and(|l| l.strip_prefix('v').is_some_and(|n| n.parse::<u32>().is_ok())) {
                    parts.pop();
                }
                w.name = parts.last().map(|s| s.to_string());
                if m.contains('.') {
                    w.homepage = Some(format!("https://{}", m));
                }
            }
        }
        Some(Kind::Python) => python(tree, &mut w),
        Some(Kind::Node) => node(tree, &mut w),
        Some(Kind::Meson) => {
            if let Some(t) = tree.text("meson.build") {
                if let Some(args) = calls(t, "project", '(', ')').first() {
                    let words = quoted(args);
                    w.name = words.first().cloned();
                    w.version = keyword(args, "version");
                }
            }
        }
        Some(Kind::CMake) => {
            if let Some(t) = tree.text("CMakeLists.txt") {
                if let Some(args) = calls(&strip_comments(t), "project", '(', ')').first() {
                    let words = words(args);
                    w.name = words.first().cloned();
                    w.version = words.iter().position(|x| x == "VERSION").and_then(|i| words.get(i + 1)).filter(|v| v.starts_with(|c: char| c.is_ascii_digit())).cloned();
                    w.description = words.iter().position(|x| x == "DESCRIPTION").and_then(|i| words.get(i + 1)).cloned();
                    w.homepage = words.iter().position(|x| x == "HOMEPAGE_URL").and_then(|i| words.get(i + 1)).cloned();
                }
            }
        }
        _ => {}
    }
    for list in [&mut w.tools, &mut w.pkg_config, &mut w.cmake, &mut w.libraries, &mut w.headers, &mut w.crates, &mut w.python] {
        dedup(list);
    }
    w
}

/// One of each name, required if anywhere it's required.
fn dedup(list: &mut Vec<Want>) {
    let mut out: Vec<Want> = Vec::new();
    for want in list.drain(..) {
        match out.iter_mut().find(|o| o.name == want.name) {
            Some(o) => {
                o.optional &= want.optional;
                o.conditional &= want.conditional;
            }
            None => out.push(want),
        }
    }
    *list = out;
}

fn want(name: &str, from: &str, optional: bool) -> Want {
    Want { name: name.to_string(), from: from.to_string(), optional, conditional: false }
}

/// The same, asked for at `at` in `text`: on a branch or not (see
/// [`on_a_branch`]).
fn want_at(name: &str, from: &str, optional: bool, text: &str, at: usize, lang: Lang) -> Want {
    Want { conditional: on_a_branch(text, at, lang), ..want(name, from, optional) }
}

#[derive(Clone, Copy, PartialEq)]
enum Lang {
    Autoconf,
    CMake,
    Meson,
}

/// Whether offset `at` of a build file is inside a branch: a shell
/// `if`/`case`, autoconf's AS_IF, AS_CASE, AC_ARG_ENABLE and AC_ARG_WITH,
/// CMake's and Meson's `if`.
fn on_a_branch(text: &str, at: usize, lang: Lang) -> bool {
    let mut depth = 0i32;
    for line in text[..at.min(text.len())].lines() {
        let l = line.trim().to_lowercase();
        let word = |w: &str| l == w || l.starts_with(&format!("{w} ")) || l.starts_with(&format!("{w}(")) || l.starts_with(&format!("{w};"));
        match lang {
            Lang::Autoconf => {
                if word("if") || word("case") {
                    depth += 1;
                } else if word("fi") || word("esac") {
                    depth -= 1;
                }
            }
            Lang::CMake | Lang::Meson => {
                if word("if") {
                    depth += 1;
                } else if word("endif") {
                    depth -= 1;
                }
            }
        }
    }
    if depth > 0 {
        return true;
    }
    lang == Lang::Autoconf && ["AS_IF", "AS_CASE", "AC_ARG_ENABLE", "AC_ARG_WITH"].iter().any(|m| calls_at(text, m, '(', ')').iter().any(|(start, args)| *start < at && at <= start + args.len() + m.len() + 2))
}

/// The arguments of each call of `name` (any case): what's between the
/// brackets, nested brackets kept.
fn calls(text: &str, name: &str, open: char, close: char) -> Vec<String> {
    calls_at(text, name, open, close).into_iter().map(|(_, a)| a).collect()
}

/// [`calls`], each with where its arguments start in `text`.
fn calls_at(text: &str, name: &str, open: char, close: char) -> Vec<(usize, String)> {
    let lower = text.to_lowercase();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = lower[at..].find(&name.to_lowercase()) {
        let start = at + i;
        at = start + name.len();
        // A word of its own (not cc.find_library inside foo_find_library), then the bracket.
        let before = lower[..start].chars().next_back();
        if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        let rest = &text[at..];
        let Some(rest) = rest.trim_start_matches([' ', '\t']).strip_prefix(open) else { continue };
        let args_at = text.len() - rest.len();
        let mut depth = 1;
        let mut end = None;
        for (j, c) in rest.char_indices() {
            if c == open {
                depth += 1;
            } else if c == close {
                depth -= 1;
                if depth == 0 {
                    end = Some(j);
                    break;
                }
            }
        }
        if let Some(j) = end {
            out.push((args_at, rest[..j].to_string()));
        }
    }
    out
}

/// CMake's comments out (`# ...` to the end of the line, outside quotes).
fn strip_comments(text: &str) -> String {
    text.lines()
        .map(|l| {
            let mut in_quote = false;
            for (i, c) in l.char_indices() {
                match c {
                    '"' => in_quote = !in_quote,
                    '#' if !in_quote => return &l[..i],
                    _ => {}
                }
            }
            l
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A CMake call's words, without quotes; none with a variable in it.
fn words(args: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for c in args.chars() {
        match c {
            '"' => in_quote = !in_quote,
            c if c.is_whitespace() && !in_quote => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// A pkg-config module list's names: `glib-2.0 >= 2.40 gio-2.0` is
/// glib-2.0 and gio-2.0.
fn modules(list: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip_next = false;
    for w in list.split(|c: char| c.is_whitespace() || c == ',') {
        if w.is_empty() {
            continue;
        }
        if skip_next {
            skip_next = false;
            continue;
        }
        if ["<", "<=", ">", ">=", "=", "=="].contains(&w) {
            skip_next = true;
            continue;
        }
        let name = w.split(['<', '>', '=']).next().unwrap_or(w);
        if !name.is_empty() && !name.contains('$') && !name.starts_with(|c: char| c.is_ascii_digit()) {
            out.push(name.to_string());
        }
    }
    out
}

const CMAKE_KEYWORDS: &[&str] = &["REQUIRED", "QUIET", "IMPORTED_TARGET", "GLOBAL", "NO_CMAKE_PATH", "NO_CMAKE_ENVIRONMENT_PATH"];

fn cmake(text: &str, path: &str, w: &mut Wants) {
    let text = strip_comments(text);
    for (at, args) in calls_at(&text, "find_package", '(', ')') {
        let ws = words(&args);
        let Some(name) = ws.first() else { continue };
        if name.contains('$') {
            continue;
        }
        if name == "PkgConfig" {
            w.tools.push(want("pkg-config", path, false));
            continue;
        }
        // Without REQUIRED, but used as if it were (cmatrix links
        // ${CURSES_LIBRARIES}, with no if(CURSES_FOUND) around it).
        let required = ws.iter().any(|x| x == "REQUIRED") || used_unguarded(&text, name);
        w.cmake.push(want_at(name, path, !required, &text, at, Lang::CMake));
    }
    for call in ["pkg_check_modules", "pkg_search_module"] {
        for (at, args) in calls_at(&text, call, '(', ')') {
            let ws = words(&args);
            let required = ws.iter().any(|x| x == "REQUIRED");
            let list: Vec<String> = ws.iter().skip(1).filter(|x| !CMAKE_KEYWORDS.contains(&x.as_str())).cloned().collect();
            for m in modules(&list.join(" ")) {
                w.pkg_config.push(want_at(&m, path, !required, &text, at, Lang::CMake));
            }
            w.tools.push(want("pkg-config", path, false));
        }
    }
    for args in calls(&text, "find_program", '(', ')') {
        let ws = words(&args);
        let name = ws.iter().position(|x| x == "NAMES").and_then(|i| ws.get(i + 1)).or(ws.get(1));
        if let Some(n) = name.filter(|n| !n.contains('$') && !n.starts_with(|c: char| c.is_ascii_uppercase())) {
            w.tools.push(want(n, path, !ws.iter().any(|x| x == "REQUIRED")));
        }
    }
}

/// Whether CMake package `name`'s variables or targets are used without
/// an `if(<NAME>_FOUND)` (an optional one is looked at first).
fn used_unguarded(text: &str, name: &str) -> bool {
    let (upper, lower) = (name.to_uppercase(), name.to_lowercase());
    let used = [format!("${{{upper}_LIBRAR"), format!("${{{name}_LIBRAR"), format!("${{{upper}_INCLUDE"), format!("${{{name}_INCLUDE"), format!("{name}::")].iter().any(|u| text.contains(u.as_str()));
    let compact: String = text.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    let guarded = compact.contains(&format!("{lower}_found")) || compact.contains(&format!("if(target{lower}::"));
    used && !guarded
}

/// The quoted strings in a Meson call's arguments, before any `key:`.
fn quoted(args: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = args;
    loop {
        let colon = rest.find(':');
        let Some(q) = rest.find('\'') else { break };
        if colon.is_some_and(|c| c < q) {
            break;
        }
        let Some(end) = rest[q + 1..].find('\'') else { break };
        out.push(rest[q + 1..q + 1 + end].to_string());
        rest = &rest[q + 1 + end + 1..];
    }
    out
}

/// A Meson keyword argument's value, if it's a string (`version: '1.2'`).
fn keyword(args: &str, key: &str) -> Option<String> {
    let i = args.find(&format!("{key}:")).or_else(|| args.find(&format!("{key} :")))?;
    let rest = &args[i + key.len()..];
    let q = rest.find('\'')?;
    if rest[..q].contains(',') {
        return None;
    }
    let end = rest[q + 1..].find('\'')?;
    Some(rest[q + 1..q + 1 + end].to_string())
}

fn meson(text: &str, path: &str, w: &mut Wants) {
    let text: String = text.lines().map(|l| l.split('#').next().unwrap_or("")).collect::<Vec<_>>().join("\n");
    let optional = |args: &str| {
        let compact: String = args.chars().filter(|c| !c.is_whitespace()).collect();
        compact.contains("required:false") || compact.contains("required:get_option") || compact.contains("required:not") || compact.contains("required:opt")
    };
    for (at, args) in calls_at(&text, "dependency", '(', ')') {
        // `foreach name: ['lua-5.4', 'lua-5.3', 'lua']` / `dependency(name, ...)`:
        // one of them, the first the catalog has.
        let var = args.split([',', ')']).next().unwrap_or("").trim();
        if !var.is_empty() && var.chars().all(|c| c.is_alphanumeric() || c == '_') {
            if let Some(list) = foreach_list(&text, var) {
                w.pkg_config.push(want_at(&list.join("|"), path, false, &text, at, Lang::Meson));
            }
            continue;
        }
        let names = quoted(&args);
        let Some(name) = names.first() else { continue };
        if ["threads", "dl", "openmp", "m", "rt", "libm", "intl", "iconv", "bash-completion"].contains(&name.as_str()) {
            continue;
        }
        let name = if name == "curses" { "ncursesw" } else { name.as_str() };
        w.pkg_config.push(want_at(name, path, optional(&args), &text, at, Lang::Meson));
    }
    for args in calls(&text, "find_program", '(', ')') {
        if let Some(name) = quoted(&args).first() {
            w.tools.push(want(name, path, optional(&args)));
        }
    }
    for args in calls(&text, "find_library", '(', ')') {
        if let Some(name) = quoted(&args).first().filter(|n| !["m", "dl", "pthread", "rt", "c"].contains(&n.as_str())) {
            w.libraries.push(want(name, path, optional(&args)));
        }
    }
}

/// The names a Meson `foreach <var> : [...]` goes through.
fn foreach_list(text: &str, var: &str) -> Option<Vec<String>> {
    for args in text.split("foreach").skip(1) {
        let (head, rest) = args.split_once(':')?;
        if head.trim() != var {
            continue;
        }
        let list = rest.trim_start().strip_prefix('[')?;
        let list = &list[..list.find(']')?];
        let names: Vec<String> = list.split(',').map(|n| n.trim().trim_matches('\'').to_string()).filter(|n| !n.is_empty()).collect();
        return (!names.is_empty()).then_some(names);
    }
    None
}

/// An m4 call's arguments: `[a], [b c]` is a, b c.
fn m4_args(args: &str) -> Vec<String> {
    let mut out = Vec::new();
    let (mut depth, mut cur) = (0, String::new());
    for c in args.chars() {
        match c {
            '[' => {
                if depth > 0 {
                    cur.push(c);
                }
                depth += 1;
            }
            ']' => {
                depth -= 1;
                if depth > 0 {
                    cur.push(c);
                }
            }
            ',' if depth == 0 => out.push(std::mem::take(&mut cur).trim().to_string()),
            c => cur.push(c),
        }
    }
    out.push(cur.trim().to_string());
    out
}

/// configure.ac's; `regenerate`: there's no configure yet, so the tools
/// that make it (gettext's autopoint, bison...) are needed too.
fn autoconf(text: &str, path: &str, regenerate: bool, w: &mut Wants) {
    let text: String = text.lines().filter(|l| !l.trim_start().starts_with("dnl") && !l.trim_start().starts_with('#')).collect::<Vec<_>>().join("\n");
    for (at, args) in calls_at(&text, "PKG_CHECK_MODULES", '(', ')') {
        let a = m4_args(&args);
        let optional = a.len() >= 4 && !a[3].contains("AC_MSG_ERROR");
        for m in a.get(1).map(|l| modules(l)).unwrap_or_default() {
            w.pkg_config.push(want_at(&m, path, optional, &text, at, Lang::Autoconf));
        }
        w.tools.push(want("pkg-config", path, false));
    }
    for (at, args) in calls_at(&text, "AC_CHECK_LIB", '(', ')') {
        let a = m4_args(&args);
        let required = a.get(3).is_some_and(|x| x.contains("AC_MSG_ERROR"));
        if let Some(lib) = a.first().filter(|l| !l.is_empty() && !l.contains('$') && !["m", "dl", "pthread", "rt", "c"].contains(&l.as_str())) {
            w.libraries.push(want_at(lib, path, !required, &text, at, Lang::Autoconf));
        }
    }
    for (at, args) in calls_at(&text, "AC_SEARCH_LIBS", '(', ')') {
        let a = m4_args(&args);
        let required = a.get(3).is_some_and(|x| x.contains("AC_MSG_ERROR"));
        for lib in a.get(1).map(|l| l.split_whitespace().map(String::from).collect::<Vec<_>>()).unwrap_or_default() {
            if !lib.contains('$') && !["m", "dl", "pthread", "rt", "c", "socket", "nsl", "resolv"].contains(&lib.as_str()) {
                w.libraries.push(want_at(&lib, path, !required, &text, at, Lang::Autoconf));
            }
        }
    }
    for args in calls(&text, "AC_CHECK_HEADERS", '(', ')').into_iter().chain(calls(&text, "AC_CHECK_HEADER", '(', ')')) {
        let a = m4_args(&args);
        let required = a.get(2).is_some_and(|x| x.contains("AC_MSG_ERROR"));
        for h in a.first().map(|l| l.split_whitespace().map(String::from).collect::<Vec<_>>()).unwrap_or_default() {
            w.headers.push(want(&h, path, !required));
        }
    }
    // Curses is often looked for with shell code of the project's own
    // (htop's list of names): it's ncurses.
    let curses = w.pkg_config.iter().chain(&w.libraries).any(|x| x.name.contains("curses"));
    if !curses && text.contains("curses") && (text.contains("AC_MSG_ERROR") || text.contains("required")) {
        w.pkg_config.push(want("ncursesw", path, false));
    }
    for (mac, tool) in [("AM_GNU_GETTEXT", "gettext"), ("AC_PROG_YACC", "bison"), ("AC_PROG_LEX", "flex"), ("AM_PROG_LEX", "flex"), ("PKG_PROG_PKG_CONFIG", "pkg-config")] {
        if text.contains(mac) && (regenerate || tool == "pkg-config") {
            w.tools.push(want(tool, path, false));
        }
    }
}

fn makefile(text: &str, path: &str, w: &mut Wants) {
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("");
        let mut rest = line;
        while let Some(i) = rest.find("pkg-config") {
            rest = &rest[i + "pkg-config".len()..];
            let end = rest.find([')', '`', ';', '|']).unwrap_or(rest.len());
            let mods: Vec<String> = rest[..end].split_whitespace().filter(|w| !w.starts_with('-') && !w.contains('$')).map(String::from).collect();
            for m in modules(&mods.join(" ")) {
                w.pkg_config.push(want(&m, path, false));
            }
            if !mods.is_empty() {
                w.tools.push(want("pkg-config", path, false));
            }
        }
        let is_link = ["LDLIBS", "LIBS", "LDFLAGS"].iter().any(|v| line.trim_start().starts_with(v));
        if is_link {
            for word in line.split_whitespace() {
                if let Some(lib) = word.strip_prefix("-l").filter(|l| !l.is_empty() && !l.contains('$') && !["m", "c", "pthread", "dl", "rt"].contains(l)) {
                    w.libraries.push(want(lib, path, false));
                }
            }
        }
    }
    // Installs to /usr/local unless told: into the package.
    for var in ["PREFIX", "prefix"] {
        let set = text.lines().any(|l| l.strip_prefix(var).is_some_and(|r| r.trim_start().starts_with(['=', '?', ':'])));
        if set {
            w.make_flags.push(format!("{var}=$(out)"));
            break;
        }
    }
}

fn cargo_lock(text: &str, w: &mut Wants) {
    let Ok(v) = toml::from_str::<toml::Value>(text) else { return };
    let names: BTreeSet<String> = v.get("package").and_then(|p| p.as_array()).into_iter().flatten().filter_map(|p| p.get("name")?.as_str().map(String::from)).collect();
    let table = &super::names::build_names().crates;
    for n in names {
        if table.contains_key(&n) || n.ends_with("-sys") || n.ends_with("_sys") {
            w.crates.push(want(&n, "Cargo.lock", false));
        }
    }
}

fn cargo_meta(tree: &Tree, w: &mut Wants) {
    let Some(t) = tree.text("Cargo.toml").and_then(|t| toml::from_str::<toml::Value>(t).ok()) else { return };
    let package = t.get("package").or_else(|| t.get("workspace").and_then(|ws| ws.get("package")));
    let s = |k: &str| package.and_then(|p| p.get(k)).and_then(|v| v.as_str()).map(String::from);
    w.name = t.get("package").and_then(|p| p.get("name")).and_then(|v| v.as_str()).map(String::from);
    w.version = s("version");
    w.description = s("description");
    w.homepage = s("homepage").or_else(|| s("repository"));
}

/// pyproject.toml, read once.
fn pyproject(tree: &Tree) -> Option<toml::Value> {
    tree.text("pyproject.toml").and_then(|t| toml::from_str(t).ok())
}

/// `requires-python`, if the project says.
pub fn requires_python(tree: &Tree) -> Option<String> {
    let t = pyproject(tree)?;
    let p = t.get("project")?.get("requires-python")?.as_str()?;
    Some(p.to_string())
}

/// What the project builds with (its PEP 517 backend and the rest of
/// [build-system].requires), by distribution name.
pub fn python_backend(tree: &Tree) -> Vec<String> {
    let requires: Option<Vec<String>> = pyproject(tree).and_then(|t| Some(t.get("build-system")?.get("requires")?.as_array()?.iter().filter_map(|r| r.as_str().map(String::from)).collect()));
    match requires {
        Some(list) => list.iter().filter(|r| marker_holds(r)).map(|r| requirement_name(r)).filter(|n| !n.is_empty()).collect(),
        // Without one, pip builds with setuptools (PEP 517's default).
        None => vec!["setuptools".into(), "wheel".into()],
    }
}

/// A requirement's distribution name: `click>=8.0; python_version > "3.8"`
/// and `rich[jupyter]` are click and rich.
fn requirement_name(req: &str) -> String {
    req.trim().split(|c: char| "<>=!~;[( @".contains(c)).next().unwrap_or("").trim().to_string()
}

/// Whether a requirement is for a Linux computer with the catalog's
/// Python (3.14): `tomli; python_version < "3.11"` and
/// `colorama; sys_platform == "win32"` aren't. Markers it can't read hold.
pub(crate) fn marker_holds(req: &str) -> bool {
    let Some((_, marker)) = req.split_once(';') else { return true };
    let clause_holds = |c: &str| -> bool {
        let c = c.trim().trim_matches(['(', ')']).trim();
        for op in ["==", "!=", "<=", ">=", "<", ">"] {
            let Some((var, val)) = c.split_once(op) else { continue };
            let (var, val) = (var.trim(), val.trim().trim_matches(['"', '\'']));
            let have = match var {
                "sys_platform" => "linux",
                "platform_system" => "Linux",
                "os_name" => "posix",
                "implementation_name" => "cpython",
                "platform_python_implementation" => "CPython",
                "python_version" | "python_full_version" => {
                    let cmp = crate::recipe::version_cmp("3.14", val);
                    return match op {
                        "==" => cmp.is_eq(),
                        "!=" => !cmp.is_eq(),
                        "<=" => cmp.is_le(),
                        ">=" => cmp.is_ge(),
                        "<" => cmp.is_lt(),
                        _ => cmp.is_gt(),
                    };
                }
                "extra" => return false,
                _ => return true,
            };
            return match op {
                "==" => have == val,
                "!=" => have != val,
                _ => true,
            };
        }
        true
    };
    // `or` of `and`s, without brackets inside each.
    marker.split(" or ").any(|any| any.split(" and ").all(clause_holds))
}

fn python(tree: &Tree, w: &mut Wants) {
    let mut deps: Vec<(String, String)> = Vec::new();
    if let Some(t) = pyproject(tree) {
        let project = t.get("project");
        let s = |k: &str| project.and_then(|p| p.get(k)).and_then(|v| v.as_str()).map(String::from);
        w.name = s("name");
        w.version = s("version");
        w.description = s("description");
        w.homepage = project.and_then(|p| p.get("urls")).and_then(|u| u.as_table()).and_then(|u| ["Homepage", "homepage", "Source", "Repository"].iter().find_map(|k| u.get(*k)?.as_str().map(String::from)));
        for d in project.and_then(|p| p.get("dependencies")).and_then(|d| d.as_array()).into_iter().flatten().filter_map(|d| d.as_str()) {
            deps.push((d.to_string(), "pyproject.toml".into()));
        }
        if project.and_then(|p| p.get("dynamic")).and_then(|d| d.as_array()).is_some_and(|d| d.iter().any(|x| x.as_str() == Some("dependencies"))) {
            w.notes.push("pyproject.toml says its dependencies are worked out when it's built: check deps".into());
        }
        // Poetry's own table.
        let poetry = t.get("tool").and_then(|t| t.get("poetry"));
        for (name, _) in poetry.and_then(|p| p.get("dependencies")).and_then(|d| d.as_table()).into_iter().flatten() {
            if name != "python" {
                deps.push((name.clone(), "pyproject.toml [tool.poetry]".into()));
            }
        }
        if w.name.is_none() {
            w.name = poetry.and_then(|p| p.get("name")).and_then(|v| v.as_str()).map(String::from);
            w.version = poetry.and_then(|p| p.get("version")).and_then(|v| v.as_str()).map(String::from);
            w.description = poetry.and_then(|p| p.get("description")).and_then(|v| v.as_str()).map(String::from);
        }
    }
    if let Some(cfg) = tree.text("setup.cfg") {
        let mut in_requires = false;
        for line in cfg.lines() {
            if line.trim_start().starts_with("install_requires") {
                in_requires = true;
                if let Some((_, v)) = line.split_once('=') {
                    deps.extend(v.split(',').map(|d| d.trim()).filter(|d| !d.is_empty()).map(|d| (d.to_string(), "setup.cfg".to_string())));
                }
                continue;
            }
            if in_requires {
                if line.starts_with([' ', '\t']) && !line.trim().is_empty() {
                    deps.push((line.trim().to_string(), "setup.cfg".into()));
                } else if !line.trim().is_empty() {
                    in_requires = false;
                }
            }
            if w.name.is_none() {
                if let Some(v) = line.strip_prefix("name").and_then(|r| r.trim_start().strip_prefix('=')) {
                    w.name = Some(v.trim().to_string());
                }
            }
        }
    }
    if let Some(py) = tree.text("setup.py") {
        if let Some(i) = py.find("install_requires") {
            let rest = &py[i..];
            if let (Some(a), Some(b)) = (rest.find('['), rest.find(']')) {
                if a < b {
                    for item in rest[a + 1..b].split(',') {
                        let item = item.trim().trim_matches(['"', '\'']);
                        if !item.is_empty() && !item.contains('(') {
                            deps.push((item.to_string(), "setup.py".into()));
                        }
                    }
                }
            }
        }
        if w.name.is_none() {
            w.name = py.split("name=").nth(1).and_then(|r| r.trim_start().strip_prefix(['"', '\''])).and_then(|r| r.split(['"', '\'']).next()).map(String::from);
        }
    }
    // requirements.txt, when the project's own files read it for its dependencies.
    let reads_requirements = tree.text("setup.py").is_some_and(|t| t.contains("requirements.txt"))
        || tree.text("setup.cfg").is_some_and(|t| t.contains("requirements.txt"))
        || tree.text("pyproject.toml").is_some_and(|t| t.contains("requirements.txt"));
    if deps.is_empty() && reads_requirements {
        if let Some(req) = tree.text("requirements.txt") {
            deps.extend(req.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with(['#', '-'])).map(|l| (l.to_string(), "requirements.txt".to_string())));
        }
    }
    for (req, from) in deps {
        if marker_holds(&req) {
            let name = requirement_name(&req);
            if !name.is_empty() {
                w.python.push(want(&name, &from, false));
            }
        }
    }
}

fn node(tree: &Tree, w: &mut Wants) {
    let Some(p) = tree.text("package.json").and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok()) else { return };
    let s = |k: &str| p[k].as_str().map(String::from);
    w.name = s("name").map(|n| n.rsplit('/').next().unwrap_or(&n).to_string());
    w.version = s("version");
    w.description = s("description");
    w.homepage = s("homepage");
    let lock = ["package-lock.json", "npm-shrinkwrap.json"].into_iter().find(|f| tree.has(f));
    if lock.is_none() {
        let other = ["yarn.lock", "pnpm-lock.yaml", "bun.lock", "bun.lockb"].into_iter().find(|f| tree.has(f));
        w.notes.push(match other {
            Some(f) => format!("NOT FOUND: package-lock.json. It pins its packages with {f}, and nami builds Node.js projects from npm's package-lock.json so far"),
            None => "NOT FOUND: package-lock.json. Without it its packages aren't pinned, and nami can't fetch them for the build".into(),
        });
    }
    if p["bin"].is_null() {
        w.notes.push("package.json names no program (`bin`): it's built and installed in lib/node_modules, with nothing in bin".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(files: &[(&str, &str)]) -> Tree {
        let mut t = Tree::default();
        for (p, text) in files {
            t.paths.insert(p.to_string());
            t.files.insert(p.to_string(), text.as_bytes().to_vec());
        }
        t
    }

    fn names(list: &[Want]) -> Vec<(&str, bool)> {
        list.iter().map(|w| (w.name.as_str(), w.optional)).collect()
    }

    #[test]
    fn reads_cmake() {
        let t = tree(&[(
            "CMakeLists.txt",
            "cmake_minimum_required(VERSION 3.10)\nproject(cmark VERSION 0.31.1 LANGUAGES C CXX)\n# find_package(Ignored)\nfind_package(ZLIB REQUIRED)\nfind_package(Doxygen)\nfind_package(PkgConfig REQUIRED)\npkg_check_modules(GLIB REQUIRED IMPORTED_TARGET glib-2.0>=2.40 gio-2.0)\nfind_package(${DEP} REQUIRED)\n",
        )]);
        let w = read(&t);
        assert_eq!(w.kind, Some(Kind::CMake));
        assert_eq!((w.name.as_deref(), w.version.as_deref()), (Some("cmark"), Some("0.31.1")));
        assert_eq!(names(&w.cmake), [("ZLIB", false), ("Doxygen", true)]);
        assert_eq!(names(&w.pkg_config), [("glib-2.0", false), ("gio-2.0", false)]);
        assert_eq!(names(&w.tools), [("pkg-config", false)]);
        let t = tree(&[("CMakeLists.txt", "find_package(Curses)\ntarget_link_libraries(cmatrix ${CURSES_LIBRARIES})\nfind_package(X11)\nif(X11_FOUND)\n  target_link_libraries(a ${X11_LIBRARIES})\nendif()\n")]);
        assert_eq!(names(&read(&t).cmake), [("Curses", false), ("X11", true)]);
    }

    #[test]
    fn reads_meson() {
        let t = tree(&[(
            "meson.build",
            "project('tio', 'c', version : '3.9', license : 'GPL-2.0-or-later')\nglib = dependency('glib-2.0', version: '>=2.0')\nlua = dependency('lua', required: false)\nthreads = dependency('threads')\n# dependency('commented')\nsd = dependency('libsystemd', required: get_option('systemd'))\nscdoc = find_program('scdoc', required: false)\n",
        )]);
        let w = read(&t);
        assert_eq!(w.kind, Some(Kind::Meson));
        assert_eq!((w.name.as_deref(), w.version.as_deref()), (Some("tio"), Some("3.9")));
        assert_eq!(names(&w.pkg_config), [("glib-2.0", false), ("lua", true), ("libsystemd", true)]);
        let t = tree(&[("src/meson.build", "foreach name: ['lua-5.4', 'lua-5.3', 'lua']\n  lua_dep = dependency(name, version: '>=5.1', required: false)\nendforeach\n")]);
        assert_eq!(names(&read(&t).pkg_config), [("lua-5.4|lua-5.3|lua", false)]);
        assert_eq!(names(&w.tools), [("scdoc", true)]);
    }

    #[test]
    fn reads_autoconf_and_makefiles() {
        let t = tree(&[(
            "configure.ac",
            "AC_INIT([htop],[3.4.1])\ndnl PKG_CHECK_MODULES([NO], [commented])\nPKG_CHECK_MODULES([NCURSES], [ncursesw >= 6], [], [AC_MSG_ERROR([no ncurses])])\nPKG_CHECK_MODULES([NL], [libnl-3.0 libnl-genl-3.0], [have_nl=yes], [have_nl=no])\nAC_CHECK_LIB([z], [inflate], [], [AC_MSG_ERROR([zlib needed])])\nAC_CHECK_LIB([m], [sqrt])\nAC_CHECK_HEADERS([sys/capability.h])\nAM_GNU_GETTEXT([external])\n",
        )]);
        let w = read(&t);
        assert_eq!(w.kind, Some(Kind::Autotools));
        assert_eq!(names(&w.pkg_config), [("ncursesw", false), ("libnl-3.0", true), ("libnl-genl-3.0", true)]);
        assert_eq!(names(&w.libraries), [("z", false)]);
        assert_eq!(names(&w.headers), [("sys/capability.h", true)]);
        assert_eq!(names(&w.tools), [("pkg-config", false), ("gettext", false)]);
        // On a branch: needed when it's taken.
        let ac = "AC_ARG_ENABLE([sensors], [AS_HELP_STRING([--enable-sensors])], [], [enable_sensors=no])\nif test \"x$enable_sensors\" = xyes; then\n  AC_CHECK_LIB([sensors], [sensors_init], [], [AC_MSG_ERROR([no libsensors])])\nfi\nAS_IF([test \"x$enable_pcp\" = xyes], [AC_CHECK_LIB([pcp], [pmLookupName], [], [AC_MSG_ERROR([no pcp])])])\nAC_CHECK_LIB([z], [inflate], [], [AC_MSG_ERROR([no zlib])])\n";
        let w = read(&tree(&[("configure.ac", ac)]));
        assert_eq!(w.libraries.iter().map(|l| (l.name.as_str(), l.conditional)).collect::<Vec<_>>(), [("sensors", true), ("pcp", true), ("z", false)]);

        let t = tree(&[("Makefile", "PREFIX ?= /usr/local\nCFLAGS += $(shell pkg-config --cflags libcurl)\nLDLIBS = -lm -lcrypto $(shell pkg-config --libs libcurl)\n"), ("main.c", "")]);
        let w = read(&t);
        assert_eq!(w.kind, Some(Kind::Make));
        assert_eq!(names(&w.pkg_config), [("libcurl", false)]);
        assert_eq!(names(&w.libraries), [("crypto", false)]);
        assert_eq!(w.make_flags, ["PREFIX=$(out)"]);
    }

    #[test]
    fn reads_rust_go_python_and_node() {
        let t = tree(&[
            ("Cargo.toml", "[package]\nname = \"ripgrep\"\nversion = \"14.1.1\"\ndescription = \"rg\"\n"),
            ("Cargo.lock", "[[package]]\nname = \"pcre2-sys\"\nversion = \"0.2.9\"\n[[package]]\nname = \"windows-sys\"\nversion = \"0.52.0\"\n[[package]]\nname = \"memchr\"\nversion = \"2.7.4\"\n"),
        ]);
        let w = read(&t);
        assert_eq!((w.kind, w.name.as_deref(), w.version.as_deref()), (Some(Kind::Cargo), Some("ripgrep"), Some("14.1.1")));
        assert_eq!(names(&w.crates), [("pcre2-sys", false), ("windows-sys", false)]);

        let w = read(&tree(&[("go.mod", "module mvdan.cc/sh/v3\n\ngo 1.22\n")]));
        assert_eq!((w.kind, w.name.as_deref()), (Some(Kind::Go), Some("sh")));

        let t = tree(&[(
            "pyproject.toml",
            "[build-system]\nrequires = [\"hatchling>=1.27\", \"hatch-vcs\", \"tomli; python_version < '3.11'\"]\nbuild-backend = \"hatchling.build\"\n\n[project]\nname = \"black\"\ndescription = \"The formatter\"\nrequires-python = \">=3.9\"\ndependencies = [\"click>=8.0.0\", \"tomli>=1.1.0; python_version < '3.11'\", \"colorama; sys_platform == 'win32'\", \"PyYAML\", \"rich[jupyter] (>=10)\"]\n",
        )]);
        let w = read(&t);
        assert_eq!((w.kind, w.name.as_deref()), (Some(Kind::Python), Some("black")));
        assert_eq!(names(&w.python), [("click", false), ("PyYAML", false), ("rich", false)]);
        assert_eq!(python_backend(&t), ["hatchling", "hatch-vcs"]);
        assert_eq!(requires_python(&t).as_deref(), Some(">=3.9"));
        assert_eq!(python_backend(&tree(&[("setup.py", "")])), ["setuptools", "wheel"]);
        assert!(marker_holds("a; python_version >= '3.8' and sys_platform != 'win32'"));
        assert!(!marker_holds("a; python_version < '3.8' or sys_platform == 'darwin'"));

        let t = tree(&[("package.json", r#"{"name": "@scope/http-server", "version": "14.1.1", "bin": {"http-server": "./bin/http-server"}}"#), ("yarn.lock", "")]);
        let w = read(&t);
        assert_eq!((w.kind, w.name.as_deref()), (Some(Kind::Node), Some("http-server")));
        assert!(w.notes[0].contains("NOT FOUND: package-lock.json. It pins its packages with yarn.lock"), "{:?}", w.notes);
    }
}
