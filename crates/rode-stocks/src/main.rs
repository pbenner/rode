fn main() {
    if let Err(error) = rode_stocks::run() {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}
