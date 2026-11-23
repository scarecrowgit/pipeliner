//! Runs the v1 import against the real old data and prints the report. Read-only: the store
//! is in memory (its blob directory is a scratch directory in the system temp directory), and
//! the import never writes to the old data directory.
//!
//! `cargo run -p store --example import_probe [-- --verbose]`

use anyhow::Result;
use base::paths::Paths;
use store::Store;
use store::import_v1::{ImportOptions, import_v1};

fn main() -> Result<()> {
    let verbose = std::env::args().any(|arg| arg == "--verbose");
    let paths = Paths::from_env();
    println!("reading {}", paths.v1_data_dir().display());
    let store = Store::in_memory()?;

    let first = import_v1(&store, &paths, &ImportOptions::default())?;
    print!("{first}");

    let second = import_v1(&store, &paths, &ImportOptions::default())?;
    println!("\nsecond run (must import nothing):");
    print!("{second}");
    anyhow::ensure!(second.imported_total() == 0, "the second import was not idempotent");

    let dry = import_v1(&store, &paths, &ImportOptions { dry_run: true })?;
    println!("\ndry run on the filled store (must import nothing):");
    print!("{dry}");

    if verbose {
        println!("\nwhat is in the in-memory store:");
        for project in store.projects()? {
            println!(
                "project {} ({}) prefix {}",
                project.name, project.id, project.key_prefix
            );
            for issue in store.issues(&project.id)? {
                let runs = store.runs_for_issue(&issue.key)?;
                println!(
                    "  {} [{:?}] {} ({} runs)",
                    issue.key,
                    issue.status,
                    issue.title,
                    runs.len()
                );
            }
        }
        for session in store.sessions(&store::SessionFilter::default())? {
            let spent = store.spend_for_session(&session.id)?;
            println!(
                "session {} {:?} {} messages, ${:.2}",
                session.id, session.title, session.message_count, spent.cost_usd
            );
        }
        println!("next issue key: {}", store.next_issue_key("HAR")?);
    }
    Ok(())
}
