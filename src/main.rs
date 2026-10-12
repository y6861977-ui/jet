//! Entry point of the `jet` binary. All the logic is in the library (`jet::cli::run`),
//! so both the CLI and `jet-server` can reuse it.

fn main() {
    jet::cli::run();
}
