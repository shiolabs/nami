//! Apps' pictures, sharp at the size they're drawn. An app's icon is often far larger than
//! it's shown (a 512-pixel PNG, or an SVG gpui draws at twice its own size, 256 pixels, for a
//! 44-point icon), and drawn smaller the screen only samples a few of its pixels: grainy, its
//! fine lines broken. Made a little larger and drawn smaller, it's soft. So each is made once
//! at the very size it's drawn at, in the screen's pixels: an SVG drawn at that size (resvg),
//! a larger picture made smaller with a good filter. Kept in ~/.cache/nami/icons, by the file
//! and when it last changed; made away from the windows, which show the nearest size made
//! (or the file) till then, and are drawn again once it's there.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, mpsc};

/// Drawn larger than this, in pixels: the file as it is.
const LARGEST: u32 = 512;

/// The screens' scale (as a hundredth): the largest a window here has said.
static SCALE: AtomicU32 = AtomicU32::new(100);

/// How many have been made: a window drawn before one was, is drawn again.
static MADE: AtomicU64 = AtomicU64::new(0);

/// What's known of each file, by size: the picture made, or none (the file is best as it is).
static KNOWN: Mutex<Option<HashMap<PathBuf, BTreeMap<u32, Option<PathBuf>>>>> = Mutex::new(None);

/// What a picture made here was made from.
static MADE_FROM: Mutex<Option<HashMap<PathBuf, PathBuf>>> = Mutex::new(None);

/// What's asked to be made and isn't yet.
static ASKED: Mutex<Option<HashSet<(PathBuf, u32)>>> = Mutex::new(None);

/// A window's scale, as it's drawn: pictures are made for the largest.
pub fn screen_scale(scale: f32) {
    SCALE.fetch_max((scale * 100.).round() as u32, Ordering::Relaxed);
}

/// The screens' scale now (the largest a window here has said), with the look's own: how
/// many pixels a point is.
pub fn scale_now() -> f32 {
    SCALE.load(Ordering::Relaxed) as f32 / 100. * crate::look::scale()
}

/// How many pictures have been made: when it changes, what's drawn may be sharper.
pub fn made() -> u64 {
    MADE.load(Ordering::Acquire)
}

/// How many pixels across a picture drawn `size` points across is; none if past the largest.
fn pixels_for(size: f32, scale: f32) -> Option<u32> {
    let pixels = (size * crate::look::scale() * scale).round() as u32;
    (1..=LARGEST).contains(&pixels).then_some(pixels)
}

/// What to draw for `path`, `size` points across: the picture made at that size once it's
/// made (asked for now if it isn't), till then the nearest made that's larger, else the file.
/// Never waits.
pub fn sharp(path: &Path, size: f32) -> PathBuf {
    let path = MADE_FROM.lock().ok().and_then(|m| m.as_ref()?.get(path).cloned()).unwrap_or_else(|| path.to_path_buf());
    if !worth_it(&path) {
        return path;
    }
    let Some(pixels) = pixels_for(size, SCALE.load(Ordering::Relaxed) as f32 / 100.) else { return path };
    let sizes = KNOWN.lock().ok().and_then(|k| k.as_ref()?.get(&path).cloned()).unwrap_or_default();
    if let Some(known) = sizes.get(&pixels) {
        return known.clone().unwrap_or(path);
    }
    ask((path.clone(), pixels));
    sizes.range(pixels..).find_map(|(_, made)| made.clone()).unwrap_or(path)
}

/// Made ahead, for drawing at these sizes (in points): what the desktop shows of the apps.
// (The desktop's; Settings draws only what it shows.)
#[allow(dead_code)]
pub fn ahead(path: &Path, sizes: &[f32]) {
    if !worth_it(path) {
        return;
    }
    let scale = SCALE.load(Ordering::Relaxed) as f32 / 100.;
    for pixels in sizes.iter().filter_map(|s| pixels_for(*s, scale)) {
        if KNOWN.lock().ok().is_some_and(|k| k.as_ref().and_then(|k| k.get(path)).is_some_and(|s| s.contains_key(&pixels))) {
            continue;
        }
        ask((path.to_path_buf(), pixels));
    }
}

/// A picture of an app's that may be drawn better made at its size: an SVG (not a theme's
/// one-colour one, drawn as a mask in the text's colour) or a PNG, JPEG or WebP.
fn worth_it(path: &Path) -> bool {
    let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    !name.ends_with("-symbolic.svg") && [".svg", ".png", ".jpg", ".jpeg", ".webp"].iter().any(|e| name.ends_with(e))
}

fn ask(key: (PathBuf, u32)) {
    if !ASKED.lock().is_ok_and(|mut a| a.get_or_insert_with(HashSet::new).insert(key.clone())) {
        return;
    }
    static MAKER: OnceLock<mpsc::Sender<(PathBuf, u32)>> = OnceLock::new();
    let maker = MAKER.get_or_init(|| {
        let (send, asked) = mpsc::channel::<(PathBuf, u32)>();
        std::thread::spawn(move || {
            // (Behind what the person's doing: it's only ever sooner sharper.)
            #[cfg(target_os = "linux")]
            unsafe {
                libc::setpriority(libc::PRIO_PROCESS, 0, 10);
            }
            for (path, pixels) in asked {
                let made = made_file(&path, pixels);
                if let (Some(file), Ok(mut from)) = (&made, MADE_FROM.lock()) {
                    from.get_or_insert_with(HashMap::new).insert(file.clone(), path.clone());
                }
                if let Ok(mut known) = KNOWN.lock() {
                    known.get_or_insert_with(HashMap::new).entry(path.clone()).or_default().insert(pixels, made);
                }
                if let Ok(mut a) = ASKED.lock() {
                    a.get_or_insert_with(HashSet::new).remove(&(path, pixels));
                }
                MADE.fetch_add(1, Ordering::Release);
            }
        });
        send
    });
    let _ = maker.send(key);
}

fn cache() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".cache")))?;
    Some(base.join("nami/icons"))
}

/// The picture of `path` at `pixels`, from the cache or made now; none if the file's as good.
fn made_file(path: &Path, pixels: u32) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let meta = std::fs::metadata(path).ok()?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    (path, meta.len(), meta.modified().ok()).hash(&mut hash);
    let dir = cache()?;
    let file = dir.join(format!("{:016x}-{pixels}.png", hash.finish()));
    // (One that's no better than the file: an empty one says so.)
    match std::fs::metadata(&file) {
        Ok(m) if m.len() == 0 => return None,
        Ok(_) => return Some(file),
        Err(_) => {}
    }
    let _ = std::fs::create_dir_all(&dir);
    let picture = picture_at(path, pixels);
    let part = file.with_extension("part");
    let saved = match &picture {
        Some(p) => p.save_with_format(&part, image::ImageFormat::Png).is_ok(),
        None => std::fs::write(&part, b"").is_ok(),
    };
    if !saved || std::fs::rename(&part, &file).is_err() {
        let _ = std::fs::remove_file(&part);
        return None;
    }
    picture.map(|_| file)
}

/// `path` at `pixels` across (and as tall, its shape kept, in the middle); none if it's a
/// picture no larger than that, which is drawn as it is.
fn picture_at(path: &Path, pixels: u32) -> Option<image::RgbaImage> {
    if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg")) {
        return drawn(&std::fs::read(path).ok()?, pixels);
    }
    let picture = image::open(path).ok()?;
    if picture.width() <= pixels && picture.height() <= pixels {
        return None;
    }
    Some(picture.resize(pixels, pixels, image::imageops::FilterType::Lanczos3).to_rgba8())
}

/// An SVG drawn `pixels` across, to fit.
fn drawn(bytes: &[u8], pixels: u32) -> Option<image::RgbaImage> {
    use resvg::{tiny_skia, usvg};
    let tree = usvg::Tree::from_data(bytes, &usvg::Options::default()).ok()?;
    let size = tree.size();
    let scale = (pixels as f32 / size.width()).min(pixels as f32 / size.height());
    let (width, height) = (((size.width() * scale).round() as u32).max(1), ((size.height() * scale).round() as u32).max(1));
    let mut pixmap = tiny_skia::Pixmap::new(width, height)?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    // (tiny-skia's are premultiplied; a PNG's aren't.)
    let pixels: Vec<u8> = pixmap.pixels().iter().flat_map(|p| {
        let c = p.demultiply();
        [c.red(), c.green(), c.blue(), c.alpha()]
    }).collect();
    image::RgbaImage::from_raw(width, height, pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn made_at_the_size_drawn_in_the_screens_pixels() {
        let pixels = |points: f32, scale: f32| (points * crate::look::scale() * scale).round() as u32;
        assert_eq!(pixels_for(44., 1.), Some(pixels(44., 1.)));
        assert_eq!(pixels_for(44., 2.), Some(pixels(44., 2.)));
        assert_eq!(pixels_for(22., 1.5), Some(pixels(22., 1.5)));
        assert_eq!(pixels_for(400., 2.), None);
    }

    #[test]
    fn an_svg_is_drawn_at_the_size_asked() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="128" height="64" viewBox="0 0 128 64"><rect width="128" height="64" fill="#45577a"/></svg>"##;
        let picture = drawn(svg, 48).unwrap();
        assert_eq!((picture.width(), picture.height()), (48, 24));
        assert_eq!(picture.get_pixel(10, 10).0, [0x45, 0x57, 0x7a, 255]);
    }

    #[test]
    fn what_is_made_and_what_is_left_as_it_is() {
        assert!(worth_it(Path::new("/x/firefox.png")));
        assert!(worth_it(Path::new("/x/org.gnome.Nautilus.svg")));
        assert!(!worth_it(Path::new("/x/audio-volume-high-symbolic.svg")));
        assert!(!worth_it(Path::new("/x/xterm.xpm")));
    }
}
