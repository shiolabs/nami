//! A nami file as it'll be on the computer, in the editor's colours (its
//! `runs`, as the website's); the lines that just changed lit for a moment. The namiOS Installer's preview of
//! what it hands namiOS, and Move to namiOS's in nami-gui.

use std::time::Duration;

use gpui::prelude::*;
use gpui::{Animation, AnimationExt as _, HighlightStyle, StyledText, TextRun, div, rgb};

use crate::look::{color, px, text};

pub fn file_view(id: &'static str, said: &str, changed: Option<&(usize, Vec<usize>)>) -> gpui::Stateful<gpui::Div> {
    let base = TextRun { len: 0, font: gpui::font("Geist Mono"), color: rgb(color::text()).into(), background_color: None, underline: None, strikethrough: None };
    let lit = rgb(color::key());
    div()
        .id(id)
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scroll()
        .rounded(px(crate::look::shape::CONTROL))
        .border_1()
        .border_color(rgb(color::line()))
        .bg(rgb(color::ground()))
        .py(px(10.))
        .font_family("Geist Mono")
        .text_size(px(text::META))
        .line_height(px(19.))
        .children(said.lines().enumerate().map(|(i, l)| {
            let mut at = 0;
            let colours: Vec<_> = crate::editor::runs(l, base.clone())
                .into_iter()
                .map(|r| {
                    let range = at..at + r.len;
                    at += r.len;
                    (range, HighlightStyle { color: Some(r.color), font_weight: Some(r.font.weight), ..Default::default() })
                })
                .collect();
            // (A long line wraps: all of it is seen.)
            let row = div().px(px(12.)).min_h(px(19.)).flex_none().child(StyledText::new(l.to_string()).with_highlights(colours));
            match changed {
                // (Lit, then fading back over a second: said once, not a show.)
                Some((n, lines)) if lines.contains(&i) => row
                    .with_animation(gpui::ElementId::Name(format!("changed-{n}-{i}").into()), Animation::new(Duration::from_millis(1400)), move |el, t| {
                        let fade = if t < 0.35 { 1. } else { 1. - crate::motion::ease_out()((t - 0.35) / 0.65) };
                        el.bg(gpui::Rgba { a: 0.16 * fade, ..lit })
                    })
                    .into_any_element(),
                _ => row.into_any_element(),
            }
        }))
}
