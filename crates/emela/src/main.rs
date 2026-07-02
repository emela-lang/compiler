fn main() {
    if let Err(error) = emela::run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
