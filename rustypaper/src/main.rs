fn main() -> std::process::ExitCode {
    std::process::ExitCode::from(rustypaper::cli::run(std::env::args_os()) as u8)
}
