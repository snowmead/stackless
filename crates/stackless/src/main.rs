//! The stackless CLI binary. Shared lifecycle lives in the `stackless` library.

fn main() -> std::process::ExitCode {
    stackless::cli::run()
}
