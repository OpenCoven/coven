//! Daemon-backed chat client for the rich TUI.
//!
//! This module intentionally stays thin: the daemon owns live session launch,
//! cwd validation, input delivery, kill, and structured errors. Local session
//! ritual verbs use the shared store path/timestamp helpers because they are
//! ledger-only mutations.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use uuid::Uuid;

use coven_client::{
    ClientError, DaemonClient, DaemonEndpoint, ReadEndpoint, WriteEndpoint, PROTOCOL_VERSION,
};

use crate::{
    api::{EventsResponse, SessionPageResponse},
    current_timestamp, daemon, harness, store, STORE_FILE_NAME,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum ChatDaemonStatus {
    Running {
        pid: u32,
    },
    Stale {
        pid: u32,
    },
    #[default]
    Stopped,
    ApiMismatch {
        expected: String,
        actual: String,
    },
    Unavailable {
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LaunchRequest {
    pub(crate) id: String,
    pub(crate) project_root: String,
    pub(crate) cwd: String,
    pub(crate) harness: String,
    pub(crate) launch_mode: harness::HarnessLaunchMode,
    pub(crate) prompt: String,
    pub(crate) title: String,
    pub(crate) conversation: Option<harness::ConversationHint>,
    /// Stable per-conversation id used to group multiple chat turns under
    /// one row in `/sessions`. Conceptually distinct from `conversation`
    /// (which drives the harness CLI's own resume args), though in
    /// practice both fields carry the same value for both harnesses:
    /// claude's chat-generated UUID is also the `conversation_id`, and
    /// codex's captured `session id: <uuid>` is reused as the
    /// `conversation_id` once we learn it. See
    /// `docs/chat-persistence.md`.
    pub(crate) conversation_id: Option<String>,
}

impl LaunchRequest {
    pub(crate) fn for_current_dir(harness: &str, prompt: &str) -> Result<Self> {
        let cwd = std::env::current_dir().context("failed to read current directory")?;
        let cwd = cwd.to_string_lossy().into_owned();
        Ok(Self {
            id: Uuid::new_v4().to_string(),
            project_root: cwd.clone(),
            cwd,
            harness: harness.to_string(),
            launch_mode: harness::HarnessLaunchMode::NonInteractive,
            prompt: prompt.to_string(),
            title: session_title(prompt),
            conversation: None,
            conversation_id: None,
        })
    }

    pub(crate) fn with_conversation(mut self, hint: harness::ConversationHint) -> Self {
        self.conversation = Some(hint);
        self
    }

    pub(crate) fn with_conversation_id(mut self, id: String) -> Self {
        self.conversation_id = Some(id);
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ChatEventQuery<'a> {
    pub(crate) session_id: &'a str,
    pub(crate) after_seq: Option<i64>,
    pub(crate) limit: Option<i64>,
}

pub(crate) const EVENT_PAGE_LIMIT: i64 = 512;
pub(crate) const EVENT_PAGES_PER_POLL: usize = 8;

/// Sessions requested per `/sessions` round trip. Stays inside the daemon's
/// 1000-row page ceiling so a page is one bounded response, not a guess.
pub(crate) const SESSION_PAGE_LIMIT: u16 = 200;
/// Ceiling on the overlay listing. The overlay renders from memory, so the
/// listing stops paging here rather than following an unbounded ledger; the
/// previous single request stopped at 100 without saying so.
pub(crate) const MAX_LISTED_SESSIONS: usize = 2_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChatEventPage {
    pub(crate) events: Vec<store::EventRecord>,
    pub(crate) next_after_seq: Option<i64>,
    pub(crate) has_more: bool,
}

pub(crate) trait ChatClient {
    fn daemon_status(&mut self) -> Result<ChatDaemonStatus>;
    fn launch_session(&mut self, request: LaunchRequest) -> Result<store::SessionRecord>;
    fn get_session(&mut self, session_id: &str) -> Result<store::SessionRecord>;
    fn list_sessions(&mut self) -> Result<Vec<store::SessionRecord>>;
    fn list_events(&mut self, query: ChatEventQuery<'_>) -> Result<Vec<store::EventRecord>>;
    fn list_event_page(&mut self, query: ChatEventQuery<'_>) -> Result<ChatEventPage> {
        let requested_limit = query.limit;
        let events = self.list_events(query)?;
        let next_after_seq = events.last().map(|event| event.seq);
        let has_more = requested_limit.is_some_and(|limit| events.len() as i64 >= limit);
        Ok(ChatEventPage {
            events,
            next_after_seq,
            has_more,
        })
    }
    fn send_input(&mut self, session_id: &str, data: &str) -> Result<()>;
    fn kill_session(&mut self, session_id: &str) -> Result<()>;
    fn archive_session(&mut self, session_id: &str) -> Result<()>;
    fn summon_session(&mut self, session_id: &str) -> Result<store::SessionRecord>;
    fn sacrifice_session(&mut self, session_id: &str) -> Result<()>;
}

pub(crate) fn validated_event_page_cursor(
    requested_after_seq: Option<i64>,
    page: &ChatEventPage,
) -> Result<Option<i64>> {
    let mut cursor = requested_after_seq;
    for event in &page.events {
        if cursor.is_some_and(|previous| event.seq <= previous) {
            anyhow::bail!(
                "daemon event page was not strictly ordered after sequence {}",
                cursor.expect("checked cursor")
            );
        }
        cursor = Some(event.seq);
    }

    let expected_cursor = page.events.last().map(|event| event.seq);
    if page.next_after_seq != expected_cursor {
        anyhow::bail!("daemon event page returned an inconsistent continuation cursor");
    }
    if page.has_more && page.events.is_empty() {
        anyhow::bail!("daemon event page claimed a continuation without advancing its cursor");
    }
    Ok(cursor)
}

/// Follow the daemon's `next_cursor` until the session listing is exhausted.
///
/// The daemon pages `/sessions` and the previous caller asked for one page of
/// 100 and dropped the continuation, so a ledger with more sessions than that
/// simply lost the remainder with no error and no marker. Paging stops at
/// [`MAX_LISTED_SESSIONS`] because the overlay renders the whole result from
/// memory.
pub(crate) fn collect_session_pages<F>(mut fetch: F) -> Result<Vec<store::SessionRecord>>
where
    F: FnMut(Option<String>) -> Result<SessionPageResponse>,
{
    let mut sessions: Vec<store::SessionRecord> = Vec::new();
    let mut cursor = None;
    loop {
        let page = fetch(cursor.clone())?;
        let page_len = page.sessions.len();
        sessions.extend(page.sessions);
        let Some(next_cursor) = page.next_cursor else {
            return Ok(sessions);
        };
        // A repeated cursor replays the same page forever, and a page that
        // advertises a continuation without returning a row cannot advance
        // one either. Both are daemon faults, not empty results.
        if page_len == 0 || Some(&next_cursor) == cursor.as_ref() {
            anyhow::bail!(
                "daemon session page claimed a continuation without advancing its cursor"
            );
        }
        if sessions.len() >= MAX_LISTED_SESSIONS {
            return Ok(sessions);
        }
        cursor = Some(next_cursor);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EventPageControl {
    Continue,
    Cancel,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct EventPageStreamStats {
    pub(crate) completed: bool,
    pub(crate) pages: usize,
    pub(crate) events: usize,
    pub(crate) peak_buffered_events: usize,
}

pub(crate) fn consume_event_pages<F>(
    client: &mut dyn ChatClient,
    session_id: &str,
    mut consume: F,
) -> Result<EventPageStreamStats>
where
    F: FnMut(&[store::EventRecord]) -> Result<EventPageControl>,
{
    let mut after_seq = None;
    let mut stats = EventPageStreamStats::default();
    loop {
        let page = client.list_event_page(ChatEventQuery {
            session_id,
            after_seq,
            limit: Some(EVENT_PAGE_LIMIT),
        })?;
        let next_after_seq = validated_event_page_cursor(after_seq, &page)?;
        let has_more = page.has_more;
        stats.pages += 1;
        stats.events = stats.events.saturating_add(page.events.len());
        stats.peak_buffered_events = stats.peak_buffered_events.max(page.events.len());
        if consume(&page.events)? == EventPageControl::Cancel {
            return Ok(stats);
        }
        if !has_more {
            stats.completed = true;
            return Ok(stats);
        }
        after_seq = next_after_seq;
    }
}

pub(crate) struct DaemonChatClient {
    coven_home: PathBuf,
    api_checked: bool,
    daemon_client: Option<DaemonClient>,
    daemon_identity: Option<daemon::DaemonStatus>,
}

impl DaemonChatClient {
    /// Resolve a client from the process environment. Fails when no Coven
    /// home can be determined instead of guessing a cwd-relative `.coven`.
    pub(crate) fn detect() -> anyhow::Result<Self> {
        Ok(Self {
            coven_home: crate::paths::coven_home_dir()?,
            api_checked: false,
            daemon_client: None,
            daemon_identity: None,
        })
    }

    /// Construct a client pinned to a specific Coven home directory. Used by
    /// the Cast follower when it needs to spin up a second client on a
    /// background thread without re-detecting `$COVEN_HOME`.
    pub(crate) fn with_coven_home(coven_home: PathBuf) -> Self {
        Self {
            coven_home,
            api_checked: false,
            daemon_client: None,
            daemon_identity: None,
        }
    }
}

impl DaemonChatClient {
    fn store_path(&self) -> PathBuf {
        self.coven_home.join(STORE_FILE_NAME)
    }

    fn open_store(&self) -> Result<rusqlite::Connection> {
        store::open_store(&self.store_path())
    }

    fn daemon_client(&mut self) -> Result<&mut DaemonClient> {
        if self.daemon_client.is_none() {
            let endpoint = DaemonEndpoint::discover(&self.coven_home)
                .map_err(|error| cli_error(error, &self.coven_home))?;
            self.daemon_client = Some(DaemonClient::new(endpoint));
        }
        Ok(self
            .daemon_client
            .as_mut()
            .expect("daemon client initialized"))
    }

    fn get_json<T: serde::de::DeserializeOwned>(&mut self, endpoint: ReadEndpoint) -> Result<T> {
        self.ensure_api_contract()?;
        match self.daemon_client()?.get_json(endpoint.clone()) {
            Ok(value) => Ok(value),
            Err(error) if safe_request_may_retry(&error) => {
                self.invalidate_daemon_cache();
                self.ensure_api_contract()?;
                let result = self.daemon_client()?.get_json(endpoint);
                match result {
                    Ok(value) => Ok(value),
                    Err(error) => Err(self.read_error(error)),
                }
            }
            Err(error) => Err(self.read_error(error)),
        }
    }

    fn post_json<T: serde::de::DeserializeOwned>(
        &mut self,
        endpoint: WriteEndpoint,
        body: &Value,
    ) -> Result<T> {
        self.ensure_api_contract()?;
        match self.daemon_client()?.post_json(endpoint.clone(), body) {
            Ok(value) => Ok(value),
            Err(error) if mutation_was_definitely_not_sent(&error) => {
                self.invalidate_daemon_cache();
                self.ensure_api_contract()?;
                match self.daemon_client()?.post_json(endpoint, body) {
                    Ok(value) => Ok(value),
                    Err(error) => Err(self.mutation_error(error)),
                }
            }
            Err(error) => Err(self.mutation_error(error)),
        }
    }

    fn post_empty(&mut self, endpoint: WriteEndpoint, body: &Value) -> Result<()> {
        self.ensure_api_contract()?;
        match self.daemon_client()?.post_empty(endpoint.clone(), body) {
            Ok(()) => Ok(()),
            Err(error) if mutation_was_definitely_not_sent(&error) => {
                self.invalidate_daemon_cache();
                self.ensure_api_contract()?;
                match self.daemon_client()?.post_empty(endpoint, body) {
                    Ok(()) => Ok(()),
                    Err(error) => Err(self.mutation_error(error)),
                }
            }
            Err(error) => Err(self.mutation_error(error)),
        }
    }

    fn ensure_api_contract(&mut self) -> Result<()> {
        self.invalidate_if_daemon_identity_changed()?;
        if self.api_checked {
            return Ok(());
        }

        let mut retried_health = false;
        loop {
            let current_exe =
                std::env::current_exe().context("failed to resolve current executable")?;
            let status = daemon::ensure_background_server(
                &self.coven_home,
                &current_exe,
                current_timestamp(),
            )
            .context("failed to start Coven daemon")?;
            let endpoint = DaemonEndpoint::discover(&self.coven_home)
                .map_err(|error| cli_error(error, &self.coven_home))?;
            let mut client = DaemonClient::new(endpoint);
            match client.health() {
                Ok(_) => {
                    self.daemon_client = Some(client);
                    self.daemon_identity = Some(status);
                    self.api_checked = true;
                    return Ok(());
                }
                Err(error) if !retried_health && safe_request_may_retry(&error) => {
                    retried_health = true;
                    self.invalidate_daemon_cache();
                }
                Err(error) => {
                    self.invalidate_daemon_cache();
                    return Err(cli_error(error, &self.coven_home));
                }
            }
        }
    }

    fn invalidate_if_daemon_identity_changed(&mut self) -> Result<()> {
        if !self.api_checked {
            return Ok(());
        }
        let current = daemon::read_status_synchronized(&self.coven_home)?;
        if current.as_ref() != self.daemon_identity.as_ref() {
            self.invalidate_daemon_cache();
        }
        Ok(())
    }

    fn invalidate_daemon_cache(&mut self) {
        self.api_checked = false;
        self.daemon_client = None;
        self.daemon_identity = None;
    }

    fn read_error(&mut self, error: ClientError) -> anyhow::Error {
        if safe_request_may_retry(&error) {
            self.invalidate_daemon_cache();
        }
        cli_error(error, &self.coven_home)
    }

    fn mutation_error(&mut self, error: ClientError) -> anyhow::Error {
        if mutation_outcome_is_ambiguous(&error) {
            self.invalidate_daemon_cache();
            anyhow::Error::new(error).context(
                "Coven daemon mutation outcome is unknown; reconcile daemon session state before retrying",
            )
        } else {
            if mutation_was_definitely_not_sent(&error) {
                self.invalidate_daemon_cache();
            }
            cli_error(error, &self.coven_home)
        }
    }
}

fn safe_request_may_retry(error: &ClientError) -> bool {
    matches!(
        error,
        ClientError::Io { .. }
            | ClientError::InvalidHttpResponse(_)
            // Unix/Windows transports only raise `DaemonInstanceChanged` while
            // confirming peer identity during connect, strictly before any
            // request bytes are written, so it is always safe to rediscover
            // and retry once.
            | ClientError::DaemonInstanceChanged
    )
}

fn mutation_was_definitely_not_sent(error: &ClientError) -> bool {
    error.request_was_definitely_not_sent()
}

fn mutation_outcome_is_ambiguous(error: &ClientError) -> bool {
    matches!(
        error,
        ClientError::Io { .. }
            | ClientError::InvalidHttpResponse(_)
            | ClientError::ResponseTooLarge { .. }
            | ClientError::InvalidUtf8(_)
            | ClientError::InvalidJson(_)
    ) && !mutation_was_definitely_not_sent(error)
}

impl ChatClient for DaemonChatClient {
    fn daemon_status(&mut self) -> Result<ChatDaemonStatus> {
        match daemon::background_server_status(&self.coven_home)? {
            Some(daemon::DaemonStatusState::Running(status)) => {
                let endpoint = match DaemonEndpoint::discover(&self.coven_home) {
                    Ok(endpoint) => endpoint,
                    Err(error) => {
                        return Ok(ChatDaemonStatus::Unavailable {
                            message: cli_error(error, &self.coven_home).to_string(),
                        })
                    }
                };
                let mut client = DaemonClient::new(endpoint);
                match client.health() {
                    Ok(_) => {}
                    Err(ClientError::ProtocolVersion { actual, .. }) => {
                        return Ok(ChatDaemonStatus::ApiMismatch {
                            expected: PROTOCOL_VERSION.to_string(),
                            actual,
                        });
                    }
                    Err(error) => {
                        return Ok(ChatDaemonStatus::Unavailable {
                            message: cli_error(error, &self.coven_home).to_string(),
                        })
                    }
                }
                Ok(ChatDaemonStatus::Running { pid: status.pid })
            }
            Some(daemon::DaemonStatusState::Stale(status)) => {
                Ok(ChatDaemonStatus::Stale { pid: status.pid })
            }
            None => Ok(ChatDaemonStatus::Stopped),
        }
    }

    fn launch_session(&mut self, request: LaunchRequest) -> Result<store::SessionRecord> {
        let mut body = json!({
            "projectRoot": request.project_root,
            "cwd": request.cwd,
            "harness": request.harness,
            "launchMode": match request.launch_mode {
                harness::HarnessLaunchMode::Interactive => "interactive",
                harness::HarnessLaunchMode::NonInteractive => "nonInteractive",
                harness::HarnessLaunchMode::Stream => "stream",
            },
            "prompt": request.prompt,
            "title": request.title,
        });
        if let Some(hint) = request.conversation.as_ref() {
            let (mode, id) = match hint {
                harness::ConversationHint::Init { id } => ("init", id),
                harness::ConversationHint::Resume { id } => ("resume", id),
            };
            body.as_object_mut()
                .expect("json! literal is an object")
                .insert("conversation".to_string(), json!({"mode": mode, "id": id}));
        }
        if let Some(conversation_id) = request.conversation_id.as_ref() {
            body.as_object_mut()
                .expect("json! literal is an object")
                .insert("conversationId".to_string(), json!(conversation_id));
        }
        self.post_json(WriteEndpoint::Sessions, &body)
    }

    fn get_session(&mut self, session_id: &str) -> Result<store::SessionRecord> {
        self.get_json(ReadEndpoint::Session {
            session_id: session_id.to_string(),
        })
    }

    fn list_sessions(&mut self) -> Result<Vec<store::SessionRecord>> {
        collect_session_pages(|cursor| {
            self.get_json(ReadEndpoint::Sessions {
                limit: Some(SESSION_PAGE_LIMIT),
                cursor,
                include_archived: false,
            })
        })
    }

    fn list_events(&mut self, query: ChatEventQuery<'_>) -> Result<Vec<store::EventRecord>> {
        Ok(self.list_event_page(query)?.events)
    }

    fn list_event_page(&mut self, query: ChatEventQuery<'_>) -> Result<ChatEventPage> {
        let response: EventsResponse = self.get_json(ReadEndpoint::Events {
            session_id: query.session_id.to_string(),
            after_seq: query.after_seq,
            limit: query.limit,
        })?;
        Ok(ChatEventPage {
            events: response.events,
            next_after_seq: response.next_cursor.map(|cursor| cursor.after_seq),
            has_more: response.has_more,
        })
    }

    fn send_input(&mut self, session_id: &str, data: &str) -> Result<()> {
        self.post_empty(
            WriteEndpoint::SessionInput {
                session_id: session_id.to_string(),
            },
            &json!({ "data": data }),
        )
    }

    fn kill_session(&mut self, session_id: &str) -> Result<()> {
        self.post_empty(
            WriteEndpoint::SessionKill {
                session_id: session_id.to_string(),
            },
            &json!({}),
        )
    }

    fn archive_session(&mut self, session_id: &str) -> Result<()> {
        let conn = self.open_store()?;
        let Some(session) = store::get_session(&conn, session_id)? else {
            anyhow::bail!("session `{session_id}` not found");
        };
        if session.status == "running" {
            anyhow::bail!("session `{session_id}` is still running; stop it before archiving");
        }
        store::archive_session(&conn, session_id, &current_timestamp())
    }

    fn summon_session(&mut self, session_id: &str) -> Result<store::SessionRecord> {
        let conn = self.open_store()?;
        let Some(session) = store::get_session(&conn, session_id)? else {
            anyhow::bail!("session `{session_id}` not found");
        };
        if session.archived_at.is_some() {
            store::summon_session(&conn, session_id, &current_timestamp())?;
            let Some(session) = store::get_session(&conn, session_id)? else {
                anyhow::bail!("session `{session_id}` not found");
            };
            return Ok(session);
        }
        Ok(session)
    }

    fn sacrifice_session(&mut self, session_id: &str) -> Result<()> {
        let conn = self.open_store()?;
        store::sacrifice_session(&conn, session_id)
    }
}

fn cli_error(error: ClientError, coven_home: &std::path::Path) -> anyhow::Error {
    match error {
        ClientError::Daemon { status, error } => {
            anyhow::anyhow!(
                "Coven daemon rejected request with HTTP {status}: {}",
                error.message
            )
        }
        ClientError::Io {
            operation: "failed to connect to Coven daemon socket",
            source,
        } => {
            #[cfg(unix)]
            {
                let _ = source;
                anyhow::anyhow!(
                    "failed to connect to Coven daemon socket {}; run `coven daemon start` and retry",
                    daemon::daemon_socket_path(coven_home).display()
                )
            }
            #[cfg(not(unix))]
            {
                let _ = coven_home;
                anyhow::anyhow!("failed to connect to Coven daemon: {source}")
            }
        }
        ClientError::Io {
            operation: "failed to connect to Coven daemon pipe",
            source,
        } => {
            #[cfg(windows)]
            {
                let _ = source;
                match daemon::windows_pipe_name(coven_home) {
                    Ok(pipe_name) => anyhow::anyhow!(
                        "failed to connect to Coven daemon pipe {pipe_name}; run `coven daemon start` and retry"
                    ),
                    Err(error) => error.context(
                        "failed to derive the selected Coven profile's daemon pipe identity",
                    ),
                }
            }
            #[cfg(not(windows))]
            {
                let _ = coven_home;
                anyhow::anyhow!("failed to connect to Coven daemon pipe: {source}")
            }
        }
        error => anyhow::Error::new(error),
    }
}

fn session_title(prompt: &str) -> String {
    let trimmed = prompt.trim();
    let mut title = String::new();
    for ch in trimmed.chars().take(48) {
        title.push(ch);
    }
    if title.is_empty() {
        "Coven chat".to_string()
    } else {
        title
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The plain unpaginated listing, used by transport-level tests that care
    /// about connection reuse rather than about the session page shape.
    #[cfg(unix)]
    fn unpaginated_sessions_read() -> ReadEndpoint {
        ReadEndpoint::Sessions {
            limit: None,
            cursor: None,
            include_archived: false,
        }
    }

    fn test_session(id: &str) -> store::SessionRecord {
        store::SessionRecord {
            id: id.to_owned(),
            project_root: "/repo".to_owned(),
            harness: "codex".to_owned(),
            title: "Listed".to_owned(),
            status: "completed".to_owned(),
            exit_code: Some(0),
            archived_at: None,
            created_at: "2026-08-16T00:00:00Z".to_owned(),
            updated_at: "2026-08-16T00:00:00Z".to_owned(),
            conversation_id: None,
            familiar_id: None,
            execution_binding: None,
            labels: Vec::new(),
            visibility: "private".to_owned(),
            external: false,
            transcript_path: None,
        }
    }

    #[test]
    fn session_listing_follows_every_daemon_page_cursor() {
        let mut requested = Vec::new();
        let sessions = collect_session_pages(|cursor| {
            requested.push(cursor.clone());
            Ok(match cursor.as_deref() {
                None => SessionPageResponse {
                    sessions: vec![test_session("a"), test_session("b")],
                    next_cursor: Some("page-2".to_owned()),
                },
                Some("page-2") => SessionPageResponse {
                    sessions: vec![test_session("c")],
                    next_cursor: None,
                },
                other => panic!("unexpected cursor {other:?}"),
            })
        })
        .expect("continuation is followed to exhaustion");

        assert_eq!(requested, vec![None, Some("page-2".to_owned())]);
        assert_eq!(
            sessions.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
    }

    #[test]
    fn session_listing_stops_at_the_in_memory_ceiling() {
        let mut pages = 0_usize;
        let sessions = collect_session_pages(|_| {
            pages += 1;
            Ok(SessionPageResponse {
                sessions: (0..usize::from(SESSION_PAGE_LIMIT))
                    .map(|index| test_session(&format!("session-{pages}-{index}")))
                    .collect(),
                next_cursor: Some(format!("page-{pages}")),
            })
        })
        .expect("an endless ledger is truncated rather than followed forever");

        assert_eq!(sessions.len(), MAX_LISTED_SESSIONS);
        assert_eq!(pages, MAX_LISTED_SESSIONS / usize::from(SESSION_PAGE_LIMIT));
    }

    #[test]
    fn session_listing_rejects_a_continuation_that_cannot_advance() {
        let repeated = collect_session_pages(|_| {
            Ok(SessionPageResponse {
                sessions: vec![test_session("a")],
                next_cursor: Some("stuck".to_owned()),
            })
        });
        let empty = collect_session_pages(|_| {
            Ok(SessionPageResponse {
                sessions: Vec::new(),
                next_cursor: Some("page-2".to_owned()),
            })
        });

        for error in [
            repeated.expect_err("a repeated cursor must not replay forever"),
            empty.expect_err("an empty page cannot carry a continuation"),
        ] {
            assert!(
                error
                    .to_string()
                    .contains("claimed a continuation without advancing"),
                "unexpected error: {error}"
            );
        }
    }

    #[test]
    fn event_page_continuation_must_advance_before_another_request() {
        let page = ChatEventPage {
            events: Vec::new(),
            next_after_seq: None,
            has_more: true,
        };

        let error = validated_event_page_cursor(Some(513), &page).unwrap_err();

        assert!(error
            .to_string()
            .contains("claimed a continuation without advancing"));
    }

    #[cfg(unix)]
    fn test_health(status: &daemon::DaemonStatus) -> String {
        serde_json::json!({
            "ok": true,
            "apiVersion": PROTOCOL_VERSION,
            "covenVersion": "test",
            "capabilities": {
                "sessions": true,
                "events": true,
                "eventCursor": "sequence",
                "structuredErrors": true
            },
            "daemon": status
        })
        .to_string()
    }

    #[cfg(unix)]
    fn read_request_path(stream: &mut std::os::unix::net::UnixStream) -> String {
        use std::io::Read;

        // BSD `accept(2)` inherits O_NONBLOCK from the listener; Linux does
        // not. Tests that poll a non-blocking listener would otherwise get
        // EWOULDBLOCK here on macOS instead of the request.
        stream
            .set_nonblocking(false)
            .expect("restore blocking mode on accepted stream");
        let mut request = String::new();
        stream
            .read_to_string(&mut request)
            .expect("read test daemon request");
        request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .expect("request path")
            .to_owned()
    }

    #[cfg(unix)]
    fn write_response(stream: &mut std::os::unix::net::UnixStream, status: u16, body: &str) {
        use std::io::Write;

        // See `read_request_path`: an accepted stream inherits the listener's
        // non-blocking mode on BSD/macOS, which would short-write the response.
        stream
            .set_nonblocking(false)
            .expect("restore blocking mode on accepted stream");
        write!(
            stream,
            "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .expect("write test daemon response");
    }

    #[cfg(unix)]
    fn bind_test_daemon(home: &std::path::Path) -> std::os::unix::net::UnixListener {
        use std::os::unix::fs::PermissionsExt;

        std::fs::set_permissions(home, std::fs::Permissions::from_mode(0o700))
            .expect("make test Coven home private");
        let socket = daemon::daemon_socket_path(home);
        let listener =
            std::os::unix::net::UnixListener::bind(&socket).expect("bind test daemon socket");
        std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))
            .expect("make test daemon socket private");
        listener
    }

    #[cfg(unix)]
    fn retire_test_daemon_socket(home: &std::path::Path, label: &str) -> std::io::Result<()> {
        std::fs::rename(
            daemon::daemon_socket_path(home),
            home.join(format!(".retired-coven-socket-{label}")),
        )
    }

    /// Reads a request from a connection that may be a pre-write
    /// `DaemonInstanceChanged` probe: the transport connects, confirms the
    /// peer/socket identity no longer matches what was negotiated, and
    /// closes without ever writing request bytes. Such a connection reads as
    /// empty; a real request always carries a request line.
    #[cfg(unix)]
    fn read_request_path_if_present(stream: &mut std::os::unix::net::UnixStream) -> Option<String> {
        use std::io::Read;

        // See `read_request_path`: an accepted stream inherits the listener's
        // non-blocking mode on BSD/macOS.
        stream
            .set_nonblocking(false)
            .expect("restore blocking mode on accepted stream");
        let mut request = String::new();
        stream
            .read_to_string(&mut request)
            .expect("read test daemon connection");
        if request.is_empty() {
            return None;
        }
        Some(
            request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .expect("request path")
                .to_owned(),
        )
    }

    #[cfg(unix)]
    #[test]
    fn cached_legacy_endpoint_is_rediscovered_and_renegotiated_after_stable_restart() -> Result<()>
    {
        use std::time::{Duration, Instant};

        let temp = tempfile::tempdir()?;
        let home = temp.path().to_path_buf();
        let old_listener = bind_test_daemon(&home);
        let old_status = daemon::DaemonStatus {
            pid: 41,
            started_at: "legacy".to_owned(),
            socket: "coven-daemon-0123456789abcdef.sock".to_owned(),
            process_creation_time: None,
        };
        let old_health = test_health(&old_status);
        let old_server = std::thread::spawn(move || {
            let (mut stream, _) = old_listener.accept().expect("accept old health");
            assert_eq!(read_request_path(&mut stream), "/api/v1/health");
            write_response(&mut stream, 200, &old_health);
        });
        let endpoint = DaemonEndpoint::discover(&home)?;
        let mut cached = DaemonClient::new(endpoint);
        cached.health()?;
        old_server.join().unwrap();
        retire_test_daemon_socket(&home, "legacy")?;

        let listener = bind_test_daemon(&home);
        listener.set_nonblocking(true)?;
        let stable_status = daemon::DaemonStatus {
            pid: std::process::id(),
            started_at: "stable".to_owned(),
            socket: daemon::daemon_socket_path(&std::fs::canonicalize(&home)?)
                .to_string_lossy()
                .into_owned(),
            process_creation_time: None,
        };
        daemon::write_status(&home, &stable_status)?;
        let health = test_health(&stable_status);
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut paths = Vec::new();
            while paths.len() < 3 && Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let path = read_request_path(&mut stream);
                        let body = if path == "/api/v1/sessions" {
                            r#"{"source":"stable"}"#
                        } else {
                            &health
                        };
                        write_response(&mut stream, 200, body);
                        paths.push(path);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("accept stable daemon request: {error}"),
                }
            }
            paths
        });

        let mut client = DaemonChatClient {
            coven_home: home,
            api_checked: true,
            daemon_client: Some(cached),
            daemon_identity: Some(old_status),
        };
        let response: Value = client.get_json(unpaginated_sessions_read())?;

        assert_eq!(response["source"], "stable");
        assert_eq!(
            server.join().unwrap(),
            vec!["/health", "/api/v1/health", "/api/v1/sessions"]
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn ambiguous_mutation_failure_is_not_replayed() -> Result<()> {
        use std::time::{Duration, Instant};

        let temp = tempfile::tempdir()?;
        let home = temp.path().to_path_buf();
        let listener = bind_test_daemon(&home);
        let status = daemon::DaemonStatus {
            pid: std::process::id(),
            started_at: "stable".to_owned(),
            socket: daemon::daemon_socket_path(&home)
                .to_string_lossy()
                .into_owned(),
            process_creation_time: None,
        };
        daemon::write_status(&home, &status)?;
        let health = test_health(&status);
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept initial health");
            assert_eq!(read_request_path(&mut stream), "/api/v1/health");
            write_response(&mut stream, 200, &health);

            let (mut stream, _) = listener.accept().expect("accept mutation");
            assert!(read_request_path(&mut stream).ends_with("/kill"));
            drop(stream);

            listener
                .set_nonblocking(true)
                .expect("make listener nonblocking");
            let deadline = Instant::now() + Duration::from_millis(400);
            let mut duplicate = false;
            while Instant::now() < deadline {
                match listener.accept() {
                    Ok((_stream, _)) => {
                        duplicate = true;
                        break;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("check duplicate mutation: {error}"),
                }
            }
            duplicate
        });

        let endpoint = DaemonEndpoint::discover(&home)?;
        let mut cached = DaemonClient::new(endpoint);
        cached.health()?;
        let mut client = DaemonChatClient {
            coven_home: home,
            api_checked: true,
            daemon_client: Some(cached),
            daemon_identity: Some(status),
        };

        let error = client
            .post_empty(
                WriteEndpoint::SessionKill {
                    session_id: "session-1".to_owned(),
                },
                &json!({}),
            )
            .expect_err("closed response leaves mutation outcome ambiguous");

        assert!(
            error.to_string().contains("reconcile"),
            "mutation error must require reconciliation: {error:#}"
        );
        assert!(!server.join().unwrap(), "mutation was replayed");
        Ok(())
    }

    #[test]
    fn mutations_retry_only_when_transport_proves_no_request_bytes_were_sent() {
        let io_error = |operation| ClientError::Io {
            operation,
            source: std::io::Error::from(std::io::ErrorKind::ConnectionReset),
        };

        assert!(mutation_was_definitely_not_sent(&io_error(
            "failed to connect to Coven daemon socket"
        )));
        assert!(!mutation_was_definitely_not_sent(&io_error(
            "failed to connect to Coven daemon socket later"
        )));
        assert!(!mutation_was_definitely_not_sent(&io_error(
            "failed to write Coven daemon request"
        )));
        assert!(mutation_outcome_is_ambiguous(&io_error(
            "failed to read Coven daemon response"
        )));
    }

    #[test]
    fn daemon_instance_changed_is_safe_to_retry_and_never_ambiguous() {
        // Both Unix and Windows transports only ever raise this error while
        // confirming peer identity during connect, strictly before any
        // request bytes are written.
        assert!(safe_request_may_retry(&ClientError::DaemonInstanceChanged));
        assert!(mutation_was_definitely_not_sent(
            &ClientError::DaemonInstanceChanged
        ));
        assert!(!mutation_outcome_is_ambiguous(
            &ClientError::DaemonInstanceChanged
        ));
    }

    /// Sets up an original daemon health negotiation, then swaps the socket
    /// for a fresh listener bound at the same path without touching the
    /// on-disk `DaemonStatus` file. This mirrors the narrow window in which
    /// a real daemon replacement outruns the identity file: the cached
    /// client's negotiated peer identity (socket device/inode) no longer
    /// matches what a live connect confirms, so the transport raises
    /// `DaemonInstanceChanged` before writing any request bytes.
    #[cfg(unix)]
    fn setup_swapped_daemon_instance(
        home: &std::path::Path,
    ) -> Result<(DaemonClient, daemon::DaemonStatus, String)> {
        let original_listener = bind_test_daemon(home);
        let status = daemon::DaemonStatus {
            pid: std::process::id(),
            started_at: "stable".to_owned(),
            socket: daemon::daemon_socket_path(&std::fs::canonicalize(home)?)
                .to_string_lossy()
                .into_owned(),
            process_creation_time: None,
        };
        daemon::write_status(home, &status)?;
        let health = test_health(&status);
        let original_health = health.clone();
        let original_server = std::thread::spawn(move || {
            let (mut stream, _) = original_listener.accept().expect("accept original health");
            assert_eq!(read_request_path(&mut stream), "/api/v1/health");
            write_response(&mut stream, 200, &original_health);
        });

        let endpoint = DaemonEndpoint::discover(home)?;
        let mut cached = DaemonClient::new(endpoint);
        cached.health()?;
        original_server.join().unwrap();

        retire_test_daemon_socket(home, "original")?;
        Ok((cached, status, health))
    }

    #[cfg(unix)]
    #[test]
    fn daemon_instance_change_retries_once_for_reads() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().to_path_buf();
        let (cached, status, health) = setup_swapped_daemon_instance(&home)?;

        let replacement_listener = bind_test_daemon(&home);
        let sessions_body = r#"{"source":"replacement"}"#;
        let replacement_server = std::thread::spawn(move || {
            let mut real_paths = Vec::new();
            while real_paths.len() < 3 {
                let (mut stream, _) = replacement_listener
                    .accept()
                    .expect("accept replacement connection");
                match read_request_path_if_present(&mut stream) {
                    None => continue, // pre-write DaemonInstanceChanged probe
                    Some(path) => {
                        let body = if path == "/health" || path == "/api/v1/health" {
                            &health
                        } else {
                            sessions_body
                        };
                        write_response(&mut stream, 200, body);
                        real_paths.push(path);
                    }
                }
            }
            real_paths
        });

        let mut client = DaemonChatClient {
            coven_home: home,
            api_checked: true,
            daemon_client: Some(cached),
            daemon_identity: Some(status),
        };
        let response: Value = client.get_json(unpaginated_sessions_read())?;

        assert_eq!(response["source"], "replacement");
        assert_eq!(
            replacement_server.join().unwrap(),
            vec!["/health", "/api/v1/health", "/api/v1/sessions"]
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn cached_negative_capability_renegotiates_once_after_daemon_replacement() -> Result<()> {
        use std::time::{Duration, Instant};

        let temp = tempfile::tempdir()?;
        let home = temp.path().to_path_buf();
        let original_listener = bind_test_daemon(&home);
        let status = daemon::DaemonStatus {
            pid: std::process::id(),
            started_at: "stable".to_owned(),
            socket: daemon::daemon_socket_path(&std::fs::canonicalize(&home)?)
                .to_string_lossy()
                .into_owned(),
            process_creation_time: None,
        };
        daemon::write_status(&home, &status)?;
        let mut incapable_health: Value = serde_json::from_str(&test_health(&status))?;
        incapable_health["capabilities"]["sessions"] = Value::Bool(false);
        let incapable_health = incapable_health.to_string();
        let original_server = std::thread::spawn(move || {
            let (mut stream, _) = original_listener.accept().expect("accept original health");
            assert_eq!(read_request_path(&mut stream), "/api/v1/health");
            write_response(&mut stream, 200, &incapable_health);
        });
        let endpoint = DaemonEndpoint::discover(&home)?;
        let mut cached = DaemonClient::new(endpoint);
        cached.health()?;
        original_server.join().unwrap();
        retire_test_daemon_socket(&home, "incapable")?;

        let replacement = bind_test_daemon(&home);
        replacement.set_nonblocking(true)?;
        let replacement_health = test_health(&status);
        let replacement_server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut empty_checks = 0;
            let mut paths = Vec::new();
            while Instant::now() < deadline && paths.len() < 3 {
                match replacement.accept() {
                    Ok((mut stream, _)) => match read_request_path_if_present(&mut stream) {
                        None => empty_checks += 1,
                        Some(path) => {
                            let body = if path == "/api/v1/sessions" {
                                r#"{"source":"replacement"}"#
                            } else {
                                &replacement_health
                            };
                            write_response(&mut stream, 200, body);
                            paths.push(path);
                        }
                    },
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("accept replacement connection: {error}"),
                }
            }
            (empty_checks, paths)
        });

        let mut client = DaemonChatClient {
            coven_home: home,
            api_checked: true,
            daemon_client: Some(cached),
            daemon_identity: Some(status),
        };
        let response = client.get_json::<Value>(unpaginated_sessions_read());
        let (empty_checks, paths) = replacement_server.join().unwrap();

        assert_eq!(response?["source"], "replacement");
        assert_eq!(empty_checks, 1);
        assert_eq!(paths, vec!["/health", "/api/v1/health", "/api/v1/sessions"]);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn cached_negative_capability_on_unchanged_daemon_does_not_retry_or_send_request() -> Result<()>
    {
        use std::time::{Duration, Instant};

        let temp = tempfile::tempdir()?;
        let home = temp.path().to_path_buf();
        let listener = bind_test_daemon(&home);
        let status = daemon::DaemonStatus {
            pid: std::process::id(),
            started_at: "stable".to_owned(),
            socket: daemon::daemon_socket_path(&std::fs::canonicalize(&home)?)
                .to_string_lossy()
                .into_owned(),
            process_creation_time: None,
        };
        daemon::write_status(&home, &status)?;
        let mut incapable_health: Value = serde_json::from_str(&test_health(&status))?;
        incapable_health["capabilities"]["sessions"] = Value::Bool(false);
        let incapable_health = incapable_health.to_string();
        let server = std::thread::spawn(move || {
            let (mut health, _) = listener.accept().expect("accept original health");
            assert_eq!(read_request_path(&mut health), "/api/v1/health");
            write_response(&mut health, 200, &incapable_health);

            listener
                .set_nonblocking(true)
                .expect("make test listener nonblocking");
            let deadline = Instant::now() + Duration::from_millis(500);
            let mut requests = Vec::new();
            while Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        requests.push(read_request_path_if_present(&mut stream));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("check unchanged daemon requests: {error}"),
                }
            }
            requests
        });
        let endpoint = DaemonEndpoint::discover(&home)?;
        let mut cached = DaemonClient::new(endpoint);
        cached.health()?;
        let mut client = DaemonChatClient {
            coven_home: home,
            api_checked: true,
            daemon_client: Some(cached),
            daemon_identity: Some(status),
        };

        let error = client
            .get_json::<Value>(unpaginated_sessions_read())
            .expect_err("unchanged incapable daemon must remain unavailable");
        let requests = server.join().unwrap();

        assert!(error.to_string().contains("capabilities.sessions"));
        assert_eq!(requests, vec![None]);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn daemon_instance_change_retries_once_for_mutations_without_duplicate_send() -> Result<()> {
        use std::time::{Duration, Instant};

        let temp = tempfile::tempdir()?;
        let home = temp.path().to_path_buf();
        let (cached, status, health) = setup_swapped_daemon_instance(&home)?;

        let replacement_listener = bind_test_daemon(&home);
        let replacement_server = std::thread::spawn(move || {
            let mut real_paths = Vec::new();
            while real_paths.len() < 3 {
                let (mut stream, _) = replacement_listener
                    .accept()
                    .expect("accept replacement connection");
                match read_request_path_if_present(&mut stream) {
                    None => continue, // pre-write DaemonInstanceChanged probe
                    Some(path) => {
                        let body = if path == "/health" || path == "/api/v1/health" {
                            &health
                        } else {
                            "{}"
                        };
                        write_response(&mut stream, 200, body);
                        real_paths.push(path);
                    }
                }
            }

            // Prove there is no duplicate send: no further connection
            // (i.e. a second kill request) ever arrives after the retry.
            replacement_listener
                .set_nonblocking(true)
                .expect("make listener nonblocking");
            let deadline = Instant::now() + Duration::from_millis(300);
            let mut duplicate = false;
            while Instant::now() < deadline {
                match replacement_listener.accept() {
                    Ok((_stream, _)) => {
                        duplicate = true;
                        break;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("check duplicate mutation: {error}"),
                }
            }
            (real_paths, duplicate)
        });

        let mut client = DaemonChatClient {
            coven_home: home,
            api_checked: true,
            daemon_client: Some(cached),
            daemon_identity: Some(status),
        };
        client.post_empty(
            WriteEndpoint::SessionKill {
                session_id: "session-1".to_owned(),
            },
            &json!({}),
        )?;

        let (real_paths, duplicate) = replacement_server.join().unwrap();
        assert_eq!(
            real_paths,
            vec![
                "/health",
                "/api/v1/health",
                "/api/v1/sessions/session-1/kill"
            ]
        );
        assert!(!duplicate, "mutation was sent more than once");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn daemon_instance_change_retry_is_bounded_when_replacement_changes_again() -> Result<()> {
        use std::time::{Duration, Instant};

        let temp = tempfile::tempdir()?;
        let home = temp.path().to_path_buf();
        let (cached, status, health) = setup_swapped_daemon_instance(&home)?;

        let home_for_thread = home.clone();
        let first_replacement = bind_test_daemon(&home);
        let replacement_server = std::thread::spawn(move || {
            // First connection against the first replacement: a pre-write
            // probe against the still-cached (original) identity.
            let (mut probe, _) = first_replacement
                .accept()
                .expect("accept first replacement probe");
            assert!(
                read_request_path_if_present(&mut probe).is_none(),
                "expected a pre-write DaemonInstanceChanged probe with no request bytes"
            );

            // Second connection: the legacy liveness probe `ensure_background_server`
            // issues while confirming the recorded daemon is still alive before
            // renegotiating.
            let (mut legacy_health, _) = first_replacement
                .accept()
                .expect("accept legacy health probe");
            assert_eq!(read_request_path(&mut legacy_health), "/health");
            write_response(&mut legacy_health, 200, &health);

            // Third connection: the real health renegotiation against the
            // first replacement. The response is deferred until after the
            // daemon is swapped again below, guaranteeing (by program
            // order, not timing) that the retried mutation attempt below
            // observes the *second* replacement rather than racing it.
            let (mut health_stream, _) = first_replacement
                .accept()
                .expect("accept replacement health");
            assert_eq!(read_request_path(&mut health_stream), "/api/v1/health");

            drop(first_replacement);
            retire_test_daemon_socket(&home_for_thread, "first-replacement")
                .expect("retire first replacement socket");
            let second_replacement = bind_test_daemon(&home_for_thread);

            write_response(&mut health_stream, 200, &health);
            drop(health_stream);

            // Fourth connection, against the second replacement: another
            // pre-write probe, since the identity just negotiated against
            // the first replacement no longer matches.
            let (mut probe, _) = second_replacement
                .accept()
                .expect("accept second replacement probe");
            assert!(
                read_request_path_if_present(&mut probe).is_none(),
                "expected a second pre-write DaemonInstanceChanged probe with no request bytes"
            );

            // The retry is bounded to one attempt: no fifth connection
            // (i.e. another retry) should ever arrive.
            second_replacement
                .set_nonblocking(true)
                .expect("make listener nonblocking");
            let deadline = Instant::now() + Duration::from_millis(300);
            let mut unbounded_retry = false;
            while Instant::now() < deadline {
                match second_replacement.accept() {
                    Ok((_stream, _)) => {
                        unbounded_retry = true;
                        break;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("check bounded retry: {error}"),
                }
            }
            unbounded_retry
        });

        let mut client = DaemonChatClient {
            coven_home: home,
            api_checked: true,
            daemon_client: Some(cached),
            daemon_identity: Some(status),
        };
        let error = client
            .post_empty(
                WriteEndpoint::SessionKill {
                    session_id: "session-1".to_owned(),
                },
                &json!({}),
            )
            .expect_err("a second instance change must fail rather than retry again");

        assert!(
            !error.to_string().contains("reconcile"),
            "a pre-write instance change must not be treated as an ambiguous mutation outcome: {error:#}"
        );
        assert!(
            !replacement_server.join().unwrap(),
            "retry must be bounded to a single attempt"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn unix_connect_error_keeps_the_pre_extraction_output() {
        let home = PathBuf::from("/private/coven-test");
        let error = cli_error(
            ClientError::Io {
                operation: "failed to connect to Coven daemon socket",
                source: std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "ignored"),
            },
            &home,
        );

        assert_eq!(
            error.to_string(),
            "failed to connect to Coven daemon socket /private/coven-test/coven.sock; run `coven daemon start` and retry"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_connect_error_keeps_the_pre_extraction_output() -> Result<()> {
        let home = tempfile::tempdir()?;
        let error = cli_error(
            ClientError::Io {
                operation: "failed to connect to Coven daemon pipe",
                source: std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "ignored"),
            },
            home.path(),
        );
        let pipe_name = daemon::windows_pipe_name(home.path())?;

        assert_eq!(
            error.to_string(),
            format!(
                "failed to connect to Coven daemon pipe {}; run `coven daemon start` and retry",
                pipe_name
            )
        );
        Ok(())
    }

    #[test]
    fn launch_request_uses_current_dir_and_non_interactive_mode_for_chat() -> Result<()> {
        let request = LaunchRequest::for_current_dir("codex", "summarize")?;

        assert_eq!(request.harness, "codex");
        assert_eq!(request.prompt, "summarize");
        assert_eq!(
            request.launch_mode,
            crate::harness::HarnessLaunchMode::NonInteractive
        );
        assert!(request.conversation.is_none());
        assert!(!request.project_root.is_empty());
        assert_eq!(request.project_root, request.cwd);
        Ok(())
    }

    #[test]
    fn with_conversation_attaches_resume_hint() -> Result<()> {
        let request = LaunchRequest::for_current_dir("claude", "next turn")?.with_conversation(
            crate::harness::ConversationHint::Resume {
                id: "abc-123".to_string(),
            },
        );

        assert_eq!(
            request.conversation,
            Some(crate::harness::ConversationHint::Resume {
                id: "abc-123".to_string()
            })
        );
        Ok(())
    }

    #[test]
    fn daemon_chat_client_preserves_adoption_retention_error() -> Result<()> {
        const DENIAL: &str = "session adoption evidence is retained; sacrifice is unavailable until an approved retention/fence contract resolves it";
        let home = tempfile::tempdir()?;
        let conn = store::open_store(&home.path().join(STORE_FILE_NAME))?;
        store::insert_session(
            &conn,
            &store::SessionRecord {
                id: "session-1".to_string(),
                project_root: "/repo".to_string(),
                harness: "codex".to_string(),
                title: "Retained".to_string(),
                status: "completed".to_string(),
                exit_code: Some(0),
                archived_at: None,
                created_at: "2026-08-16T00:00:00Z".to_string(),
                updated_at: "2026-08-16T00:00:00Z".to_string(),
                conversation_id: None,
                familiar_id: None,
                execution_binding: None,
                labels: Vec::new(),
                visibility: "private".to_string(),
                external: false,
                transcript_path: None,
            },
        )?;
        conn.execute(
            "INSERT INTO request_adoptions (
                id, adoption_key, contract, operation, request_digest, session_id,
                execution_binding_json, adopted_at
             ) VALUES (
                'retained-input', 'input-key', 'psyche.request_adoption.v1', 'input',
                'sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd',
                'session-1', '{}', '2026-08-16T00:00:00Z'
             )",
            [],
        )?;
        drop(conn);
        let mut client = DaemonChatClient::with_coven_home(home.path().to_path_buf());

        let error = client
            .sacrifice_session("session-1")
            .expect_err("retained evidence must deny sacrifice");

        assert!(error.is::<store::AdoptionRetentionError>());
        assert_eq!(error.to_string(), DENIAL);
        assert_eq!(
            format!("{:?}", error.root_cause()),
            "AdoptionRetentionError"
        );
        Ok(())
    }
}
