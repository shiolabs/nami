//! A text editor for nami files (developer mode): lines, typing, selecting,
//! pasting, IME, line numbers, Vim's keys (vim.rs). A nami file's parts in
//! colour, as the website has them (lang.rs). For a kind of nami file
//! (`kind`): what fits as you type, under the cursor (keys at its place, a
//! key's values, the catalog's packages, a package's versions), from nami
//! itself and never in typing's way (assist.rs); a key's or a package's help
//! where the pointer rests; nami check's problems underlined where they are,
//! said on the line under the editor, and fixed with a click or Ctrl+. as
//! they suggest. Enter keeps the line's indent, goes in after `key:` and
//! starts a list's next item; quotes and brackets come in pairs. From the
//! one-line field (input.rs), made multi-line.

use std::ops::Range;
use std::time::Duration;

use gpui::prelude::*;
use gpui::{
    Animation, AnimationExt as _, AnyElement, App, Bounds, ClipboardItem, Context, CursorStyle, ElementId, ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable, FontWeight,
    GlobalElementId, Hsla, KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, ShapedLine, Style, TextRun, UTF16Selection, UnderlineStyle,
    Window, actions, anchored, deferred, div, fill, point, relative, rgb, rgba, size,
};
use crate::look::px;
use crate::look::text;
use unicode_segmentation::*;

#[path = "editor_lang.rs"]
pub mod lang;
#[path = "editor_assist.rs"]
mod assist;

use lang::{Mark, NewLine};

actions!(
    editor,
    [
        Backspace, Delete, Left, Right, Up, Down, SelectLeft, SelectRight, SelectUp, SelectDown, SelectAll, Home, End, Enter, Tab, Paste, Cut, Copy, ShowCharacterPalette, Undo, Redo, Save,
        Escape, Complete, QuickFix
    ]
);

/// Save was asked for (ctrl-s, or vim's :w).
pub struct SaveAsked;

/// The editor's keys (Linux's: ctrl, not cmd).
pub fn bind_keys(cx: &mut App) {
    let c = Some("Editor");
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new("left", Left, c),
        KeyBinding::new("right", Right, c),
        KeyBinding::new("up", Up, c),
        KeyBinding::new("down", Down, c),
        KeyBinding::new("shift-left", SelectLeft, c),
        KeyBinding::new("shift-right", SelectRight, c),
        KeyBinding::new("shift-up", SelectUp, c),
        KeyBinding::new("shift-down", SelectDown, c),
        KeyBinding::new("ctrl-a", SelectAll, c),
        KeyBinding::new("home", Home, c),
        KeyBinding::new("end", End, c),
        KeyBinding::new("enter", Enter, c),
        KeyBinding::new("tab", Tab, c),
        KeyBinding::new("ctrl-v", Paste, c),
        KeyBinding::new("ctrl-c", Copy, c),
        KeyBinding::new("ctrl-x", Cut, c),
        KeyBinding::new("ctrl-z", Undo, c),
        KeyBinding::new("ctrl-shift-z", Redo, c),
        KeyBinding::new("ctrl-r", Redo, c),
        KeyBinding::new("ctrl-s", Save, c),
        KeyBinding::new("escape", Escape, c),
        KeyBinding::new("ctrl-space", Complete, c),
        KeyBinding::new("ctrl-.", QuickFix, c),
    ]);
}

const LINE: f32 = 20.;
/// The line numbers' column.
const GUTTER: f32 = 44.;
/// How many of what fits show at once.
const ROWS: usize = 8;
const ROW: f32 = 24.;
/// The popover's room round its rows (a menu's).
const INSET: f32 = 5.;
/// How long the pointer rests on a key, a package or a problem before it's said what it is.
const HOVER_DELAY: u64 = 500;

/// What fits where the cursor is, shown under it.
struct Popup {
    items: Vec<lang::Item>,
    /// Where the word it replaces starts.
    word: usize,
    picked: usize,
    /// The first row shown.
    top: usize,
    /// Which time it's been opened: it comes in once, not at each key.
    id: u64,
}

/// What the pointer's resting on, said.
#[derive(Clone)]
enum Card {
    Help { key: String, help: lang::Help },
    Package { name: String, version: String, about: String },
    Problem(Mark),
}

struct Hover {
    range: Range<usize>,
    card: Card,
}

/// What the pointer's on, before it's said.
enum Pointed {
    Problem(Mark),
    Key { path: Vec<lang::Seg>, key: String },
    Package(String),
}

impl Pointed {
    /// What's said of it (off the window's thread: it may ask nami).
    fn card(self, kind: Option<String>) -> Option<Card> {
        match self {
            Pointed::Problem(m) => Some(Card::Problem(m)),
            Pointed::Key { path, key } => {
                let help = assist::knowledge(&kind?).help(&path, &key)?;
                Some(Card::Help { key, help })
            }
            Pointed::Package(name) => {
                let (version, about) = assist::package(&name)?;
                Some(Card::Package { name, version, about })
            }
        }
    }
}

pub struct Editor {
    pub(crate) focus_handle: FocusHandle,
    pub content: String,
    /// Vim's keys (see vim.rs), and its state.
    pub vim: bool,
    /// Prose, not a nami file: no line numbers, no colours, no Vim.
    pub plain: bool,
    pub(crate) vi: crate::vim::State,
    /// What kind of nami file it is (`user`, `computer`, `package`...), for what fits as you
    /// type and the help where the pointer rests; none: neither (the Installer's, a report).
    pub kind: Option<String>,
    /// nami check's problems, a line each.
    pub problems: Vec<Mark>,
    /// Earlier texts (and cursors), to undo to; undone ones, to redo.
    undo: Vec<(String, usize)>,
    redo: Vec<(String, usize)>,
    /// Typing goes on: one undo for all of it.
    typing: bool,
    /// A line Show in file went to (1-based), marked until the text's set again.
    pub highlight: Option<usize>,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    /// Where up and down keep to, across short lines.
    column_x: Option<Pixels>,
    layouts: Vec<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
    popup: Option<Popup>,
    /// Which asking of what fits is the latest: an answer to an older one is dropped.
    asked: u64,
    opened: u64,
    hover: Option<Hover>,
    /// What the pointer's on now, and which resting on something is the latest.
    pointing: Option<Range<usize>>,
    pointed: u64,
    on_card: bool,
    closing: u64,
}

impl Editor {
    pub fn new(cx: &mut Context<Self>, content: &str) -> Self {
        Editor {
            focus_handle: cx.focus_handle(),
            content: content.to_string(),
            vim: crate::pages::dev::setting("vim"),
            plain: false,
            vi: Default::default(),
            kind: None,
            problems: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            typing: false,
            highlight: None,
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            column_x: None,
            layouts: Vec::new(),
            last_bounds: None,
            is_selecting: false,
            popup: None,
            asked: 0,
            opened: 0,
            hover: None,
            pointing: None,
            pointed: 0,
            on_card: false,
            closing: 0,
        }
    }

    pub fn set(&mut self, cx: &mut Context<Self>, text: &str) {
        self.content = text.to_string();
        self.selected_range = 0..0;
        self.marked_range = None;
        self.undo.clear();
        self.redo.clear();
        self.vi = Default::default();
        self.highlight = None;
        self.popup = None;
        self.hover = None;
        self.problems.clear();
        cx.notify();
    }

    /// What kind of nami file `name` is (user.nami, system.nami...), for `kind`.
    pub fn kind_of(name: &str) -> Option<String> {
        let stem = name.split('.').next().unwrap_or_default();
        ["user", "computer", "package", "project", "service"].contains(&stem).then(|| stem.to_string())
    }

    /// The cursor at the start of line `n` (1-based), the line marked.
    pub fn go_to_line(&mut self, n: usize, cx: &mut Context<Self>) {
        let at: usize = self.content.split('\n').take(n.saturating_sub(1)).map(|l| l.len() + 1).sum();
        let at = at.min(self.content.len());
        self.selected_range = at..at;
        self.highlight = Some(n);
        cx.notify();
    }

    /// Where line `n` (1-based) was drawn last, in the window.
    pub fn line_top(&self, n: usize) -> Option<Pixels> {
        Some(self.last_bounds?.top() + px(LINE) * n.saturating_sub(1) as f32)
    }

    /// Note the text as it is, to undo to (typing on: once for all of it).
    pub(crate) fn remember(&mut self, typing: bool) {
        if !(typing && self.typing) {
            self.undo.push((self.content.clone(), self.cursor_offset()));
            self.redo.clear();
        }
        self.typing = typing;
    }

    /// `range` is about to be `text`: the problems on the lines it changes go (nami check says
    /// again in a moment), and those below move with their lines.
    fn edit_problems(&mut self, range: &Range<usize>, text: &str) {
        if self.problems.is_empty() {
            return;
        }
        let first = self.content[..range.start].matches('\n').count() + 1;
        let gone = self.content[range.clone()].matches('\n').count();
        let added = text.matches('\n').count();
        self.problems.retain(|m| !(first..=first + gone).contains(&m.line));
        for m in &mut self.problems {
            if m.line > first + gone {
                m.line = m.line + added - gone;
            }
        }
    }

    /// Replace `range` with `text`, the cursor after it (or at `cursor`).
    pub(crate) fn splice(&mut self, range: Range<usize>, text: &str, cursor: Option<usize>, cx: &mut Context<Self>) {
        self.remember(false);
        self.edit_problems(&range, text);
        self.content = self.content[..range.start].to_owned() + text + &self.content[range.end..];
        let at = cursor.unwrap_or(range.start + text.len()).min(self.content.len());
        self.selected_range = at..at;
        self.selection_reversed = false;
        self.column_x = None;
        self.hover = None;
        cx.notify();
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        self.undo_once(cx);
    }

    pub(crate) fn undo_once(&mut self, cx: &mut Context<Self>) {
        if let Some((text, at)) = self.undo.pop() {
            self.redo.push((std::mem::replace(&mut self.content, text), self.cursor_offset()));
            self.selected_range = at..at;
            self.typing = false;
            self.popup = None;
            cx.notify();
        }
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((text, at)) = self.redo.pop() {
            self.undo.push((std::mem::replace(&mut self.content, text), self.cursor_offset()));
            self.selected_range = at..at;
            self.typing = false;
            self.popup = None;
            cx.notify();
        }
    }

    fn save(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(SaveAsked);
    }

    fn escape(&mut self, _: &Escape, _: &mut Window, cx: &mut Context<Self>) {
        let open = self.popup.take().is_some() | self.hover.take().is_some();
        if self.vim {
            self.vim_escape(cx);
        } else if open {
            cx.notify();
        }
    }

    /// Each line's start, as a byte offset.
    pub(crate) fn starts(&self) -> Vec<usize> {
        std::iter::once(0).chain(self.content.match_indices('\n').map(|(i, _)| i + 1)).collect()
    }

    /// The line `offset` is on, and where that line starts.
    pub(crate) fn line_of(&self, offset: usize) -> (usize, usize) {
        let starts = self.starts();
        let line = starts.iter().rposition(|s| *s <= offset).unwrap_or(0);
        (line, starts[line])
    }

    pub(crate) fn line_end(&self, start: usize) -> usize {
        self.content[start..].find('\n').map_or(self.content.len(), |i| start + i)
    }

    pub(crate) fn cursor_offset(&self) -> usize {
        if self.selection_reversed { self.selected_range.start } else { self.selected_range.end }
    }

    pub(crate) fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if !(self.vim && self.vi.mode == crate::vim::Mode::Insert) {
            self.typing = false;
        }
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        self.popup = None;
        cx.notify()
    }

    pub(crate) fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        self.popup = None;
        cx.notify()
    }

    pub(crate) fn previous_boundary(&self, offset: usize) -> usize {
        self.content.grapheme_indices(true).rev().find_map(|(i, _)| (i < offset).then_some(i)).unwrap_or(0)
    }

    pub(crate) fn next_boundary(&self, offset: usize) -> usize {
        self.content.grapheme_indices(true).find_map(|(i, _)| (i > offset).then_some(i)).unwrap_or(self.content.len())
    }

    /// The offset a line up (-1) or down (1) from `offset`, at the same place across.
    fn vertical(&mut self, offset: usize, by: isize) -> usize {
        let (line, start) = self.line_of(offset);
        let x = self.column_x.unwrap_or_else(|| self.layouts.get(line).map_or(px(0.), |l| l.x_for_index(offset - start)));
        self.column_x = Some(x);
        let starts = self.starts();
        let target = line as isize + by;
        if target < 0 {
            return 0;
        }
        let Some(&s) = starts.get(target as usize) else { return self.content.len() };
        let col = self.layouts.get(target as usize).map_or(0, |l| l.closest_index_for_x(x));
        (s + col).min(self.line_end(s))
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        self.column_x = None;
        let to = if self.selected_range.is_empty() { self.previous_boundary(self.cursor_offset()) } else { self.selected_range.start };
        self.move_to(to, cx);
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        self.column_x = None;
        let to = if self.selected_range.is_empty() { self.next_boundary(self.selected_range.end) } else { self.selected_range.end };
        self.move_to(to, cx);
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        if self.pick(-1, cx) {
            return;
        }
        let to = self.vertical(self.cursor_offset(), -1);
        let x = self.column_x;
        self.move_to(to, cx);
        self.column_x = x;
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        if self.pick(1, cx) {
            return;
        }
        let to = self.vertical(self.cursor_offset(), 1);
        let x = self.column_x;
        self.move_to(to, cx);
        self.column_x = x;
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        let to = self.vertical(self.cursor_offset(), -1);
        self.select_to(to, cx);
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        let to = self.vertical(self.cursor_offset(), 1);
        self.select_to(to, cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.column_x = None;
        let (_, start) = self.line_of(self.cursor_offset());
        self.move_to(start, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.column_x = None;
        let (_, start) = self.line_of(self.cursor_offset());
        let end = self.line_end(start);
        self.move_to(end, cx);
    }

    /// Typing goes into the text here (not Vim's Normal or Visual mode).
    fn inserting(&self) -> bool {
        !self.vim || self.vi.mode == crate::vim::Mode::Insert
    }

    /// A new line: as indented as this one, further in after `key:` (with a dash where the key
    /// holds a list), the next item in a list; an empty item ends its list.
    fn enter(&mut self, _: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        if !self.inserting() {
            return self.vim_key('\n', cx);
        }
        if self.popup.is_some() {
            return self.accept(None, cx);
        }
        let at = self.cursor_offset();
        let (_, start) = self.line_of(at);
        let before = self.content[start..at].to_string();
        if self.plain || !self.selected_range.is_empty() || self.marked_range.is_some() {
            let indent: String = before.chars().take_while(|c| *c == ' ').collect();
            let dash = if !self.plain && before.trim_start().starts_with("- ") { "- " } else { "" };
            return self.replace_text_in_range(None, &format!("\n{indent}{dash}"), window, cx);
        }
        let known = self.kind.as_deref().and_then(assist::cached);
        let top = !before.starts_with(' ');
        let list = |key: &str| top && known.as_ref().is_some_and(|k| k.is_list(&[], key));
        let rest_empty = self.content[at..self.line_end(start)].trim().is_empty();
        match lang::new_line(&before, list) {
            NewLine::EndList if rest_empty => {
                let indent = before.len() - before.trim_start().len();
                self.splice(start..at, &" ".repeat(indent.saturating_sub(2)), None, cx);
            }
            NewLine::EndList => self.splice(at..at, &format!("\n{}", before.trim_end()), None, cx),
            NewLine::Type(next) => {
                // (The spaces at the end of the line go.)
                let trail = before.len() - before.trim_end().len();
                self.splice(at - trail..at, &format!("\n{next}"), None, cx);
            }
        }
    }

    fn tab(&mut self, _: &Tab, window: &mut Window, cx: &mut Context<Self>) {
        if !self.inserting() {
            return;
        }
        if self.popup.is_some() {
            return self.accept(None, cx);
        }
        self.replace_text_in_range(None, "  ", window, cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if !self.inserting() {
            return self.vim_key('\u{8}', cx);
        }
        if self.selected_range.is_empty() {
            let at = self.cursor_offset();
            let prev = self.previous_boundary(at);
            if at == prev {
                return;
            }
            // Between a pair with nothing in it: both go.
            let pair = matches!((self.content[prev..at].chars().next(), self.content[at..].chars().next()), (Some('"'), Some('"')) | (Some('{'), Some('}')) | (Some('['), Some(']')));
            if pair && !self.plain {
                let open = self.popup.is_some();
                self.splice(prev..at + 1, "", None, cx);
                if open {
                    self.ask(false, cx);
                }
                return;
            }
            self.select_to(prev, cx)
        }
        let open = self.popup.is_some();
        self.replace_text_in_range(None, "", window, cx);
        if open {
            self.ask(false, cx);
        }
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            if self.cursor_offset() == next {
                return;
            }
            self.select_to(next, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.is_selecting = true;
        self.column_x = None;
        self.hover = None;
        window.focus(&self.focus_handle, cx);
        let at = self.index_for_mouse_position(event.position);
        if event.modifiers.shift { self.select_to(at, cx) } else { self.move_to(at, cx) }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            return self.select_to(self.index_for_mouse_position(event.position), cx);
        }
        self.point_at(event.position, cx);
    }

    fn show_character_palette(&mut self, _: &ShowCharacterPalette, window: &mut Window, _: &mut Context<Self>) {
        window.show_character_palette();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text.replace("\r\n", "\n"), window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.content[self.selected_range.clone()].to_string()));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.content[self.selected_range.clone()].to_string()));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        let Some(bounds) = self.last_bounds else { return 0 };
        if position.y < bounds.top() {
            return 0;
        }
        let line = ((position.y - bounds.top()) / px(LINE)).floor() as usize;
        let starts = self.starts();
        let Some(&start) = starts.get(line) else { return self.content.len() };
        let col = self.layouts.get(line).map_or(0, |l| l.closest_index_for_x(position.x - bounds.left()));
        (start + col).min(self.line_end(start))
    }

    /// The character the pointer's on, if it's on one.
    fn index_under(&self, position: Point<Pixels>) -> Option<usize> {
        let bounds = self.last_bounds?;
        if !bounds.contains(&position) {
            return None;
        }
        let line = ((position.y - bounds.top()) / px(LINE)).floor() as usize;
        let layout = self.layouts.get(line)?;
        let x = position.x - bounds.left();
        if x > layout.width() {
            return None;
        }
        let start = *self.starts().get(line)?;
        let i = layout.index_for_x(x).unwrap_or_else(|| layout.closest_index_for_x(x));
        Some((start + i).min(self.line_end(start)))
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let (mut utf8, mut utf16) = (0, 0);
        for ch in self.content.chars() {
            if utf16 >= offset {
                break;
            }
            utf16 += ch.len_utf16();
            utf8 += ch.len_utf8();
        }
        utf8
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let (mut utf16, mut utf8) = (0, 0);
        for ch in self.content.chars() {
            if utf8 >= offset {
                break;
            }
            utf8 += ch.len_utf8();
            utf16 += ch.len_utf16();
        }
        utf16
    }

    fn range_to_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(r.start)..self.offset_to_utf16(r.end)
    }

    fn range_from_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(r.start)..self.offset_from_utf16(r.end)
    }
}

/// What fits, the help, and the problems.
impl Editor {
    /// Ask what fits at the cursor, off the window's thread; an answer comes only while the
    /// text and the cursor are as they were (`asked`: Ctrl+Space, even with nothing typed).
    fn ask(&mut self, asked: bool, cx: &mut Context<Self>) {
        let Some(kind) = self.kind.clone() else { return };
        if self.plain || !self.inserting() || !self.selected_range.is_empty() || self.marked_range.is_some() {
            self.popup = None;
            return;
        }
        self.asked += 1;
        let (turn, text, at) = (self.asked, self.content.clone(), self.cursor_offset());
        cx.spawn(async move |this, cx| {
            let words = text.clone();
            let found = cx.background_executor().spawn(async move { assist::found(&words, at, &kind, asked) }).await;
            let _ = this.update(cx, |e, cx| {
                if e.asked != turn || e.content != text || e.cursor_offset() != at {
                    return;
                }
                e.popup = found.map(|f| {
                    let id = match &e.popup {
                        Some(p) => p.id,
                        None => {
                            e.opened += 1;
                            e.opened
                        }
                    };
                    Popup { items: f.items, word: f.word.start, picked: 0, top: 0, id }
                });
                cx.notify();
            });
        })
        .detach();
    }

    fn complete(&mut self, _: &Complete, _: &mut Window, cx: &mut Context<Self>) {
        self.ask(true, cx);
    }

    /// Up or down the list of what fits, if it's open.
    fn pick(&mut self, by: isize, cx: &mut Context<Self>) -> bool {
        let Some(p) = &mut self.popup else { return false };
        p.picked = (p.picked as isize + by).clamp(0, p.items.len() as isize - 1) as usize;
        if p.picked < p.top {
            p.top = p.picked;
        } else if p.picked >= p.top + ROWS {
            p.top = p.picked + 1 - ROWS;
        }
        cx.notify();
        true
    }

    /// What fits, typed in place of the word at the cursor (the picked one, or row `row`); a
    /// key's values offered next.
    fn accept(&mut self, row: Option<usize>, cx: &mut Context<Self>) {
        let Some(p) = self.popup.take() else { return };
        let Some(item) = p.items.get(row.unwrap_or(p.picked)).cloned() else { return };
        let at = self.cursor_offset();
        let word = lang::place(&self.content, at).map(|pl| pl.word).unwrap_or(p.word.min(at)..at);
        self.splice(word, &item.insert, None, cx);
        if item.insert.ends_with(' ') {
            self.ask(false, cx);
        }
    }

    /// Typed, one character: its pair with it (`"`, `{`, `[`), or over the one there already.
    /// Whether it's been done.
    fn pair(&mut self, c: char, cx: &mut Context<Self>) -> bool {
        let at = self.cursor_offset();
        let next = self.content[at..].chars().next();
        let (_, start) = self.line_of(at);
        let inside = self.content[start..at].matches('"').count() % 2 == 1;
        if next == Some(c) && (matches!(c, '}' | ']') || (c == '"' && inside)) {
            self.remember(true);
            self.selected_range = at + 1..at + 1;
            cx.notify();
            return true;
        }
        let close = match c {
            '"' if !inside => '"',
            '{' => '}',
            '[' => ']',
            _ => return false,
        };
        let prev = self.content[..at].chars().next_back();
        if !next.is_none_or(|n| n.is_whitespace() || matches!(n, '}' | ']' | ',')) || (c == '"' && prev.is_some_and(|p| p.is_alphanumeric())) {
            return false;
        }
        self.splice(at..at, &format!("{c}{close}"), Some(at + 1), cx);
        true
    }

    fn quick_fix(&mut self, _: &QuickFix, _: &mut Window, cx: &mut Context<Self>) {
        let line = self.line_of(self.cursor_offset()).0 + 1;
        if let Some(m) = self.problems.iter().find(|m| m.line == line && lang::fix(&self.content, m).is_some()).cloned() {
            self.fix(&m, cx);
        }
    }

    /// A problem's suggestion, made.
    fn fix(&mut self, m: &Mark, cx: &mut Context<Self>) {
        let Some((range, with)) = lang::fix(&self.content, m) else { return };
        let end = range.start + with.len();
        let cursor = if self.inserting() { end } else { self.previous_boundary(end).max(range.start) };
        self.splice(range, &with, Some(cursor), cx);
        self.problems.retain(|p| p != m);
        self.hover = None;
        self.popup = None;
    }

    /// The pointer's at `position`: what's under it is said once it's rested there a moment.
    fn point_at(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.plain {
            return;
        }
        let found = self.index_under(position).and_then(|i| self.pointed_at(i));
        let range = found.as_ref().map(|(r, _)| r.clone());
        if range == self.pointing {
            return;
        }
        self.pointing = range.clone();
        self.pointed += 1;
        if self.hover.is_some() {
            self.close_card_soon(cx);
        }
        let (Some((range, what)), turn, kind) = (found, self.pointed, self.kind.clone()) else { return };
        // (From one to the next, at once.)
        let delay = if self.hover.is_some() { 0 } else { HOVER_DELAY };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(delay)).await;
            if !this.update(cx, |e, _| e.pointed == turn).unwrap_or(false) {
                return;
            }
            let card = cx.background_executor().spawn(async move { what.card(kind) }).await;
            let _ = this.update(cx, |e, cx| {
                if e.pointed == turn
                    && let Some(card) = card
                {
                    e.hover = Some(Hover { range, card });
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// What's at `at` to say something about: a problem first, then a key or a package.
    fn pointed_at(&self, at: usize) -> Option<(Range<usize>, Pointed)> {
        let (line, start) = self.line_of(at);
        let text = &self.content[start..self.line_end(start)];
        for m in self.problems.iter().filter(|m| m.line == line + 1) {
            let (r, _) = lang::mark_span(m, text);
            if r.contains(&(at - start)) {
                return Some((start + r.start..start + r.end, Pointed::Problem(m.clone())));
            }
        }
        self.kind.as_ref()?;
        let (r, t) = lang::target(&self.content, at)?;
        Some((
            r,
            match t {
                lang::Target::Key { path, key } => Pointed::Key { path, key },
                lang::Target::Package(name) => Pointed::Package(name),
            },
        ))
    }

    /// The card goes in a moment, unless the pointer's gone onto it or back.
    fn close_card_soon(&mut self, cx: &mut Context<Self>) {
        self.closing += 1;
        let turn = self.closing;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(150)).await;
            let _ = this.update(cx, |e, cx| {
                let back = e.hover.as_ref().is_some_and(|h| e.pointing.as_ref() == Some(&h.range));
                if e.closing == turn && !e.on_card && !back && e.hover.take().is_some() {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Where `offset` is drawn, from the text's top left: its line, and how far across.
    fn spot(&self, offset: usize, advance: Pixels) -> (usize, Pixels) {
        let (line, start) = self.line_of(offset);
        (line, advance * self.content[start..offset].chars().count() as f32)
    }
}

impl EntityInputHandler for Editor {
    fn text_for_range(&mut self, range_utf16: Range<usize>, actual_range: &mut Option<Range<usize>>, _: &mut Window, _: &mut Context<Self>) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: self.range_to_utf16(&self.selected_range), reversed: self.selection_reversed })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range.as_ref().map(|r| self.range_to_utf16(r))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(&mut self, range_utf16: Option<Range<usize>>, new_text: &str, _: &mut Window, cx: &mut Context<Self>) {
        if !self.inserting() && range_utf16.is_none() && self.marked_range.is_none() {
            for c in new_text.chars() {
                self.vim_key(c, cx);
            }
            return;
        }
        // One character typed: a quote's or a bracket's pair, and what fits after it.
        let typed = (range_utf16.is_none() && self.marked_range.is_none() && !self.plain).then(|| {
            let mut chars = new_text.chars();
            chars.next().filter(|_| chars.next().is_none())
        });
        let typed = typed.flatten();
        if let Some(c) = typed
            && self.selected_range.is_empty()
            && self.pair(c, cx)
        {
            self.popup = None;
            return;
        }
        self.remember(new_text.chars().all(|c| !c.is_whitespace() || c == ' ') && !new_text.is_empty());
        let range = range_utf16.as_ref().map(|r| self.range_from_utf16(r)).or(self.marked_range.clone()).unwrap_or(self.selected_range.clone());
        self.edit_problems(&range, new_text);
        self.content = self.content[..range.start].to_owned() + new_text + &self.content[range.end..];
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.selection_reversed = false;
        self.marked_range = None;
        self.column_x = None;
        self.hover = None;
        match typed {
            Some(c) if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '@' | ' ' | '+') => self.ask(false, cx),
            _ => self.popup = None,
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(&mut self, range_utf16: Option<Range<usize>>, new_text: &str, new_selected: Option<Range<usize>>, _: &mut Window, cx: &mut Context<Self>) {
        let range = range_utf16.as_ref().map(|r| self.range_from_utf16(r)).or(self.marked_range.clone()).unwrap_or(self.selected_range.clone());
        self.content = self.content[..range.start].to_owned() + new_text + &self.content[range.end..];
        self.marked_range = (!new_text.is_empty()).then(|| range.start..range.start + new_text.len());
        self.selected_range = new_selected
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .map(|r| r.start + range.start..r.end + range.start)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        self.popup = None;
        cx.notify();
    }

    fn bounds_for_range(&mut self, range_utf16: Range<usize>, bounds: Bounds<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        let (line, start) = self.line_of(range.start);
        let layout = self.layouts.get(line)?;
        let top = bounds.top() + px(LINE) * line as f32;
        Some(Bounds::from_corners(point(bounds.left() + layout.x_for_index(range.start - start), top), point(bounds.left() + layout.x_for_index((range.end - start).min(layout.len())), top + px(LINE))))
    }

    fn character_index_for_point(&mut self, p: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        Some(self.offset_to_utf16(self.index_for_mouse_position(p)))
    }
}

/// A line's colours, as the website's (lang.rs `spans`): keys the text's, a little heavier;
/// paths slate; numbers, strings and `!secret` in the muted state colours; dashes and comments
/// quiet.
pub(crate) fn runs(line: &str, base: TextRun) -> Vec<TextRun> {
    use crate::ui::color;
    use lang::Tone;
    lang::spans(line)
        .into_iter()
        .map(|(r, tone)| {
            let color: Hsla = match tone {
                Tone::Plain | Tone::Key => base.color,
                Tone::InnerKey => rgb(color::code_inner_key()).into(),
                Tone::Path => rgb(color::key()).into(),
                Tone::Number => rgb(color::code_number()).into(),
                Tone::Quoted => rgb(color::code_string()).into(),
                Tone::Secret => rgb(color::code_secret()).into(),
                Tone::Quiet => rgb(color::comment()).into(),
            };
            let font = if tone == Tone::Key { gpui::Font { weight: FontWeight::MEDIUM, ..base.font.clone() } } else { base.font.clone() };
            TextRun { len: r.len(), color, font, ..base.clone() }
        })
        .collect()
}

/// What nami says (nami check's problems), as it's shown anywhere in the app: its `code` in
/// Geist Mono, never its backticks; in `colour`, at the size of what it's in.
pub fn said(text: &str, colour: u32) -> gpui::StyledText {
    let parts = lang::code_spans(text);
    let words: String = parts.iter().map(|(p, _)| p.as_str()).collect();
    let runs = parts
        .iter()
        .map(|(p, code)| TextRun { len: p.len(), font: gpui::font(if *code { "Geist Mono" } else { "Geist" }), color: rgb(colour).into(), background_color: None, underline: None, strikethrough: None })
        .collect();
    gpui::StyledText::new(words).with_runs(runs)
}

/// `runs` with `span` underlined, wavy, in `colour` (a problem's place).
fn underlined(runs: Vec<TextRun>, span: Range<usize>, colour: Hsla) -> Vec<TextRun> {
    let mut out = Vec::new();
    let mut at = 0;
    for run in runs {
        let (a, b) = (at, at + run.len);
        at = b;
        let (s, e) = (span.start.clamp(a, b), span.end.clamp(a, b));
        if s >= e {
            out.push(run);
            continue;
        }
        for (from, to, lined) in [(a, s, false), (s, e, true), (e, b, false)] {
            if to > from {
                let underline = if lined { Some(UnderlineStyle { color: Some(colour), thickness: px(1.), wavy: true }) } else { run.underline.clone() };
                out.push(TextRun { len: to - from, underline, ..run.clone() });
            }
        }
    }
    out
}

struct EditorElement {
    editor: Entity<Editor>,
}

struct Prepaint {
    lines: Vec<ShapedLine>,
    quads: Vec<PaintQuad>,
    cursor: Option<PaintQuad>,
}

impl IntoElement for EditorElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let lines = self.editor.read(cx).content.split('\n').count();
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = (px(LINE) * lines as f32).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), window: &mut Window, cx: &mut App) -> Prepaint {
        use crate::ui::color;
        let editor = self.editor.read(cx);
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let base = TextRun { len: 0, font: style.font(), color: style.color, background_color: None, underline: None, strikethrough: None };
        let (sel, cursor_at, marked) = (editor.selected_range.clone(), editor.cursor_offset(), editor.marked_range.clone());
        let block = editor.vim && editor.vi.mode == crate::vim::Mode::Normal;
        let focused = editor.focus_handle.is_focused(window);
        let cursor_line = editor.line_of(cursor_at).0;
        // Across the whole box, the line numbers too.
        let row = |top: Pixels| Bounds::new(point(bounds.left() - px(GUTTER), top), size(bounds.size.width + px(GUTTER + 12.), px(LINE)));
        let mut lines = Vec::new();
        let mut quads = Vec::new();
        let mut cursor = None;
        let mut start = 0;
        for (i, text) in editor.content.split('\n').enumerate() {
            let end = start + text.len();
            let top = bounds.top() + px(LINE) * i as f32;
            if editor.highlight == Some(i + 1) {
                quads.push(fill(row(top), rgb(color::key_soft())));
            } else if !editor.plain && focused && sel.is_empty() && i == cursor_line {
                quads.push(fill(row(top), color::current_line()));
            }
            let mut r = if editor.plain { vec![TextRun { len: text.len(), ..base.clone() }] } else { runs(text, base.clone()) };
            for m in editor.problems.iter().filter(|m| m.line == i + 1) {
                let (span, _) = lang::mark_span(m, text);
                r = underlined(r, span, rgb(if m.error { color::bad() } else { color::code_number() }).into());
            }
            if let Some(m) = marked.clone().filter(|m| m.start >= start && m.end <= end) {
                // The text an input method is composing: underlined.
                let mut at = 0;
                for run in &mut r {
                    if at < m.end - start && at + run.len > m.start - start {
                        run.underline = Some(UnderlineStyle { color: Some(run.color), thickness: px(1.), wavy: false });
                    }
                    at += run.len;
                }
            }
            let line = window.text_system().shape_line(text.to_string().into(), font_size, &r, None);
            if sel.start <= end && sel.end >= start && !sel.is_empty() {
                let a = sel.start.max(start) - start;
                let b = sel.end.min(end) - start;
                let extra = if sel.end > end { px(6.) } else { px(0.) };
                quads.push(fill(Bounds::from_corners(point(bounds.left() + line.x_for_index(a), top), point(bounds.left() + line.x_for_index(b) + extra, top + px(LINE))), rgba((color::key() << 8) | 0x48)));
            }
            if sel.is_empty() && cursor_at >= start && cursor_at <= end {
                let x = line.x_for_index(cursor_at - start);
                cursor = Some(if block {
                    // Vim's Normal mode: a block over the character.
                    let w = if cursor_at < end { line.x_for_index(editor.next_boundary(cursor_at) - start) - x } else { px(8.) };
                    fill(Bounds::new(point(bounds.left() + x, top + px(1.)), size(w.max(px(2.)), px(LINE - 2.))), rgba((color::text() << 8) | 0x60))
                } else {
                    fill(Bounds::new(point(bounds.left() + x, top + px(2.)), size(px(2.), px(LINE - 4.))), rgb(color::text()))
                });
            }
            lines.push(line);
            start = end + 1;
        }
        // The guide down the block the cursor's in, from its key.
        if !editor.plain
            && focused
            && let Some((top, column, last)) = lang::block(&editor.content, cursor_line)
            && let Some(at) = lines.get(top)
        {
            let x = bounds.left() + at.x_for_index(column.min(at.len())).max(px(0.)) + px(3.);
            quads.push(fill(Bounds::new(point(x, bounds.top() + px(LINE) * (top + 1) as f32), size(px(1.), px(LINE) * (last - top) as f32)), color::indent_guide()));
        }
        Prepaint { lines, quads, cursor }
    }

    fn paint(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, bounds: Bounds<Pixels>, _: &mut (), prepaint: &mut Prepaint, window: &mut Window, cx: &mut App) {
        let focus = self.editor.read(cx).focus_handle.clone();
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.editor.clone()), cx);
        for q in prepaint.quads.drain(..) {
            window.paint_quad(q);
        }
        let lines = std::mem::take(&mut prepaint.lines);
        for (i, line) in lines.iter().enumerate() {
            let _ = line.paint(point(bounds.left(), bounds.top() + px(LINE) * i as f32), px(LINE), gpui::TextAlign::Left, None, window, cx);
        }
        if focus.is_focused(window)
            && let Some(c) = prepaint.cursor.take()
        {
            window.paint_quad(c);
        }
        self.editor.update(cx, |e, _| {
            e.layouts = lines;
            e.last_bounds = Some(bounds);
        });
    }
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.content.split('\n').count();
        // (Its edge says it has the keys, as a field's does.)
        let focused = self.focus_handle.is_focused(window);
        if !focused {
            self.popup = None;
        }
        let errors: Vec<usize> = self.problems.iter().filter(|m| m.error).map(|m| m.line).collect();
        let status = self.status(cx);
        div()
            .flex()
            .flex_col()
            .w_full()
            .gap(px(4.))
            .key_context("Editor")
            .track_focus(&self.focus_handle(cx))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::enter))
            .on_action(cx.listener(Self::tab))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::escape))
            .on_action(cx.listener(Self::complete))
            .on_action(cx.listener(Self::quick_fix))
            .child(self.lines(count, errors, focused, window, cx))
            .children(status)
    }
}

impl Editor {
    fn lines(&self, count: usize, errors: Vec<usize>, focused: bool, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A character's width (Geist Mono's are all one): where the popover and the cards go.
        let font = window.text_system().resolve_font(&gpui::font("Geist Mono"));
        let advance = window.text_system().advance(font, px(text::CALLOUT), 'm').map(|s| s.width).unwrap_or(px(7.8));
        let mut over: Vec<AnyElement> = Vec::new();
        if let Some(p) = self.popup.as_ref().filter(|_| focused) {
            let (line, x) = self.spot(p.word.min(self.content.len()), advance);
            over.push(below(line, x, self.popup_view(p, advance, cx).into_any_element()));
        }
        if let Some(h) = &self.hover {
            let (line, x) = self.spot(h.range.start.min(self.content.len()), advance);
            over.push(below(line, x, self.card_view(&h.card, cx).into_any_element()));
        }
        div()
            .flex()
            .w_full()
            .min_h(px(if self.plain { 140. } else { 320. }))
            .py(px(10.))
            .when(self.plain, |d| d.pl(px(12.)))
            .rounded(px(crate::look::shape::CONTROL))
            .border_1()
            .border_color(rgb(if focused { crate::ui::color::key() } else { crate::ui::color::line() }))
            .bg(rgb(crate::ui::color::panel()))
            .font_family(if self.plain { "Geist" } else { "Geist Mono" })
            .text_size(px(if self.plain { text::BODY } else { text::CALLOUT }))
            .line_height(px(LINE))
            .when(!self.plain, |d| {
                d.child(
                    div()
                        .w(px(GUTTER))
                        .flex_none()
                        .flex()
                        .flex_col()
                        .items_end()
                        .pr(px(12.))
                        .text_color(rgb(crate::ui::color::faint()))
                        .children((1..=count).map(|n| div().h(px(LINE)).when(errors.contains(&n), |d| d.text_color(rgb(crate::ui::color::bad()))).child(n.to_string()))),
                )
            })
            .child(
                div()
                    .id("editor-text")
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .cursor(CursorStyle::IBeam)
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
                    .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
                    .on_mouse_move(cx.listener(Self::on_mouse_move))
                    .on_hover(cx.listener(|e, on: &bool, _, cx| {
                        if !*on {
                            e.pointing = None;
                            e.pointed += 1;
                            if e.hover.is_some() {
                                e.close_card_soon(cx);
                            }
                        }
                    }))
                    .mr(px(12.))
                    .child(EditorElement { editor: cx.entity() })
                    .children(over),
            )
    }

    /// What fits: a menu's surface, its rows, the picked one grey.
    fn popup_view(&self, p: &Popup, advance: Pixels, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::ui::color;
        // (What each is, in a column: after the widest name shown, as far as a few words' room.)
        let widest = p.items.iter().skip(p.top).take(ROWS).map(|i| i.label.chars().count()).max().unwrap_or(0);
        let label = (advance * (text::META / text::CALLOUT) * widest as f32 + px(2.)).min(px(200.));
        let rows = p.items.iter().enumerate().skip(p.top).take(ROWS).map(|(i, item)| {
            div()
                .id(("editor-fits", i))
                .flex()
                .items_center()
                .gap(px(12.))
                .h(px(ROW))
                .px(px(8.))
                .rounded(px(crate::look::shape::MENU - INSET))
                .when(i == p.picked, |d| d.bg(color::row_picked()))
                .when(i != p.picked, |d| d.hover(|s| s.bg(color::row_hover())))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |e, _, _, cx| {
                        cx.stop_propagation();
                        e.accept(Some(i), cx);
                    }),
                )
                .child(div().flex_none().w(label).truncate().font_family("Geist Mono").text_size(px(text::META)).text_color(rgb(color::text())).child(item.label.clone()))
                .child(div().flex_1().min_w_0().truncate().font_family("Geist").text_size(px(text::META)).text_color(rgb(color::muted())).child(item.about.clone()))
        });
        let menu = div()
            .id("editor-popup")
            .occlude()
            .w(px(380.))
            .p(px(INSET))
            .flex()
            .flex_col()
            .rounded(px(crate::look::shape::MENU))
            .border_1()
            .border_color(rgb(color::line()))
            .bg(rgb(color::panel()))
            .shadow(crate::look::popup_shadow())
            .children(rows);
        appearing(menu, ElementId::NamedInteger("editor-popup-in".into(), p.id), cx)
    }

    /// What the pointer's resting on: a card in a tooltip's material, its words and an example;
    /// a problem's with its fix.
    fn card_view(&self, card: &Card, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::ui::color;
        let words = |t: String| div().text_size(px(text::META)).line_height(px(17.)).text_color(rgb(color::text())).child(t);
        let quiet = |t: String| div().text_size(px(text::META)).text_color(rgb(color::muted())).child(t);
        let mono = |t: String| div().font_family("Geist Mono").text_size(px(text::META)).child(t);
        let (width, body): (f32, Vec<AnyElement>) = match card {
            Card::Help { key, help } => {
                let mut body = vec![mono(format!("{key}:")).font_weight(FontWeight::MEDIUM).text_color(rgb(color::text())).into_any_element(), words(help.summary.clone()).into_any_element()];
                if !help.example.is_empty() {
                    let example = div()
                        .mt(px(2.))
                        .px(px(8.))
                        .py(px(6.))
                        .rounded(px(crate::look::shape::SMALL))
                        .bg(rgb(color::soft()))
                        .flex()
                        .flex_col()
                        .children(help.example.lines().map(|l| mono(if l.is_empty() { " ".into() } else { l.to_string() }).line_height(px(16.)).whitespace_nowrap().text_color(rgb(color::muted()))));
                    body.push(example.into_any_element());
                }
                (360., body)
            }
            Card::Package { name, version, about } => {
                let head = div().flex().gap(px(8.)).child(mono(name.clone()).font_weight(FontWeight::MEDIUM).text_color(rgb(color::text()))).child(mono(version.clone()).text_color(rgb(color::muted())));
                let w = (about.chars().count().max(name.len() + version.len() + 2) as f32 * 6.4 + 28.).clamp(200., 360.);
                (w, vec![head.into_any_element(), quiet(about.clone()).into_any_element()])
            }
            Card::Problem(m) => {
                let message = m.said();
                let mut body = vec![div().text_size(px(text::META)).line_height(px(17.)).child(said(&message, color::text())).into_any_element()];
                if let Some(with) = lang::fix(&self.content, m).map(|(_, w)| w) {
                    let m = m.clone();
                    let row = div()
                        .id("editor-card-fix")
                        .mx(px(-6.))
                        .px(px(6.))
                        .h(px(ROW))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(12.))
                        .rounded(px(crate::look::shape::SMALL))
                        .cursor_pointer()
                        .hover(|s| s.bg(color::row_hover()))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |e, _, _, cx| {
                                cx.stop_propagation();
                                e.fix(&m, cx);
                            }),
                        )
                        .child(div().text_size(px(text::META)).text_color(rgb(color::text())).child(format!("Change to “{with}”")))
                        .child(quiet("Ctrl+.".into()));
                    body.push(row.into_any_element());
                }
                ((message.chars().count() as f32 * 6.2 + 28.).clamp(200., 380.), body)
            }
        };
        let card = div()
            .id("editor-card")
            .occlude()
            .w(px(width))
            .px(px(10.))
            .py(px(8.))
            .flex()
            .flex_col()
            .gap(px(4.))
            .font_family("Geist")
            .rounded(px(crate::look::shape::ROW))
            .border_1()
            .border_color(rgb(color::line()))
            .bg(rgb(color::panel()))
            .shadow(crate::look::popup_shadow())
            .on_hover(cx.listener(|e, on: &bool, _, cx| {
                e.on_card = *on;
                if !*on {
                    e.close_card_soon(cx);
                }
            }))
            .children(body);
        let id = ElementId::NamedInteger("editor-card-in".into(), self.pointed);
        appearing(card, id, cx)
    }

    /// The cursor's line (1-based).
    pub fn cursor_line(&self) -> usize {
        self.line_of(self.cursor_offset()).0 + 1
    }

    /// The cursor to line `n` (1-based), where its words start, and the keys to the editor.
    fn to_line(&mut self, n: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(&start) = self.starts().get(n.saturating_sub(1)) else { return };
        let line = &self.content[start..self.line_end(start)];
        let indent = line.len() - line.trim_start().len();
        let dash = if line[indent..].starts_with("- ") { 2 } else { 0 };
        let words = start + indent + dash;
        self.move_to(words, cx);
        window.focus(&self.focus_handle, cx);
    }

    /// The line under the editor: Vim's mode; the problem on the cursor's line, with its fix;
    /// and while there are problems on other lines, how many there are and on which lines, to
    /// go to.
    fn status(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        use crate::ui::color;
        if self.plain || (self.kind.is_none() && !self.vim && self.problems.is_empty()) {
            return None;
        }
        let line = self.cursor_line();
        let mark = self.problems.iter().find(|m| m.line == line).cloned();
        let fixable = mark.as_ref().is_some_and(|m| lang::fix(&self.content, m).is_some());
        let mode = self.vim.then(|| self.vi.line());
        let mut lines: Vec<usize> = self.problems.iter().map(|m| m.line).collect();
        lines.sort();
        lines.dedup();
        let count = self.problems.len();
        let elsewhere = lines.iter().any(|l| *l != line);
        let shown = 4;
        let summary = elsewhere.then(|| {
            let mut d = div().flex_none().flex().items_center().text_color(rgb(color::muted())).child(format!("{count} problem{} · ", if count == 1 { "" } else { "s" }));
            for (i, n) in lines.iter().copied().take(shown).enumerate() {
                if i > 0 {
                    d = d.child(", ");
                }
                d = d.child(
                    div()
                        .id(("editor-problem-line", n))
                        .cursor_pointer()
                        .when(n == line, |d| d.text_color(rgb(color::text())))
                        .hover(|s| s.text_color(rgb(color::text())))
                        .on_click(cx.listener(move |e, _, window, cx| e.to_line(n, window, cx)))
                        .child(format!("Line {n}")),
                );
            }
            if lines.len() > shown {
                d = d.child(", …");
            }
            d
        });
        Some(
            div()
                .h(px(20.))
                .px(px(4.))
                .flex()
                .items_center()
                .gap(px(12.))
                .text_size(px(text::META))
                .when_some(mode, |d, m| d.child(div().flex_none().font_family("Geist Mono").text_color(rgb(color::muted())).child(m)))
                .child(div().flex_1().min_w_0().when_some(mark.clone(), |d, m| d.truncate().child(said(&m.said(), if m.error { color::bad() } else { color::muted() }))))
                .when_some(mark, |d, m| {
                    let fix = m.clone();
                    d.when(fixable, |d| {
                        d.child(
                            div()
                                .id("editor-fix")
                                .flex_none()
                                .h(px(20.))
                                .px(px(10.))
                                .flex()
                                .items_center()
                                .rounded_full()
                                .bg(color::push())
                                .text_color(rgb(color::text()))
                                .font_weight(FontWeight::MEDIUM)
                                .cursor_pointer()
                                .hover(|s| s.bg(color::round_off()))
                                .on_click(cx.listener(move |e, _, _, cx| e.fix(&fix, cx)))
                                .child("Fix"),
                        )
                    })
                })
                .children(summary),
        )
    }
}

/// `el` floating under line `line` at `x` across the text (over it, where there's no room
/// below), over everything else.
fn below(line: usize, x: Pixels, el: AnyElement) -> AnyElement {
    div()
        .absolute()
        .left(x - px(INSET + 9.))
        .top(px(LINE) * line as f32)
        .child(deferred(anchored().child(div().pt(px(LINE + 2.)).child(el))).with_priority(1))
        .into_any_element()
}

/// What floats, coming in: a fade as it settles from a little above, quick, eased out; at once
/// with Animations off or Reduce motion.
fn appearing(el: gpui::Stateful<gpui::Div>, id: ElementId, cx: &App) -> AnyElement {
    if !crate::motion::moves(cx) {
        return el.into_any_element();
    }
    el.relative()
        .with_animation(id, Animation::new(Duration::from_millis(120)).with_easing(crate::motion::ease_out()), |el, t| el.opacity(t).top(px(-3. * (1. - t))))
        .into_any_element()
}

impl gpui::EventEmitter<SaveAsked> for Editor {}

impl Focusable for Editor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
