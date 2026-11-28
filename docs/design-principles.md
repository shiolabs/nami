# Design principles

How nami and namiOS look, move and speak. Written to be reused: nothing here depends on
nami, apart from the examples.

## 1. Behave like a Mac, look like yourself

- Copy the behaviour people already know: shortcuts, window handling, Settings layout,
  Undo, an Unlock button for admin settings, dialogs with Cancel on the left and the main
  action on the right.
- Don't copy the look. No traffic-light buttons, no Apple icons, no SF Pro. Use your own
  shapes, your own accent and your own font.
- The bar is Apple level: consistent, quick, no rough edges. Polish comes from many small
  details done the same way everywhere, not from one big effect.

## 2. Colour: one accent, and rarely

- **One accent colour** (slate `#45577a`; `#a9bad6` in dark mode). There's no second
  brand colour.
- **One filled accent button per screen**: the main action (Install, Update All, Save).
  Every other button is grey.
- **Selection is grey**: a soft grey fill with dark text. It's never an accent bar or
  coloured chip.
- **Badges, counts and filters are grey.**
- **Hierarchy comes from weight and grey tones of text**, not colour.
- **Colour only where it means something**: links, the main action, state (running,
  changed, error). App icons and photos bring the rest of the colour.
- **State colours are muted**, never neon: green `#4f7a5a`, yellow `#8a6a2e`, red `#a8504a`.
  Dark mode gets lighter tints of the same colours.
- **Every colour is a token** with a light and a dark value. There are no colour literals
  in components.

## 3. Type

- **One sans and one mono from the same family:** Geist and Geist Mono. Fall back to the
  system UI font.
- **Text tones:** `strong` (#0c0c0e) for titles, `text` (#18181b) for body, `text-2`
  (#52525b) for secondary text and `text-3` (#6f6f78) for hints.
- **Big headlines are tight and heavy:** large size, negative letter-spacing, short lines.
  Body text is calm and roomy.
- **Title Case** for buttons, menu items, window titles and headings. **Sentence case** for
  setting labels and descriptions.

## 4. Shape, space, depth

- **Radii come from a small scale:**
  - rows: 8
  - controls: 10
  - menus: 12
  - cards: 14
  - floating panels: 18
  - control groups: 22
  - pills: 999

  Inner radius = outer radius − padding.
- **Segmented controls, filters, search fields and command boxes are pills.** A segmented
  control is a white raised pill on a grey track (`rgb(24 24 27 / .07)`).
- **Lines are hairlines:** `rgb(24 24 27 / .10)`, or 0.5px where the screen allows it.
- **Shadows are soft and layered:** a 1px contact shadow plus a wide, faint ambient one
  (`0 1px 2px / .03, 0 12px 32px -8px / .10`). There are no hard drop shadows.
- **Glass:**
  - Frosted tint at about 66% white, with blur behind.
  - A bright rim on the side facing the light.
  - Optional lensing that bends what's behind at the rounded edges.
  - Bigger surfaces are "thicker".
  - Reduce transparency and high contrast turn all of it off.
- **Generous whitespace.** When in doubt, remove a border before adding one.

## 5. Motion


- **Ease-out for things entering and leaving.** Use a strong curve:
  `cubic-bezier(0.16, 1, 0.3, 1)` or `(0.23, 1, 0.32, 1)`.
- **UI animations stay under 300 ms.** Crossfades in a showcase may take 700–900 ms.
- **No animation on things done 100 times a day:** launcher, app switcher, typing.
- **Nothing scales up from 0.** Start at 0.95–0.97 plus a fade. Popovers grow from the
  button that opened them.
- **Transitions can be interrupted**; never make the user wait for one to finish.
- **Always honour reduce-motion.**
- **Nothing jumps:** reserve space for content that arrives later, using fixed heights for
  typed or swapped text.

## 6. Words on screen

This is the macOS/HIG voice.

- **Plain and short, with concrete nouns.** Don't use clever phrasing, clause chains or code-comment style
  ("delete forgets it").
- **Standard verbs:** Add, Remove, Turn On, Turn Off, Clear All, Cancel, Done, Undo.
- **Empty states are a title plus one sentence:** "No Items" / "Text and images you copy
  appear here."
- **Errors say what happened and what to do**, without blame or jargon.
- **Short names:** `settings`, `shell`, `unfree`, not `desktop-settings`, `shell-init` or
  `allow-unfree`. The docs and error messages carry the clarity, not longer keys.
- **Correct domain language:** for example "Linux distribution", never "Linuxes" or "new
  operating system".

## 10. How we work

- **Audit, then plan, then fix,** in rounds. Look at screenshots of every change, light and
  dark, desktop and phone, before calling it done.
- **Test with the real habits of the user** (for example Super+Tab, not Alt+Tab).
- **When something feels "too much"** (colour, text, motion), the answer is almost always to
  remove, not to rebalance.
