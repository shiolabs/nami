//! Vim's keys for the editor: Normal, Insert and Visual (v, V) modes.
//! Moving: h j k l w b e 0 ^ $ gg G, Enter, with counts (3j). Editing: i a
//! I A o O, x X, d c y with a motion (dw, ce, y$...) or doubled (dd, cc,
//! yy), D C Y, p P, r, J, u (ctrl-r redoes). :w and :wq save.

use gpui::Context;

use crate::editor::{Editor, SaveAsked};

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
    Visual,
    VisualLine,
}

#[derive(Default)]
pub struct State {
    pub mode: Mode,
    /// What's typed of a command so far (a count, an operator, g, r).
    pub pending: String,
    /// A : line being typed.
    pub command: Option<String>,
    /// What d, c and y took, and whether it's whole lines.
    pub register: String,
    pub linewise: bool,
    /// Visual mode: where it started, and where the cursor is.
    pub anchor: usize,
    pub cursor: usize,
}

impl State {
    /// The line under the editor: the mode, or the : line being typed.
    pub fn line(&self) -> String {
        if let Some(cmd) = &self.command {
            return format!(":{cmd}");
        }
        let mode = match self.mode {
            Mode::Normal => "NORMAL",
            Mode::Insert => "-- INSERT --",
            Mode::Visual => "-- VISUAL --",
            Mode::VisualLine => "-- VISUAL LINE --",
        };
        format!("{mode}  {}", self.pending)
    }
}

/// Which run a character is in, for w, b and e.
fn class(c: char) -> u8 {
    if c.is_whitespace() {
        0
    } else if c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

/// Where a motion goes, and how an operator takes it.
struct Motion {
    to: usize,
    linewise: bool,
    inclusive: bool,
}

impl Editor {
    fn at(&self) -> usize {
        if matches!(self.vi.mode, Mode::Visual | Mode::VisualLine) { self.vi.cursor } else { self.cursor_offset() }
    }

    fn char_at(&self, i: usize) -> Option<char> {
        self.content[i..].chars().next()
    }

    fn first_blank(&self, start: usize) -> usize {
        let end = self.line_end(start);
        self.content[start..end].find(|c: char| c != ' ' && c != '\t').map_or(end, |i| start + i)
    }

    /// Normal mode keeps off a line's end (unless the line is empty).
    fn clamp(&self, i: usize) -> usize {
        let (_, start) = self.line_of(i);
        let end = self.line_end(start);
        if i >= end && end > start { self.previous_boundary(end) } else { i }
    }

    fn put_cursor(&mut self, i: usize, cx: &mut Context<Self>) {
        match self.vi.mode {
            Mode::Visual | Mode::VisualLine => {
                self.vi.cursor = i;
                self.show_visual(cx);
            }
            Mode::Insert => self.move_to(i, cx),
            Mode::Normal => {
                let i = self.clamp(i);
                self.move_to(i, cx)
            }
        }
    }

    /// The selection Visual mode shows: the characters (or lines) between
    /// where it started and the cursor.
    fn visual_range(&self) -> std::ops::Range<usize> {
        let (a, b) = (self.vi.anchor.min(self.vi.cursor), self.vi.anchor.max(self.vi.cursor));
        if self.vi.mode == Mode::VisualLine {
            let (_, s) = self.line_of(a);
            let (_, e) = self.line_of(b);
            s..(self.line_end(e) + 1).min(self.content.len())
        } else {
            a..self.next_boundary(b).min(self.content.len().max(b))
        }
    }

    fn show_visual(&mut self, cx: &mut Context<Self>) {
        let r = self.visual_range();
        self.move_to(r.start, cx);
        self.select_to(r.end, cx);
    }

    fn motion(&self, c: char, count: usize) -> Option<Motion> {
        let mut i = self.at();
        let starts = self.starts();
        let (line, start) = self.line_of(i);
        let exclusive = |to| Some(Motion { to, linewise: false, inclusive: false });
        match c {
            'h' => {
                for _ in 0..count {
                    let (_, s) = self.line_of(i);
                    if i > s {
                        i = self.previous_boundary(i);
                    }
                }
                exclusive(i)
            }
            'l' | ' ' => {
                for _ in 0..count {
                    let (_, s) = self.line_of(i);
                    if i < self.line_end(s) {
                        i = self.next_boundary(i);
                    }
                }
                exclusive(i)
            }
            'j' | 'k' | '\n' | '+' | '-' => {
                let target = if matches!(c, 'k' | '-') { line.saturating_sub(count) } else { (line + count).min(starts.len() - 1) };
                let s = starts[target];
                let to = if matches!(c, '\n' | '+' | '-') {
                    self.first_blank(s)
                } else {
                    // The same column, as far as the line goes.
                    let col = self.content[start..i].chars().count();
                    self.content[s..self.line_end(s)].char_indices().nth(col).map_or(self.line_end(s), |(o, _)| s + o)
                };
                Some(Motion { to, linewise: true, inclusive: false })
            }
            '0' => exclusive(start),
            '^' => exclusive(self.first_blank(start)),
            '$' => {
                let mut s = start;
                for _ in 1..count {
                    s = starts.get(self.line_of(s).0 + 1).copied().unwrap_or(s);
                }
                Some(Motion { to: self.line_end(s), linewise: false, inclusive: false })
            }
            'w' => {
                for _ in 0..count {
                    if let Some(k) = self.char_at(i).map(class).filter(|k| *k != 0) {
                        while self.char_at(i).is_some_and(|c| class(c) == k) {
                            i = self.next_boundary(i);
                        }
                    }
                    while self.char_at(i).is_some_and(char::is_whitespace) {
                        i = self.next_boundary(i);
                    }
                }
                exclusive(i)
            }
            'e' => {
                for _ in 0..count {
                    i = self.next_boundary(i);
                    while self.char_at(i).is_some_and(char::is_whitespace) {
                        i = self.next_boundary(i);
                    }
                    if let Some(k) = self.char_at(i).map(class) {
                        while self.char_at(self.next_boundary(i)).is_some_and(|c| class(c) == k) && self.next_boundary(i) < self.content.len() {
                            i = self.next_boundary(i);
                        }
                    }
                }
                Some(Motion { to: i.min(self.content.len()), linewise: false, inclusive: true })
            }
            'b' => {
                for _ in 0..count {
                    i = self.previous_boundary(i);
                    while i > 0 && self.char_at(i).is_some_and(char::is_whitespace) {
                        i = self.previous_boundary(i);
                    }
                    if let Some(k) = self.char_at(i).map(class) {
                        while i > 0 && self.char_at(self.previous_boundary(i)).is_some_and(|c| class(c) == k) {
                            i = self.previous_boundary(i);
                        }
                    }
                }
                exclusive(i)
            }
            'G' => {
                let target = if self.vi.pending.chars().any(|c| c.is_ascii_digit()) { (count - 1).min(starts.len() - 1) } else { starts.len() - 1 };
                Some(Motion { to: self.first_blank(starts[target]), linewise: true, inclusive: false })
            }
            _ => None,
        }
    }

    /// The text an operator takes, from the cursor to where a motion goes.
    fn span(&self, m: &Motion) -> std::ops::Range<usize> {
        let at = self.at();
        let (a, b) = (at.min(m.to), at.max(m.to));
        if m.linewise {
            let (_, s) = self.line_of(a);
            let (_, e) = self.line_of(b);
            let end = self.line_end(e);
            if end < self.content.len() {
                s..end + 1
            } else {
                // The last line: the line break before it goes instead.
                s.saturating_sub(if s > 0 { 1 } else { 0 })..end
            }
        } else if m.inclusive {
            a..self.next_boundary(b)
        } else {
            a..b
        }
    }

    /// d, c or y over `range` (`linewise`: whole lines).
    fn operate(&mut self, op: char, range: std::ops::Range<usize>, linewise: bool, cx: &mut Context<Self>) {
        let mut text = self.content[range.clone()].to_string();
        if linewise && !text.ends_with('\n') {
            text = text.trim_start_matches('\n').to_string() + "\n";
        }
        self.vi.register = text;
        self.vi.linewise = linewise;
        match op {
            'y' => {
                self.vi.mode = Mode::Normal;
                self.put_cursor(range.start, cx)
            }
            'd' => {
                self.vi.mode = Mode::Normal;
                self.splice(range.clone(), "", None, cx);
                let at = if linewise { self.first_blank(self.line_of(range.start.min(self.content.len())).1) } else { range.start };
                self.put_cursor(at, cx);
            }
            'c' => {
                // Whole lines: their indent stays, and the cursor goes after it.
                let (range, keep) = if linewise {
                    let r = range.start + if range.start > 0 && self.content.as_bytes()[range.start] == b'\n' { 1 } else { 0 }..range.end;
                    let indent: String = self.content[r.clone()].chars().take_while(|c| *c == ' ').collect();
                    let end = if self.content[r.clone()].ends_with('\n') { r.end - 1 } else { r.end };
                    (r.start..end, indent)
                } else {
                    (range, String::new())
                };
                self.splice(range.clone(), &keep, None, cx);
                self.vi.mode = Mode::Insert;
                self.move_to(range.start + keep.len(), cx);
            }
            _ => {}
        }
    }

    pub(crate) fn vim_escape(&mut self, cx: &mut Context<Self>) {
        let was = self.vi.mode;
        self.vi.mode = Mode::Normal;
        self.vi.pending.clear();
        self.vi.command = None;
        let at = match was {
            Mode::Visual | Mode::VisualLine => self.vi.cursor,
            Mode::Insert => {
                let i = self.cursor_offset();
                let (_, s) = self.line_of(i);
                if i > s { self.previous_boundary(i) } else { i }
            }
            Mode::Normal => self.cursor_offset(),
        };
        self.put_cursor(at, cx);
        cx.notify();
    }

    fn insert_at(&mut self, i: usize, cx: &mut Context<Self>) {
        self.vi.mode = Mode::Insert;
        self.vi.pending.clear();
        self.move_to(i, cx);
    }

    /// A key typed in Normal or Visual mode.
    pub(crate) fn vim_key(&mut self, c: char, cx: &mut Context<Self>) {
        if let Some(cmd) = &mut self.vi.command {
            if c == '\u{8}' {
                if cmd.pop().is_none() {
                    self.vi.command = None;
                }
            } else if c == '\n' {
                let cmd = self.vi.command.take().unwrap_or_default();
                if matches!(cmd.trim(), "w" | "wq" | "x" | "write") {
                    cx.emit(SaveAsked);
                }
            } else {
                cmd.push(c);
            }
            return cx.notify();
        }
        // Backspace moves left, as h does.
        let c = if c == '\u{8}' { 'h' } else { c };
        // A count: digits first (0 alone is a motion).
        if c.is_ascii_digit() && (c != '0' || self.vi.pending.chars().last().is_some_and(|p| p.is_ascii_digit())) {
            self.vi.pending.push(c);
            return cx.notify();
        }
        let digits: String = self.vi.pending.chars().filter(|c| c.is_ascii_digit()).collect();
        let count = digits.parse::<usize>().unwrap_or(1).max(1);
        let pending: String = self.vi.pending.chars().filter(|c| !c.is_ascii_digit()).collect();
        let visual = matches!(self.vi.mode, Mode::Visual | Mode::VisualLine);

        // r{c}: the character under the cursor becomes c.
        if pending == "r" {
            self.vi.pending.clear();
            let i = self.at();
            if self.char_at(i).is_some_and(|ch| ch != '\n') && c != '\n' {
                self.splice(i..self.next_boundary(i), &c.to_string(), Some(i), cx);
            }
            return cx.notify();
        }
        if pending == "g" {
            self.vi.pending.clear();
            if c == 'g' {
                let to = self.first_blank(0);
                self.put_cursor(to, cx);
            }
            return cx.notify();
        }
        // An operator waiting for its motion: dw, c$, yy...
        if let Some(op) = pending.chars().next().filter(|o| matches!(o, 'd' | 'c' | 'y')) {
            self.vi.pending.clear();
            if c == op {
                // dd, cc, yy: whole lines.
                let m = self.motion('j', count - 1).unwrap_or(Motion { to: self.at(), linewise: true, inclusive: false });
                let m = Motion { linewise: true, ..m };
                let r = self.span(&m);
                return self.operate(op, r, true, cx);
            }
            // cw changes to the word's end, as vim does.
            let c = if op == 'c' && c == 'w' && self.char_at(self.at()).is_some_and(|ch| !ch.is_whitespace()) { 'e' } else { c };
            if c == 'g' {
                self.vi.pending = format!("{op}g");
                return cx.notify();
            }
            if let Some(m) = self.motion(c, count) {
                let r = self.span(&m);
                self.operate(op, r, m.linewise, cx);
            }
            return cx.notify();
        }
        if pending == "dg" || pending == "cg" || pending == "yg" {
            self.vi.pending.clear();
            if c == 'g' {
                let m = Motion { to: 0, linewise: true, inclusive: false };
                let r = self.span(&m);
                self.operate(pending.chars().next().unwrap(), r, true, cx);
            }
            return cx.notify();
        }

        if let Some(m) = self.motion(c, count) {
            self.vi.pending.clear();
            return self.put_cursor(m.to, cx);
        }

        self.vi.pending.clear();
        let i = self.at();
        let (_, start) = self.line_of(i);
        let end = self.line_end(start);
        if visual {
            match c {
                'd' | 'x' | 'y' | 'c' => {
                    let r = self.visual_range();
                    let line = self.vi.mode == Mode::VisualLine;
                    let op = if c == 'x' { 'd' } else { c };
                    self.vi.mode = Mode::Normal;
                    self.move_to(r.start, cx);
                    return self.operate(op, r, line, cx);
                }
                'v' | 'V' => {
                    let want = if c == 'v' { Mode::Visual } else { Mode::VisualLine };
                    if self.vi.mode == want {
                        return self.vim_escape(cx);
                    }
                    self.vi.mode = want;
                    return self.show_visual(cx);
                }
                _ => return cx.notify(),
            }
        }
        match c {
            'i' => self.insert_at(i, cx),
            'a' => self.insert_at(if i < end { self.next_boundary(i) } else { i }, cx),
            'I' => self.insert_at(self.first_blank(start), cx),
            'A' => self.insert_at(end, cx),
            'o' | 'O' => {
                let indent: String = self.content[start..end].chars().take_while(|c| *c == ' ').collect();
                let (at, text) = if c == 'o' { (end, format!("\n{indent}")) } else { (start, format!("{indent}\n")) };
                let cursor = if c == 'o' { at + text.len() } else { at + indent.len() };
                self.splice(at..at, &text, Some(cursor), cx);
                self.vi.mode = Mode::Insert;
                self.move_to(cursor, cx);
            }
            'x' | 'X' => {
                let m = self.motion(if c == 'x' { 'l' } else { 'h' }, count).unwrap();
                if m.to != i {
                    let r = self.span(&m);
                    self.operate('d', r, false, cx);
                }
            }
            'D' | 'C' | 'Y' => {
                if c == 'Y' {
                    let m = Motion { to: i, linewise: true, inclusive: false };
                    let r = self.span(&m);
                    return self.operate('y', r, true, cx);
                }
                self.operate(if c == 'D' { 'd' } else { 'c' }, i..end, false, cx);
            }
            'p' | 'P' => {
                let text = self.vi.register.repeat(count);
                if text.is_empty() {
                    return;
                }
                if self.vi.linewise {
                    let at = if c == 'p' { (end + 1).min(self.content.len()) } else { start };
                    let (at, text) = if c == 'p' && end == self.content.len() { (end, format!("\n{}", text.trim_end_matches('\n'))) } else { (at, text) };
                    let line_at = if text.starts_with('\n') { at + 1 } else { at };
                    self.splice(at..at, &text, Some(line_at), cx);
                    let to = self.first_blank(line_at);
                    self.put_cursor(to, cx);
                } else {
                    let at = if c == 'p' && i < end { self.next_boundary(i) } else { i };
                    self.splice(at..at, &text, None, cx);
                    let to = self.previous_boundary(at + text.len());
                    self.put_cursor(to, cx);
                }
            }
            'J' => {
                if end < self.content.len() {
                    let next_start = end + 1;
                    let blank = self.first_blank(next_start);
                    self.splice(end..blank, " ", Some(end), cx);
                }
            }
            'u' => {
                for _ in 0..count {
                    self.undo_once(cx);
                }
                let at = self.cursor_offset();
                self.put_cursor(at, cx);
            }
            'v' | 'V' => {
                self.vi.mode = if c == 'v' { Mode::Visual } else { Mode::VisualLine };
                self.vi.anchor = i;
                self.vi.cursor = i;
                self.show_visual(cx);
            }
            ':' => self.vi.command = Some(String::new()),
            'd' | 'c' | 'y' | 'g' | 'r' => self.vi.pending = format!("{digits}{c}"),
            _ => {}
        }
        cx.notify();
    }
}
