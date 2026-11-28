//! A picker: type to find one of a list (time zones, languages, keyboard
//! layouts), click it to choose. Choosing emits `Chosen` with its value.

use gpui::prelude::*;
use crate::look::Press;
use gpui::{Context, ElementId, Entity, EventEmitter, FontWeight, ParentElement, Render, Styled, Window, div, rgb};
use crate::look::px;

use crate::input::TextInput;
use crate::look::Stop;
use crate::look::text;
use crate::ui::color;

pub struct Chosen(pub String);

pub struct Picker {
    pub input: Entity<TextInput>,
    /// Each choice: its value, and what it's called.
    options: Vec<(String, String)>,
    /// What's chosen now.
    pub value: String,
}

impl EventEmitter<Chosen> for Picker {}

impl Picker {
    pub fn new(cx: &mut Context<Self>, placeholder: &str, options: Vec<(String, String)>, value: &str) -> Self {
        let shown = label(&options, value);
        let input = cx.new(|cx| TextInput::new(cx, placeholder, &shown, false));
        cx.observe(&input, |_, _, cx| cx.notify()).detach();
        Picker { input, options, value: value.to_string() }
    }

    /// What's chosen, changed elsewhere (the file was put back).
    pub fn set(&mut self, value: &str, cx: &mut Context<Self>) {
        self.value = value.to_string();
        let shown = label(&self.options, value);
        self.input.update(cx, |i, cx| i.set(cx, &shown));
    }

    fn choose(&mut self, value: String, cx: &mut Context<Self>) {
        self.set(&value, cx);
        cx.emit(Chosen(value));
        cx.notify();
    }

    /// The choices matching what's typed (when it's not what's chosen).
    fn matches(&self, cx: &gpui::App) -> Vec<(String, String)> {
        let typed = self.input.read(cx).content.trim().to_lowercase();
        if typed.is_empty() || typed == label(&self.options, &self.value).to_lowercase() {
            return Vec::new();
        }
        let mut found: Vec<_> = self.options.iter().filter(|(v, l)| l.to_lowercase().contains(&typed) || v.to_lowercase().contains(&typed)).cloned().collect();
        // Those starting with it first.
        found.sort_by_key(|(v, l)| !(l.to_lowercase().starts_with(&typed) || v.to_lowercase().starts_with(&typed)));
        found.truncate(6);
        found
    }
}

/// How a value is shown: its name and value, or the value alone.
fn label(options: &[(String, String)], value: &str) -> String {
    match options.iter().find(|(v, _)| v == value) {
        Some((v, l)) if l != v => format!("{l} ({v})"),
        _ => value.to_string(),
    }
}

impl Render for Picker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let found = self.matches(cx);
        let typed = !self.input.read(cx).content.trim().is_empty() && found.is_empty() && self.input.read(cx).content.trim() != label(&self.options, &self.value);
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(self.input.clone())
            .when(!found.is_empty(), |d| {
                d.child(div().flex().flex_col().p(px(4.)).rounded(px(crate::look::shape::ROW + 2.)).border_1().border_color(color::hairline()).bg(rgb(color::panel())).shadow(crate::look::soft_shadow()).children(found.into_iter().enumerate().map(|(n, (v, l))| {
                    let shown = if l == v { v.clone() } else { l.clone() };
                    let value = v.clone();
                    div()
                        .id(ElementId::NamedInteger("choice".into(), n as u64))
                        .flex()
                        .justify_between()
                        .gap(px(12.))
                        .px(px(10.))
                        .py(px(6.))
                        .rounded(px(6.))
                        .text_size(px(text::CALLOUT))
                        .cursor_pointer()
                        .press_row()
                        .hover(|s| s.bg(color::key_wash()))
                        .stop(cx)
                        .on_click(cx.listener(move |this, _, _, cx| this.choose(value.clone(), cx)))
                        .child(div().min_w_0().font_weight(FontWeight::MEDIUM).child(crate::tip::cut(shown)))
                        .when(l != v, |d| d.child(div().flex_none().max_w(px(150.)).truncate().font_family("Geist Mono").text_size(px(text::META)).text_color(rgb(color::muted())).child(v.clone())))
                })))
            })
            .when(typed, |d| d.child(div().text_size(px(text::META)).text_color(rgb(color::muted())).child("No matches.")))
    }
}
