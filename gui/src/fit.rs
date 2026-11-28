//! What fits in the window: its width, noted as each frame is drawn, and the page's layout
//! from it, as a Mac's apps reflow (never animated: a window being resized follows the
//! pointer). One set of breakpoints for every page, not a width of its own in each.
//!
//!   window < 720   the sidebar folds away (the toolbar's button shows it over the page), and
//!                  the side room shrinks (20, then 16 below 560)
//!   page   < 600   cards stack (picture above, words below); < 640 an app's Install goes
//!                  under its name, master and detail stack, forms go two by two
//!   page   > 760   a settings page (groups, forms) stops growing; any other past 1120
//!   grids          as many columns as tiles of their least width fit, up to their most

use std::sync::atomic::{AtomicU32, Ordering};

/// The window's width, in logical pixels (as gpui lays out): set in `Shell::body`.
static WINDOW: AtomicU32 = AtomicU32::new(0);

/// How wide a page's content gets at most (set as each page is drawn): a settings page's
/// groups keep their labels near their controls, a store's grid its tiles near each other.
static WIDEST: AtomicU32 = AtomicU32::new(0);

/// The most a settings page (groups of settings, forms) is wide, as a Mac's System Settings.
pub const SETTINGS_WIDEST: f32 = 760.;
/// The most any other page is wide (Discover's grid, Installed's list).
pub const PAGE_WIDEST: f32 = 1120.;

/// Below this window width the sidebar folds away.
pub const SIDEBAR_FROM: f32 = 720.;

pub fn set_window(width: f32) {
    WINDOW.store(width.to_bits(), Ordering::Relaxed);
}

/// The window's width (as wide as the first window Settings opens, before it's drawn).
pub fn window() -> f32 {
    match f32::from_bits(WINDOW.load(Ordering::Relaxed)) {
        w if w > 0. => w,
        _ => 980.,
    }
}

pub fn set_widest(width: f32) {
    WIDEST.store(width.to_bits(), Ordering::Relaxed);
}

fn widest() -> f32 {
    match f32::from_bits(WIDEST.load(Ordering::Relaxed)) {
        w if w > 0. => w,
        _ => PAGE_WIDEST,
    }
}

/// Whether the sidebar's folded away, at this window width.
pub fn sidebar_folds_at(window: f32) -> bool {
    window < SIDEBAR_FROM
}

pub fn sidebar_folds() -> bool {
    sidebar_folds_at(window())
}

/// The page's room at each side (and below), for this window width: less in a narrow window.
pub fn gutter_at(window: f32) -> f32 {
    if window < 560. {
        16.
    } else if window < SIDEBAR_FROM {
        20.
    } else {
        crate::look::shape::GUTTER
    }
}

pub fn gutter() -> f32 {
    gutter_at(window())
}

/// How wide a page's content is in a window this wide: less the sidebar (when it's there)
/// and the gutters.
pub fn page_at(window: f32) -> f32 {
    let side = if sidebar_folds_at(window) { 0. } else { crate::look::shape::SIDEBAR };
    (window - side - 2. * gutter_at(window)).max(0.)
}

/// The page's content width now (at most as wide as the page shown gets).
pub fn page() -> f32 {
    page_at(window()).min(widest())
}

/// The room at each side of a page held to its widest, past the gutter: for what's lined up
/// with it (the toolbar's title).
pub fn spare() -> f32 {
    ((page_at(window()) - widest()) / 2.).max(0.)
}

/// Whether the page is narrower than `width`.
pub fn narrower(width: f32) -> bool {
    page() < width
}

/// How many columns of tiles at least `least` wide, `gap` apart, fit in `width`: one at least,
/// `most` at most.
pub fn columns_in(width: f32, least: f32, gap: f32, most: usize) -> usize {
    (((width + gap) / (least + gap)).floor() as usize).clamp(1, most.max(1))
}

/// The same, in the page now.
pub fn columns(least: f32, gap: f32, most: usize) -> usize {
    columns_in(page(), least, gap, most)
}

/// A grid of `columns(least, gap, most)` columns, its tiles as wide as each other.
pub fn grid(least: f32, gap: f32, most: usize) -> gpui::Div {
    use gpui::{Styled, div};
    div().grid().grid_cols(columns(least, gap, most) as u16).gap(crate::look::px(gap)).w_full()
}

/// A row that stays on one line (a row of filters): wider than the page, it slides sideways
/// (a touchpad's swipe, or Shift and the wheel; the wheel alone still scrolls the page) and
/// fades into `fill` at the edge where more is, as a Mac's segmented rows in a narrow window.
/// `about`: how wide the row is about to be, for its first frame (before it's been laid out).
pub fn sliding(id: &'static str, scroll: &gpui::ScrollHandle, fill: gpui::Rgba, about: f32, row: gpui::Div) -> gpui::Div {
    use gpui::{InteractiveElement, ParentElement, StatefulInteractiveElement, Styled, div};
    let laid_out = scroll.bounds().size.width > gpui::px(0.);
    let (at, most) = if laid_out { (-scroll.offset().x, scroll.max_offset().x) } else { (gpui::px(0.), gpui::px((about - page()).max(0.))) };
    let clear = gpui::Rgba { a: 0., ..fill };
    let fade = || div().absolute().top_0().bottom_0().w(crate::look::px(32.));
    let left = (at > gpui::px(0.5)).then(|| fade().left_0().bg(gpui::linear_gradient(90., gpui::linear_color_stop(fill, 0.), gpui::linear_color_stop(clear, 1.))));
    let right = (at < most - gpui::px(0.5)).then(|| fade().right_0().bg(gpui::linear_gradient(270., gpui::linear_color_stop(fill, 0.), gpui::linear_color_stop(clear, 1.))));
    div()
        .relative()
        .w_full()
        .min_w_0()
        .child(div().id(id).w_full().flex().overflow_x_scroll().restrict_scroll_to_axis().track_scroll(scroll).child(row.flex_none()))
        .children(left)
        .children(right)
}

/// About how wide a row of pills with these labels is (Geist at the callout size, each pill's
/// room about its label, and the track's): for `sliding`'s first frame.
pub fn pills_about<'a>(labels: impl IntoIterator<Item = &'a str>) -> f32 {
    labels.into_iter().map(|l| l.chars().count() as f32 * 7.4 + 26. + 2.).sum::<f32>() + 6.
}

/// A tile's least width for an app in a grid: its icon, name, a line and its button side by side.
pub const APP_TILE: f32 = 236.;
/// A tile's least width for an app stacked (icon, then name and line, then the button).
pub const APP_CARD: f32 = 168.;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sidebar_folds_in_a_narrow_window() {
        assert!(sidebar_folds_at(530.));
        assert!(sidebar_folds_at(719.));
        assert!(!sidebar_folds_at(720.));
        // Half of a laptop's screen, the sidebar folded: the page has nearly all of it.
        assert_eq!(page_at(530.), 530. - 32.);
        assert_eq!(page_at(1200.), 1200. - 220. - 56.);
    }

    #[test]
    fn grids_take_as_many_columns_as_fit() {
        // Discover's tiles: four in a wide window, then three, two, one.
        let cols = |w: f32| columns_in(page_at(w), APP_TILE, 12., 4);
        assert_eq!(cols(1600.), 4);
        assert_eq!(cols(1200.), 3);
        assert_eq!(cols(900.), 2);
        assert_eq!(cols(720.), 1);
        assert_eq!(cols(560.), 2);
        assert_eq!(cols(480.), 1);
        // Never none, never past the most.
        assert_eq!(columns_in(0., 200., 12., 4), 1);
        assert_eq!(columns_in(5000., 200., 12., 4), 4);
    }
}
