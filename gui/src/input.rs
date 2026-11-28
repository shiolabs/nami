//! A one-line text field (the computer's name, yours, a password), from
//! gpui's own input example: typing, selecting, pasting, IME.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, GlobalElementId, KeyBinding, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    ShapedLine, SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window, actions,
    div, fill, point, prelude::*, relative, rgb, rgba, size,
};
use crate::look::{Press, px, text};
use std::sync::atomic::{AtomicBool, Ordering};
use unicode_segmentation::*;

/// This program's boxed fields are in its groups' family (Settings', `set_inset`): smaller, a
/// well of the text's colour in place of the panel's white and its grey line. Otherwise as the
/// desktop's dialogs have them.
static INSET: AtomicBool = AtomicBool::new(false);

#[allow(dead_code)]
pub fn set_inset() {
    INSET.store(true, Ordering::Relaxed);
}

actions!(
    text_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
    ]
);

/// The keys a text field takes (Linux's: ctrl, not cmd).
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("TextInput")),
        KeyBinding::new("delete", Delete, Some("TextInput")),
        KeyBinding::new("left", Left, Some("TextInput")),
        KeyBinding::new("right", Right, Some("TextInput")),
        KeyBinding::new("shift-left", SelectLeft, Some("TextInput")),
        KeyBinding::new("shift-right", SelectRight, Some("TextInput")),
        KeyBinding::new("ctrl-a", SelectAll, Some("TextInput")),
        KeyBinding::new("ctrl-v", Paste, Some("TextInput")),
        KeyBinding::new("ctrl-c", Copy, Some("TextInput")),
        KeyBinding::new("ctrl-x", Cut, Some("TextInput")),
        KeyBinding::new("home", Home, Some("TextInput")),
        KeyBinding::new("end", End, Some("TextInput")),
    ]);
}

pub struct TextInput {
    focus_handle: FocusHandle,
    pub content: SharedString,
    placeholder: SharedString,
    /// A password: shown as dots, never copied out.
    masked: bool,
    /// A password shown as it's typed, for now: its eye pressed. Back to dots as soon as
    /// the field (and its eye) lose the keys, or it's set again.
    revealed: bool,
    /// The eye beside a password: a button of its own, reached with Tab.
    eye: FocusHandle,
    /// Back to dots when the keys go elsewhere.
    hide_on_leave: Option<gpui::Subscription>,
    /// With no box around it, and large: for a window that's about this line.
    pub plain: bool,
    /// With no box around it, at the usual size: for a field whose box is drawn round it.
    pub bare: bool,
    /// With no box around it, small: in the login and lock screens' password pill.
    pub pill: bool,
    /// With no box around it, at a row's size: in a sidebar's search field.
    pub small: bool,
    /// With no box around it, at a name's size under an icon: renaming a file on the desktop.
    pub tiny: bool,
    /// The rest of what might be meant, in faint letters after the caret.
    hint: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
    /// How far along it's scrolled, when it's longer than the field: the caret always in view.
    scroll: Pixels,
}

impl TextInput {
    pub fn new(cx: &mut Context<Self>, placeholder: &str, content: &str, masked: bool) -> Self {
        TextInput {
            focus_handle: cx.focus_handle().tab_stop(true),
            content: content.to_string().into(),
            placeholder: placeholder.to_string().into(),
            masked,
            revealed: false,
            eye: cx.focus_handle().tab_stop(true),
            hide_on_leave: None,
            plain: false,
            bare: false,
            pill: false,
            small: false,
            tiny: false,
            hint: SharedString::default(),
            selected_range: content.len()..content.len(),
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            is_selecting: false,
            scroll: Pixels::ZERO,
        }
    }

    /// Put `text` in the field.
    pub fn set(&mut self, cx: &mut Context<Self>, text: &str) {
        self.content = text.to_string().into();
        self.selected_range = text.len()..text.len();
        self.marked_range = None;
        self.revealed = false;
        cx.notify();
    }

    /// Whether it's a password's field (its eye pressed or not).
    #[allow(dead_code)]
    pub fn is_masked(&self) -> bool {
        self.masked
    }

    /// Whether what's typed is drawn as dots: a password whose eye isn't pressed.
    fn hidden(&self) -> bool {
        self.masked && !self.revealed
    }

    /// A password's eye pressed: what's typed shown, or back to dots.
    pub fn toggle_revealed(&mut self, cx: &mut Context<Self>) {
        if self.masked {
            self.revealed = !self.revealed;
            cx.notify();
        }
    }

    /// Whether a password's shown as it's typed (its eye pressed).
    #[allow(dead_code)]
    pub fn revealed(&self) -> bool {
        self.revealed
    }

    /// All of it picked: what's typed next takes its place.
    pub fn pick_all(&mut self, cx: &mut Context<Self>) {
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;
        cx.notify();
    }

    /// Part of it picked (`range`, in bytes): a file's name without its extension, to rename.
    #[allow(dead_code)]
    pub fn pick_range(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        let end = range.end.min(self.content.len());
        self.selected_range = range.start.min(end)..end;
        self.selection_reversed = false;
        cx.notify();
    }

    pub fn set_hint(&mut self, hint: &str, cx: &mut Context<Self>) {
        if self.hint != hint {
            self.hint = hint.to_string().into();
            cx.notify();
        }
    }

    /// The hint becomes what's typed, if there is one and the caret's at the end.
    pub fn accept_hint(&mut self, cx: &mut Context<Self>) -> bool {
        if self.hint.is_empty() || self.cursor_offset() != self.content.len() {
            return false;
        }
        let whole = format!("{}{}", self.content, self.hint);
        self.hint = SharedString::default();
        self.set(cx, &whole);
        true
    }

    /// Where `offset` in the content is in what's shown (dots for a password).
    fn shown(&self, offset: usize) -> usize {
        if self.hidden() { self.content[..offset].chars().count() * '•'.len_utf8() } else { offset }
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.accept_hint(cx) {
            return;
        }
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let prev = self.previous_boundary(self.cursor_offset());
            if self.cursor_offset() == prev {
                window.play_system_bell();
                return;
            }
            self.select_to(prev, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        // (Nothing to delete here: it's for whatever's around the field.)
        if self.selected_range.is_empty() && self.cursor_offset() == self.content.len() {
            cx.propagate();
            return;
        }
        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            if self.cursor_offset() == next {
                window.play_system_bell();
                return;
            }
            self.select_to(next, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.is_selecting = true;
        window.focus(&self.focus_handle, cx);

        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text.replace("\n", " "), window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() && !self.masked {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }
    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() && !self.masked {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }

        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() || self.hidden() {
            return self.content.len();
        }
        // (Not into the hint, which isn't typed.)
        line.closest_index_for_x(position.x - bounds.left() + self.scroll).min(self.content.len())
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;

        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }

        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;

        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }

        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .map(|new_range| new_range.start + range.start..new_range.end + range.end)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());

        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        Some(Bounds::from_corners(
            point(
                bounds.left() + last_layout.x_for_index(range.start),
                bounds.top(),
            ),
            point(
                bounds.left() + last_layout.x_for_index(range.end),
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;
        let last_layout = self.last_layout.as_ref()?;

        let utf8_index = last_layout.index_for_x(point.x - line_point.x)?;
        Some(self.offset_to_utf16(utf8_index))
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    /// How far along it's drawn from (longer than the field: the caret kept in view).
    scroll: Pixels,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let style = window.text_style();

        let (display_text, text_color) = if content.is_empty() {
            (input.placeholder.clone(), rgb(crate::ui::color::faint()).into())
        } else if input.hidden() {
            (SharedString::from("•".repeat(content.chars().count())), style.color)
        } else {
            (content, style.color)
        };
        let selected_range = input.shown(selected_range.start)..input.shown(selected_range.end);
        let cursor = input.shown(cursor);

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let hinted = !input.content.is_empty() && !input.hint.is_empty() && !input.masked && input.marked_range.is_none() && input.cursor_offset() == input.content.len();
        let (display_text, runs) = if hinted {
            let typed = display_text.len();
            let whole = SharedString::from(format!("{display_text}{}", input.hint));
            let runs = vec![TextRun { len: typed, ..run.clone() }, TextRun { len: input.hint.len(), color: rgb(crate::ui::color::faint()).into(), ..run.clone() }];
            (whole, Some(runs))
        } else {
            (display_text, None)
        };
        let runs = if let Some(runs) = runs {
            runs
        } else if let Some(marked_range) = input.marked_range.as_ref().filter(|_| !input.hidden()) {
            vec![
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len() - marked_range.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);

        let scroll = scrolled(input.scroll, line.x_for_index(cursor), line.width, bounds.size.width);
        let cursor_pos = line.x_for_index(cursor) - scroll;
        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + cursor_pos, bounds.top()),
                        size(px(2.), bounds.bottom() - bounds.top()),
                    ),
                    rgb(crate::ui::color::text()),
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            bounds.left() + line.x_for_index(selected_range.start) - scroll,
                            bounds.top(),
                        ),
                        point(
                            bounds.left() + line.x_for_index(selected_range.end) - scroll,
                            bounds.bottom(),
                        ),
                    ),
                    // (Slate, as everything picked is.)
                    // (A name to type over, under an icon: plainly picked.)
                    rgba((crate::ui::color::key() << 8) | if input.tiny { 0x9c } else { 0x5c }),
                )),
                None,
            )
        };
        PrepaintState {
            line: Some(line),
            scroll,
            cursor,
            selection,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        let line = prepaint.line.take().unwrap();
        let scroll = prepaint.scroll;
        // (Nothing drawn past its edges, when it's scrolled.)
        let mask = (scroll > Pixels::ZERO || line.width > bounds.size.width).then(|| gpui::ContentMask { bounds });
        window.with_content_mask(mask, |window| {
            if let Some(selection) = prepaint.selection.take() {
                window.paint_quad(selection)
            }
            line.paint(
                point(bounds.origin.x - scroll, bounds.origin.y),
                window.line_height(),
                gpui::TextAlign::Left,
                None,
                window,
                cx,
            )
            .unwrap();

            if focus_handle.is_focused(window)
                && let Some(cursor) = prepaint.cursor.take()
            {
                window.paint_quad(cursor);
            }
        });

        self.input.update(cx, |input, _cx| {
            input.scroll = scroll;
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A password goes back to dots once the keys leave it and its eye (to elsewhere in the
        // window, or the window going); and whatever happened unseen, it's dots when drawn
        // without them.
        if self.masked && self.hide_on_leave.is_none() {
            self.hide_on_leave = Some(cx.on_focus_out(&self.focus_handle, window, |this, _, _, cx| {
                if this.revealed {
                    this.revealed = false;
                    cx.notify();
                }
            }));
        }
        if self.revealed && !self.focus_handle.contains_focused(window, cx) {
            self.revealed = false;
        }
        let eye = self.masked.then(|| self.eye_button(window, cx));
        let boxed = !self.plain && !self.bare && !self.pill && !self.small && !self.tiny;
        let inset = INSET.load(Ordering::Relaxed);
        div()
            // A field to the screen reader, a password's as one, named by what it asks for.
            .id(gpui::ElementId::Name(format!("text-input-{}", cx.entity_id()).into()))
            .role(if self.masked { gpui::Role::PasswordInput } else { gpui::Role::TextInput })
            .aria_label(self.placeholder.clone())
            .flex()
            .key_context("TextInput")
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .w_full()
            .when(self.plain, |d| d.h(px(44.)).px(px(6.)).text_size(px(19.)).line_height(px(26.)))
            .when(self.bare, |d| d.h(px(40.)).text_size(px(text::SECTION)).line_height(px(20.)))
            .when(self.pill, |d| d.h(px(30.)).text_size(px(text::SECTION)).line_height(px(20.)))
            .when(self.small, |d| d.h(px(26.)).text_size(px(text::BODY)).line_height(px(20.)))
            .when(self.tiny, |d| d.h(px(18.)).text_size(px(text::META)).line_height(px(15.)))
            .when(boxed && !inset, |d| d.h(px(crate::look::shape::BUTTON)).px(px(10.)).text_size(px(text::BODY)).line_height(px(20.)).rounded(px(crate::look::shape::CONTROL)).border_1())
            .when(boxed && inset, |d| d.h(px(crate::look::shape::PUSH + 2.)).px(px(9.)).text_size(px(text::BODY)).line_height(px(20.)).rounded(px(crate::look::shape::ROW)).border_1())
            .items_center()
            .border_color(match (self.focus_handle.is_focused(window), inset) {
                (true, _) => rgb(crate::ui::color::key()),
                (false, false) => rgb(crate::ui::color::field_line()),
                (false, true) => crate::ui::color::hairline(),
            })
            .when(boxed && !inset, |d| d.bg(rgb(crate::ui::color::panel())))
            .when(boxed && inset, |d| d.bg(crate::ui::color::push()))
            // (The pointer down on the text, not on its eye beside it.)
            .child(div().flex_1().min_w_0().h_full().flex().items_center().on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down)).child(TextElement { input: cx.entity() }))
            .children(eye)
    }
}

impl TextInput {
    /// The eye at a password's end: shows what's typed, or keeps it to dots. Pressed with the
    /// pointer, the keys stay in the field; reached with Tab, Space presses it (gpui's own
    /// keyboard click).
    fn eye_button(&self, window: &mut Window, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        use crate::ui::color;
        let large = self.plain;
        let on_eye = self.eye.is_focused(window);
        div()
            .id("eye")
            .track_focus(&self.eye)
            .flex_none()
            .ml(px(6.))
            .size(px(if large { 30. } else { 24. }))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(crate::look::shape::SMALL))
            .cursor(CursorStyle::PointingHand)
            .press_row()
            .border_1()
            .border_color(if on_eye { rgb(color::key()) } else { gpui::transparent_black().into() })
            .hover(|s| s.bg(rgb(color::soft())))
            .on_click(cx.listener(|this, event: &gpui::ClickEvent, window, cx| {
                match event {
                    // Enter is the field's (it joins, it asks): only Space presses the eye.
                    gpui::ClickEvent::Keyboard(k) if matches!(k.button, gpui::KeyboardButton::Enter) => {}
                    gpui::ClickEvent::Keyboard(_) => this.toggle_revealed(cx),
                    // With the pointer, the keys go back to the field, to type on.
                    _ => {
                        this.toggle_revealed(cx);
                        window.focus(&this.focus_handle, cx);
                    }
                }
            }))
            .relative()
            .child(crate::look::placed("password-eye"))
            .child(crate::icons::glyph(if self.revealed { "eye-off" } else { "eye" }, if large { 18. } else { 16. }, color::key()))
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}


/// How far along a line `wide` is drawn from in a field `room` wide, the caret at `caret`: as
/// it was (`was`), moved only as far as the caret needs to be in view; none when it all fits.
fn scrolled(was: Pixels, caret: Pixels, wide: Pixels, room: Pixels) -> Pixels {
    let room = (room - px(2.)).max(Pixels::ZERO);
    let mut scroll = was;
    if caret - scroll > room {
        scroll = caret - room;
    }
    if caret < scroll {
        scroll = caret;
    }
    scroll.min((wide - room).max(Pixels::ZERO)).max(Pixels::ZERO)
}

#[cfg(test)]
mod scroll_tests {
    use super::scrolled;
    use gpui::px;

    #[test]
    fn a_long_line_scrolls_to_keep_the_caret_in_view() {
        // Fits: never scrolled.
        assert_eq!(scrolled(px(0.), px(50.), px(80.), px(100.)), px(0.));
        // The caret past the right edge: just far enough to see it.
        assert_eq!(scrolled(px(0.), px(250.), px(300.), px(102.)), px(150.));
        // Back to the start: all the way back.
        assert_eq!(scrolled(px(150.), px(0.), px(300.), px(102.)), px(0.));
        // In view already: left as it was.
        assert_eq!(scrolled(px(150.), px(200.), px(300.), px(102.)), px(150.));
        // Never past the end.
        assert_eq!(scrolled(px(900.), px(300.), px(300.), px(102.)), px(200.));
    }
}
