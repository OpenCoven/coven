//! `coven familiar-ledger`: the owner's terminal for the familiar ledger
//! (coven#857, slice 3).
//!
//! Each subcommand sends one `coven.familiars.ledger.*.v1` control action
//! through the API in this process, which runs as the owner exactly like
//! owner-local IPC. Commands that change a familiar's head first read the
//! current head and send it as `expectedRevisionId`, so a concurrent change
//! is reported instead of overwritten. Every invocation uses a fresh adoption
//! key.

use std::path::Path;

use anyhow::{bail, Context, Result};
use clap::Subcommand;
use serde_json::{json, Value};

use crate::{api, paths};

#[derive(Subcommand, Debug)]
pub(crate) enum FamiliarCommand {
    #[command(about = "Record a familiar's identity in the ledger for the first time")]
    Register {
        #[arg(help = "Familiar id (familiars.toml key)")]
        familiar: String,
        #[arg(long, help = "Print the ledger response as JSON")]
        json: bool,
    },
    #[command(
        about = "Record a familiar's changed roster entry or declaration files as a new revision"
    )]
    Adopt {
        #[arg(help = "Familiar id (familiars.toml key)")]
        familiar: String,
        #[arg(long, help = "Print the ledger response as JSON")]
        json: bool,
    },
    #[command(about = "Retire a familiar; its history stays readable")]
    Retire {
        #[arg(help = "Familiar id (familiars.toml key)")]
        familiar: String,
        #[arg(long, help = "Print the ledger response as JSON")]
        json: bool,
    },
    #[command(about = "Revoke one familiar revision so it can never be embodied")]
    Revoke {
        #[arg(help = "Revision id (familiar-revision:…)")]
        revision: String,
        #[arg(long, value_name = "TEXT", help = "Why the revision is revoked")]
        reason: String,
        #[arg(long, help = "Print the ledger response as JSON")]
        json: bool,
    },
    #[command(about = "Restore a retired familiar root with a new revision")]
    Restore {
        #[arg(help = "Root id (familiar:…), as `coven familiar-ledger history` shows it")]
        root: String,
        #[arg(long, help = "Print the ledger response as JSON")]
        json: bool,
    },
    #[command(about = "Show a familiar's ledger roots and revisions")]
    History {
        #[arg(help = "Familiar id (familiars.toml key), or a root id (familiar:…)")]
        familiar: String,
        #[arg(long, help = "Print the ledger response as JSON")]
        json: bool,
    },
}

pub(crate) fn run(command: FamiliarCommand) -> Result<()> {
    let coven_home = paths::coven_home_dir()?;
    let (result, json) = run_at(&coven_home, command)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&result)
                .context("failed to render the ledger response")?
        );
    } else {
        print!("{}", render(&result));
    }
    Ok(())
}

/// Runs one subcommand against `coven_home`, returning the ledger's result
/// and whether JSON output was asked for.
fn run_at(coven_home: &Path, command: FamiliarCommand) -> Result<(Value, bool)> {
    let action = |name: &str| format!("coven.familiars.ledger.{name}.v1");
    Ok(match command {
        FamiliarCommand::Register { familiar, json } => (
            send(
                coven_home,
                json!({ "action": action("register"), "adoptionKey": adoption_key()?, "familiarId": familiar }),
            )?,
            json,
        ),
        FamiliarCommand::Adopt { familiar, json } => {
            let head = live_head(coven_home, &familiar)?;
            (
                send(
                    coven_home,
                    json!({
                        "action": action("adopt"), "adoptionKey": adoption_key()?,
                        "familiarId": familiar, "expectedRevisionId": head,
                    }),
                )?,
                json,
            )
        }
        FamiliarCommand::Retire { familiar, json } => {
            let head = live_head(coven_home, &familiar)?;
            (
                send(
                    coven_home,
                    json!({
                        "action": action("retire"), "adoptionKey": adoption_key()?,
                        "familiarId": familiar, "expectedRevisionId": head,
                    }),
                )?,
                json,
            )
        }
        FamiliarCommand::Revoke {
            revision,
            reason,
            json,
        } => (
            send(
                coven_home,
                json!({
                    "action": action("revoke"), "adoptionKey": adoption_key()?,
                    "revisionId": revision, "reason": reason,
                }),
            )?,
            json,
        ),
        FamiliarCommand::Restore { root, json } => {
            let history = send(
                coven_home,
                json!({ "action": action("get"), "rootId": root }),
            )?;
            let head = history["roots"][0]["revisions"]
                .as_array()
                .and_then(|revisions| revisions.last())
                .and_then(|revision| revision["revisionId"].as_str())
                .with_context(|| format!("familiar root `{root}` does not exist"))?
                .to_owned();
            (
                send(
                    coven_home,
                    json!({
                        "action": action("restore"), "adoptionKey": adoption_key()?,
                        "rootId": root, "expectedRevisionId": head,
                    }),
                )?,
                json,
            )
        }
        FamiliarCommand::History { familiar, json } => {
            let key = if familiar.starts_with("familiar:") {
                "rootId"
            } else {
                "familiarId"
            };
            (
                send(
                    coven_home,
                    json!({ "action": action("get"), key: familiar }),
                )?,
                json,
            )
        }
    })
}

/// The head revision of the live root registered for `familiar`.
fn live_head(coven_home: &Path, familiar: &str) -> Result<String> {
    let history = send(
        coven_home,
        json!({ "action": "coven.familiars.ledger.get.v1", "familiarId": familiar }),
    )?;
    history["roots"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|root| root["retiredAt"].is_null())
        .and_then(|root| root["revisions"].as_array()?.last())
        .and_then(|revision| revision["revisionId"].as_str())
        .map(ToOwned::to_owned)
        .with_context(|| {
            format!(
                "familiar `{familiar}` is not registered; run `coven familiar-ledger register {familiar}`"
            )
        })
}

/// Sends one ledger action and returns its result, or the ledger's refusal
/// as an error.
fn send(coven_home: &Path, request: Value) -> Result<Value> {
    let response = api::handle_request_with_body(
        "POST",
        "/api/v1/actions",
        coven_home,
        None,
        Some(&request.to_string()),
    )?;
    let body: Value = serde_json::from_str(&response.body)
        .context("the familiar ledger answered with invalid JSON")?;
    if response.status != 200 || body["ok"] != json!(true) {
        let code = body["error"]["code"].as_str().unwrap_or("ERROR");
        let message = body["error"]["message"]
            .as_str()
            .or_else(|| body["reason"].as_str())
            .unwrap_or("the familiar ledger refused the command");
        bail!("{code}: {message}");
    }
    Ok(body["result"].clone())
}

fn adoption_key() -> Result<String> {
    use ring::rand::SecureRandom as _;
    let mut bytes = [0_u8; 16];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| anyhow::anyhow!("failed to draw an adoption key"))?;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!("cli:familiar-ledger:{hex}"))
}

/// The human-readable form of a ledger result.
fn render(result: &Value) -> String {
    if let Some(roots) = result["roots"].as_array() {
        if roots.is_empty() {
            return "No ledger roots.\n".to_owned();
        }
        let mut out = String::new();
        for root in roots {
            let state = if root["retiredAt"].is_null() {
                "live".to_owned()
            } else {
                format!("retired {}", text(&root["retiredAt"]))
            };
            out.push_str(&format!(
                "{} ({}; {state}; generation {})\n",
                text(&root["rootId"]),
                text(&root["familiarId"]),
                root["generation"]
            ));
            for revision in root["revisions"].as_array().into_iter().flatten() {
                out.push_str(&format!(
                    "  {}  {:<10}  {:<22}  recorded {}\n",
                    text(&revision["revisionId"]),
                    text(&revision["status"]),
                    text(&revision["relationship"]),
                    text(&revision["recordedAt"]),
                ));
            }
        }
        return out;
    }
    let revision = &result["revision"];
    let outcome = text(&result["outcome"]);
    let detail = format!(
        "{} (revision {}, {}, generation {})",
        text(&result["rootId"]),
        text(&revision["revisionId"]),
        text(&revision["status"]),
        result["generation"]
    );
    match outcome {
        "unchanged" => format!("Unchanged: the declarations already match {detail}.\n"),
        "registered" => format!("Registered {detail}.\n"),
        "adopted" => format!("Adopted the current declarations as {detail}.\n"),
        "retired" => format!("Retired {detail}.\n"),
        "revoked" => format!("Revoked {}.\n", text(&revision["revisionId"])),
        "restored" => format!("Restored {detail}.\n"),
        other => format!("{other}: {detail}\n"),
    }
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    const WARD: &str = "principal_key_fingerprint = \"SHA256:abc\"\nprotected_surface = [\"SOUL.md\"]\n\n[[surface]]\npath = \"SOUL.md\"\ntier = 0\n";

    fn home() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        crate::store::initialize_store(&crate::api::store_path(dir.path())).unwrap();
        std::fs::write(
            dir.path().join("familiars.toml"),
            "[[familiar]]\nid = \"sage\"\ndisplay_name = \"Sage\"\nrole = \"Research\"\ndescription = \"Finds things.\"\n",
        )
        .unwrap();
        let workspace = dir.path().join("familiars").join("sage");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("IDENTITY.md"), "# IDENTITY.md - Sage\n").unwrap();
        std::fs::write(workspace.join("SOUL.md"), "# SOUL\nFirst.\n").unwrap();
        std::fs::write(workspace.join("ward.toml"), WARD).unwrap();
        dir
    }

    fn run(home: &Path, command: FamiliarCommand) -> Result<Value> {
        run_at(home, command).map(|(result, _)| result)
    }

    #[test]
    fn the_owner_registers_adopts_retires_and_restores_from_the_terminal() {
        let dir = home();
        let home = dir.path();
        let register = || FamiliarCommand::Register {
            familiar: "sage".into(),
            json: false,
        };
        let registered = run(home, register()).unwrap();
        assert!(render(&registered).starts_with("Registered familiar:"));
        let second = run(home, register()).unwrap_err();
        assert!(
            format!("{second:#}").starts_with("ILLEGAL_TRANSITION:"),
            "{second:#}"
        );

        let adopt = || FamiliarCommand::Adopt {
            familiar: "sage".into(),
            json: false,
        };
        assert!(render(&run(home, adopt()).unwrap()).starts_with("Unchanged:"));
        std::fs::write(home.join("familiars/sage/SOUL.md"), "# SOUL\nSecond.\n").unwrap();
        let adopted = run(home, adopt()).unwrap();
        assert_eq!(adopted["outcome"], "adopted");
        assert_eq!(adopted["revision"]["lineagePosition"], 1);

        let root = registered["rootId"].as_str().unwrap().to_owned();
        run(
            home,
            FamiliarCommand::Retire {
                familiar: "sage".into(),
                json: false,
            },
        )
        .unwrap();
        let missing = run(home, adopt()).unwrap_err();
        assert!(
            format!("{missing:#}").contains("coven familiar-ledger register sage"),
            "{missing:#}"
        );
        let restored = run(
            home,
            FamiliarCommand::Restore {
                root: root.clone(),
                json: false,
            },
        )
        .unwrap();
        assert_eq!(restored["outcome"], "restored");

        let by_alias = run(
            home,
            FamiliarCommand::History {
                familiar: "sage".into(),
                json: false,
            },
        )
        .unwrap();
        let by_root = run(
            home,
            FamiliarCommand::History {
                familiar: root.clone(),
                json: false,
            },
        )
        .unwrap();
        assert_eq!(by_alias, by_root);
        let rendered = render(&by_root);
        assert!(
            rendered.starts_with(&format!("{root} (sage; live; generation 4)")),
            "{rendered}"
        );
        assert_eq!(rendered.lines().count(), 4, "{rendered}");

        let head = restored["revision"]["revisionId"]
            .as_str()
            .unwrap()
            .to_owned();
        let revoked = run(
            home,
            FamiliarCommand::Revoke {
                revision: head.clone(),
                reason: "test".into(),
                json: false,
            },
        )
        .unwrap();
        assert_eq!(render(&revoked), format!("Revoked {head}.\n"));
    }

    #[test]
    fn missing_declarations_are_reported_with_the_ledger_code() {
        let dir = home();
        std::fs::remove_file(dir.path().join("familiars/sage/SOUL.md")).unwrap();
        let error = run(
            dir.path(),
            FamiliarCommand::Register {
                familiar: "sage".into(),
                json: true,
            },
        )
        .unwrap_err();
        assert!(
            format!("{error:#}").starts_with("VALIDATION_FAILED: familiar `sage` has no SOUL.md"),
            "{error:#}"
        );
        assert_eq!(render(&json!({"roots": []})), "No ledger roots.\n");
    }
}
