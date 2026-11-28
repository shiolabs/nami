//! The window's own icons, Lucide's (ISC licence): line drawings in the
//! colour of the text around them.

use std::borrow::Cow;

use gpui::{SharedString, Styled, Svg, rgb, svg};
use crate::look::px;

const ICONS: &[(&str, &str)] = &[
    ("home", r#"<path d="M15 21v-8a1 1 0 0 0-1-1h-4a1 1 0 0 0-1 1v8"/><path d="M3 10a2 2 0 0 1 .709-1.528l7-5.999a2 2 0 0 1 2.582 0l7 5.999A2 2 0 0 1 21 10v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>"#),
    ("apps", r#"<rect width="7" height="7" x="3" y="3" rx="1"/><rect width="7" height="7" x="14" y="3" rx="1"/><rect width="7" height="7" x="14" y="14" rx="1"/><rect width="7" height="7" x="3" y="14" rx="1"/>"#),
    ("setup", r#"<path d="M20 7h-9"/><path d="M14 17H5"/><circle cx="17" cy="17" r="3"/><circle cx="7" cy="7" r="3"/>"#),
    ("people", r#"<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M22 21v-2a4 4 0 0 0-3-3.87"/><path d="M16 3.13a4 4 0 0 1 0 7.75"/>"#),
    ("settings", r#"<path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z"/><circle cx="12" cy="12" r="3"/>"#),
    ("updates", r#"<path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8"/><path d="M21 3v5h-5"/><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16"/><path d="M8 16H3v5"/>"#),
    ("sync", r#"<path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/><path d="M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16"/><path d="M16 16h5v5"/>"#),
    ("backups", r#"<rect width="20" height="5" x="2" y="3" rx="1"/><path d="M4 8v11a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8"/><path d="M10 12h4"/>"#),
    ("history", r#"<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/><path d="M12 7v5l4 2"/>"#),
    ("computer", r#"<rect width="20" height="14" x="2" y="3" rx="2"/><line x1="8" x2="16" y1="21" y2="21"/><line x1="12" x2="12" y1="17" y2="21"/>"#),
    ("report", r#"<path d="M7.9 20A9 9 0 1 0 4 16.1L2 22Z"/><path d="M12 8v4"/><path d="M12 16h.01"/>"#),
    ("disk", r#"<line x1="22" x2="2" y1="12" y2="12"/><path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z"/><line x1="6" x2="6.01" y1="16" y2="16"/><line x1="10" x2="10.01" y1="16" y2="16"/>"#),
    // Discover: the store's front page.
    ("compass", r#"<circle cx="12" cy="12" r="10"/><path d="m16.24 7.76-2.12 6.36-6.36 2.12 2.12-6.36 6.36-2.12z"/>"#),
    ("plus", r#"<path d="M5 12h14"/><path d="M12 5v14"/>"#),
    ("clock", r#"<circle cx="12" cy="12" r="10"/><path d="M12 6v6l4 2"/>"#),
    ("arrow", r#"<path d="M5 12h14"/><path d="m12 5 7 7-7 7"/>"#),
    // The island's shelf: what's kept a while.
    ("inbox", r#"<polyline points="22 12 16 12 14 15 10 15 8 12 2 12"/><path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z"/>"#),
    ("file", r#"<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/>"#),
    ("folder", r#"<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"/>"#),
    ("code", r#"<path d="m16 18 6-6-6-6"/><path d="m8 6-6 6 6 6"/>"#),
    ("check", r#"<path d="M20 6 9 17l-5-5"/>"#),
    ("trash", r#"<path d="M3 6h18"/><path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6"/><path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2"/>"#),
    // A spinner's: its track, and the arc that turns on it.
    ("ring", r#"<circle cx="12" cy="12" r="9"/>"#),
    ("arc", r#"<path d="M12 3a9 9 0 0 1 9 9"/>"#),
    ("cpu", r#"<rect width="16" height="16" x="4" y="4" rx="2"/><rect width="6" height="6" x="9" y="9" rx="1"/><path d="M15 2v2"/><path d="M15 20v2"/><path d="M2 15h2"/><path d="M2 9h2"/><path d="M20 15h2"/><path d="M20 9h2"/><path d="M9 2v2"/><path d="M9 20v2"/>"#),
    ("bell", r#"<path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9"/><path d="M10.3 21a1.94 1.94 0 0 0 3.4 0"/>"#),
    // Do Not Disturb: the bell, struck through.
    ("bell-off", r#"<path d="M8.7 3A6 6 0 0 1 18 8a21.3 21.3 0 0 0 .6 5"/><path d="M17 17H3s3-2 3-9a4.67 4.67 0 0 1 .3-1.7"/><path d="M10.3 21a1.94 1.94 0 0 0 3.4 0"/><path d="m2 2 20 20"/>"#),
    // Painting (the welcome's picture of Krita, until the catalog's icon is there).
    ("brush", r#"<path d="m9.06 11.9 8.07-8.06a2.85 2.85 0 1 1 4.03 4.03l-8.06 8.08"/><path d="M7.07 14.94c-1.66 0-3 1.35-3 3.02 0 1.33-2.5 1.52-2 2.02 1.08 1.1 2.49 2.02 4 2.02 2.2 0 4-1.8 4-4.04a3.01 3.01 0 0 0-3-3.02z"/>"#),
    // The control center: three sliders.
    ("controls", r#"<line x1="21" x2="14" y1="4" y2="4"/><line x1="10" x2="3" y1="4" y2="4"/><line x1="21" x2="12" y1="12" y2="12"/><line x1="8" x2="3" y1="12" y2="12"/><line x1="21" x2="16" y1="20" y2="20"/><line x1="12" x2="3" y1="20" y2="20"/><line x1="14" x2="14" y1="2" y2="6"/><line x1="8" x2="8" y1="10" y2="14"/><line x1="16" x2="16" y1="18" y2="22"/>"#),
    ("clipboard", r#"<rect width="8" height="4" x="8" y="2" rx="1" ry="1"/><path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2"/>"#),
    ("memory", r#"<path d="M6 19v-3"/><path d="M10 19v-3"/><path d="M14 19v-3"/><path d="M18 19v-3"/><path d="M8 11V9"/><path d="M16 11V9"/><path d="M12 11V9"/><path d="M2 15h20"/><path d="M2 7a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2v1.1a2 2 0 0 0 0 3.837V17a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2v-5.1a2 2 0 0 0 0-3.837Z"/>"#),
    ("wifi", r#"<path d="M12 20h.01"/><path d="M2 8.82a15 15 0 0 1 20 0"/><path d="M5 12.859a10 10 0 0 1 14 0"/><path d="M8.5 16.429a5 5 0 0 1 7 0"/>"#),
    ("wifi-off", r#"<path d="M12 20h.01"/><path d="M8.5 16.429a5 5 0 0 1 7 0"/><path d="M5 12.859a10 10 0 0 1 5.17-2.69"/><path d="M19 12.859a10 10 0 0 0-2.007-1.523"/><path d="M2 8.82a15 15 0 0 1 4.177-2.643"/><path d="M22 8.82a15 15 0 0 0-11.288-3.764"/><path d="m2 2 20 20"/>"#),
    ("network", r#"<rect x="16" y="16" width="6" height="6" rx="1"/><rect x="2" y="16" width="6" height="6" rx="1"/><rect x="9" y="2" width="6" height="6" rx="1"/><path d="M5 16v-3a1 1 0 0 1 1-1h12a1 1 0 0 1 1 1v3"/><path d="M12 12V8"/>"#),
    ("volume", r#"<path d="M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298z"/><path d="M16 9a5 5 0 0 1 0 6"/><path d="M19.364 18.364a9 9 0 0 0 0-12.728"/>"#),
    ("volume-mute", r#"<path d="M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298z"/><line x1="22" x2="16" y1="9" y2="15"/><line x1="16" x2="22" y1="9" y2="15"/>"#),
    // The levels' own (osd.rs, the island, the bar): the speaker with a wave for each third of
    // its volume (crossed out muted), the sun with its rays growing with the brightness, the
    // keyboard with its light's. Drawn here in Lucide's line.
    ("volume-0", r#"<path d="M10 4.702a.705.705 0 0 0-1.203-.498L5.413 7.587A1.4 1.4 0 0 1 4.416 8H2a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 10 19.298z"/>"#),
    ("volume-1", r#"<path d="M10 4.702a.705.705 0 0 0-1.203-.498L5.413 7.587A1.4 1.4 0 0 1 4.416 8H2a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 10 19.298z"/><path d="M13.09 9.50a3.6 3.6 0 0 1 0 5.00"/>"#),
    ("volume-2", r#"<path d="M10 4.702a.705.705 0 0 0-1.203-.498L5.413 7.587A1.4 1.4 0 0 1 4.416 8H2a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 10 19.298z"/><path d="M13.09 9.50a3.6 3.6 0 0 1 0 5.00"/><path d="M15.36 6.96a7 7 0 0 1 0 10.07"/>"#),
    ("volume-3", r#"<path d="M10 4.702a.705.705 0 0 0-1.203-.498L5.413 7.587A1.4 1.4 0 0 1 4.416 8H2a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 10 19.298z"/><path d="M13.09 9.50a3.6 3.6 0 0 1 0 5.00"/><path d="M15.36 6.96a7 7 0 0 1 0 10.07"/><path d="M17.59 4.39a10.4 10.4 0 0 1 0 15.21"/>"#),
    ("volume-muted", r#"<path d="M10 4.702a.705.705 0 0 0-1.203-.498L5.413 7.587A1.4 1.4 0 0 1 4.416 8H2a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 10 19.298z"/><path d="m21 9-6 6"/><path d="m15 9 6 6"/>"#),
    ("sun-0", r#"<circle cx="12" cy="12" r="4"/><path d="M19.50 12.00h.01"/><path d="M17.30 17.30h.01"/><path d="M12.00 19.50h.01"/><path d="M6.70 17.30h.01"/><path d="M4.50 12.00h.01"/><path d="M6.70 6.70h.01"/><path d="M12.00 4.50h.01"/><path d="M17.30 6.70h.01"/>"#),
    ("sun-1", r#"<circle cx="12" cy="12" r="4"/><path d="M19.50 12.00L20.40 12.00"/><path d="M17.30 17.30L17.94 17.94"/><path d="M12.00 19.50L12.00 20.40"/><path d="M6.70 17.30L6.06 17.94"/><path d="M4.50 12.00L3.60 12.00"/><path d="M6.70 6.70L6.06 6.06"/><path d="M12.00 4.50L12.00 3.60"/><path d="M17.30 6.70L17.94 6.06"/>"#),
    ("sun-2", r#"<circle cx="12" cy="12" r="4"/><path d="M19.50 12.00L21.30 12.00"/><path d="M17.30 17.30L18.58 18.58"/><path d="M12.00 19.50L12.00 21.30"/><path d="M6.70 17.30L5.42 18.58"/><path d="M4.50 12.00L2.70 12.00"/><path d="M6.70 6.70L5.42 5.42"/><path d="M12.00 4.50L12.00 2.70"/><path d="M17.30 6.70L18.58 5.42"/>"#),
    ("sun-3", r#"<circle cx="12" cy="12" r="4"/><path d="M19.50 12.00L22.20 12.00"/><path d="M17.30 17.30L19.21 19.21"/><path d="M12.00 19.50L12.00 22.20"/><path d="M6.70 17.30L4.79 19.21"/><path d="M4.50 12.00L1.80 12.00"/><path d="M6.70 6.70L4.79 4.79"/><path d="M12.00 4.50L12.00 1.80"/><path d="M17.30 6.70L19.21 4.79"/>"#),
    ("keyboard-light-0", r#"<rect width="20" height="10" x="2" y="11" rx="2"/><path d="M6.5 14.5h.01"/><path d="M10.17 14.5h.01"/><path d="M13.83 14.5h.01"/><path d="M17.5 14.5h.01"/><path d="M8 17.5h8"/>"#),
    ("keyboard-light-1", r#"<rect width="20" height="10" x="2" y="11" rx="2"/><path d="M6.5 14.5h.01"/><path d="M10.17 14.5h.01"/><path d="M13.83 14.5h.01"/><path d="M17.5 14.5h.01"/><path d="M8 17.5h8"/><path d="M12 7.5h.01"/><path d="M7 8h.01"/><path d="M17 8h.01"/>"#),
    ("keyboard-light-2", r#"<rect width="20" height="10" x="2" y="11" rx="2"/><path d="M6.5 14.5h.01"/><path d="M10.17 14.5h.01"/><path d="M13.83 14.5h.01"/><path d="M17.5 14.5h.01"/><path d="M8 17.5h8"/><path d="M12 7.5L12.00 5.25"/><path d="M7 8L6.00 6.40"/><path d="M17 8L18.00 6.40"/>"#),
    ("keyboard-light-3", r#"<rect width="20" height="10" x="2" y="11" rx="2"/><path d="M6.5 14.5h.01"/><path d="M10.17 14.5h.01"/><path d="M13.83 14.5h.01"/><path d="M17.5 14.5h.01"/><path d="M8 17.5h8"/><path d="M12 7.5L12.00 3.00"/><path d="M7 8L5.00 4.80"/><path d="M17 8L19.00 4.80"/>"#),
    ("battery", r#"<rect width="16" height="10" x="2" y="7" rx="2" ry="2"/><line x1="22" x2="22" y1="11" y2="13"/><line x1="6" x2="6" y1="11" y2="13"/><line x1="10" x2="10" y1="11" y2="13"/>"#),
    // How windows go: side by side in a row (Lucide's gallery-horizontal), the screen split
    // (layout-dashboard), over each other (copy).
    ("windows-scrolling", r#"<path d="M2 3v18"/><rect width="12" height="18" x="6" y="3" rx="2"/><path d="M22 3v18"/>"#),
    ("windows-tiling", r#"<rect width="7" height="9" x="3" y="3" rx="1"/><rect width="7" height="5" x="14" y="3" rx="1"/><rect width="7" height="9" x="14" y="12" rx="1"/><rect width="7" height="5" x="3" y="16" rx="1"/>"#),
    ("windows-floating", r#"<rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>"#),
    // One of a list, to open (the first start's pickers).
    ("chevrons-up-down", r#"<path d="m7 15 5 5 5-5"/><path d="m7 9 5-5 5 5"/>"#),
    // A pen tablet, over Bluetooth.
    ("pen", r#"<path d="M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z"/>"#),
    // How full, for the login and lock screens' corner.
    ("battery-full", r#"<rect width="16" height="10" x="2" y="7" rx="2" ry="2"/><line x1="22" x2="22" y1="11" y2="13"/><line x1="6" x2="6" y1="11" y2="13"/><line x1="10" x2="10" y1="11" y2="13"/><line x1="14" x2="14" y1="11" y2="13"/>"#),
    ("battery-medium", r#"<rect width="16" height="10" x="2" y="7" rx="2" ry="2"/><line x1="22" x2="22" y1="11" y2="13"/><line x1="6" x2="6" y1="11" y2="13"/><line x1="10" x2="10" y1="11" y2="13"/>"#),
    ("battery-low", r#"<rect width="16" height="10" x="2" y="7" rx="2" ry="2"/><line x1="22" x2="22" y1="11" y2="13"/><line x1="6" x2="6" y1="11" y2="13"/>"#),
    ("battery-empty", r#"<rect width="16" height="10" x="2" y="7" rx="2" ry="2"/><line x1="22" x2="22" y1="11" y2="13"/>"#),
    ("battery-charging",r#"<path d="M15 7h1a2 2 0 0 1 2 2v6a2 2 0 0 1-2 2h-2"/><path d="M6 7H4a2 2 0 0 0-2 2v6a2 2 0 0 0 2 2h1"/><path d="m11 7-3 5h4l-3 5"/><line x1="22" x2="22" y1="11" y2="13"/>"#),
    ("power", r#"<path d="M12 2v10"/><path d="M18.4 6.6a9 9 0 1 1-12.77.04"/>"#),
    ("chevron", r#"<path d="m9 18 6-6-6-6"/>"#),
    ("close", r#"<path d="M18 6 6 18"/><path d="m6 6 12 12"/>"#),
    ("bluetooth", r#"<path d="m7 7 10 10-5 5V2l5 5L7 17"/>"#),
    ("mic", r#"<path d="M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3Z"/><path d="M19 10v2a7 7 0 0 1-14 0v-2"/><line x1="12" x2="12" y1="19" y2="22"/>"#),
    ("mic-off", r#"<line x1="2" x2="22" y1="2" y2="22"/><path d="M18.89 13.23A7.12 7.12 0 0 0 19 12v-2"/><path d="M5 10v2a7 7 0 0 0 12 5"/><path d="M15 9.34V5a3 3 0 0 0-5.68-1.33"/><path d="M9 9v3a3 3 0 0 0 5.12 2.12"/><line x1="12" x2="12" y1="19" y2="22"/>"#),
    ("moon", r#"<path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z"/>"#),
    // Night: warmer colours (the moon is Do Not Disturb's).
    ("sun-moon", r#"<path d="M12 8a2.83 2.83 0 0 0 4 4 4 4 0 1 1-4-4"/><path d="M12 2v2"/><path d="M12 20v2"/><path d="m4.9 4.9 1.4 1.4"/><path d="m17.7 17.7 1.4 1.4"/><path d="M2 12h2"/><path d="M20 12h2"/><path d="m6.3 17.7-1.4 1.4"/><path d="m19.1 4.9-1.4 1.4"/>"#),
    // Sleep, a Focus.
    ("bed", r#"<path d="M2 4v16"/><path d="M2 8h18a2 2 0 0 1 2 2v10"/><path d="M2 17h20"/><path d="M6 8v9"/>"#),
    ("contrast", r#"<circle cx="12" cy="12" r="10"/><path d="M12 18a6 6 0 0 0 0-12v12z"/>"#),
    // Caps Lock is on (Lucide's arrow-big-up, with the bar under it).
    ("caps-lock", r#"<path d="M9 17v-5H5l7-7 7 7h-4v5H9z"/><path d="M9 21h6"/>"#),
    ("fingerprint", r#"<path d="M12 10a2 2 0 0 0-2 2c0 1.02-.1 2.51-.26 4"/><path d="M14 13.12c0 2.38 0 6.38-1 8.88"/><path d="M17.29 21.02c.12-.6.43-2.3.5-3.02"/><path d="M2 12a10 10 0 0 1 18-6"/><path d="M2 16h.01"/><path d="M21.8 16c.2-2 .131-5.354 0-6"/><path d="M5 19.5C5.5 18 6 15 6 12a6 6 0 0 1 .34-2"/><path d="M8.65 22c.21-.66.45-1.32.57-2"/><path d="M9 6.8a6 6 0 0 1 9 5.2v2"/>"#),
    ("lock", r#"<rect width="18" height="11" x="3" y="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 10 0v4"/>"#),
    // Settings unlocked by an admin's password (Lucide's lock-open).
    ("unlock", r#"<rect width="18" height="11" x="3" y="11" rx="2" ry="2"/><path d="M7 11V7a5 5 0 0 1 9.9-1"/>"#),
    ("log-out", r#"<path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4"/><polyline points="16 17 21 12 16 7"/><line x1="21" x2="9" y1="12" y2="12"/>"#),
    ("restart", r#"<path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/>"#),
    ("sun", r#"<circle cx="12" cy="12" r="4"/><path d="M12 2v2"/><path d="M12 20v2"/><path d="m4.93 4.93 1.41 1.41"/><path d="m17.66 17.66 1.41 1.41"/><path d="M2 12h2"/><path d="M20 12h2"/><path d="m6.34 17.66-1.41 1.41"/><path d="m19.07 4.93-1.41 1.41"/>"#),
    ("back", r#"<path d="m15 18-6-6 6-6"/>"#),
    // The toolbar's button that shows the sidebar, in a window too narrow for it beside the page.
    ("sidebar", r#"<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M9 3v18"/>"#),
    ("sound-low", r#"<path d="M11 4.702a.705.705 0 0 0-1.203-.498L6.413 7.587A1.4 1.4 0 0 1 5.416 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.416a1.4 1.4 0 0 1 .997.413l3.383 3.384A.705.705 0 0 0 11 19.298z"/><path d="M16 9a5 5 0 0 1 0 6"/>"#),
    ("privacy", r#"<path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"/><path d="m9 12 2 2 4-4"/>"#),
    ("location", r#"<path d="M20 10c0 4.993-5.539 10.193-7.399 11.799a1 1 0 0 1-1.202 0C9.539 20.193 4 14.993 4 10a8 8 0 0 1 16 0"/><circle cx="12" cy="10" r="3"/>"#),
    ("shield", r#"<circle cx="12" cy="16" r="1"/><rect x="3" y="10" width="18" height="12" rx="2"/><path d="M7 10V7a5 5 0 0 1 10 0v3"/>"#),
    ("pointer", r#"<path d="M4.037 4.688a.495.495 0 0 1 .651-.651l16 6.5a.5.5 0 0 1-.063.947l-6.124 1.58a2 2 0 0 0-1.438 1.435l-1.579 6.126a.5.5 0 0 1-.947.063z"/>"#),
    ("accessibility", r#"<circle cx="16" cy="4" r="1"/><path d="m18 19 1-7-6 1"/><path d="m5 8 3-3 5.5 3-2.36 3.5"/><path d="M4.24 14.5a5 5 0 0 0 6.88 6"/><path d="M13.76 17.5a5 5 0 0 0-6.88-6"/>"#),
    ("keyboard", r#"<path d="M10 8h.01"/><path d="M12 12h.01"/><path d="M14 8h.01"/><path d="M16 12h.01"/><path d="M18 8h.01"/><path d="M6 8h.01"/><path d="M7 16h10"/><path d="M8 12h.01"/><rect width="20" height="16" x="2" y="4" rx="2"/>"#),
    ("window", r#"<rect x="2" y="4" width="20" height="16" rx="2"/><path d="M10 4v4"/><path d="M2 8h20"/><path d="M6 4v4"/>"#),
    ("menu-bar", r#"<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M3 9h18"/>"#),
    ("monitor", r#"<rect width="20" height="14" x="2" y="3" rx="2"/><line x1="8" x2="16" y1="21" y2="21"/><line x1="12" x2="12" y1="17" y2="21"/>"#),
    ("screen-share", r#"<path d="M13 3H4a2 2 0 0 0-2 2v10a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-3"/><path d="M8 21h8"/><path d="M12 17v4"/><path d="m17 8 5-5"/><path d="M17 3h5v5"/>"#),
    ("terminal", r#"<polyline points="4 17 10 11 4 5"/><line x1="12" x2="20" y1="19" y2="19"/>"#),
    ("area", r#"<path d="M5 3a2 2 0 0 0-2 2"/><path d="M19 3a2 2 0 0 1 2 2"/><path d="M21 19a2 2 0 0 1-2 2"/><path d="M5 21a2 2 0 0 1-2-2"/><path d="M9 3h1"/><path d="M9 21h1"/><path d="M14 3h1"/><path d="M14 21h1"/><path d="M3 9v1"/><path d="M21 9v1"/><path d="M3 14v1"/><path d="M21 14v1"/>"#),
    ("record-screen", r#"<rect width="20" height="14" x="2" y="3" rx="2"/><line x1="8" x2="16" y1="21" y2="21"/><line x1="12" x2="12" y1="17" y2="21"/><circle cx="12" cy="10" r="2.5" fill="black"/>"#),
    ("record-area", r#"<path d="M5 3a2 2 0 0 0-2 2"/><path d="M19 3a2 2 0 0 1 2 2"/><path d="M21 19a2 2 0 0 1-2 2"/><path d="M5 21a2 2 0 0 1-2-2"/><path d="M9 3h1"/><path d="M9 21h1"/><path d="M14 3h1"/><path d="M14 21h1"/><path d="M3 9v1"/><path d="M21 9v1"/><path d="M3 14v1"/><path d="M21 14v1"/><circle cx="12" cy="12" r="3" fill="black"/>"#),
    ("stop", r#"<rect width="14" height="14" x="5" y="5" rx="2.5" fill="black"/>"#),
    ("video", r#"<path d="m16 13 5.223 3.482a.5.5 0 0 0 .777-.416V7.87a.5.5 0 0 0-.752-.432L16 10.5"/><rect x="2" y="6" width="14" height="12" rx="2"/>"#),
    ("camera", r#"<path d="M14.5 4h-5L7 7H4a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2V9a2 2 0 0 0-2-2h-3l-2.5-3z"/><circle cx="12" cy="13" r="3"/>"#),
    // (The emoji picker's sections.)
    ("smile", r#"<circle cx="12" cy="12" r="10"/><path d="M8 14s1.5 2 4 2 4-2 4-2"/><line x1="9" x2="9.01" y1="9" y2="9"/><line x1="15" x2="15.01" y1="9" y2="9"/>"#),
    ("leaf", r#"<path d="M11 20A7 7 0 0 1 9.8 6.1C15.5 5 17 4.48 19 2c1 2 2 4.18 2 8 0 5.5-4.78 10-10 10Z"/><path d="M2 21c0-3 1.85-5.36 5.08-6C9.5 14.52 12 13 13 12"/>"#),
    ("apple", r#"<path d="M12 20.94c1.5 0 2.75 1.06 4 1.06 3 0 6-8 6-12.22A4.91 4.91 0 0 0 17 5c-2.22 0-4 1.44-5 2-1-.56-2.78-2-5-2a4.9 4.9 0 0 0-5 4.78C2 14 5 22 8 22c1.25 0 2.5-1.06 4-1.06Z"/><path d="M10 2c1 .5 2 2 2 5"/>"#),
    ("trophy", r#"<path d="M6 9H4.5a2.5 2.5 0 0 1 0-5H6"/><path d="M18 9h1.5a2.5 2.5 0 0 0 0-5H18"/><path d="M4 22h16"/><path d="M10 14.66V17c0 .55-.47.98-.97 1.21C7.85 18.75 7 20.24 7 22"/><path d="M14 14.66V17c0 .55.47.98.97 1.21C16.15 18.75 17 20.24 17 22"/><path d="M18 2H6v7a6 6 0 0 0 12 0V2Z"/>"#),
    ("car", r#"<path d="M19 17h2c.6 0 1-.4 1-1v-3c0-.9-.7-1.7-1.5-1.9C18.7 10.6 16 10 16 10s-1.3-1.4-2.2-2.3c-.5-.4-1.1-.7-1.8-.7H5c-.6 0-1.1.4-1.4.9l-1.4 2.9A3.7 3.7 0 0 0 2 12v4c0 .6.4 1 1 1h2"/><circle cx="7" cy="17" r="2"/><path d="M9 17h6"/><circle cx="17" cy="17" r="2"/>"#),
    ("lightbulb", r#"<path d="M15 14c.2-1 .7-1.7 1.5-2.5 1-.9 1.5-2.2 1.5-3.5A6 6 0 0 0 6 8c0 1 .2 2.2 1.5 3.5.7.7 1.3 1.5 1.5 2.5"/><path d="M9 18h6"/><path d="M10 22h4"/>"#),
    ("heart", r#"<path d="M19 14c1.49-1.46 3-3.21 3-5.5A5.5 5.5 0 0 0 16.5 3c-1.76 0-3 .5-4.5 2-1.5-1.5-2.74-2-4.5-2A5.5 5.5 0 0 0 2 8.5c0 2.3 1.5 4.05 3 5.5l7 7Z"/>"#),
    ("flag", r#"<path d="M4 15s1-1 4-1 5 2 8 2 4-1 4-1V3s-1 1-4 1-5-2-8-2-4 1-4 1z"/><line x1="4" x2="4" y1="22" y2="15"/>"#),
    ("search", r#"<circle cx="11" cy="11" r="8"/><path d="m21 21-4.3-4.3"/>"#),
    ("default-apps", r#"<rect width="7" height="7" x="3" y="3" rx="1"/><rect width="7" height="7" x="14" y="3" rx="1"/><rect width="7" height="7" x="14" y="14" rx="1"/><rect width="7" height="7" x="3" y="14" rx="1"/>"#),
    ("cloud", r#"<path d="M17.5 19H9a7 7 0 1 1 6.71-9h1.79a4.5 4.5 0 1 1 0 9Z"/>"#),
    ("rain", r#"<path d="M4 14.9A7 7 0 1 1 15.71 8h1.79a4.5 4.5 0 0 1 2.5 8.24"/><path d="M16 14v6"/><path d="M8 14v6"/><path d="M12 16v6"/>"#),
    ("snow", r#"<path d="M4 14.9A7 7 0 1 1 15.71 8h1.79a4.5 4.5 0 0 1 2.5 8.24"/><path d="M8 15h.01"/><path d="M8 19h.01"/><path d="M12 17h.01"/><path d="M12 21h.01"/><path d="M16 15h.01"/><path d="M16 19h.01"/>"#),
    ("storm", r#"<path d="M6 16.33A7 7 0 1 1 15.71 8h1.79a4.5 4.5 0 0 1 .5 8.97"/><path d="m13 12-3 5h4l-3 5"/>"#),
    ("widgets", r#"<rect width="7" height="9" x="3" y="3" rx="1"/><rect width="7" height="5" x="14" y="3" rx="1"/><rect width="7" height="9" x="14" y="12" rx="1"/><rect width="7" height="5" x="3" y="16" rx="1"/>"#),
    ("plug", r#"<path d="M12 22v-5"/><path d="M9 8V2"/><path d="M15 8V2"/><path d="M18 8v5a4 4 0 0 1-4 4h-4a4 4 0 0 1-4-4V8Z"/>"#),
    ("calculator", r#"<rect width="16" height="20" x="4" y="2" rx="2"/><line x1="8" x2="16" y1="6" y2="6"/><line x1="16" x2="16" y1="14" y2="18"/><path d="M16 10h.01"/><path d="M12 10h.01"/><path d="M8 10h.01"/><path d="M12 14h.01"/><path d="M8 14h.01"/><path d="M12 18h.01"/><path d="M8 18h.01"/>"#),
    ("image", r#"<rect width="18" height="18" x="3" y="3" rx="2"/><circle cx="9" cy="9" r="2"/><path d="m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21"/>"#),
    ("cloud-sun", r#"<path d="M12 2v2"/><path d="m4.93 4.93 1.41 1.41"/><path d="M20 12h2"/><path d="m19.07 4.93-1.41 1.41"/><path d="M15.947 12.65a4 4 0 0 0-5.925-4.128"/><path d="M13 22H7a5 5 0 1 1 4.9-6H13a3 3 0 0 1 0 6Z"/>"#),
    ("cloud-fog", r#"<path d="M4 14.899A7 7 0 1 1 15.71 8h1.79a4.5 4.5 0 0 1 2.5 8.242"/><path d="M16 17H7"/><path d="M17 21H9"/>"#),
    ("music", r#"<path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/>"#),
    ("headphones", r#"<path d="M3 14h3a2 2 0 0 1 2 2v3a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-7a9 9 0 0 1 18 0v7a2 2 0 0 1-2 2h-1a2 2 0 0 1-2-2v-3a2 2 0 0 1 2-2h3"/>"#),
    // AirPods, namiOS's own drawing of them: two buds, each a round head and its stem; their case.
    ("earbuds", r#"<path d="M11 7v11.75a1.25 1.25 0 0 1-2.5 0v-8.4A3.5 3.5 0 1 1 11 7"/><path d="M13 7v11.75a1.25 1.25 0 0 0 2.5 0v-8.4A3.5 3.5 0 1 0 13 7"/>"#),
    // AirPods, filled, namiOS's own drawing of them: the head, the tip and the short stem of the
    // Pro; the long stem of the others; the pair facing out.
    ("airpods-pro", r#"<g fill="black" stroke="none"><g transform="matrix(-0.15 0 0 0.15 14.75 2.25)"><path d="M22 44C22 24 40 12 58 14C76 16 86 30 84 46C82 62 68 72 52 72C34 72 22 62 22 44Z"/><ellipse cx="80" cy="57" rx="13" ry="16.5" transform="rotate(24 80 57)"/><rect x="30" y="58" width="20" height="60" rx="10" transform="rotate(-9 40 92)"/></g><g transform="matrix(0.15 0 0 0.15 9.25 2.25)"><path d="M22 44C22 24 40 12 58 14C76 16 86 30 84 46C82 62 68 72 52 72C34 72 22 62 22 44Z"/><ellipse cx="80" cy="57" rx="13" ry="16.5" transform="rotate(24 80 57)"/><rect x="30" y="58" width="20" height="60" rx="10" transform="rotate(-9 40 92)"/></g></g>"#),
    ("airpods", r#"<g fill="black" stroke="none"><g transform="matrix(-0.15 0 0 0.15 13.5 1.2)"><path d="M24 40C24 22 40 12 57 14C74 16 82 28 80 42C78 56 66 64 51 64C35 64 24 55 24 40Z"/><rect x="31" y="44" width="18" height="88" rx="9" transform="rotate(-6 40 90)"/></g><g transform="matrix(0.15 0 0 0.15 10.5 1.2)"><path d="M24 40C24 22 40 12 57 14C74 16 82 28 80 42C78 56 66 64 51 64C35 64 24 55 24 40Z"/><rect x="31" y="44" width="18" height="88" rx="9" transform="rotate(-6 40 90)"/></g></g>"#),
    ("earbuds-case",r#"<rect x="3" y="6" width="18" height="13" rx="5"/><path d="M3 11h18"/><circle cx="12" cy="14.5" r=".5"/>"#),
    // Charging.
    ("zap", r#"<path d="M4 14a1 1 0 0 1-.78-1.63l9.9-10.2a.5.5 0 0 1 .86.46l-1.92 6.02A1 1 0 0 0 13 10h7a1 1 0 0 1 .78 1.63l-9.9 10.2a.5.5 0 0 1-.86-.46l1.92-6.02A1 1 0 0 0 11 14z"/>"#),
    // Noise control: you alone; what's around let in (waves both ways); shut out (rings round
    // you); and as it's needed (shut on one side, let in on the other).
    ("noise-off", r#"<circle cx="12" cy="12" r="6.5"/><path d="m5 5 14 14"/>"#),
    ("noise-transparency", r#"<circle cx="12" cy="12" r="3"/><path d="M6.3 7.5a7 7 0 0 0 0 9"/><path d="M17.7 7.5a7 7 0 0 1 0 9"/><path d="M3.5 5a11 11 0 0 0 0 14"/><path d="M20.5 5a11 11 0 0 1 0 14"/>"#),
    ("noise-cancellation", r#"<circle cx="12" cy="12" r="3"/><circle cx="12" cy="12" r="6.5"/><circle cx="12" cy="12" r="10"/>"#),
    ("noise-adaptive", r#"<circle cx="12" cy="12" r="3"/><path d="M12 4a8 8 0 0 0 0 16"/><path d="M17.7 7.5a7 7 0 0 1 0 9"/><path d="M20.5 5a11 11 0 0 1 0 14"/>"#),
    // Where the sound plays (Control Center's Sound module).
    ("speaker", r#"<rect width="16" height="20" x="4" y="2" rx="2"/><path d="M12 6h.01"/><circle cx="12" cy="14" r="4"/><path d="M12 14h.01"/>"#),
    ("mouse", r#"<rect x="5" y="2" width="14" height="20" rx="7"/><path d="M12 6v4"/>"#),
    ("sticky-note", r#"<path d="M16 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2V8Z"/><path d="M15 3v4a2 2 0 0 0 2 2h4"/>"#),
    ("calendar", r#"<path d="M8 2v4"/><path d="M16 2v4"/><rect width="18" height="18" x="3" y="4" rx="2"/><path d="M3 10h18"/>"#),
    ("minus", r#"<path d="M5 12h14"/>"#),
    ("update", r#"<circle cx="12" cy="12" r="9"/><path d="m16 12-4-4-4 4"/><path d="M12 16V8"/>"#),
    ("download", r#"<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" x2="12" y1="15" y2="3"/>"#),
    ("gamepad", r#"<line x1="6" x2="10" y1="12" y2="12"/><line x1="8" x2="8" y1="10" y2="14"/><line x1="15" x2="15.01" y1="13" y2="13"/><line x1="18" x2="18.01" y1="11" y2="11"/><rect width="20" height="12" x="2" y="6" rx="2"/>"#),
    ("phone", r#"<rect width="14" height="20" x="5" y="2" rx="2" ry="2"/><path d="M12 18h.01"/>"#),
    ("text", r#"<path d="M17 6.1H3"/><path d="M21 12.1H3"/><path d="M15.1 18H3"/>"#),
    ("play", r#"<polygon points="6 3 20 12 6 21 6 3"/>"#),
    ("pause", r#"<rect x="14" y="4" width="4" height="16" rx="1"/><rect x="6" y="4" width="4" height="16" rx="1"/>"#),
    ("next", r#"<polygon points="5 4 15 12 5 20 5 4"/><line x1="19" x2="19" y1="5" y2="19"/>"#),
    ("previous", r#"<polygon points="19 20 9 12 19 4 19 20"/><line x1="5" x2="5" y1="19" y2="5"/>"#),
    ("printer", r#"<path d="M6 18H4a2 2 0 0 1-2-2v-5a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2v5a2 2 0 0 1-2 2h-2"/><path d="M6 9V3a1 1 0 0 1 1-1h10a1 1 0 0 1 1 1v6"/><rect x="6" y="14" width="12" height="8" rx="1"/>"#),
    ("startup", r#"<path d="M5 12h14"/><path d="m12 5 7 7-7 7"/>"#),
    // A password, shown or kept to dots (the eye in a password field).
    ("eye", r#"<path d="M2.062 12.348a1 1 0 0 1 0-.696 10.75 10.75 0 0 1 19.876 0 1 1 0 0 1 0 .696 10.75 10.75 0 0 1-19.876 0"/><circle cx="12" cy="12" r="3"/>"#),
    ("eye-off", r#"<path d="M10.733 5.076a10.744 10.744 0 0 1 11.205 6.575 1 1 0 0 1 0 .696 10.747 10.747 0 0 1-1.444 2.49"/><path d="M14.084 14.158a3 3 0 0 1-4.242-4.242"/><path d="M17.479 17.499a10.75 10.75 0 0 1-15.417-5.151 1 1 0 0 1 0-.696 10.75 10.75 0 0 1 4.446-5.143"/><path d="m2 2 20 20"/>"#),
    ("command", r#"<path d="M15 6v12a3 3 0 1 0 3-3H6a3 3 0 1 0 3 3V6a3 3 0 1 0-3 3h12a3 3 0 1 0-3-3"/>"#),
];

/// namiOS's mark, `size` across (its 24 with the square 16 of it): filled, not stroked, its
/// ring set for the size it's drawn at, so it's as fine large as small. The ring is
/// 0.42 * sqrt(the square's pixels), as namios-look's wallpaper's and namios-boot's are: 1.4 at
/// the bar's 17 (as it always was), thinner for its size the larger it's drawn.
pub fn mark_ring(size: f32) -> f32 {
    let square = size * 16. / 24.;
    let ring = (0.42 * square.max(1.).sqrt()).max(1.);
    (ring * 24. / size.max(1.)).clamp(0.6, 2.2)
}

fn mark_svg(ring: f32) -> String {
    // A rounded square's outline as a path, for the ring's evenodd hole.
    let rr = |x: f32, y: f32, w: f32, r: f32| {
        format!("M{a} {y}H{b}A{r} {r} 0 0 1 {e} {c}V{d}A{r} {r} 0 0 1 {b} {e}H{a}A{r} {r} 0 0 1 {x} {d}V{c}A{r} {r} 0 0 1 {a} {y}Z", a = x + r, b = x + w - r, c = y + r, d = y + w - r, e = x + w)
    };
    let hole = rr(4. + ring, 4. + ring, 16. - 2. * ring, (4.5 - ring).max(0.3 * ring));
    format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="black"><path fill-rule="evenodd" d="{}{hole}"/><rect x="11.2" y="11.2" width="5" height="5" rx="1.4"/></svg>"#, rr(4., 4., 16., 4.5))
}

/// A window's buttons' marks (look::window_buttons): the same as namios-icons' (GTK's and
/// Firefox's) and niri's title bars'. 14 across, as the buttons are, drawn over the whole circle:
/// − and + bars 8 long and 2 thick on whole pixels, 3 clear of each side, so they're crisp and in
/// the middle; × two such, 2 thick, their ends 6 apart each way.
const WINDOW_MARKS: &[(&str, &str)] = &[
    ("window-close", r#"<rect x="2.757" y="6" width="8.485" height="2" transform="rotate(45 7 7)"/><rect x="2.757" y="6" width="8.485" height="2" transform="rotate(-45 7 7)"/>"#),
    ("window-minimize", r#"<rect x="3" y="6" width="8" height="2"/>"#),
    ("window-zoom", r#"<path d="M3 6h3V3h2v3h3v2H8v3H6V8H3z"/>"#),
];

/// Languages' and developer tools' marks, Simple Icons' (CC0): for the
/// packages the catalog has no picture of (nodejs, cargo, postgresql).
const DEV: &[(&str, &str)] = &[
    ("bun", include_str!("../icons/dev/bun.svg")),
    ("c", include_str!("../icons/dev/c.svg")),
    ("cmake", include_str!("../icons/dev/cmake.svg")),
    ("cplusplus", include_str!("../icons/dev/cplusplus.svg")),
    ("deno", include_str!("../icons/dev/deno.svg")),
    ("docker", include_str!("../icons/dev/docker.svg")),
    ("dotnet", include_str!("../icons/dev/dotnet.svg")),
    ("elixir", include_str!("../icons/dev/elixir.svg")),
    ("git", include_str!("../icons/dev/git.svg")),
    ("go", include_str!("../icons/dev/go.svg")),
    ("haskell", include_str!("../icons/dev/haskell.svg")),
    ("kotlin", include_str!("../icons/dev/kotlin.svg")),
    ("lua", include_str!("../icons/dev/lua.svg")),
    ("mysql", include_str!("../icons/dev/mysql.svg")),
    ("nodedotjs", include_str!("../icons/dev/nodedotjs.svg")),
    ("npm", include_str!("../icons/dev/npm.svg")),
    ("openjdk", include_str!("../icons/dev/openjdk.svg")),
    ("php", include_str!("../icons/dev/php.svg")),
    ("pnpm", include_str!("../icons/dev/pnpm.svg")),
    ("postgresql", include_str!("../icons/dev/postgresql.svg")),
    ("python", include_str!("../icons/dev/python.svg")),
    ("redis", include_str!("../icons/dev/redis.svg")),
    ("ruby", include_str!("../icons/dev/ruby.svg")),
    ("rust", include_str!("../icons/dev/rust.svg")),
    ("typescript", include_str!("../icons/dev/typescript.svg")),
    ("yarn", include_str!("../icons/dev/yarn.svg")),
    ("zig", include_str!("../icons/dev/zig.svg")),
];

/// The mark of a developer tool, by its package's name.
pub fn dev_mark(attr: &str) -> Option<&'static str> {
    let a = attr.rsplit('.').next().unwrap_or(attr).to_lowercase();
    let is = |p: &str| a == p || a.starts_with(&format!("{p}_")) || a.starts_with(&format!("{p}-"));
    let mark = match () {
        _ if is("nodejs") || a.starts_with("nodejs") => "nodedotjs",
        _ if a.starts_with("python") => "python",
        _ if ["rustc", "cargo", "rustup", "rust-analyzer", "clippy", "rustfmt"].iter().any(|p| is(p)) => "rust",
        _ if is("go") || is("gopls") || is("gotools") => "go",
        _ if is("gcc") || is("clang") || is("gnumake") => "c",
        _ if is("postgresql") || a.starts_with("postgresql") => "postgresql",
        _ if is("mysql") || a.starts_with("mysql") || is("mariadb") => "mysql",
        _ if is("jdk") || a.starts_with("openjdk") || a.starts_with("jdk") => "openjdk",
        _ if is("ghc") || is("cabal-install") || is("stack") => "haskell",
        _ if is("dotnet-sdk") || a.starts_with("dotnet") => "dotnet",
        _ if a.starts_with("lua") => "lua",
        _ if a.starts_with("ruby") => "ruby",
        _ if a.starts_with("php") => "php",
        _ if a.starts_with("typescript") => "typescript",
        _ => a.as_str(),
    };
    DEV.iter().find(|(n, _)| *n == mark).map(|(n, _)| *n)
}

/// Where gpui finds them: `icons/<name>.svg`, `dev/<name>.svg`.
pub struct Assets;

impl gpui::AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        if let Some(mark) = path.strip_prefix("dev/").and_then(|p| p.strip_suffix(".svg")) {
            return Ok(DEV.iter().find(|(n, _)| *n == mark).map(|(_, svg)| Cow::Borrowed(svg.as_bytes())));
        }
        let name = path.strip_prefix("icons/").and_then(|p| p.strip_suffix(".svg")).unwrap_or(path);
        if let Some(ring) = name.strip_prefix("mark@").and_then(|r| r.parse::<f32>().ok()) {
            return Ok(Some(Cow::Owned(mark_svg(ring.clamp(0.6, 2.2)).into_bytes())));
        }
        if name == "mark" {
            return Ok(Some(Cow::Owned(mark_svg(2.).into_bytes())));
        }
        if let Some((_, body)) = WINDOW_MARKS.iter().find(|(n, _)| *n == name) {
            return Ok(Some(Cow::Owned(format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 14 14" fill="black">{body}</svg>"#).into_bytes())));
        }
        // A light line (1.5); its dots (a path a
        // hundredth long, as Lucide draws them) keep the full 2, or they'd vanish.
        Ok(ICONS.iter().find(|(n, _)| *n == name).map(|(_, body)| {
            let body = body.replace(r#"h.01"/>"#, r#"h.01" stroke-width="2"/>"#);
            Cow::Owned(format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">{body}</svg>"#).into_bytes())
        }))
    }

    fn list(&self, _: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(ICONS.iter().chain(WINDOW_MARKS).map(|(n, _)| format!("icons/{n}.svg").into()).chain(["icons/mark.svg".into()]).collect())
    }
}

/// An app with no picture of its own: its first letter on a soft slate tile, rounded as app
/// icons are, rather than a grey grid that reads as something missing.
#[allow(dead_code)]
pub fn letter_tile(name: &str, size: f32) -> gpui::Div {
    use gpui::{ParentElement, Styled};
    let letter: String = name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "?".into());
    gpui::div()
        .size(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(size * 0.24))
        .bg(rgb(crate::look::color::key_soft()))
        .text_color(rgb(crate::look::color::key()))
        .text_size(px(size * 0.46))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .child(letter)
}

/// An icon by its name, `size` across, in `colour`.
pub fn glyph(name: &str, size: f32, colour: u32) -> Svg {
    // The mark's ring for the size it's drawn at, Text size and all (in hundredths, so a
    // few files at most).
    let path = if name == "mark" { format!("icons/mark@{:.2}.svg", mark_ring(size * crate::look::scale())) } else { format!("icons/{name}.svg") };
    svg().path(path).size(px(size)).flex_none().text_color(rgb(colour))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_airpods_glyphs_fill_their_square_and_no_more() {
        for name in ["airpods-pro", "airpods"] {
            let svg = gpui::AssetSource::load(&Assets, &format!("icons/{name}.svg")).unwrap().unwrap();
            let tree = resvg::usvg::Tree::from_data(&svg, &resvg::usvg::Options::default()).unwrap();
            let b = tree.root().abs_bounding_box();
            // (Both buds, apart, within the 24 square, most of it high.)
            assert!(b.left() >= 0. && b.right() <= 24. && b.top() >= 0. && b.bottom() <= 24., "{name}: {b:?}");
            assert!(b.width() > 18. && b.height() > 14., "{name}: {b:?}");
        }
    }

    #[test]
    fn each_level_has_its_glyph_within_its_square() {
        let mut names: Vec<&str> = (0..=100).flat_map(|p| [crate::levels::sound_glyph(p, false), crate::levels::sun_glyph(p), crate::levels::keyboard_glyph(p)]).collect();
        names.push(crate::levels::sound_glyph(50, true));
        names.sort();
        names.dedup();
        assert_eq!(names.len(), 13, "{names:?}");
        for name in names {
            let svg = gpui::AssetSource::load(&Assets, &format!("icons/{name}.svg")).unwrap().unwrap_or_else(|| panic!("{name}"));
            let tree = resvg::usvg::Tree::from_data(&svg, &resvg::usvg::Options::default()).unwrap();
            // (Its strokes too, 2 wide: none cut off at the edge.)
            let b = tree.root().abs_stroke_bounding_box();
            assert!(b.left() >= -0.01 && b.right() <= 24.01 && b.top() >= -0.01 && b.bottom() <= 24.01, "{name}: {b:?}");
        }
    }

    #[test]
    fn each_way_windows_go_has_its_icon() {
        for (_, _, icon) in crate::look::WINDOWS_MODES {
            let svg = gpui::AssetSource::load(&Assets, &format!("icons/{icon}.svg")).unwrap().unwrap_or_else(|| panic!("{icon}"));
            assert!(resvg::usvg::Tree::from_data(&svg, &resvg::usvg::Options::default()).is_ok(), "{icon}");
        }
    }

    #[test]
    fn the_mark_is_as_fine_large_as_small() {
        // The bar's 17: the ring it always had (2 of 24).
        assert!((mark_ring(17.) - 2.).abs() < 0.02, "{}", mark_ring(17.));
        // Larger, it's thinner for its size, though more pixels.
        let (mut last, mut last_px) = (mark_ring(12.), mark_ring(12.) * 12. / 24.);
        for s in [17., 22.1, 34., 44.2, 64., 128.] {
            let (r, r_px) = (mark_ring(s), mark_ring(s) * s / 24.);
            assert!(r < last && r_px > last_px, "{s}: {r} after {last}");
            (last, last_px) = (r, r_px);
        }
        // Every size's picture is one a renderer takes, by the name glyph asks for.
        for s in [16., 17., 22.1, 44.2, 128.] {
            let svg = mark_svg(mark_ring(s));
            assert!(resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).is_ok(), "{svg}");
            let named = format!("{:.2}", mark_ring(s));
            let loaded = gpui::AssetSource::load(&Assets, &format!("icons/mark@{named}.svg")).unwrap();
            assert_eq!(loaded.as_deref(), Some(mark_svg(named.parse().unwrap()).as_bytes()));
        }
    }
}
