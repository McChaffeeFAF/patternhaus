//! `patternhaus-cli`: see the library ([`ph_cli`]) for the commands. A front end that adds
//! instrument importers calls [`ph_cli::run`] the same way, with its own [`ph_cli::Options`].

fn main() -> std::process::ExitCode {
    ph_cli::run(ph_cli::Options::default())
}
