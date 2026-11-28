fn main() {
    let who = std::env::args().nth(1).unwrap_or_else(|| "world".into());
    println!("rust-hello {}: hello, {who}", env!("CARGO_PKG_VERSION"));
}
