fn main() {
    match veyra_engine::fixture::run() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(3);
        }
    }
}
