// observe.rs — read-path CLI commands with Cave/daemon API parity.
//
// Every command here renders through the in-process API router
// (`api::handle_request`), so `--json` output carries exactly the body the
// daemon socket serves for the same route (pretty-printed) and the human
// tables are a rendering of that same body — one source of truth for shapes.
//
// These are offline reads: the audited routes re-read `~/.coven` files or
// the SQLite store per request and hold no daemon state, so going through
// the router in-process cannot disagree with a running daemon.

use std::path::Path;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::{api, coven_home_dir, daemon, theme};

/// Column cap for free-text table cells (descriptions, requests, reasons).
const TEXT_CELL_LIMIT: usize = 48;

/// Which read-only view to render. Mirrors the top-level CLI observability
/// commands one-to-one; the Cast shell and the chat UI reuse these views so
/// every surface renders the same data the same way.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ObserveView {
    Status,
    Familiars,
    Skills,
    Memory,
    Research,
    Calls,
    HubStatus,
}

impl ObserveView {
    /// The CLI command this view mirrors, shown on plan/outcome cards so
    /// users learn the scriptable spelling.
    pub(crate) fn command(self) -> &'static str {
        match self {
            ObserveView::Status => "coven status",
            ObserveView::Familiars => "coven familiars",
            ObserveView::Skills => "coven skills",
            ObserveView::Memory => "coven memory",
            ObserveView::Research => "coven research",
            ObserveView::Calls => "coven calls",
            ObserveView::HubStatus => "coven hub status",
        }
    }

    pub(crate) fn headline(self) -> &'static str {
        match self {
            ObserveView::Status => "Show the coven status overview",
            ObserveView::Familiars => "List the familiar roster",
            ObserveView::Skills => "List installed skills",
            ObserveView::Memory => "List familiar memory files",
            ObserveView::Research => "Show the research loop log",
            ObserveView::Calls => "Show the Coven Calls ledger",
            ObserveView::HubStatus => "Show hub control-plane status",
        }
    }
}

/// Render a view's human text against an explicit Coven home. For every
/// view in [`ObserveView`] this is the single render path — the CLI
/// command, the Cast shell, and the chat UI all call it, so the surfaces
/// cannot drift. (CLI-only leaves like `hub nodes/jobs/routing` and the
/// `calls <id>` detail live outside the view enum and render directly.)
pub(crate) fn view_text(coven_home: &Path, view: ObserveView) -> Result<String> {
    Ok(match view {
        ObserveView::Status => {
            let daemon_state = daemon::background_server_status(coven_home)?;
            let live = match &daemon_state {
                Some(daemon::DaemonStatusState::Running(status)) => Some(status.clone()),
                _ => None,
            };
            let health = api_get_with_daemon(coven_home, "/api/v1/health", live)?;
            let overview = api_get(coven_home, "/api/v1/overview")?;
            render_status(daemon_state.as_ref(), &health, &overview)
        }
        ObserveView::Familiars => render_familiars(&api_get(coven_home, "/api/v1/familiars")?),
        ObserveView::Skills => render_skills(&api_get(coven_home, "/api/v1/skills")?),
        ObserveView::Memory => render_memory(&api_get(coven_home, "/api/v1/memory")?),
        ObserveView::Research => render_research(&api_get(coven_home, "/api/v1/research")?),
        ObserveView::Calls => render_calls(&api_get(coven_home, "/api/v1/coven-calls")?),
        ObserveView::HubStatus => render_hub_status(&api_get(coven_home, "/api/v1/hub/status")?),
    })
}

// ── API access ───────────────────────────────────────────────────────────────

fn api_get(coven_home: &Path, path: &str) -> Result<Value> {
    api_get_with_daemon(coven_home, path, None)
}

fn api_get_with_daemon(
    coven_home: &Path,
    path: &str,
    daemon_status: Option<daemon::DaemonStatus>,
) -> Result<Value> {
    let response = api::handle_request("GET", path, coven_home, daemon_status)?;
    let body: Value = serde_json::from_str(&response.body)
        .with_context(|| format!("failed to parse API response for {path}"))?;
    if response.status >= 400 {
        let code = body
            .pointer("/error/code")
            .and_then(Value::as_str)
            .unwrap_or("unknown_error");
        let message = body
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("Request failed.");
        bail!("{message} ({code})");
    }
    Ok(body)
}

fn print_json(body: &Value) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(body).context("failed to serialize response as JSON")?
    );
    Ok(())
}

// ── Table rendering ──────────────────────────────────────────────────────────

/// Render fixed-width columns with two-space gutters. Widths fit the widest
/// cell so full ids always survive; pass pre-truncated cells for free text.
fn render_table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if let Some(width) = widths.get_mut(i) {
                *width = (*width).max(cell.chars().count());
            }
        }
    }
    let mut out = String::new();
    let render_row = |cells: Vec<String>| -> String {
        let mut line = String::new();
        let last = cells.len().saturating_sub(1);
        for (i, cell) in cells.iter().enumerate() {
            if i == last {
                // No trailing padding on the final column.
                line.push_str(cell);
            } else {
                let pad = widths[i].saturating_sub(cell.chars().count());
                line.push_str(cell);
                line.extend(std::iter::repeat_n(' ', pad + 2));
            }
        }
        line.trim_end().to_string()
    };
    out.push_str(&render_row(
        headers.iter().map(|h| h.to_string()).collect::<Vec<_>>(),
    ));
    out.push('\n');
    for row in rows {
        out.push_str(&render_row(row.clone()));
        out.push('\n');
    }
    out
}

fn str_cell(value: &Value, key: &str) -> String {
    match value.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => "—".to_string(),
        Some(other) => other.to_string(),
    }
}

// ── coven status ─────────────────────────────────────────────────────────────

pub(crate) fn run_status(json: bool) -> Result<()> {
    let coven_home = coven_home_dir()?;
    if json {
        let daemon_state = daemon::background_server_status(&coven_home)?;
        let live = match &daemon_state {
            Some(daemon::DaemonStatusState::Running(status)) => Some(status.clone()),
            _ => None,
        };
        let health = api_get_with_daemon(&coven_home, "/api/v1/health", live)?;
        let overview = api_get(&coven_home, "/api/v1/overview")?;
        // CLI-level composition of the two stable API bodies; see
        // docs/reference/cli-observe.md.
        return print_json(&serde_json::json!({
            "health": health,
            "overview": overview,
        }));
    }
    print!("{}", view_text(&coven_home, ObserveView::Status)?);
    Ok(())
}

fn render_status(
    daemon_state: Option<&daemon::DaemonStatusState>,
    health: &Value,
    overview: &Value,
) -> String {
    let mut out = String::new();
    out.push_str("Coven status\n\n");

    let daemon_line = match daemon_state {
        Some(daemon::DaemonStatusState::Running(status)) => {
            format!("running (pid {}, socket {})", status.pid, status.socket)
        }
        Some(daemon::DaemonStatusState::Stale(status)) => format!(
            "stale (pid {} is gone) — run `coven daemon restart`",
            status.pid
        ),
        None => "not running — start it with `coven daemon start`".to_string(),
    };
    out.push_str(&format!("  daemon     {daemon_line}\n"));
    out.push_str(&format!(
        "  version    {}\n",
        health
            .get("covenVersion")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    ));

    let count = |key: &str| overview.get(key).and_then(Value::as_u64).unwrap_or(0);
    out.push_str(&format!("  sessions   {} open\n", count("open_sessions")));
    let total_familiars = count("total_familiars");
    if total_familiars == 0 {
        out.push_str("  familiars  none — add [[familiar]] entries to ~/.coven/familiars.toml\n");
    } else {
        out.push_str(&format!(
            "  familiars  {} active / {} total\n",
            count("active_familiars"),
            total_familiars
        ));
    }
    out.push_str(&format!(
        "  skills     {} installed\n",
        count("skills_count")
    ));
    let research_iterations = count("research_iterations");
    if research_iterations > 0 {
        out.push_str(&format!(
            "  research   {} iterations (last Δ {})\n",
            research_iterations,
            overview
                .get("last_research_delta")
                .and_then(Value::as_i64)
                .unwrap_or(0)
        ));
    }
    if let Some(hub) = health.get("hub") {
        let nodes_total = hub.get("nodesTotal").and_then(Value::as_u64).unwrap_or(0);
        if nodes_total > 0 {
            out.push_str(&format!(
                "  hub        {}/{} nodes available (details: coven hub status)\n",
                hub.get("nodesAvailable")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                nodes_total
            ));
        }
    }

    out.push_str("\nNext: coven sessions · coven familiars · coven run <harness> \"<task>\"\n");
    out
}

// ── coven familiars / skills / memory / research ─────────────────────────────

pub(crate) fn run_familiars(json: bool) -> Result<()> {
    let coven_home = coven_home_dir()?;
    if json {
        return print_json(&api_get(&coven_home, "/api/v1/familiars")?);
    }
    print!("{}", view_text(&coven_home, ObserveView::Familiars)?);
    Ok(())
}

fn render_familiars(body: &Value) -> String {
    let familiars = body.as_array().cloned().unwrap_or_default();
    if familiars.is_empty() {
        return "No familiars configured.\n\
                Add [[familiar]] entries to ~/.coven/familiars.toml to build your roster.\n"
            .to_string();
    }
    let rows: Vec<Vec<String>> = familiars
        .iter()
        .map(|f| {
            vec![
                str_cell(f, "id"),
                str_cell(f, "display_name"),
                str_cell(f, "role"),
                str_cell(f, "status"),
                str_cell(f, "memory_freshness"),
                theme::fit_chars(&str_cell(f, "description"), TEXT_CELL_LIMIT),
            ]
        })
        .collect();
    let mut out = render_table(
        &["ID", "NAME", "ROLE", "STATUS", "MEMORY", "DESCRIPTION"],
        &rows,
    );
    out.push_str(&format!(
        "\n{} familiar(s) · roster file: ~/.coven/familiars.toml\n",
        familiars.len()
    ));
    out
}

pub(crate) fn run_skills(json: bool) -> Result<()> {
    let coven_home = coven_home_dir()?;
    if json {
        return print_json(&api_get(&coven_home, "/api/v1/skills")?);
    }
    print!("{}", view_text(&coven_home, ObserveView::Skills)?);
    Ok(())
}

fn render_skills(body: &Value) -> String {
    let skills = body.as_array().cloned().unwrap_or_default();
    if skills.is_empty() {
        return "No skills installed.\n\
                Skills live under ~/.coven/skills/<id>/metadata.json.\n"
            .to_string();
    }
    let rows: Vec<Vec<String>> = skills
        .iter()
        .map(|s| {
            vec![
                str_cell(s, "id"),
                str_cell(s, "version"),
                str_cell(s, "category"),
                str_cell(s, "owner"),
                theme::fit_chars(&str_cell(s, "description"), TEXT_CELL_LIMIT),
            ]
        })
        .collect();
    let mut out = render_table(
        &["ID", "VERSION", "CATEGORY", "OWNER", "DESCRIPTION"],
        &rows,
    );
    out.push_str(&format!("\n{} skill(s)\n", skills.len()));
    out
}

pub(crate) fn run_memory(json: bool) -> Result<()> {
    let coven_home = coven_home_dir()?;
    if json {
        return print_json(&api_get(&coven_home, "/api/v1/memory")?);
    }
    print!("{}", view_text(&coven_home, ObserveView::Memory)?);
    Ok(())
}

fn render_memory(body: &Value) -> String {
    let files = body.as_array().cloned().unwrap_or_default();
    if files.is_empty() {
        return "No memory files.\n\
                Familiars keep durable notes under ~/.coven/memory/<familiar>/.\n"
            .to_string();
    }
    let rows: Vec<Vec<String>> = files
        .iter()
        .map(|m| {
            vec![
                str_cell(m, "familiar_id"),
                str_cell(m, "title"),
                str_cell(m, "updated_at"),
                theme::fit_chars(&str_cell(m, "excerpt"), TEXT_CELL_LIMIT),
            ]
        })
        .collect();
    let mut out = render_table(&["FAMILIAR", "TITLE", "UPDATED", "EXCERPT"], &rows);
    out.push_str(&format!("\n{} memory file(s)\n", files.len()));
    out
}

pub(crate) fn run_research(json: bool) -> Result<()> {
    let coven_home = coven_home_dir()?;
    if json {
        return print_json(&api_get(&coven_home, "/api/v1/research")?);
    }
    print!("{}", view_text(&coven_home, ObserveView::Research)?);
    Ok(())
}

fn render_research(body: &Value) -> String {
    let rows_json = body.as_array().cloned().unwrap_or_default();
    if rows_json.is_empty() {
        return "No research log.\n\
                Rows come from ~/.coven/research/results.tsv.\n"
            .to_string();
    }
    let rows: Vec<Vec<String>> = rows_json
        .iter()
        .map(|r| {
            vec![
                r.get("iteration")
                    .and_then(Value::as_u64)
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "—".to_string()),
                theme::fit_chars(&str_cell(r, "topic"), TEXT_CELL_LIMIT),
                str_cell(r, "score"),
                str_cell(r, "delta"),
                str_cell(r, "decision"),
                str_cell(r, "source"),
            ]
        })
        .collect();
    let mut out = render_table(
        &["ITER", "TOPIC", "SCORE", "DELTA", "DECISION", "SOURCE"],
        &rows,
    );
    out.push_str(&format!("\n{} research iteration(s)\n", rows_json.len()));
    out
}

// ── coven calls ──────────────────────────────────────────────────────────────

pub(crate) fn run_calls(id: Option<&str>, json: bool) -> Result<()> {
    let coven_home = coven_home_dir()?;
    match id {
        Some(id) => {
            let body = api_get(&coven_home, &format!("/api/v1/coven-calls/{id}"))?;
            if json {
                return print_json(&body);
            }
            print!("{}", render_call_detail(&body));
        }
        None => {
            if json {
                return print_json(&api_get(&coven_home, "/api/v1/coven-calls")?);
            }
            print!("{}", view_text(&coven_home, ObserveView::Calls)?);
        }
    }
    Ok(())
}

fn render_calls(body: &Value) -> String {
    let calls = body
        .get("calls")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if calls.is_empty() {
        return "No delegation calls recorded.\n\
                Calls appear here when one familiar delegates work to another through Cast.\n"
            .to_string();
    }
    let rows: Vec<Vec<String>> = calls
        .iter()
        .map(|c| {
            vec![
                str_cell(c, "id"),
                format!(
                    "{} → {}",
                    str_cell(c, "callerFamiliarId"),
                    str_cell(c, "calleeFamiliarId")
                ),
                str_cell(c, "status"),
                str_cell(c, "createdAt"),
                theme::fit_chars(&str_cell(c, "request"), TEXT_CELL_LIMIT),
            ]
        })
        .collect();
    let mut out = render_table(&["ID", "CALL", "STATUS", "CREATED", "REQUEST"], &rows);
    out.push_str(&format!(
        "\n{} call(s) · detail: coven calls <id>\n",
        calls.len()
    ));
    out
}

fn render_call_detail(body: &Value) -> String {
    let call = body.get("call").cloned().unwrap_or(Value::Null);
    let mut out = String::new();
    out.push_str(&format!("Coven call {}\n\n", str_cell(&call, "id")));
    out.push_str(&format!(
        "  caller     {}\n",
        str_cell(&call, "callerFamiliarId")
    ));
    out.push_str(&format!(
        "  callee     {}\n",
        str_cell(&call, "calleeFamiliarId")
    ));
    out.push_str(&format!("  status     {}\n", str_cell(&call, "status")));
    out.push_str(&format!("  created    {}\n", str_cell(&call, "createdAt")));
    if call.get("endedAt").and_then(Value::as_str).is_some() {
        out.push_str(&format!("  ended      {}\n", str_cell(&call, "endedAt")));
    }
    if let Some(session) = call.get("sessionId").and_then(Value::as_str) {
        out.push_str(&format!(
            "  session    {session} (coven sessions show {session})\n"
        ));
    }
    out.push_str(&format!(
        "\n  request\n    {}\n",
        str_cell(&call, "request")
    ));
    if let Some(artifact) = call.get("artifact").and_then(Value::as_str) {
        out.push_str(&format!("\n  artifact\n    {artifact}\n"));
    }
    out
}

// ── coven hub ────────────────────────────────────────────────────────────────

pub(crate) fn run_hub_status(json: bool) -> Result<()> {
    let coven_home = coven_home_dir()?;
    if json {
        return print_json(&api_get(&coven_home, "/api/v1/hub/status")?);
    }
    print!("{}", view_text(&coven_home, ObserveView::HubStatus)?);
    Ok(())
}

fn render_hub_status(body: &Value) -> String {
    let mut out = String::new();
    out.push_str("Hub status\n\n");
    out.push_str(&format!("  role       {}\n", str_cell(body, "role")));
    out.push_str(&format!("  hub id     {}\n", str_cell(body, "hubId")));
    let total = body.get("nodesTotal").and_then(Value::as_u64).unwrap_or(0);
    let available = body
        .get("nodesAvailable")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    out.push_str(&format!("  nodes      {available}/{total} available\n"));
    if let Some(queue) = body.get("globalQueue") {
        let n = |key: &str| queue.get(key).and_then(Value::as_u64).unwrap_or(0);
        out.push_str(&format!(
            "  queue      {} queued · {} assigned · {} held ({} total)\n",
            n("queued"),
            n("assigned"),
            n("held"),
            n("total")
        ));
    }
    if total == 0 {
        out.push_str(
            "\nNo executor nodes registered — this daemon is running single-host.\n\
             See docs/HUB-OPERATIONS.md to register nodes.\n",
        );
        return out;
    }
    let nodes = body
        .get("nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let rows: Vec<Vec<String>> = nodes
        .iter()
        .map(|n| {
            vec![
                str_cell(n, "nodeId"),
                str_cell(n, "available"),
                str_cell(n, "queuePressure"),
                str_cell(n, "lastHealthAt"),
            ]
        })
        .collect();
    out.push('\n');
    out.push_str(&render_table(
        &["NODE", "AVAILABLE", "PRESSURE", "LAST HEALTH"],
        &rows,
    ));
    out
}

pub(crate) fn run_hub_nodes(id: Option<&str>, json: bool) -> Result<()> {
    let coven_home = coven_home_dir()?;
    match id {
        Some(id) => {
            let body = api_get(&coven_home, &format!("/api/v1/hub/nodes/{id}"))?;
            if json {
                return print_json(&body);
            }
            print!("{}", render_hub_node_detail(&body));
        }
        None => {
            let body = api_get(&coven_home, "/api/v1/hub/nodes")?;
            if json {
                return print_json(&body);
            }
            print!("{}", render_hub_nodes(&body));
        }
    }
    Ok(())
}

fn render_hub_nodes(body: &Value) -> String {
    let nodes = body
        .get("nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if nodes.is_empty() {
        return "No executor nodes registered.\n\
                Register nodes over the hub API — see docs/HUB-OPERATIONS.md.\n"
            .to_string();
    }
    let rows: Vec<Vec<String>> = nodes
        .iter()
        .map(|n| {
            let capabilities = n
                .get("capabilities")
                .and_then(Value::as_array)
                .map(|caps| {
                    caps.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_else(|| "—".to_string());
            vec![
                str_cell(n, "nodeId"),
                str_cell(n, "role"),
                str_cell(n, "transport"),
                str_cell(n, "available"),
                str_cell(n, "queuePressure"),
                capabilities,
                str_cell(n, "updatedAt"),
            ]
        })
        .collect();
    let mut out = render_table(
        &[
            "NODE",
            "ROLE",
            "TRANSPORT",
            "AVAILABLE",
            "PRESSURE",
            "CAPABILITIES",
            "UPDATED",
        ],
        &rows,
    );
    out.push_str(&format!(
        "\n{} node(s) · detail: coven hub nodes <id>\n",
        nodes.len()
    ));
    out
}

/// Joined string list from a JSON array field, or an em dash when absent.
fn list_cell(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|joined| !joined.is_empty())
        .unwrap_or_else(|| "—".to_string())
}

fn render_hub_node_detail(node: &Value) -> String {
    let node_id = str_cell(node, "nodeId");
    let mut out = String::new();
    out.push_str(&format!("Hub node {node_id}\n\n"));
    out.push_str(&format!("  role          {}\n", str_cell(node, "role")));
    out.push_str(&format!(
        "  transport     {}\n",
        str_cell(node, "transport")
    ));
    out.push_str(&format!(
        "  available     {}\n",
        str_cell(node, "available")
    ));
    out.push_str(&format!(
        "  pressure      {}\n",
        str_cell(node, "queuePressure")
    ));
    out.push_str(&format!(
        "  capabilities  {}\n",
        list_cell(node, "capabilities")
    ));
    out.push_str(&format!(
        "  last health   {}\n",
        str_cell(node, "lastHealthAt")
    ));
    if node.get("lastError").and_then(Value::as_str).is_some() {
        out.push_str(&format!(
            "  last error    {}\n",
            str_cell(node, "lastError")
        ));
    }
    out.push_str(&format!(
        "  registered    {}\n",
        str_cell(node, "registeredAt")
    ));
    out.push_str(&format!(
        "  updated       {}\n",
        str_cell(node, "updatedAt")
    ));
    out.push_str(&format!(
        "\n  jobs: coven hub jobs --state assigned · full record: coven hub nodes {node_id} --json\n"
    ));
    out
}

pub(crate) fn run_hub_jobs(id: Option<&str>, state: Option<&str>, json: bool) -> Result<()> {
    let coven_home = coven_home_dir()?;
    if let Some(id) = id {
        let body = api_get(&coven_home, &format!("/api/v1/hub/jobs/{id}"))?;
        if json {
            return print_json(&body);
        }
        print!("{}", render_hub_job_detail(&body));
        return Ok(());
    }
    let path = match state {
        Some(state) => format!("/api/v1/hub/jobs?state={state}"),
        None => "/api/v1/hub/jobs".to_string(),
    };
    let body = api_get(&coven_home, &path)?;
    if json {
        return print_json(&body);
    }
    print!("{}", render_hub_jobs(&body));
    Ok(())
}

fn render_hub_jobs(body: &Value) -> String {
    let jobs = body
        .get("jobs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if jobs.is_empty() {
        return "No hub jobs found.\n\
                Jobs are enqueued over the hub API — see docs/HUB-OPERATIONS.md.\n"
            .to_string();
    }
    let rows: Vec<Vec<String>> = jobs
        .iter()
        .map(|j| {
            vec![
                str_cell(j, "jobId"),
                str_cell(j, "state"),
                str_cell(j, "priority"),
                str_cell(j, "assignedNodeId"),
                str_cell(j, "loopId"),
                str_cell(j, "updatedAt"),
            ]
        })
        .collect();
    let mut out = render_table(
        &["JOB", "STATE", "PRIORITY", "NODE", "LOOP", "UPDATED"],
        &rows,
    );
    out.push_str(&format!(
        "\n{} job(s) · detail: coven hub jobs <id> · filter with --state <queued|assigned|held|completed|failed|cancelled>\n",
        jobs.len()
    ));
    out
}

/// Character budget for free-text/JSON previews in detail views. Wider than
/// [`TEXT_CELL_LIMIT`] because detail lines own the whole row; `--json` is the
/// escape hatch for the full record.
const DETAIL_TEXT_LIMIT: usize = 160;

fn render_hub_job_detail(job: &Value) -> String {
    let job_id = str_cell(job, "jobId");
    let mut out = String::new();
    out.push_str(&format!("Hub job {job_id}\n\n"));
    out.push_str(&format!("  state      {}\n", str_cell(job, "state")));
    out.push_str(&format!("  priority   {}\n", str_cell(job, "priority")));
    out.push_str(&format!(
        "  requires   {}\n",
        list_cell(job, "requiredCapabilities")
    ));
    out.push_str(&format!(
        "  node       {}\n",
        str_cell(job, "assignedNodeId")
    ));
    out.push_str(&format!("  loop       {}\n", str_cell(job, "loopId")));
    out.push_str(&format!("  created    {}\n", str_cell(job, "createdAt")));
    out.push_str(&format!("  updated    {}\n", str_cell(job, "updatedAt")));
    if let Some(route) = job.get("route").filter(|route| !route.is_null()) {
        out.push_str("\n  route\n");
        out.push_str(&format!("    node       {}\n", str_cell(route, "nodeId")));
        out.push_str(&format!(
            "    decision   {}\n",
            str_cell(route, "decisionId")
        ));
        out.push_str(&format!(
            "    reason     {}\n",
            theme::fit_chars(&str_cell(route, "reason"), DETAIL_TEXT_LIMIT)
        ));
    }
    if let Some(payload) = job.get("payload").filter(|payload| !payload.is_null()) {
        out.push_str(&format!(
            "\n  payload\n    {}\n",
            theme::fit_chars(&payload.to_string(), DETAIL_TEXT_LIMIT)
        ));
    }
    out.push_str(&format!(
        "\n  dispatch record: coven hub dispatch {job_id} · full record: coven hub jobs {job_id} --json\n"
    ));
    out
}

pub(crate) fn run_hub_dispatch(job_id: &str, json: bool) -> Result<()> {
    let body = api_get(
        &coven_home_dir()?,
        &format!("/api/v1/hub/dispatches/{job_id}"),
    )?;
    if json {
        return print_json(&body);
    }
    print!("{}", render_hub_dispatch(&body));
    Ok(())
}

fn render_hub_dispatch(body: &Value) -> String {
    let job_id = str_cell(body, "jobId");
    let mut out = String::new();
    out.push_str(&format!("Executor dispatch {job_id}\n\n"));
    out.push_str(&format!("  node       {}\n", str_cell(body, "nodeId")));
    out.push_str(&format!("  status     {}\n", str_cell(body, "status")));
    out.push_str(&format!("  created    {}\n", str_cell(body, "createdAt")));
    out.push_str(&format!("  updated    {}\n", str_cell(body, "updatedAt")));
    if let Some(job) = body.get("job").filter(|job| !job.is_null()) {
        out.push_str(&format!(
            "\n  job spec\n    {}\n",
            theme::fit_chars(&job.to_string(), DETAIL_TEXT_LIMIT)
        ));
    }
    match body.get("envelope").filter(|envelope| !envelope.is_null()) {
        Some(envelope) => out.push_str(&format!(
            "\n  result envelope\n    {}\n",
            theme::fit_chars(&envelope.to_string(), DETAIL_TEXT_LIMIT)
        )),
        None => out.push_str("\n  result envelope\n    — (no result reported yet)\n"),
    }
    out.push_str(&format!(
        "\n  full record: coven hub dispatch {job_id} --json\n"
    ));
    out
}

pub(crate) fn run_hub_routing(json: bool) -> Result<()> {
    let body = api_get(&coven_home_dir()?, "/api/v1/hub/routing")?;
    if json {
        return print_json(&body);
    }
    print!("{}", render_hub_routing(&body));
    Ok(())
}

fn render_hub_routing(body: &Value) -> String {
    let routes = body
        .get("routes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if routes.is_empty() {
        return "No routing decisions recorded.\n".to_string();
    }
    let rows: Vec<Vec<String>> = routes
        .iter()
        .map(|r| {
            vec![
                str_cell(r, "jobId"),
                str_cell(r, "nodeId"),
                str_cell(r, "decisionId"),
                theme::fit_chars(&str_cell(r, "reason"), TEXT_CELL_LIMIT),
                str_cell(r, "updatedAt"),
            ]
        })
        .collect();
    let mut out = render_table(&["JOB", "NODE", "DECISION", "REASON", "UPDATED"], &rows);
    out.push_str(&format!("\n{} route(s)\n", routes.len()));
    out
}

// ── coven sessions show/events/log ───────────────────────────────────────────

pub(crate) fn run_session_show(reference: &str, json: bool) -> Result<()> {
    let coven_home = coven_home_dir()?;
    let session_id = resolve_full_session_id(reference)?;
    let body = api_get(&coven_home, &format!("/api/v1/sessions/{session_id}"))?;
    if json {
        return print_json(&body);
    }
    print!("{}", render_session_detail(&body));
    Ok(())
}

fn render_session_detail(body: &Value) -> String {
    let mut out = String::new();
    let id = str_cell(body, "id");
    out.push_str(&format!("Session {id}\n\n"));
    out.push_str(&format!("  title       {}\n", str_cell(body, "title")));
    out.push_str(&format!("  status      {}\n", str_cell(body, "status")));
    out.push_str(&format!("  harness     {}\n", str_cell(body, "harness")));
    out.push_str(&format!(
        "  project     {}\n",
        str_cell(body, "project_root")
    ));
    if let Some(familiar) = body.get("familiar_id").and_then(Value::as_str) {
        out.push_str(&format!("  familiar    {familiar}\n"));
    }
    if let Some(labels) = body.get("labels").and_then(Value::as_array) {
        if !labels.is_empty() {
            let joined = labels
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!("  labels      {joined}\n"));
        }
    }
    out.push_str(&format!("  visibility  {}\n", str_cell(body, "visibility")));
    out.push_str(&format!("  created     {}\n", str_cell(body, "created_at")));
    out.push_str(&format!("  updated     {}\n", str_cell(body, "updated_at")));
    if let Some(exit_code) = body.get("exit_code").and_then(Value::as_i64) {
        out.push_str(&format!("  exit code   {exit_code}\n"));
    }
    if let Some(archived) = body.get("archived_at").and_then(Value::as_str) {
        out.push_str(&format!("  archived    {archived}\n"));
    }
    out.push_str(&format!(
        "\nNext: coven attach {id} · coven sessions events {id} · coven sessions log {id}\n"
    ));
    out
}

pub(crate) fn run_session_events(
    reference: &str,
    after_seq: Option<u64>,
    limit: Option<u64>,
    json: bool,
) -> Result<()> {
    let coven_home = coven_home_dir()?;
    let session_id = resolve_full_session_id(reference)?;
    let mut query = Vec::new();
    if let Some(after_seq) = after_seq {
        query.push(format!("afterSeq={after_seq}"));
    }
    if let Some(limit) = limit {
        query.push(format!("limit={limit}"));
    }
    let path = if query.is_empty() {
        format!("/api/v1/sessions/{session_id}/events")
    } else {
        format!("/api/v1/sessions/{session_id}/events?{}", query.join("&"))
    };
    let body = api_get(&coven_home, &path)?;
    if json {
        return print_json(&body);
    }
    print!("{}", render_session_events(&body));
    Ok(())
}

fn render_session_events(body: &Value) -> String {
    let events = body
        .get("events")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if events.is_empty() {
        return "No events recorded for this session yet.\n".to_string();
    }
    let rows: Vec<Vec<String>> = events
        .iter()
        .map(|e| {
            let payload = e
                .get("payload_json")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .replace(['\n', '\r'], " ");
            vec![
                e.get("seq")
                    .and_then(Value::as_i64)
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "—".to_string()),
                str_cell(e, "created_at"),
                str_cell(e, "kind"),
                theme::fit_chars(payload.trim(), 64),
            ]
        })
        .collect();
    let mut out = render_table(&["SEQ", "CREATED", "KIND", "PAYLOAD"], &rows);
    let mut footer = format!("\n{} event(s)", events.len());
    if body.get("hasMore").and_then(Value::as_bool) == Some(true) {
        if let Some(cursor) = body.pointer("/nextCursor/afterSeq").and_then(Value::as_i64) {
            footer.push_str(&format!(" · more: --after-seq {cursor}"));
        }
    }
    footer.push('\n');
    out.push_str(&footer);
    out
}

pub(crate) fn run_session_log(reference: &str, json: bool) -> Result<()> {
    let coven_home = coven_home_dir()?;
    let session_id = resolve_full_session_id(reference)?;
    let body = api_get(&coven_home, &format!("/api/v1/sessions/{session_id}/log"))?;
    if json {
        return print_json(&body);
    }
    print!("{}", render_session_log(&body));
    Ok(())
}

fn render_session_log(body: &Value) -> String {
    let lines = body.as_array().cloned().unwrap_or_default();
    if lines.is_empty() {
        return "No log lines recorded for this session yet.\n".to_string();
    }
    let mut out = String::new();
    for line in &lines {
        out.push_str(&format!(
            "{} [{}] {}\n",
            str_cell(line, "ts"),
            str_cell(line, "level"),
            str_cell(line, "message")
        ));
    }
    out
}

/// Resolve a full or unique-prefix session reference to a full session id,
/// reusing the lifecycle commands' resolver so hints stay consistent.
fn resolve_full_session_id(reference: &str) -> Result<String> {
    let store_path = crate::coven_store_path()?;
    let conn = crate::store::open_store(&store_path)?;
    let session = crate::resolve_session_ref(&conn, reference)?;
    Ok(session.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn render_table_pads_columns_and_trims_trailing_space() {
        let out = render_table(
            &["ID", "NAME"],
            &[
                vec!["a".to_string(), "Alpha".to_string()],
                vec!["longer-id".to_string(), "B".to_string()],
            ],
        );
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "ID         NAME");
        assert_eq!(lines[1], "a          Alpha");
        assert_eq!(lines[2], "longer-id  B");
    }

    #[test]
    fn render_status_shows_counts_and_daemon_state() {
        let health = json!({
            "covenVersion": "1.2.3",
            "hub": { "nodesTotal": 2, "nodesAvailable": 1 }
        });
        let overview = json!({
            "open_sessions": 3,
            "active_familiars": 1,
            "total_familiars": 2,
            "skills_count": 4,
            "research_iterations": 5,
            "last_research_delta": 2
        });
        let out = render_status(None, &health, &overview);
        assert!(out.contains("daemon     not running — start it with `coven daemon start`"));
        assert!(out.contains("version    1.2.3"));
        assert!(out.contains("sessions   3 open"));
        assert!(out.contains("familiars  1 active / 2 total"));
        assert!(out.contains("skills     4 installed"));
        assert!(out.contains("research   5 iterations (last Δ 2)"));
        assert!(out.contains("hub        1/2 nodes available"));
        assert!(out.contains("Next: coven sessions"));
    }

    #[test]
    fn render_status_teaches_first_run_users() {
        let health = json!({ "covenVersion": "1.2.3" });
        let overview = json!({
            "open_sessions": 0,
            "active_familiars": 0,
            "total_familiars": 0,
            "skills_count": 0,
            "research_iterations": 0,
            "last_research_delta": 0
        });
        let out = render_status(None, &health, &overview);
        assert!(out.contains("familiars  none — add [[familiar]] entries"));
        // Zero-iteration research and zero-node hub stay quiet.
        assert!(!out.contains("research"));
        assert!(!out.contains("hub "));
    }

    #[test]
    fn render_familiars_lists_roster_and_hints_file() {
        let body = json!([
            {
                "id": "charm",
                "display_name": "Charm",
                "role": "steward",
                "status": "offline",
                "memory_freshness": "2d ago",
                "description": "keeps the hearth"
            }
        ]);
        let out = render_familiars(&body);
        assert!(out.contains("ID"));
        assert!(out.contains("charm"));
        assert!(out.contains("steward"));
        assert!(out.contains("1 familiar(s)"));
        assert!(out.contains("~/.coven/familiars.toml"));
    }

    #[test]
    fn render_familiars_empty_state_teaches_setup() {
        let out = render_familiars(&json!([]));
        assert!(out.contains("No familiars configured."));
        assert!(out.contains("familiars.toml"));
    }

    #[test]
    fn render_skills_lists_inventory() {
        let body = json!([
            {
                "id": "eval-loop",
                "version": "1.0.0",
                "category": "general",
                "owner": "coven",
                "description": "run the eval loop"
            }
        ]);
        let out = render_skills(&body);
        assert!(out.contains("eval-loop"));
        assert!(out.contains("1 skill(s)"));
    }

    #[test]
    fn render_memory_and_research_have_empty_hints() {
        assert!(render_memory(&json!([])).contains("~/.coven/memory/"));
        assert!(render_research(&json!([])).contains("results.tsv"));
    }

    #[test]
    fn render_calls_lists_delegations_with_arrow() {
        let body = json!({
            "ok": true,
            "calls": [{
                "id": "call-1",
                "callerFamiliarId": "nova",
                "calleeFamiliarId": "sage",
                "status": "running",
                "createdAt": "2026-01-01T00:00:00Z",
                "request": "summarize the release notes"
            }]
        });
        let out = render_calls(&body);
        assert!(out.contains("nova → sage"));
        assert!(out.contains("running"));
        assert!(out.contains("coven calls <id>"));
    }

    #[test]
    fn render_call_detail_links_session_inspection() {
        let body = json!({
            "ok": true,
            "call": {
                "id": "call-1",
                "callerFamiliarId": "nova",
                "calleeFamiliarId": "sage",
                "status": "completed",
                "createdAt": "2026-01-01T00:00:00Z",
                "endedAt": "2026-01-01T01:00:00Z",
                "sessionId": "sess-9",
                "request": "summarize",
                "artifact": "done"
            }
        });
        let out = render_call_detail(&body);
        assert!(out.contains("caller     nova"));
        assert!(out.contains("ended      2026-01-01T01:00:00Z"));
        assert!(out.contains("coven sessions show sess-9"));
        assert!(out.contains("artifact"));
    }

    #[test]
    fn render_hub_status_single_host_explains_itself() {
        let body = json!({
            "role": "hub",
            "hubId": "hub_1",
            "nodesTotal": 0,
            "nodesAvailable": 0,
            "globalQueue": { "queued": 0, "assigned": 0, "held": 0, "total": 0 },
            "nodes": []
        });
        let out = render_hub_status(&body);
        assert!(out.contains("single-host"));
        assert!(out.contains("docs/HUB-OPERATIONS.md"));
    }

    #[test]
    fn render_hub_status_lists_nodes_when_registered() {
        let body = json!({
            "role": "hub",
            "hubId": "hub_1",
            "nodesTotal": 1,
            "nodesAvailable": 1,
            "globalQueue": { "queued": 2, "assigned": 1, "held": 0, "total": 3 },
            "nodes": [{
                "nodeId": "node_a",
                "available": true,
                "queuePressure": 0,
                "lastHealthAt": "2026-01-01T00:00:00Z"
            }]
        });
        let out = render_hub_status(&body);
        assert!(out.contains("nodes      1/1 available"));
        assert!(out.contains("2 queued · 1 assigned · 0 held (3 total)"));
        assert!(out.contains("node_a"));
    }

    #[test]
    fn render_hub_jobs_hints_state_filter() {
        let body = json!({
            "jobs": [{
                "jobId": "job-1",
                "state": "queued",
                "priority": 5,
                "assignedNodeId": null,
                "loopId": null,
                "updatedAt": "2026-01-01T00:00:00Z"
            }]
        });
        let out = render_hub_jobs(&body);
        assert!(out.contains("job-1"));
        assert!(out.contains("--state <queued|assigned|held|completed|failed|cancelled>"));
    }

    #[test]
    fn render_session_detail_suggests_next_commands() {
        let body = json!({
            "id": "sess-1",
            "title": "fix auth",
            "status": "running",
            "harness": "codex",
            "project_root": "/tmp/project",
            "visibility": "private",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:05:00Z",
            "labels": ["auth", "bug"],
            "familiar_id": "charm"
        });
        let out = render_session_detail(&body);
        assert!(out.contains("Session sess-1"));
        assert!(out.contains("title       fix auth"));
        assert!(out.contains("familiar    charm"));
        assert!(out.contains("labels      auth, bug"));
        assert!(out.contains("coven attach sess-1"));
        assert!(out.contains("coven sessions events sess-1"));
    }

    #[test]
    fn render_session_events_shows_cursor_hint_when_more() {
        let body = json!({
            "events": [{
                "seq": 7,
                "id": "evt-7",
                "session_id": "sess-1",
                "kind": "pty_output",
                "payload_json": "{\"text\":\"hello\\nworld\"}",
                "created_at": "2026-01-01T00:00:00Z"
            }],
            "nextCursor": { "afterSeq": 7 },
            "hasMore": true
        });
        let out = render_session_events(&body);
        assert!(out.contains("pty_output"));
        assert!(!out.contains('\r'));
        assert!(out.contains("--after-seq 7"));
    }

    #[test]
    fn render_session_log_prints_ts_level_message_lines() {
        let body = json!([
            { "ts": "2026-01-01T00:00:00Z", "level": "info", "message": "hello" }
        ]);
        let out = render_session_log(&body);
        assert_eq!(out, "2026-01-01T00:00:00Z [info] hello\n");
    }

    // ── End-to-end parity: seeded home → real API body → renderer ───────────
    //
    // These tests exist to catch key drift: if a DTO field is renamed on the
    // API side, the rendered output loses real values and the assertions
    // below fail — the fixture-only tests above cannot see that.

    fn get_body(home: &Path, path: &str) -> Result<Value> {
        let response = crate::api::handle_request("GET", path, home, None)?;
        anyhow::ensure!(
            response.status < 400,
            "GET {path} failed with {}: {}",
            response.status,
            response.body
        );
        serde_json::from_str(&response.body).map_err(Into::into)
    }

    #[test]
    fn familiars_renderer_matches_live_api_body() -> Result<()> {
        let temp = tempfile::tempdir()?;
        std::fs::write(
            temp.path().join("familiars.toml"),
            "[[familiar]]\n\
             id = \"charm\"\n\
             display_name = \"Charm\"\n\
             role = \"steward\"\n\
             description = \"keeps the hearth\"\n",
        )?;
        let body = get_body(temp.path(), "/api/v1/familiars")?;
        let out = render_familiars(&body);
        assert!(out.contains("charm"), "id column lost: {out}");
        assert!(out.contains("Charm"), "display_name column lost: {out}");
        assert!(out.contains("steward"), "role column lost: {out}");
        assert!(out.contains("keeps the hearth"), "description lost: {out}");
        Ok(())
    }

    #[test]
    fn skills_renderer_matches_live_api_body() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let dir = temp.path().join("skills").join("eval-loop");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            dir.join("metadata.json"),
            r#"{"name":"eval-loop","description":"run the loop","version":"2.0.0","author":"coven","category":"ops"}"#,
        )?;
        let body = get_body(temp.path(), "/api/v1/skills")?;
        let out = render_skills(&body);
        assert!(out.contains("eval-loop"), "id lost: {out}");
        assert!(out.contains("2.0.0"), "version lost: {out}");
        assert!(out.contains("ops"), "category lost: {out}");
        assert!(out.contains("run the loop"), "description lost: {out}");
        Ok(())
    }

    #[test]
    fn memory_renderer_matches_live_api_body() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let dir = temp.path().join("memory").join("charm");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("hearth-notes.md"), "# Hearth\nembers burning\n")?;
        let body = get_body(temp.path(), "/api/v1/memory")?;
        let out = render_memory(&body);
        assert!(out.contains("charm"), "familiar_id lost: {out}");
        assert!(out.contains("hearth-notes"), "title lost: {out}");
        Ok(())
    }

    #[test]
    fn research_renderer_matches_live_api_body() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let dir = temp.path().join("research");
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            dir.join("results.tsv"),
            "3\tstream continuity\t9.0\t2.5\tadopt\tnotes.md\n",
        )?;
        let body = get_body(temp.path(), "/api/v1/research")?;
        let out = render_research(&body);
        assert!(out.contains("stream continuity"), "topic lost: {out}");
        assert!(out.contains("adopt"), "decision lost: {out}");
        assert!(out.contains("2.5"), "delta lost: {out}");
        Ok(())
    }

    #[test]
    fn calls_renderer_matches_live_api_body() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let call_id = crate::coven_calls::emit_running(
            temp.path(),
            "nova",
            "sage",
            "summarize the release notes",
            Some("sess-9"),
        )?;
        let list = get_body(temp.path(), "/api/v1/coven-calls")?;
        let out = render_calls(&list);
        assert!(out.contains("nova → sage"), "caller/callee lost: {out}");
        assert!(out.contains("running"), "status lost: {out}");

        let detail = get_body(temp.path(), &format!("/api/v1/coven-calls/{call_id}"))?;
        let out = render_call_detail(&detail);
        assert!(out.contains("caller     nova"), "caller lost: {out}");
        assert!(
            out.contains("coven sessions show sess-9"),
            "session link lost: {out}"
        );
        Ok(())
    }

    #[test]
    fn hub_renderers_match_live_api_bodies() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let register = crate::api::handle_request_with_body(
            "POST",
            "/api/v1/hub/nodes",
            temp.path(),
            None,
            Some(
                r#"{"nodeId":"node_a","role":"compute_executor","transport":"ssh","capabilities":["gpu"]}"#,
            ),
        )?;
        anyhow::ensure!(register.status == 201, "register: {}", register.body);
        let enqueue = crate::api::handle_request_with_body(
            "POST",
            "/api/v1/hub/jobs",
            temp.path(),
            None,
            Some(r#"{"jobId":"job-1","requiredCapabilities":["gpu"],"priority":5}"#),
        )?;
        anyhow::ensure!(enqueue.status == 201, "enqueue: {}", enqueue.body);

        let status = get_body(temp.path(), "/api/v1/hub/status")?;
        let out = render_hub_status(&status);
        assert!(out.contains("role       hub"), "role lost: {out}");
        assert!(
            out.contains("nodes      1/1 available"),
            "nodes lost: {out}"
        );
        assert!(out.contains("1 queued"), "queue lost: {out}");
        assert!(out.contains("node_a"), "node row lost: {out}");

        let nodes = get_body(temp.path(), "/api/v1/hub/nodes")?;
        let out = render_hub_nodes(&nodes);
        assert!(out.contains("node_a"), "nodeId lost: {out}");
        assert!(out.contains("compute_executor"), "role lost: {out}");
        assert!(out.contains("gpu"), "capabilities lost: {out}");

        let jobs = get_body(temp.path(), "/api/v1/hub/jobs?state=queued")?;
        let out = render_hub_jobs(&jobs);
        assert!(out.contains("job-1"), "jobId lost: {out}");
        assert!(out.contains("queued"), "state lost: {out}");

        let routing = get_body(temp.path(), "/api/v1/hub/routing")?;
        let out = render_hub_routing(&routing);
        assert!(
            out.contains("No routing decisions recorded."),
            "unexpected routing render: {out}"
        );

        // Assign the job so the routing table gains a row, then confirm the
        // renderer surfaces the live envelope's route fields.
        let assign = crate::api::handle_request_with_body(
            "POST",
            "/api/v1/hub/jobs/job-1/assign",
            temp.path(),
            None,
            Some(r#"{"nodeId":"node_a"}"#),
        )?;
        anyhow::ensure!(assign.status == 200, "assign: {}", assign.body);

        let routing = get_body(temp.path(), "/api/v1/hub/routing")?;
        let out = render_hub_routing(&routing);
        assert!(out.contains("job-1"), "routed jobId lost: {out}");
        assert!(out.contains("node_a"), "routed nodeId lost: {out}");

        let assigned = get_body(temp.path(), "/api/v1/hub/jobs?state=assigned")?;
        let out = render_hub_jobs(&assigned);
        assert!(out.contains("job-1"), "assigned job lost: {out}");
        assert!(out.contains("node_a"), "assigned node lost: {out}");
        Ok(())
    }

    #[test]
    fn hub_detail_renderers_match_live_api_bodies() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let register = crate::api::handle_request_with_body(
            "POST",
            "/api/v1/hub/nodes",
            temp.path(),
            None,
            Some(
                r#"{"nodeId":"node_a","role":"compute_executor","transport":"ssh","capabilities":["gpu"]}"#,
            ),
        )?;
        anyhow::ensure!(register.status == 201, "register: {}", register.body);
        let enqueue = crate::api::handle_request_with_body(
            "POST",
            "/api/v1/hub/jobs",
            temp.path(),
            None,
            Some(r#"{"jobId":"job-1","requiredCapabilities":["gpu"],"priority":5}"#),
        )?;
        anyhow::ensure!(enqueue.status == 201, "enqueue: {}", enqueue.body);
        let assign = crate::api::handle_request_with_body(
            "POST",
            "/api/v1/hub/jobs/job-1/assign",
            temp.path(),
            None,
            Some(r#"{"nodeId":"node_a"}"#),
        )?;
        anyhow::ensure!(assign.status == 200, "assign: {}", assign.body);

        let node = get_body(temp.path(), "/api/v1/hub/nodes/node_a")?;
        let out = render_hub_node_detail(&node);
        assert!(out.contains("Hub node node_a"), "node id lost: {out}");
        assert!(out.contains("compute_executor"), "role lost: {out}");
        assert!(out.contains("gpu"), "capabilities lost: {out}");
        assert!(
            out.contains("coven hub nodes node_a --json"),
            "json hint lost: {out}"
        );

        let job = get_body(temp.path(), "/api/v1/hub/jobs/job-1")?;
        let out = render_hub_job_detail(&job);
        assert!(out.contains("Hub job job-1"), "job id lost: {out}");
        assert!(out.contains("assigned"), "state lost: {out}");
        assert!(out.contains("node_a"), "assigned node lost: {out}");
        assert!(out.contains("route"), "route section lost: {out}");
        assert!(
            out.contains("coven hub dispatch job-1"),
            "dispatch hint lost: {out}"
        );

        // Persist a dispatch record directly: the dispatch route runs a live
        // transport, which a unit test must not do.
        let conn = crate::store::open_store(&temp.path().join(crate::STORE_FILE_NAME))?;
        crate::store::upsert_executor_dispatch(
            &conn,
            &crate::store::ExecutorDispatchRecord {
                job_id: "job-1".into(),
                node_id: "node_a".into(),
                status: "completed".into(),
                job_json: r#"{"jobId":"job-1","command":["echo","ok"]}"#.into(),
                envelope_json: Some(
                    r#"{"jobId":"job-1","status":"completed","exitCode":0}"#.into(),
                ),
                created_at: "2026-01-01T00:00:00Z".into(),
                updated_at: "2026-01-01T00:00:05Z".into(),
            },
        )?;
        let dispatch = get_body(temp.path(), "/api/v1/hub/dispatches/job-1")?;
        let out = render_hub_dispatch(&dispatch);
        assert!(
            out.contains("Executor dispatch job-1"),
            "dispatch id lost: {out}"
        );
        assert!(out.contains("node_a"), "dispatch node lost: {out}");
        assert!(out.contains("completed"), "dispatch status lost: {out}");
        assert!(out.contains("exitCode"), "envelope lost: {out}");
        Ok(())
    }

    #[test]
    fn render_hub_dispatch_marks_missing_envelope() {
        let out = render_hub_dispatch(&json!({
            "jobId": "job-2",
            "nodeId": "node_a",
            "status": "dispatched",
            "job": {"command": ["true"]},
            "envelope": null,
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-01T00:00:00Z",
        }));
        assert!(
            out.contains("no result reported yet"),
            "missing-envelope state lost: {out}"
        );
    }

    #[test]
    fn session_renderers_match_live_api_bodies() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let conn = crate::store::open_store(&home.join(crate::STORE_FILE_NAME))?;
        crate::store::insert_session(
            &conn,
            &crate::store::SessionRecord {
                id: "sess-live".into(),
                project_root: "/tmp/project".into(),
                harness: "codex".into(),
                title: "fix auth".into(),
                status: "running".into(),
                exit_code: None,
                archived_at: None,
                created_at: "2026-01-01T00:00:00Z".into(),
                updated_at: "2026-01-01T00:05:00Z".into(),
                conversation_id: None,
                familiar_id: Some("charm".into()),
                labels: vec!["auth".into()],
                visibility: "private".into(),
                external: false,
                transcript_path: None,
            },
        )?;
        crate::store::insert_event_with_privacy(
            &conn,
            home,
            &crate::store::EventRecord {
                seq: 0,
                id: "evt-1".into(),
                session_id: "sess-live".into(),
                kind: "output".into(),
                payload_json: r#"{"data":"hello world"}"#.into(),
                created_at: "2026-01-01T00:01:00Z".into(),
            },
        )?;
        drop(conn);

        let detail = get_body(home, "/api/v1/sessions/sess-live")?;
        let out = render_session_detail(&detail);
        assert!(out.contains("Session sess-live"), "id lost: {out}");
        assert!(out.contains("title       fix auth"), "title lost: {out}");
        assert!(out.contains("familiar    charm"), "familiar lost: {out}");
        assert!(out.contains("labels      auth"), "labels lost: {out}");

        let events = get_body(home, "/api/v1/sessions/sess-live/events?limit=10")?;
        let out = render_session_events(&events);
        assert!(out.contains("output"), "kind lost: {out}");
        assert!(out.contains("hello world"), "payload lost: {out}");

        let log = get_body(home, "/api/v1/sessions/sess-live/log")?;
        let out = render_session_log(&log);
        assert!(out.contains("2026-01-01T00:01:00Z"), "ts lost: {out}");
        Ok(())
    }
}
