use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "kode", version, about = "Kode terminal IDE — Rust migration")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Version,
}

fn main() {
    use clap::CommandFactory;
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Version) => println!("{}", env!("CARGO_PKG_VERSION")),
        None => {
            Cli::command().print_help().expect("write CLI help");
            println!();
        }
    }
}
