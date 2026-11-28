//! Report a problem: what happened, in your words, sent with `nami report`
//! (with nami doctor, the last steps and your nami files, secrets left out),
//! all of which can be read here before it goes.

use gpui::prelude::*;
use crate::look::Press;
use gpui::{Context, Div, Entity, FontWeight, ParentElement, StatefulInteractiveElement, Styled, div, rgb};
use crate::look::px;
use crate::look::text;

use crate::editor::Editor;
use crate::nami;
use crate::ui::{button, color, tick};
use crate::Shell;

pub struct Report {
    pub editor: Entity<Editor>,
    /// The nami files go too.
    pub files: bool,
    /// What's sent, shown (and as `nami report --show` has it).
    pub showing: bool,
    pub preview: Option<Result<String, String>>,
    pub sending: bool,
    /// The report's number, or why it didn't go.
    pub sent: Option<Result<String, String>>,
}

impl Report {
    pub fn new(cx: &mut Context<Shell>) -> Self {
        let editor = cx.new(|cx| {
            let mut e = Editor::new(cx, "");
            e.plain = true;
            e.vim = false;
            e
        });
        // What's shown of what's sent follows what's typed.
        cx.observe(&editor, |g, _, cx| {
            if g.report.showing {
                preview(g, cx);
            }
        })
        .detach();
        Report { editor, files: true, showing: false, preview: None, sending: false, sent: None }
    }
}

fn message(g: &Shell, cx: &gpui::App) -> String {
    g.report.editor.read(cx).content.trim().to_string()
}

fn preview(g: &mut Shell, cx: &mut Context<Shell>) {
    let (text, files) = (message(g, cx), g.report.files);
    cx.spawn(async move |this, cx| {
        let shown = cx.background_executor().spawn(async move { nami::report_preview(&text, files) }).await;
        let _ = this.update(cx, |g, cx| {
            g.report.preview = Some(shown);
            cx.notify();
        });
    })
    .detach();
}

fn send(g: &mut Shell, cx: &mut Context<Shell>) {
    let (text, files) = (message(g, cx), g.report.files);
    g.report.sending = true;
    g.report.sent = None;
    cx.notify();
    cx.spawn(async move |this, cx| {
        let sent = cx.background_executor().spawn(async move { nami::report_send(&text, files) }).await;
        let _ = this.update(cx, |g, cx| {
            g.report.sending = false;
            if sent.is_ok() {
                g.report.editor.update(cx, |e, cx| e.set(cx, ""));
                g.report.showing = false;
            }
            g.report.sent = Some(sent);
            cx.notify();
        });
    })
    .detach();
}

pub fn render(g: &mut Shell, cx: &mut Context<Shell>) -> Div {
    let r = &g.report;
    let ready = !message(g, cx).is_empty() && !r.sending;
    let mut page = div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(crate::ui::intro("Something not working, or unclear? Describe what you did, what you expected and what happened instead. Your report goes to the nami team."))
        .child(r.editor.clone())
        .child(
            div()
                .id("files")
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(px(text::CALLOUT))
                .cursor_pointer()
                .press()
                .on_click(cx.listener(|g, _, _, cx| {
                    g.report.files = !g.report.files;
                    if g.report.showing {
                        preview(g, cx);
                    }
                    cx.notify();
                }))
                .child(tick("files-tick".into(), r.files))
                .child("Include my nami files (secrets are removed)"),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(button("send", if r.sending { "Sending…" } else { "Send" }, true, ready).when(ready, |b| b.on_click(cx.listener(|g, _, _, cx| send(g, cx)))))
                .child(button("show", if r.showing { "Hide Report Contents" } else { "Show Report Contents" }, false, true).on_click(cx.listener(|g, _, _, cx| {
                    g.report.showing = !g.report.showing;
                    if g.report.showing {
                        preview(g, cx);
                    }
                    cx.notify();
                }))),
        );
    page = match &r.sent {
        Some(Ok(id)) => page.child(div().p(px(14.)).rounded(px(crate::look::shape::CONTROL)).bg(rgb(color::good_soft())).text_size(px(text::BODY)).text_color(rgb(color::good())).child(format!("Report {id} sent. Thank you! Include this number if you follow up."))),
        Some(Err(why)) => page.child(div().p(px(14.)).rounded(px(crate::look::shape::CONTROL)).bg(rgb(color::bad_soft())).text_size(px(text::CALLOUT)).text_color(rgb(color::bad())).child(format!("Couldn't send the report. {why}"))),
        None => page,
    };
    if r.showing {
        let shown = match &r.preview {
            None => "…".to_string(),
            Some(Ok(t)) => t.clone(),
            Some(Err(why)) => format!("Couldn't show the report contents. {why}"),
        };
        page = page.child(div().flex().flex_col().gap(px(6.)).child(div().text_size(px(text::CALLOUT)).font_weight(FontWeight::SEMIBOLD).child("Report Contents")).child(div().p(px(12.)).rounded(px(crate::look::shape::ROW)).bg(color::push()).text_size(px(text::META)).font_family("Geist Mono").child(shown)));
    }
    page
}
