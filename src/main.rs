fn main() {
    if let Err(error) = gh_mcp_router::cli::try_run(std::env::args().skip(1)) {
        eprintln!("gh-mcp-router: {error}");
        std::process::exit(error.exit_code());
    }
}
