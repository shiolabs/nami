// What winit does to talk to X11: open libX11 at run time (dlopen).
// The program names no library, so nami bundle . finds it in Cargo.lock.
fn main() {
    match x11_dl::xlib::Xlib::open() {
        Ok(_) => println!("rust-x11: libX11 opened"),
        Err(e) => {
            eprintln!("rust-x11: {e}");
            std::process::exit(1);
        }
    }
}
