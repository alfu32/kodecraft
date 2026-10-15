mod cli;
use clap::Parser;
use cli::{Cli, Command};

#[tokio::main]
async fn main() {
    use clap::CommandFactory;
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Version) => println!("{}", env!("CARGO_PKG_VERSION")),
        None if cli.target.is_none() => {
            Cli::command().print_help().expect("write CLI help");
            println!();
        }
        None => {
            let current = std::env::current_dir().expect("current directory");
            match cli::resolve_target(cli.target.as_deref().expect("target checked"), &current) {
                Ok(target) => {
                    if let Err(error) = kode_tui::app::runner::run_with_resources(
                        target.project_root,
                        target.file,
                        cli.resources,
                    )
                    .await
                    {
                        eprintln!("{error}");
                        std::process::exit(1);
                    }
                }
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(1);
                }
            }
        }
    }
}
