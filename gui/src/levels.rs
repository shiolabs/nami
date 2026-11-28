//! The levels the keys change (the volume, a screen's brightness, the keyboard's light): in
//! sixteen steps, as a Mac's are, or a quarter of one with Shift+Alt (Option-Shift); and the
//! glyph that says each level.
// (Shared: nami-gui has the steps, nami-desktop the glyphs too.)
#![allow(dead_code)]

/// A key's step: a sixteenth.
pub const STEPS: u32 = 16;
/// Shift+Alt with it: a quarter of one.
pub const FINE: u32 = 64;

/// A level (0 to 1) a step up or down, on a grid of `steps`: from off the grid, to the next line
/// of it; from on it (give or take what a tool rounds it to), to the line after.
pub fn step(level: f32, up: bool, steps: u32) -> f32 {
    let steps = steps.max(1) as f32;
    let n = level.clamp(0., 1.) * steps;
    let near = n.round();
    // (wpctl says a volume to two places, a backlight in whole percents: on the line, near enough.)
    let at = if (n - near).abs() <= 0.006 * steps { near } else if up { n.floor() } else { n.ceil() };
    let to = if up { at + 1. } else { at - 1. };
    to.clamp(0., steps) / steps
}

/// A level as a whole percent.
pub fn percent(level: f32) -> u32 {
    (level.clamp(0., 1.) * 100.).round() as u32
}

/// The speaker for the volume: crossed out muted; silent at nothing; one, two or three waves up
/// to a third, two thirds, and over.
pub fn sound_glyph(percent: u32, muted: bool) -> &'static str {
    match percent {
        _ if muted => "volume-muted",
        0 => "volume-0",
        1..=33 => "volume-1",
        34..=66 => "volume-2",
        _ => "volume-3",
    }
}

/// The sun for a screen's brightness: its rays grow with it, dots at the dimmest.
pub fn sun_glyph(percent: u32) -> &'static str {
    match percent {
        0..=12 => "sun-0",
        13..=45 => "sun-1",
        46..=78 => "sun-2",
        _ => "sun-3",
    }
}

/// The keyboard for its light: no rays when it's off, dots, short rays, long.
pub fn keyboard_glyph(percent: u32) -> &'static str {
    match percent {
        0 => "keyboard-light-0",
        1..=33 => "keyboard-light-1",
        34..=66 => "keyboard-light-2",
        _ => "keyboard-light-3",
    }
}

/// A keyboard light's next raw value: one of its own at a time where it has few (a ThinkPad's
/// off, low, high), else a sixteenth.
pub fn keyboard_step(now: u32, max: u32, up: bool, steps: u32) -> u32 {
    if max <= steps {
        return if up { (now + 1).min(max) } else { now.saturating_sub(1) };
    }
    (step(now as f32 / max as f32, up, steps) * max as f32).round() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sixteen_steps_from_nothing_to_full() {
        let mut level = 0.;
        let mut seen = vec![];
        for _ in 0..20 {
            level = step(level, true, STEPS);
            seen.push(level);
        }
        assert_eq!(seen[0], 1. / 16.);
        assert_eq!(seen[15], 1.);
        assert_eq!(seen[19], 1.);
        for _ in 0..20 {
            level = step(level, false, STEPS);
        }
        assert_eq!(level, 0.);
    }

    #[test]
    fn off_the_grid_goes_to_its_next_line() {
        // 40%: up to 7/16 (43.75%), down to 6/16 (37.5%).
        assert_eq!(step(0.40, true, STEPS), 7. / 16.);
        assert_eq!(step(0.40, false, STEPS), 6. / 16.);
        // As wpctl says 7/16 (0.4375): 0.44, still on its line.
        assert_eq!(step(0.44, true, STEPS), 8. / 16.);
        assert_eq!(step(0.44, false, STEPS), 6. / 16.);
        // A backlight in whole percents: 6% is 1/16.
        assert_eq!(step(0.06, true, STEPS), 2. / 16.);
    }

    #[test]
    fn a_quarter_step_with_shift_alt() {
        assert_eq!(step(0.5, true, FINE), 33. / 64.);
        assert_eq!(step(0.5, false, FINE), 31. / 64.);
        // 33/64 (0.515625) as wpctl says it, 0.52: on its line.
        assert_eq!(step(0.52, true, FINE), 34. / 64.);
        // Four of them are one of the others.
        let mut level = 0.25;
        for _ in 0..4 {
            level = step(level, true, FINE);
        }
        assert_eq!(level, step(0.25, true, STEPS));
    }

    #[test]
    fn the_glyphs_follow_the_level() {
        assert_eq!(sound_glyph(50, true), "volume-muted");
        assert_eq!(sound_glyph(0, false), "volume-0");
        assert_eq!(sound_glyph(6, false), "volume-1");
        assert_eq!(sound_glyph(33, false), "volume-1");
        assert_eq!(sound_glyph(50, false), "volume-2");
        assert_eq!(sound_glyph(67, false), "volume-3");
        assert_eq!(sound_glyph(100, false), "volume-3");
        assert_eq!((sun_glyph(0), sun_glyph(25), sun_glyph(60), sun_glyph(100)), ("sun-0", "sun-1", "sun-2", "sun-3"));
        assert_eq!((keyboard_glyph(0), keyboard_glyph(30), keyboard_glyph(50), keyboard_glyph(100)), ("keyboard-light-0", "keyboard-light-1", "keyboard-light-2", "keyboard-light-3"));
    }

    #[test]
    fn a_keyboard_light_steps_by_its_own_levels() {
        // A ThinkPad's: off, low, high.
        assert_eq!(keyboard_step(0, 2, true, STEPS), 1);
        assert_eq!(keyboard_step(2, 2, true, STEPS), 2);
        assert_eq!(keyboard_step(1, 2, false, STEPS), 0);
        // A Mac's, 0 to 255: sixteenths.
        assert_eq!(keyboard_step(0, 255, true, STEPS), 16);
        assert_eq!(keyboard_step(255, 255, false, STEPS), 239);
    }
}
