fn main() {
    std::process::exit(se_ghostty_observer::dispatch(env!("CARGO_PKG_VERSION")).unwrap_or(64));
}
