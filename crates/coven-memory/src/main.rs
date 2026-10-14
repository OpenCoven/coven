//! coven-memory CLI — archival memory for Coven familiars
//!
//! Commands:
//!   ingest  <path> --familiar <name>   Ingest a file or directory
//!   search  <query> [--familiar <name>] [--k <n>]  Semantic search
//!   status                             Index stats

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use coven_memory::{
    db::MetaDb,
    default_db_path, default_index_path,
    embed::Embedder,
    index::VecIndex,
    ingest::{ingest_dir, ingest_file},
    rebuild::rebuild_index,
};

#[derive(Parser)]
#[command(
    name = "coven-memory",
    about = "Archival memory layer for Coven familiars"
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
    /// Path to vector index (default: ~/.coven/memory/archival.tvim)
    #[arg(long, global = true)]
    index: Option<PathBuf>,
    /// Path to SQLite metadata db (default: ~/.coven/memory/archival.sqlite3)
    #[arg(long, global = true)]
    db: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Ingest a file or directory into archival memory
    Ingest {
        /// Path to file or directory
        path: PathBuf,
        /// Familiar this memory belongs to (e.g. sage, echo, coven)
        #[arg(long, default_value = "coven")]
        familiar: String,
    },
    /// Search archival memory
    Search {
        /// Natural language query
        query: String,
        /// Restrict results to a specific familiar
        #[arg(long)]
        familiar: Option<String>,
        /// Number of results (default: 5)
        #[arg(short, long, default_value_t = 5)]
        k: usize,
    },
    /// Show index stats
    Status,
    /// Re-embed persisted chunks into a NEW index, preserving the old index and DB
    Rebuild {
        /// New destination; existing files are refused. Use --index with this file afterward
        #[arg(long)]
        output: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let index_path = cli.index.unwrap_or_else(default_index_path);
    let db_path = cli.db.unwrap_or_else(default_db_path);

    let db = MetaDb::open(&db_path)?;
    if let Cmd::Rebuild { output } = &cli.command {
        // Rebuilding must work even when the old index format cannot be loaded.
        let mut embedder = None;
        let count = rebuild_index(&db, output, |texts| {
            if embedder.is_none() {
                eprintln!("⟳  Initialising embedder (downloads model on first run)…");
                embedder = Some(Embedder::new()?);
            }
            embedder
                .as_mut()
                .expect("embedder is initialized")
                .embed_batch(&texts.iter().map(String::as_str).collect::<Vec<_>>())
        })?;
        println!("✓  Rebuilt {count} vectors in {}. Use --index with this file; the old index is preserved.", output.display());
        return Ok(());
    }
    let mut index = VecIndex::open(&index_path)?;

    match cli.command {
        Cmd::Ingest { path, familiar } => {
            eprintln!("⟳  Initialising embedder (downloads model on first run)…");
            let mut embedder = Embedder::new()?;

            if path.is_dir() {
                eprintln!("⟳  Ingesting directory: {}", path.display());
                let (files, chunks) = ingest_dir(&path, &familiar, &db, &mut index, &mut embedder)?;
                index.save()?;
                println!(
                    "✓  Ingested {files} new files ({chunks} chunks) for familiar '{familiar}'"
                );
            } else {
                eprintln!("⟳  Ingesting file: {}", path.display());
                let n = ingest_file(&path, &familiar, &db, &mut index, &mut embedder)?;
                index.save()?;
                println!(
                    "✓  Ingested {n} new chunks from {} for familiar '{familiar}'",
                    path.display()
                );
            }
        }

        Cmd::Search { query, familiar, k } => {
            eprintln!("⟳  Initialising embedder…");
            let mut embedder = Embedder::new()?;

            let q_vec = embedder.embed_query(&query)?;

            let (scores, ids) = if let Some(ref fam) = familiar {
                let allowlist = db.ids_for_familiar(fam)?;
                if allowlist.is_empty() {
                    println!("No documents found for familiar '{fam}'");
                    return Ok(());
                }
                index.search_filtered(&q_vec, k, &allowlist)?
            } else {
                index.search(&q_vec, k)?
            };

            if ids.is_empty() {
                println!("No results.");
                return Ok(());
            }

            let docs = db.get_many(&ids)?;

            println!("\n── Archival Memory Search ──────────────────────────");
            println!("  Query: {query}");
            if let Some(f) = &familiar {
                println!("  Familiar: {f}");
            }
            println!("────────────────────────────────────────────────────\n");

            for (doc, score) in docs.iter().zip(scores.iter()) {
                let short_path = doc.path.replace(
                    &dirs_next::home_dir()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                    "~",
                );
                println!("  [{score:.3}] {short_path}  (familiar: {})", doc.familiar);
                println!("  {}\n", truncate(&doc.chunk, 280));
            }
        }

        Cmd::Status => {
            let total = db.count()?;
            let indexed = index.len();
            println!("── coven-memory status ─────────────────────────────");
            println!("  Index:    {}", index_path.display());
            println!("  DB:       {}", db_path.display());
            println!("  Docs:     {total} chunks in db");
            println!("  Indexed:  {indexed} vectors in turbovec");
            println!("────────────────────────────────────────────────────");
        }
        Cmd::Rebuild { .. } => unreachable!("rebuild is handled before loading the old index"),
    }

    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    let mut s = s.replace('\n', " ");
    if s.len() > max {
        s.truncate(max);
        s.push('…');
    }
    s
}
