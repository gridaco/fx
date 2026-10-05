//! The `grida-fx` command.

mod args;
mod cli;
mod print;
mod verbs;

fn main() -> std::process::ExitCode {
    cli::main(std::env::args_os().collect())
}
