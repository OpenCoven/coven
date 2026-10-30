//! The scoped read vectors (`vectors.json`), and the budget, limit and mount
//! cases a vector file cannot express.

use serde_json::{json, Value};

use super::scope::{Coverage, Grants, RelativePath, Request};
use super::{Refusal, R0_READ_POLICY};

const VECTORS: &str = include_str!("vectors.json");

fn vectors() -> Value {
    serde_json::from_str(VECTORS).expect("the scoped read vectors are JSON")
}

fn cases<'a>(vectors: &'a Value, section: &str) -> &'a [Value] {
    vectors[section]
        .as_array()
        .unwrap_or_else(|| panic!("the vectors have a {section} section"))
}

fn outcome<T>(result: Result<T, Refusal>, success: &'static str) -> &'static str {
    result.map_or_else(Refusal::code, |_| success)
}

fn text<'a>(case: &'a Value, field: &str) -> &'a str {
    case[field]
        .as_str()
        .unwrap_or_else(|| panic!("vector {} has a string {field}", case["id"]))
}

/// `read:notes/today.md`, with an optional `workspace-` then `recursive-`
/// prefix, as a profile filesystem scope.
fn scope(shorthand: &str) -> Value {
    let (form, path) = shorthand
        .split_once(':')
        .unwrap_or_else(|| panic!("scope shorthand {shorthand}"));
    let (root, form) = form
        .strip_prefix("workspace-")
        .map_or(("project", form), |rest| ("workspace", rest));
    let (recursive, access) = form
        .strip_prefix("recursive-")
        .map_or((false, form), |access| (true, access));
    json!({"kind": "filesystem", "root": root, "path": path, "access": access, "recursive": recursive})
}

fn scopes(case: &Value) -> Vec<Value> {
    case["scopes"]
        .as_array()
        .expect("a vector lists its scopes")
        .iter()
        .map(|shorthand| scope(shorthand.as_str().expect("a scope shorthand")))
        .collect()
}

#[test]
fn path_vectors() {
    let vectors = vectors();
    for case in cases(&vectors, "paths") {
        let got = outcome(
            RelativePath::parse(text(case, "path"), &R0_READ_POLICY),
            "ok",
        );
        assert_eq!(got, text(case, "expect"), "path vector {}", case["id"]);
    }
}

#[test]
fn grant_vectors() {
    let vectors = vectors();
    for case in cases(&vectors, "grants") {
        let scopes = case["scopes"]
            .as_array()
            .expect("a vector lists its scopes");
        let got = outcome(Grants::from_scopes(scopes, &R0_READ_POLICY), "ok");
        assert_eq!(got, text(case, "expect"), "grant vector {}", case["id"]);
    }
}

#[test]
fn request_vectors() {
    let vectors = vectors();
    for case in cases(&vectors, "requests") {
        let grants = Grants::from_scopes(&scopes(case), &R0_READ_POLICY)
            .unwrap_or_else(|refusal| panic!("vector {} grant: {}", case["id"], refusal.code()));
        let got = match Request::parse(&case["request"], &R0_READ_POLICY)
            .and_then(|request| grants.authorize(&request))
        {
            Ok(Coverage::Exact) => "exact",
            Ok(Coverage::Subtree) => "subtree",
            Err(refusal) => refusal.code(),
        };
        assert_eq!(got, text(case, "expect"), "request vector {}", case["id"]);
    }
}

#[test]
fn path_limits_count_components_and_bytes() {
    let deepest = vec!["a"; R0_READ_POLICY.depth].join("/");
    assert!(RelativePath::parse(&deepest, &R0_READ_POLICY).is_ok());
    assert_eq!(
        RelativePath::parse(&format!("{deepest}/a"), &R0_READ_POLICY),
        Err(Refusal::PathLimit)
    );
    let longest = "a".repeat(R0_READ_POLICY.path_bytes);
    assert!(RelativePath::parse(&longest, &R0_READ_POLICY).is_ok());
    assert_eq!(
        RelativePath::parse(&format!("{longest}a"), &R0_READ_POLICY),
        Err(Refusal::PathLimit)
    );
}

#[test]
fn implicit_exclusion_aliases_do_not_expand_exact_grants() {
    let grants =
        Grants::from_scopes(&[scope("read:work/repo/.e\u{200d}nv")], &R0_READ_POLICY).unwrap();
    for (path, expected) in [
        ("work/repo/.e\u{200d}nv", Ok(Coverage::Exact)),
        ("work/repo/.env", Err(Refusal::ScopeMissing)),
        ("work/repo/.E\u{200d}NV", Err(Refusal::ScopeMissing)),
    ] {
        let request = Request::parse(
            &json!({"operation": "read_file", "root": "project", "path": path}),
            &R0_READ_POLICY,
        )
        .unwrap();
        assert_eq!(grants.authorize(&request), expected, "{path:?}");
    }
}

#[test]
fn the_r0_policy_is_the_designs() {
    assert_eq!(R0_READ_POLICY.file_bytes, 1 << 20);
    assert_eq!(R0_READ_POLICY.attempt_bytes, 8 << 20);
    assert_eq!(R0_READ_POLICY.calls, 1024);
    assert_eq!(R0_READ_POLICY.depth, 32);
    assert_eq!(R0_READ_POLICY.scan_bytes, 64 << 20);
    assert_eq!(R0_READ_POLICY.page_entries, 256);
    assert_eq!(R0_READ_POLICY.page_matches, 256);
}

#[test]
fn a_pattern_is_at_most_its_bytes() {
    let grants =
        Grants::from_scopes(&[scope("recursive-read:docs/guides")], &R0_READ_POLICY).unwrap();
    let search = |pattern: String| {
        Request::parse(
            &json!({"operation": "search", "root": "project", "path": "docs/guides", "pattern": pattern}),
            &R0_READ_POLICY,
        )
        .and_then(|request| grants.authorize(&request))
    };
    let longest = "a".repeat(R0_READ_POLICY.pattern_bytes);
    assert_eq!(search(longest.clone()), Ok(Coverage::Subtree));
    assert_eq!(search(format!("{longest}a")), Err(Refusal::RequestInvalid));
}

#[cfg(not(unix))]
#[test]
fn every_read_is_refused_without_a_reviewed_open() {
    use super::open::ScopedReader;
    use super::scope::RootKind;

    let scratch = tempfile::tempdir().expect("scratch directory");
    let refused = ScopedReader::new(
        &[scope("read:notes/today.md")],
        &[(RootKind::Project, scratch.path())],
        &[],
        R0_READ_POLICY,
    );
    assert_eq!(refused.err(), Some(Refusal::PlatformUnsupported));
}

#[cfg(unix)]
mod filesystem {
    use std::ffi::CString;
    use std::fs;
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    use serde_json::{json, Value};

    use super::super::open::{Body, Response, ScopedReader, Step};
    use super::super::scope::RootKind;
    use super::super::{ReadPolicy, Refusal, R0_READ_POLICY};
    use super::{cases, scope, scopes, text, vectors};
    use crate::automations::contract::canonical_json::{canonicalize, sha256_hex};

    /// A short scratch base: a socket path has to fit in about 100 bytes.
    fn scratch() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("csr")
            .tempdir_in("/tmp")
            .expect("scratch directory")
    }

    fn parent(path: &Path) {
        fs::create_dir_all(path.parent().expect("a tree entry has a parent")).expect("tree parent");
    }

    fn mkfifo(path: &Path) {
        let name = CString::new(path.as_os_str().as_bytes()).expect("a FIFO path");
        // SAFETY: `name` is a valid NUL-terminated path for the call's duration.
        let status = unsafe { libc::mkfifo(name.as_ptr(), 0o600) };
        assert_eq!(status, 0, "mkfifo {}", path.display());
    }

    /// Creates one tree entry, or applies one race step, under `base`.
    fn apply(base: &Path, entry: &Value) {
        let at = |field: &str| base.join(text(entry, field));
        if entry.get("dir").is_some() {
            fs::create_dir_all(at("dir")).expect("tree directory");
        } else if entry.get("file").is_some() {
            let path = at("file");
            parent(&path);
            let content = if let Some(size) = entry.get("size").and_then(Value::as_u64) {
                vec![b'a'; size as usize]
            } else if let Some(hex) = entry.get("hex").and_then(Value::as_str) {
                (0..hex.len())
                    .step_by(2)
                    .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).expect("hex bytes"))
                    .collect()
            } else {
                text(entry, "text").as_bytes().to_vec()
            };
            fs::write(&path, content).expect("tree file");
        } else if entry.get("symlink").is_some() {
            let path = at("symlink");
            parent(&path);
            std::os::unix::fs::symlink(text(entry, "target"), &path).expect("tree symlink");
        } else if entry.get("hardlink").is_some() {
            let path = at("hardlink");
            parent(&path);
            fs::hard_link(at("target"), &path).expect("tree hard link");
        } else if entry.get("fifo").is_some() {
            let path = at("fifo");
            parent(&path);
            mkfifo(&path);
        } else if entry.get("socket").is_some() {
            let path = at("socket");
            parent(&path);
            // The socket file outlives the listener.
            std::os::unix::net::UnixListener::bind(&path).expect("tree socket");
        } else if let Some(pair) = entry.get("rename").and_then(Value::as_array) {
            let [from, to] = pair.as_slice() else {
                panic!("a rename names two paths")
            };
            let to = base.join(to.as_str().expect("a rename target"));
            parent(&to);
            fs::rename(base.join(from.as_str().expect("a rename source")), to)
                .expect("race rename");
        } else if entry.get("remove").is_some() {
            fs::remove_file(at("remove")).expect("race remove");
        } else {
            panic!("unknown tree entry {entry}");
        }
    }

    /// Whether this filesystem opens a name by another spelling of it.
    fn equates(base: &Path, alias: &str) -> bool {
        let (name, other) = match alias {
            "case" => ("alias-probe", "ALIAS-PROBE"),
            "unicode" => ("alias-probe-\u{e9}", "alias-probe-e\u{301}"),
            other => panic!("unknown alias kind {other}"),
        };
        fs::write(base.join(name), b"").expect("alias probe");
        let equated = base.join(other).exists();
        fs::remove_file(base.join(name)).expect("alias probe removal");
        equated
    }

    fn step(at: &str) -> Step {
        let index = |prefix: &str| -> Option<usize> {
            at.strip_prefix(prefix)
                .map(|n| n.parse().expect("a step index"))
        };
        match at {
            "root" => Step::Root,
            "before_open" => Step::BeforeOpen,
            "opened" => Step::Opened,
            "read" => Step::Read,
            _ => index("before_directory:")
                .map(Step::BeforeDirectory)
                .or_else(|| index("directory:").map(Step::Directory))
                .unwrap_or_else(|| panic!("unknown race step {at}")),
        }
    }

    #[test]
    fn filesystem_vectors() {
        let vectors = vectors();
        for case in cases(&vectors, "filesystem") {
            let id = &case["id"];
            let scratch = scratch();
            let base = scratch.path();
            for folder in ["root", "outside"] {
                fs::create_dir_all(base.join(folder)).expect("scratch folder");
            }
            for entry in case["tree"].as_array().expect("a vector has a tree") {
                apply(base, entry);
            }
            let protected: Vec<PathBuf> = case
                .get("protected")
                .and_then(Value::as_array)
                .map(|paths| {
                    paths
                        .iter()
                        .map(|path| base.join(path.as_str().expect("a protected path")))
                        .collect()
                })
                .unwrap_or_default();
            let protected: Vec<&Path> = protected.iter().map(PathBuf::as_path).collect();
            let root = base.join("root");
            let expect = &case["expect"];

            let built = ScopedReader::new(
                &scopes(case),
                &[(RootKind::Project, root.as_path())],
                &protected,
                R0_READ_POLICY,
            );
            if let Some(code) = expect.get("bind") {
                assert_eq!(
                    built.err().map(Refusal::code),
                    code.as_str(),
                    "vector {id} binds"
                );
                continue;
            }
            let mut reader =
                built.unwrap_or_else(|refusal| panic!("vector {id} binds: {}", refusal.code()));
            let refusal = match case.get("alias").and_then(Value::as_str) {
                Some(alias) if !equates(base, alias) => Some("resource_not_found"),
                _ => expect.get("refusal").and_then(Value::as_str),
            };

            let mut request = json!({
                "operation": case.get("operation").cloned().unwrap_or(json!("read_file")),
                "root": case.get("requestRoot").cloned().unwrap_or(json!("project")),
                "path": case["request"],
            });
            for field in ["after", "pattern"] {
                if let Some(value) = case.get(field) {
                    request[field] = value.clone();
                }
            }
            let race = case.get("race");
            // `opened#2` is the second time the read reaches `opened`.
            let race_at = race.map(|race| {
                let at = text(race, "at");
                let (at, occurrence) = at.split_once('#').map_or((at, 1), |(at, n)| {
                    (at, n.parse::<usize>().expect("a step occurrence"))
                });
                (step(at), occurrence)
            });
            let mut steps = Vec::new();
            let mut fired = false;
            let result = reader.serve_with(&request, &mut |reached| {
                steps.push(reached);
                let reached_count = steps.iter().filter(|step| **step == reached).count();
                if race_at == Some((reached, reached_count)) && !fired {
                    fired = true;
                    for action in race
                        .and_then(|race| race["do"].as_array())
                        .into_iter()
                        .flatten()
                    {
                        apply(base, action);
                    }
                }
            });
            assert_eq!(race.is_some(), fired, "vector {id} reached its race step");
            if let Some(want) = expect.get("lastStep").and_then(Value::as_str) {
                assert_eq!(
                    steps.last(),
                    Some(&step(want)),
                    "vector {id} stopped at {steps:?}"
                );
            }
            if expect.get("filesystem") == Some(&Value::Bool(false)) {
                assert!(
                    steps.is_empty(),
                    "vector {id} touched the filesystem: {steps:?}"
                );
            }

            match (result, refusal) {
                (Err(got), Some(code)) => assert_eq!(got.code(), code, "vector {id}"),
                (Err(got), None) => panic!("vector {id} refused: {}", got.code()),
                (Ok(response), Some(code)) => {
                    panic!(
                        "vector {id} answered {} instead of refusing {code}",
                        response.to_json()
                    )
                }
                (Ok(response), None) => check_response(id, &response, expect),
            }
        }
    }

    /// Checks a vector's successful response: no sentinel anywhere in it, a
    /// digest of its JCS text, and the body the vector expects.
    fn check_response(id: &Value, response: &Response, expect: &Value) {
        let json = response.to_json();
        assert!(
            !json.to_string().contains("SENTINEL"),
            "vector {id} disclosed a sentinel: {json}"
        );
        assert_eq!(
            response.digest(),
            sha256_hex(&canonicalize(&json).unwrap()),
            "vector {id} digest"
        );
        match &response.body {
            Body::File { content } => {
                if let Some(want) = expect.get("text") {
                    assert_eq!(content, want.as_str().unwrap(), "vector {id}");
                }
                if let Some(want) = expect.get("length").and_then(Value::as_u64) {
                    assert_eq!(content.len() as u64, want, "vector {id} length");
                }
            }
            Body::Listing { .. } => {
                let entries: Vec<Value> = json["entries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|entry| json!([entry["name"], entry["kind"]]))
                    .collect();
                assert_eq!(
                    Value::from(entries),
                    expect["entries"],
                    "vector {id} entries"
                );
                assert_eq!(json["next"], expect["next"], "vector {id} next");
            }
            Body::Search { .. } => {
                let matches: Vec<Value> = json["matches"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|found| json!([found["path"], found["line"], found["text"]]))
                    .collect();
                assert_eq!(
                    Value::from(matches),
                    expect["matches"],
                    "vector {id} matches"
                );
                for field in ["next", "skipped", "complete"] {
                    assert_eq!(json[field], expect[field], "vector {id} {field}");
                }
            }
        }
    }

    fn reader(base: &Path, scopes: &[Value]) -> ScopedReader {
        reader_with(base, scopes, R0_READ_POLICY)
    }

    fn reader_with(base: &Path, scopes: &[Value], policy: ReadPolicy) -> ScopedReader {
        let root = base.join("root");
        ScopedReader::new(scopes, &[(RootKind::Project, root.as_path())], &[], policy)
            .unwrap_or_else(|refusal| panic!("binds: {}", refusal.code()))
    }

    fn content(response: Response) -> String {
        match response.body {
            Body::File { content } => content,
            other => panic!("not a file: {other:?}"),
        }
    }

    fn read_request(path: &str) -> Value {
        json!({"operation": "read_file", "root": "project", "path": path})
    }

    #[test]
    fn every_call_counts_against_the_attempt() {
        let scratch = scratch();
        let file = scratch.path().join("root/notes/today.md");
        parent(&file);
        fs::write(&file, b"x").expect("file");
        let mut reader = reader(scratch.path(), &[scope("read:notes/today.md")]);
        for call in 0..R0_READ_POLICY.calls {
            // Refused calls count too: alternate them with reads.
            let path = if call % 2 == 0 {
                "notes/today.md"
            } else {
                "notes/other.md"
            };
            let _ = reader.serve(&read_request(path));
        }
        assert_eq!(
            reader.serve(&read_request("notes/today.md")).err(),
            Some(Refusal::LimitCalls)
        );
        assert_eq!(
            reader.serve(&json!("not even a request")).err(),
            Some(Refusal::LimitCalls)
        );
    }

    #[test]
    fn an_attempt_discloses_at_most_its_total() {
        let scratch = scratch();
        let file = scratch.path().join("root/data.txt");
        parent(&file);
        fs::write(&file, vec![b'a'; R0_READ_POLICY.file_bytes as usize]).expect("file");
        let small = scratch.path().join("root/small.txt");
        fs::write(&small, b"x").expect("file");
        let mut reader = reader(
            scratch.path(),
            &[scope("read:data.txt"), scope("read:small.txt")],
        );
        let reads = R0_READ_POLICY.attempt_bytes / R0_READ_POLICY.file_bytes;
        for _ in 0..reads {
            let read = content(
                reader
                    .serve(&read_request("data.txt"))
                    .expect("within the total"),
            );
            assert_eq!(read.len() as u64, R0_READ_POLICY.file_bytes);
        }
        // Refused before any of the file is read.
        let mut steps = Vec::new();
        assert_eq!(
            reader
                .serve_with(&read_request("small.txt"), &mut |step| steps.push(step))
                .err(),
            Some(Refusal::LimitAttemptDisclosure)
        );
        assert_eq!(steps.last(), Some(&Step::Opened), "stopped at {steps:?}");
    }

    #[test]
    fn a_mount_crossing_is_refused() {
        // `/dev` is its own filesystem on every Unix the daemon runs on.
        let mut reader = ScopedReader::new(
            &[scope("read:dev/null")],
            &[(RootKind::Project, Path::new("/"))],
            &[],
            R0_READ_POLICY,
        )
        .unwrap_or_else(|refusal| panic!("binds /: {}", refusal.code()));
        assert_eq!(
            reader.serve(&read_request("dev/null")).err(),
            Some(Refusal::ResourceMountCrossing)
        );
    }

    #[test]
    fn a_root_that_is_not_a_directory_is_unbound() {
        let scratch = scratch();
        let file = scratch.path().join("root");
        fs::write(&file, b"x").expect("file");
        let missing = scratch.path().join("missing");
        for path in [file.as_path(), missing.as_path()] {
            let refused = ScopedReader::new(
                &[scope("read:notes/today.md")],
                &[(RootKind::Project, path)],
                &[],
                R0_READ_POLICY,
            );
            assert_eq!(
                refused.err(),
                Some(Refusal::RootUnbound),
                "{}",
                path.display()
            );
        }
    }

    fn list_request(path: &str, after: Option<&str>) -> Value {
        let mut request = json!({"operation": "list_directory", "root": "project", "path": path});
        if let Some(after) = after {
            request["after"] = json!(after);
        }
        request
    }

    fn search_request(path: &str, pattern: &str, after: Option<Value>) -> Value {
        let mut request =
            json!({"operation": "search", "root": "project", "path": path, "pattern": pattern});
        if let Some(after) = after {
            request["after"] = after;
        }
        request
    }

    #[test]
    fn a_listing_pages_through_a_large_directory() {
        let scratch = scratch();
        let directory = scratch.path().join("root/docs/many");
        fs::create_dir_all(&directory).expect("directory");
        let count = R0_READ_POLICY.page_entries + 44;
        for index in 0..count {
            fs::write(directory.join(format!("{index:04}.md")), b"x").expect("file");
        }
        let mut reader = reader(scratch.path(), &[scope("recursive-read:docs/many")]);
        let first = reader
            .serve(&list_request("docs/many", None))
            .expect("first page")
            .to_json();
        let entries = first["entries"].as_array().unwrap();
        assert_eq!(entries.len(), R0_READ_POLICY.page_entries);
        assert_eq!(entries[0]["name"], "0000.md");
        assert_eq!(first["next"], "0255.md");
        let second = reader
            .serve(&list_request("docs/many", Some("0255.md")))
            .expect("second page")
            .to_json();
        let entries = second["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 44);
        assert_eq!(entries[0]["name"], "0256.md");
        assert_eq!(second["next"], Value::Null);
    }

    #[test]
    fn a_search_pages_through_many_matches() {
        let scratch = scratch();
        let file = scratch.path().join("root/docs/guides/a.md");
        parent(&file);
        let count = R0_READ_POLICY.page_matches + 44;
        fs::write(&file, "TODO\n".repeat(count)).expect("file");
        let mut reader = reader(scratch.path(), &[scope("recursive-read:docs/guides")]);
        let first = reader
            .serve(&search_request("docs/guides", "TODO", None))
            .expect("first page")
            .to_json();
        assert_eq!(
            first["matches"].as_array().unwrap().len(),
            R0_READ_POLICY.page_matches
        );
        assert_eq!(
            first["next"],
            json!({"path": "docs/guides/a.md", "line": 256})
        );
        assert_eq!(first["complete"], false);
        let second = reader
            .serve(&search_request(
                "docs/guides",
                "TODO",
                Some(first["next"].clone()),
            ))
            .expect("second page")
            .to_json();
        let matches = second["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 44);
        assert_eq!(matches[0]["line"], 257);
        assert_eq!(second["next"], Value::Null);
        assert_eq!(second["complete"], true);
    }

    #[test]
    fn a_search_stops_at_the_scan_total() {
        let scratch = scratch();
        let guides = scratch.path().join("root/docs/guides");
        fs::create_dir_all(&guides).expect("directory");
        fs::write(guides.join("a.md"), "TODO a\n").expect("file");
        fs::write(guides.join("b.md"), "TODO b\n").expect("file");
        // The two names (8 bytes) and a.md (7 bytes), and nothing for b.md.
        let policy = ReadPolicy {
            scan_bytes: 8 + 7,
            ..R0_READ_POLICY
        };
        let mut reader = reader_with(
            scratch.path(),
            &[scope("recursive-read:docs/guides")],
            policy,
        );
        let found = reader
            .serve(&search_request("docs/guides", "TODO", None))
            .expect("a partial search")
            .to_json();
        assert_eq!(
            found["matches"],
            json!([{"path": "docs/guides/a.md", "line": 1, "text": "TODO a"}])
        );
        assert_eq!(found["complete"], false);
        assert_eq!(found["next"], Value::Null);
        // The scan total is spent, so listings and searches are refused. A
        // read scans nothing and is still served.
        assert_eq!(
            reader
                .serve(&search_request("docs/guides", "TODO", None))
                .err(),
            Some(Refusal::LimitScan)
        );
        assert_eq!(
            reader.serve(&list_request("docs/guides", None)).err(),
            Some(Refusal::LimitScan)
        );
        assert!(reader.serve(&read_request("docs/guides/b.md")).is_ok());
    }

    #[test]
    fn excluded_names_are_charged_to_the_scan_total() {
        let scratch = scratch();
        let repo = scratch.path().join("root/work/repo");
        fs::create_dir_all(repo.join(".git")).expect("directory");
        fs::write(repo.join(".git/config"), "TODO SENTINEL").expect("file");
        fs::write(repo.join(".env"), "TODO SENTINEL").expect("file");
        fs::write(repo.join(".env.local"), "TODO SENTINEL").expect("file");
        fs::write(repo.join("a.md"), "TODO a").expect("file");
        // `.git`, `.env` and `.env.local` (18 bytes) and `a.md` (4 bytes):
        // the excluded names are charged before they are left out.
        let names = 4 + 4 + 10 + 4;
        let scoped = |scan_bytes| {
            reader_with(
                scratch.path(),
                &[scope("recursive-read:work/repo")],
                ReadPolicy {
                    scan_bytes,
                    ..R0_READ_POLICY
                },
            )
        };

        let mut reader = scoped(names - 1);
        assert_eq!(
            reader.serve(&list_request("work/repo", None)).err(),
            Some(Refusal::LimitScan)
        );
        let mut reader = scoped(names - 1);
        let found = reader
            .serve(&search_request("work/repo", "TODO", None))
            .expect("a partial search")
            .to_json();
        assert_eq!(found["matches"], json!([]));
        assert_eq!(found["complete"], false);
        assert_eq!(found["next"], Value::Null);

        // Exactly the names: the listing is served and spends the total.
        let mut reader = scoped(names);
        let listed = reader
            .serve(&list_request("work/repo", None))
            .expect("a listing")
            .to_json();
        assert_eq!(listed["entries"], json!([{"name": "a.md", "kind": "file"}]));
        assert_eq!(
            reader.serve(&list_request("work/repo", None)).err(),
            Some(Refusal::LimitScan)
        );
    }

    #[test]
    fn implicit_exclusion_alias_requests_refuse_before_filesystem_access() {
        let scratch = scratch();
        fs::create_dir_all(scratch.path().join("root/work/repo")).unwrap();
        for ignored in ['\u{200d}', '\u{00ad}', '\u{034f}', '\u{fe0f}', '\u{feff}'] {
            let env = format!("work/repo/.e{ignored}nv");
            let git = format!("work/repo/.g{ignored}it");
            for request in [
                read_request(&env),
                read_request(&format!("{env}.local")),
                read_request(&format!("{git}/config")),
                list_request(&git, None),
                search_request(&git, "TODO", None),
            ] {
                let mut reader = reader(scratch.path(), &[scope("recursive-read:work/repo")]);
                let mut steps = Vec::new();
                let result = reader.serve_with(&request, &mut |step| steps.push(step));
                assert_eq!(result.err(), Some(Refusal::ScopeExcluded), "{request}");
                assert!(
                    steps.is_empty(),
                    "{request} touched the filesystem: {steps:?}"
                );
            }
        }
    }

    #[test]
    fn implicit_exclusion_aliases_stay_out_of_listings_and_searches() {
        for ignored in ['\u{200d}', '\u{00ad}', '\u{034f}', '\u{fe0f}', '\u{feff}'] {
            let scratch = scratch();
            let repo = scratch.path().join("root/work/repo");
            let env = format!(".e{ignored}nv");
            let git = format!(".g{ignored}it");
            fs::create_dir_all(repo.join(&git)).unwrap();
            for name in [&env, &format!("{env}.local"), &format!("{git}/config")] {
                fs::write(repo.join(name), "TODO SENTINEL").unwrap();
            }
            fs::write(repo.join("a.md"), "TODO a").unwrap();
            let mut reader = reader(
                scratch.path(),
                &[
                    scope("recursive-read:work/repo"),
                    scope(&format!("read:work/repo/{env}")),
                ],
            );
            let listing = reader
                .serve(&list_request("work/repo", None))
                .unwrap()
                .to_json();
            assert_eq!(
                listing["entries"],
                json!([{"name": "a.md", "kind": "file"}]),
                "{ignored:?}"
            );
            let found = reader
                .serve(&search_request("work/repo", "TODO", None))
                .unwrap()
                .to_json();
            assert_eq!(
                found["matches"],
                json!([{"path": "work/repo/a.md", "line": 1, "text": "TODO a"}]),
                "{ignored:?}"
            );
            assert_eq!(found["complete"], true);
            assert_eq!(found["skipped"], 0);
            // Deliberately naming the exact excluded alias still permits reading it.
            assert_eq!(
                content(
                    reader
                        .serve(&read_request(&format!("work/repo/{env}")))
                        .unwrap()
                ),
                "TODO SENTINEL"
            );
        }
    }

    #[test]
    fn implicit_exclusion_alias_search_never_opens_hidden_files() {
        let scratch = scratch();
        let repo = scratch.path().join("root/work/repo");
        fs::create_dir_all(repo.join(".g\u{200d}it")).unwrap();
        fs::write(repo.join(".g\u{200d}it/config"), "TODO SENTINEL git").unwrap();
        fs::write(repo.join(".e\u{200d}nv"), "TODO SENTINEL env").unwrap();
        fs::write(repo.join("a.md"), "TODO a").unwrap();
        let mut reader = reader(scratch.path(), &[scope("recursive-read:work/repo")]);
        let mut reads = 0;
        let found = reader
            .serve_with(&search_request("work/repo", "TODO", None), &mut |step| {
                if step == Step::Read {
                    reads += 1;
                }
            })
            .unwrap()
            .to_json();
        assert_eq!(
            found["matches"],
            json!([{"path": "work/repo/a.md", "line": 1, "text": "TODO a"}])
        );
        // One read enumerates the requested directory; one reads a.md.
        assert_eq!(reads, 2, "no hidden file may be read");
        assert_eq!(found["complete"], true);
        assert_eq!(found["skipped"], 0);
    }

    #[test]
    fn implicit_exclusion_aliases_charge_original_name_bytes() {
        let scratch = scratch();
        let repo = scratch.path().join("root/work/repo");
        fs::create_dir_all(repo.join(".g\u{200d}it")).unwrap();
        fs::write(repo.join(".g\u{200d}it/config"), "TODO SENTINEL").unwrap();
        fs::write(repo.join(".e\u{200d}nv"), "TODO SENTINEL").unwrap();
        fs::write(repo.join(".env\u{200d}.local"), "TODO SENTINEL").unwrap();
        fs::write(repo.join("a.md"), "TODO a").unwrap();
        // Raw UTF-8 name lengths: 7 + 7 + 13 + 4 = 31, not the 22 normalized bytes.
        let scoped = |scan_bytes| {
            reader_with(
                scratch.path(),
                &[scope("recursive-read:work/repo")],
                ReadPolicy {
                    scan_bytes,
                    ..R0_READ_POLICY
                },
            )
        };
        let mut listing = scoped(30);
        assert_eq!(
            listing.serve(&list_request("work/repo", None)).err(),
            Some(Refusal::LimitScan)
        );
        let mut search = scoped(30);
        let found = search
            .serve(&search_request("work/repo", "TODO", None))
            .unwrap()
            .to_json();
        assert_eq!(found["matches"], json!([]));
        assert_eq!(found["complete"], false);
        let mut listing = scoped(31);
        let listed = listing
            .serve(&list_request("work/repo", None))
            .unwrap()
            .to_json();
        assert_eq!(listed["entries"], json!([{"name": "a.md", "kind": "file"}]));
        assert_eq!(
            listing.serve(&list_request("work/repo", None)).err(),
            Some(Refusal::LimitScan)
        );
    }

    #[test]
    fn a_named_exclusion_alias_cannot_override_a_protected_path() {
        let scratch = scratch();
        let repo = scratch.path().join("root/work/repo");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join(".e\u{200d}nv"), "TODO SENTINEL").unwrap();
        let mut reader = reader_protecting(
            scratch.path(),
            &[scope("read:work/repo/.e\u{200d}nv")],
            &["root/work/repo/.env"],
        );
        assert_eq!(
            reader.serve(&read_request("work/repo/.e\u{200d}nv")).err(),
            Some(Refusal::ResourceProtected)
        );
    }

    #[test]
    fn a_search_reports_incomplete_when_content_uses_the_exact_scan_budget() {
        let scratch = scratch();
        let file = scratch.path().join("root/docs/guides/a");
        parent(&file);
        fs::write(&file, "hit").unwrap();
        let mut reader = reader_with(
            scratch.path(),
            &[scope("recursive-read:docs/guides")],
            ReadPolicy {
                scan_bytes: 4,
                ..R0_READ_POLICY
            },
        );
        let found = reader
            .serve(&search_request("docs/guides", "hit", None))
            .unwrap()
            .to_json();
        assert_eq!(
            found["matches"],
            json!([{"path":"docs/guides/a", "line":1, "text":"hit"}])
        );
        assert_eq!(found["complete"], false);
        assert_eq!(
            reader.serve(&list_request("docs/guides", None)).err(),
            Some(Refusal::LimitScan)
        );
    }

    #[test]
    fn a_search_charges_bytes_consumed_before_a_growing_file_is_refused() {
        let scratch = scratch();
        let directory = scratch.path().join("root/docs/guides");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("a"), "x").unwrap();
        fs::write(directory.join("b"), "hit").unwrap();
        let mut reader = reader_with(
            scratch.path(),
            &[scope("recursive-read:docs/guides")],
            ReadPolicy {
                file_bytes: 4,
                scan_bytes: 7,
                ..R0_READ_POLICY
            },
        );
        let mut reads = 0;
        let result = reader
            .serve_with(&search_request("docs/guides", "hit", None), &mut |step| {
                if step == Step::Read {
                    reads += 1;
                    if reads == 2 {
                        fs::write(directory.join("a"), "grown").unwrap();
                    }
                }
            })
            .unwrap()
            .to_json();
        assert_eq!(
            reads, 2,
            "the directory and growing file were read; no later file was read"
        );
        assert_eq!(result["matches"], json!([]));
        assert_eq!(result["complete"], false);
        assert_eq!(result["skipped"], 1);
        let mut probed = false;
        assert_eq!(
            reader
                .serve_with(&list_request("docs/guides", None), &mut |_| probed = true)
                .err(),
            Some(Refusal::LimitScan)
        );
        assert!(
            !probed,
            "the exhausted attempt refuses before filesystem access"
        );
    }

    #[test]
    fn a_growing_search_file_cannot_spend_the_scan_boundary_sentinel() {
        for scan_bytes in [5, 6] {
            let scratch = scratch();
            let directory = scratch.path().join("root/docs/guides");
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join("a"), "x").unwrap();
            fs::write(directory.join("b"), "hit").unwrap();
            let mut reader = reader_with(
                scratch.path(),
                &[scope("recursive-read:docs/guides")],
                ReadPolicy {
                    file_bytes: 4,
                    scan_bytes,
                    ..R0_READ_POLICY
                },
            );
            let mut reads = 0;
            let found = reader
                .serve_with(&search_request("docs/guides", "hit", None), &mut |step| {
                    if step == Step::Read {
                        reads += 1;
                        if reads == 2 {
                            fs::write(directory.join("a"), "grown").unwrap();
                        }
                    }
                })
                .unwrap()
                .to_json();
            assert_eq!(reads, 2);
            assert_eq!(found["matches"], json!([]));
            assert_eq!(found["complete"], false);
            assert_eq!(
                reader.serve(&list_request("docs/guides", None)).err(),
                Some(Refusal::LimitScan)
            );
        }
    }

    #[test]
    fn a_refused_listing_spends_the_enumeration_scan_budget() {
        let scratch = scratch();
        let large = scratch.path().join("root/docs/guides/large");
        let small = scratch.path().join("root/docs/guides/small");
        fs::create_dir_all(&large).unwrap();
        fs::create_dir_all(&small).unwrap();
        fs::write(large.join("a.md"), "TODO a").unwrap();
        fs::write(large.join("b.md"), "TODO b").unwrap();
        fs::write(small.join("x"), "x").unwrap();
        let mut reader = reader_with(
            scratch.path(),
            &[scope("recursive-read:docs/guides")],
            ReadPolicy {
                scan_bytes: 7,
                ..R0_READ_POLICY
            },
        );
        assert_eq!(
            reader.serve(&list_request("docs/guides/large", None)).err(),
            Some(Refusal::LimitScan)
        );
        let mut steps = Vec::new();
        assert_eq!(
            reader
                .serve_with(&list_request("docs/guides/small", None), &mut |step| steps
                    .push(step))
                .err(),
            Some(Refusal::LimitScan)
        );
        assert!(
            steps.is_empty(),
            "an exhausted scan must refuse before filesystem access: {steps:?}"
        );
        assert_eq!(
            content(reader.serve(&read_request("docs/guides/small/x")).unwrap()),
            "x"
        );
    }

    #[test]
    fn a_partial_search_spends_a_refused_enumeration_scan_budget() {
        let scratch = scratch();
        let large = scratch.path().join("root/docs/guides/large");
        let small = scratch.path().join("root/docs/guides/small");
        fs::create_dir_all(&large).unwrap();
        fs::create_dir_all(&small).unwrap();
        fs::write(large.join("a.md"), "TODO a").unwrap();
        fs::write(large.join("b.md"), "TODO b").unwrap();
        fs::write(small.join("x"), "x").unwrap();
        let mut reader = reader_with(
            scratch.path(),
            &[scope("recursive-read:docs/guides")],
            ReadPolicy {
                scan_bytes: 7,
                ..R0_READ_POLICY
            },
        );
        let found = reader
            .serve(&search_request("docs/guides/large", "TODO", None))
            .unwrap()
            .to_json();
        assert_eq!(found["complete"], false);
        assert_eq!(found["matches"], json!([]));
        let mut steps = Vec::new();
        assert_eq!(
            reader
                .serve_with(&list_request("docs/guides/small", None), &mut |step| steps
                    .push(step))
                .err(),
            Some(Refusal::LimitScan)
        );
        assert!(
            steps.is_empty(),
            "an exhausted scan must refuse before filesystem access: {steps:?}"
        );
    }

    #[test]
    fn a_search_checks_the_complete_child_path_byte_limit() {
        let scratch = scratch();
        let directory = scratch.path().join("root/docs/guides/deep");
        fs::create_dir_all(&directory).unwrap();
        fs::write(scratch.path().join("root/docs/guides/a.md"), "TODO shallow").unwrap();
        fs::write(directory.join("a.x"), "TODO at limit").unwrap();
        fs::write(directory.join("b.md"), "TODO outside path limit").unwrap();
        let policy = ReadPolicy {
            path_bytes: 20,
            ..R0_READ_POLICY
        };
        assert_eq!("docs/guides/deep/a.x".len(), policy.path_bytes);
        assert!("docs/guides/deep/b.md".len() > policy.path_bytes);
        let mut reader = reader_with(
            scratch.path(),
            &[scope("recursive-read:docs/guides")],
            policy,
        );
        assert_eq!(
            reader.serve(&read_request("docs/guides/deep/b.md")).err(),
            Some(Refusal::PathLimit)
        );
        let found = reader
            .serve(&search_request("docs/guides", "TODO", None))
            .unwrap()
            .to_json();
        let paths: Vec<_> = found["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["path"].as_str().unwrap())
            .collect();
        assert_eq!(paths, ["docs/guides/a.md", "docs/guides/deep/a.x"]);
        assert_eq!(found["complete"], true);
    }

    #[test]
    fn a_listing_omits_children_beyond_the_complete_path_byte_limit() {
        let scratch = scratch();
        let directory = scratch.path().join("root/docs/guides/deep");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("a.x"), "at limit").unwrap();
        fs::write(directory.join("b.md"), "outside path limit").unwrap();
        let mut reader = reader_with(
            scratch.path(),
            &[scope("recursive-read:docs/guides")],
            ReadPolicy {
                path_bytes: 20,
                ..R0_READ_POLICY
            },
        );
        let listed = reader
            .serve(&list_request("docs/guides/deep", None))
            .unwrap()
            .to_json();
        assert_eq!(listed["entries"], json!([{"name":"a.x", "kind":"file"}]));
    }

    #[test]
    fn the_largest_continuation_line_skips_that_file_without_overflow() {
        let scratch = scratch();
        let directory = scratch.path().join("root/docs/guides");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("a.md"), "TODO before cursor").unwrap();
        fs::write(directory.join("b.md"), "TODO after cursor").unwrap();
        let mut reader = reader(scratch.path(), &[scope("recursive-read:docs/guides")]);
        let found = reader
            .serve(&search_request(
                "docs/guides",
                "TODO",
                Some(json!({"path":"docs/guides/a.md", "line":u64::MAX})),
            ))
            .unwrap()
            .to_json();
        assert_eq!(
            found["matches"],
            json!([{"path":"docs/guides/b.md", "line":1, "text":"TODO after cursor"}])
        );
        assert_eq!(found["complete"], true);
    }

    #[test]
    fn a_search_discloses_at_most_the_attempt_total() {
        let scratch = scratch();
        let file = scratch.path().join("root/docs/guides/a.md");
        parent(&file);
        fs::write(&file, "TODO one\nTODO two\n").expect("file");
        let policy = ReadPolicy {
            attempt_bytes: "TODO one".len() as u64,
            ..R0_READ_POLICY
        };
        let mut reader = reader_with(
            scratch.path(),
            &[scope("recursive-read:docs/guides")],
            policy,
        );
        let found = reader
            .serve(&search_request("docs/guides", "TODO", None))
            .expect("a search")
            .to_json();
        assert_eq!(
            found["matches"],
            json!([{"path": "docs/guides/a.md", "line": 1, "text": "TODO one"}])
        );
        assert_eq!(found["complete"], false);
        assert_eq!(
            reader.serve(&read_request("docs/guides/a.md")).err(),
            Some(Refusal::LimitAttemptDisclosure)
        );
    }

    #[test]
    fn a_search_descends_at_most_the_policy_depth() {
        let scratch = scratch();
        let policy = ReadPolicy {
            depth: 4,
            ..R0_READ_POLICY
        };
        // docs/guides/a/b.md is four components; docs/guides/a/b/c.md is five.
        let shallow = scratch.path().join("root/docs/guides/a/b.md");
        let deep = scratch.path().join("root/docs/guides/a/b/c.md");
        parent(&deep);
        fs::write(&shallow, "TODO shallow").expect("file");
        fs::write(&deep, "TODO deep").expect("file");
        let mut reader = reader_with(
            scratch.path(),
            &[scope("recursive-read:docs/guides")],
            policy,
        );
        let found = reader
            .serve(&search_request("docs/guides", "TODO", None))
            .expect("a search")
            .to_json();
        assert_eq!(
            found["matches"],
            json!([{"path": "docs/guides/a/b.md", "line": 1, "text": "TODO shallow"}])
        );
    }

    #[test]
    fn a_long_line_is_cut_on_a_character_boundary() {
        let scratch = scratch();
        let file = scratch.path().join("root/docs/guides/a.md");
        parent(&file);
        // 511 ASCII bytes then a two-byte character straddling the limit.
        let line = format!(
            "TODO{}\u{e9}tail",
            "a".repeat(R0_READ_POLICY.match_bytes - 5)
        );
        fs::write(&file, &line).expect("file");
        let mut reader = reader(scratch.path(), &[scope("recursive-read:docs/guides")]);
        let found = reader
            .serve(&search_request("docs/guides", "TODO", None))
            .expect("a search")
            .to_json();
        let text = found["matches"][0]["text"].as_str().unwrap();
        assert_eq!(text.len(), R0_READ_POLICY.match_bytes - 1);
        assert!(line.starts_with(text));
    }

    /// A reader over `base/root` that protects each of `protected`, given
    /// relative to `base`.
    fn reader_protecting(base: &Path, scopes: &[Value], protected: &[&str]) -> ScopedReader {
        let root = base.join("root");
        let protected: Vec<PathBuf> = protected.iter().map(|path| base.join(path)).collect();
        let protected: Vec<&Path> = protected.iter().map(PathBuf::as_path).collect();
        ScopedReader::new(
            scopes,
            &[(RootKind::Project, root.as_path())],
            &protected,
            R0_READ_POLICY,
        )
        .unwrap_or_else(|refusal| panic!("binds: {}", refusal.code()))
    }

    /// Replaces `path` the way an editor's atomic save does: a new file is
    /// written beside it and renamed over it, so the name gets a new inode.
    fn atomic_replace(path: &Path, content: &str) {
        let temporary = path.with_extension("tmp-save");
        fs::write(&temporary, content).expect("replacement");
        fs::rename(&temporary, path).expect("atomic rename");
    }

    /// Reads, lists and searches `docs/guides`, asserting that the protected
    /// `secret.cfg` is refused, left out of the listing, and never searched.
    fn assert_secret_withheld(reader: &mut ScopedReader) {
        assert_eq!(
            reader.serve(&read_request("docs/guides/secret.cfg")).err(),
            Some(Refusal::ResourceProtected),
            "a read of the protected secret.cfg"
        );
        assert_secret_unlisted_and_unsearched(reader);
    }

    fn assert_secret_unlisted_and_unsearched(reader: &mut ScopedReader) {
        let listing = reader
            .serve(&list_request("docs/guides", None))
            .expect("a listing")
            .to_json();
        assert_eq!(
            listing["entries"],
            json!([{"name": "a.md", "kind": "file"}]),
            "the listing leaves the protected secret.cfg out"
        );
        let found = reader
            .serve(&search_request("docs/guides", "TODO", None))
            .expect("a search")
            .to_json();
        assert!(
            !found.to_string().contains("SENTINEL"),
            "the search disclosed the protected secret.cfg: {found}"
        );
        assert_eq!(
            found["matches"],
            json!([{"path": "docs/guides/a.md", "line": 1, "text": "TODO a"}])
        );
        assert_eq!(found["skipped"], 0);
        assert_eq!(found["complete"], true);
    }

    #[test]
    fn a_protected_file_replaced_by_an_atomic_save_stays_refused() {
        let scratch = scratch();
        let guides = scratch.path().join("root/docs/guides");
        fs::create_dir_all(&guides).unwrap();
        fs::write(guides.join("a.md"), "TODO a").unwrap();
        fs::write(guides.join("secret.cfg"), "TODO SENTINEL original").unwrap();
        let mut reader = reader_protecting(
            scratch.path(),
            &[scope("recursive-read:docs/guides")],
            &["root/docs/guides/secret.cfg"],
        );
        assert_secret_withheld(&mut reader);
        atomic_replace(&guides.join("secret.cfg"), "TODO SENTINEL replaced");
        assert_secret_withheld(&mut reader);
    }

    #[test]
    fn a_protected_file_replaced_during_a_request_stays_refused() {
        let scratch = scratch();
        let guides = scratch.path().join("root/docs/guides");
        fs::create_dir_all(&guides).unwrap();
        fs::write(guides.join("a.md"), "TODO a").unwrap();
        fs::write(guides.join("secret.cfg"), "TODO SENTINEL original").unwrap();
        let mut reader = reader_protecting(
            scratch.path(),
            &[scope("recursive-read:docs/guides")],
            &["root/docs/guides/secret.cfg"],
        );
        let env = guides.join("secret.cfg");
        let mut replaced = 0;
        let mut replace_at_root = |step: Step| {
            if step == Step::Root {
                replaced += 1;
                atomic_replace(&env, &format!("TODO SENTINEL replaced {replaced}"));
            }
        };
        assert_eq!(
            reader
                .serve_with(
                    &read_request("docs/guides/secret.cfg"),
                    &mut replace_at_root
                )
                .err(),
            Some(Refusal::ResourceProtected)
        );
        let listing = reader
            .serve_with(&list_request("docs/guides", None), &mut replace_at_root)
            .expect("a listing")
            .to_json();
        assert_eq!(
            listing["entries"],
            json!([{"name": "a.md", "kind": "file"}])
        );
        let found = reader
            .serve_with(
                &search_request("docs/guides", "TODO", None),
                &mut replace_at_root,
            )
            .expect("a search")
            .to_json();
        assert!(!found.to_string().contains("SENTINEL"), "{found}");
    }

    #[test]
    fn a_protected_path_created_after_the_reader_is_built_is_refused() {
        let scratch = scratch();
        let guides = scratch.path().join("root/docs/guides");
        fs::create_dir_all(&guides).unwrap();
        fs::write(guides.join("a.md"), "TODO a").unwrap();
        let mut reader = reader_protecting(
            scratch.path(),
            &[scope("recursive-read:docs/guides")],
            &["root/docs/guides/secret.cfg"],
        );
        fs::write(guides.join("secret.cfg"), "TODO SENTINEL created").unwrap();
        assert_secret_withheld(&mut reader);
    }

    #[test]
    fn a_protected_directory_created_after_the_reader_is_built_is_refused() {
        let scratch = scratch();
        let guides = scratch.path().join("root/docs/guides");
        fs::create_dir_all(&guides).unwrap();
        fs::write(guides.join("a.md"), "TODO a").unwrap();
        let mut reader = reader_protecting(
            scratch.path(),
            &[scope("recursive-read:docs/guides")],
            &["root/docs/guides/secret.cfg"],
        );
        fs::create_dir(guides.join("secret.cfg")).unwrap();
        fs::write(guides.join("secret.cfg/key"), "TODO SENTINEL created").unwrap();
        assert_eq!(
            reader
                .serve(&read_request("docs/guides/secret.cfg/key"))
                .err(),
            Some(Refusal::ResourceProtected)
        );
        assert_secret_unlisted_and_unsearched(&mut reader);
    }

    /// Reads, lists and searches `docs/guides` after a protected entry was
    /// renamed to `moved`, asserting `moved` stays refused, hidden and
    /// unsearched: what was protected at the build stays protected.
    fn assert_moved_withheld(reader: &mut ScopedReader, read: &str) {
        assert_eq!(
            reader.serve(&read_request(read)).err(),
            Some(Refusal::ResourceProtected),
            "a read of the renamed protected resource"
        );
        let listing = reader
            .serve(&list_request("docs/guides", None))
            .expect("a listing")
            .to_json();
        assert_eq!(
            listing["entries"],
            json!([{"name": "a.md", "kind": "file"}]),
            "the listing leaves the renamed protected resource out"
        );
        let found = reader
            .serve(&search_request("docs/guides", "TODO", None))
            .expect("a search")
            .to_json();
        assert!(!found.to_string().contains("SENTINEL"), "{found}");
        assert_eq!(
            found["matches"],
            json!([{"path": "docs/guides/a.md", "line": 1, "text": "TODO a"}])
        );
    }

    #[test]
    fn a_protected_directory_renamed_inside_a_scope_stays_refused() {
        let scratch = scratch();
        let guides = scratch.path().join("root/docs/guides");
        fs::create_dir_all(guides.join("vault")).unwrap();
        fs::write(guides.join("a.md"), "TODO a").unwrap();
        fs::write(guides.join("vault/k"), "TODO SENTINEL key").unwrap();
        let mut reader = reader_protecting(
            scratch.path(),
            &[scope("recursive-read:docs/guides")],
            &["root/docs/guides/vault"],
        );
        fs::rename(guides.join("vault"), guides.join("moved")).unwrap();
        assert_moved_withheld(&mut reader, "docs/guides/moved/k");
    }

    #[test]
    fn a_protected_file_renamed_inside_a_scope_stays_refused() {
        let scratch = scratch();
        let guides = scratch.path().join("root/docs/guides");
        fs::create_dir_all(&guides).unwrap();
        fs::write(guides.join("a.md"), "TODO a").unwrap();
        fs::write(guides.join("secret.cfg"), "TODO SENTINEL key").unwrap();
        let mut reader = reader_protecting(
            scratch.path(),
            &[scope("recursive-read:docs/guides")],
            &["root/docs/guides/secret.cfg"],
        );
        fs::rename(guides.join("secret.cfg"), guides.join("moved")).unwrap();
        assert_moved_withheld(&mut reader, "docs/guides/moved");
    }

    #[test]
    fn a_replacement_observed_during_an_attempt_stays_protected_after_rename() {
        for created in [false, true] {
            let scratch = scratch();
            let guides = scratch.path().join("root/docs/guides");
            fs::create_dir_all(&guides).unwrap();
            fs::write(guides.join("a.md"), "TODO a").unwrap();
            if !created {
                fs::write(guides.join("secret.cfg"), "original").unwrap();
            }
            let mut reader = reader_protecting(
                scratch.path(),
                &[scope("recursive-read:docs/guides")],
                &["root/docs/guides/secret.cfg"],
            );
            atomic_replace(&guides.join("secret.cfg"), "TODO SENTINEL replacement");
            assert_eq!(
                content(reader.serve(&read_request("docs/guides/a.md")).unwrap()),
                "TODO a"
            );
            fs::rename(guides.join("secret.cfg"), guides.join("moved")).unwrap();
            assert_moved_withheld(&mut reader, "docs/guides/moved");
        }
    }

    #[test]
    fn a_protected_identity_first_encountered_during_a_refusal_is_retained() {
        let scratch = scratch();
        let guides = scratch.path().join("root/docs/guides");
        fs::create_dir_all(&guides).unwrap();
        fs::write(guides.join("a.md"), "TODO a").unwrap();
        let mut reader = reader_protecting(
            scratch.path(),
            &[scope("recursive-read:docs/guides")],
            &["root/docs/guides/secret.cfg"],
        );
        let refused = reader.serve_with(&read_request("docs/guides/secret.cfg"), &mut |step| {
            if step == Step::Root {
                fs::write(guides.join("secret.cfg"), "TODO SENTINEL created").unwrap();
            }
        });
        assert_eq!(refused.err(), Some(Refusal::ResourceProtected));
        fs::rename(guides.join("secret.cfg"), guides.join("moved")).unwrap();
        assert_moved_withheld(&mut reader, "docs/guides/moved");
    }

    #[test]
    fn aliases_of_a_protected_name_created_mid_request_are_refused() {
        for (alias_kind, protected, alias) in [
            ("case", "secret.cfg", "SECRET.CFG"),
            ("unicode", "secret-é", "secret-e\u{301}"),
        ] {
            let scratch = scratch();
            if !equates(scratch.path(), alias_kind) {
                continue;
            }
            let guides = scratch.path().join("root/docs/guides");
            fs::create_dir_all(&guides).unwrap();
            let protected_path = format!("root/docs/guides/{protected}");
            let mut reader = reader_protecting(
                scratch.path(),
                &[scope("recursive-read:docs/guides")],
                &[&protected_path],
            );
            let result = reader.serve_with(
                &read_request(&format!("docs/guides/{alias}")),
                &mut |step| {
                    if step == Step::Root {
                        fs::write(guides.join(protected), "TODO SENTINEL alias").unwrap();
                    }
                },
            );
            assert_eq!(
                result.err(),
                Some(Refusal::ResourceProtected),
                "{alias_kind} alias must not disclose protected content"
            );
        }
    }

    #[test]
    fn a_bound_root_that_becomes_protected_refuses_every_operation() {
        let scratch = scratch();
        let guides = scratch.path().join("root/docs/guides");
        fs::create_dir_all(&guides).unwrap();
        fs::write(guides.join("a.md"), "TODO SENTINEL private root").unwrap();
        let mut reader = reader_protecting(
            scratch.path(),
            &[scope("recursive-read:docs/guides")],
            &["later-protected"],
        );
        std::os::unix::fs::symlink(
            scratch.path().join("root"),
            scratch.path().join("later-protected"),
        )
        .unwrap();
        for request in [
            read_request("docs/guides/a.md"),
            list_request("docs/guides", None),
            search_request("docs/guides", "TODO", None),
        ] {
            assert_eq!(reader.serve(&request).err(), Some(Refusal::RootProtected));
        }
    }

    #[test]
    fn a_bound_root_inside_a_newly_protected_ancestor_refuses_every_operation() {
        let scratch = scratch();
        let guides = scratch.path().join("root/docs/guides");
        fs::create_dir_all(&guides).unwrap();
        fs::write(guides.join("a.md"), "TODO SENTINEL private ancestor").unwrap();
        let mut reader = reader_protecting(
            scratch.path(),
            &[scope("recursive-read:docs/guides")],
            &["later-protected"],
        );
        std::os::unix::fs::symlink(scratch.path(), scratch.path().join("later-protected")).unwrap();
        for request in [
            read_request("docs/guides/a.md"),
            list_request("docs/guides", None),
            search_request("docs/guides", "TODO", None),
        ] {
            assert_eq!(reader.serve(&request).err(), Some(Refusal::RootProtected));
        }
    }

    #[test]
    fn a_search_whose_directory_fails_to_list_is_refused_and_charged() {
        use super::super::open::FAIL_LISTING_AFTER;

        let scratch = scratch();
        let guides = scratch.path().join("root/docs/guides");
        fs::create_dir_all(&guides).unwrap();
        // Two names of four bytes each, so the fault fires after the first
        // whichever order the directory returns them in.
        fs::write(guides.join("a.md"), "TODO a").unwrap();
        fs::write(guides.join("b.md"), "TODO b").unwrap();
        let mut reader = reader_with(
            scratch.path(),
            &[scope("recursive-read:docs/guides")],
            ReadPolicy {
                scan_bytes: 8,
                ..R0_READ_POLICY
            },
        );
        FAIL_LISTING_AFTER.with(|after| after.set(Some(4)));
        let mut steps = Vec::new();
        let refused = reader
            .serve_with(&search_request("docs/guides", "TODO", None), &mut |step| {
                steps.push(step)
            });
        let fired = FAIL_LISTING_AFTER.with(|after| after.take()).is_none();
        assert!(fired, "the listing fault fired");
        assert_eq!(
            steps.last(),
            Some(&Step::Read),
            "the directory opened and its checks passed before the listing failed"
        );
        assert_eq!(refused.err(), Some(Refusal::ResourceUnavailable));
        // The four name bytes scanned before the failure stay charged: a
        // listing of the same eight bytes no longer fits the scan total.
        assert_eq!(
            reader.serve(&list_request("docs/guides", None)).err(),
            Some(Refusal::LimitScan)
        );
    }

    /// Whether this process ignores permission bits, as root does.
    fn privileged() -> bool {
        // SAFETY: geteuid() only reads the effective uid and cannot fail.
        unsafe { libc::geteuid() == 0 }
    }

    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("permissions");
    }

    #[test]
    fn a_search_that_cannot_open_a_subdirectory_is_incomplete() {
        if privileged() {
            eprintln!("skipped: permission bits do not bind a privileged process");
            return;
        }
        let scratch = scratch();
        let guides = scratch.path().join("root/docs/guides");
        fs::create_dir_all(guides.join("locked")).unwrap();
        fs::write(guides.join("a.md"), "TODO a").unwrap();
        fs::write(guides.join("locked/x.md"), "TODO x").unwrap();
        fs::write(guides.join("z.md"), "TODO z").unwrap();
        set_mode(&guides.join("locked"), 0o000);
        let mut reader = reader(scratch.path(), &[scope("recursive-read:docs/guides")]);
        let found = reader.serve(&search_request("docs/guides", "TODO", None));
        set_mode(&guides.join("locked"), 0o755);
        let found = found.expect("a search").to_json();
        assert_eq!(
            found["matches"],
            json!([
                {"path": "docs/guides/a.md", "line": 1, "text": "TODO a"},
                {"path": "docs/guides/z.md", "line": 1, "text": "TODO z"},
            ]),
            "the walk continues past the unopenable directory"
        );
        assert_eq!(found["skipped"], 1);
        assert_eq!(found["complete"], false);
        assert_eq!(found["next"], Value::Null);
    }

    #[test]
    fn a_search_that_cannot_examine_a_subdirectorys_entries_is_incomplete() {
        if privileged() {
            eprintln!("skipped: permission bits do not bind a privileged process");
            return;
        }
        let scratch = scratch();
        let guides = scratch.path().join("root/docs/guides");
        fs::create_dir_all(guides.join("opaque")).unwrap();
        fs::write(guides.join("a.md"), "TODO a").unwrap();
        fs::write(guides.join("opaque/x.md"), "TODO x").unwrap();
        // Readable but not searchable: its names list, but none can be examined.
        set_mode(&guides.join("opaque"), 0o400);
        let mut reader = reader(scratch.path(), &[scope("recursive-read:docs/guides")]);
        let found = reader.serve(&search_request("docs/guides", "TODO", None));
        set_mode(&guides.join("opaque"), 0o755);
        let found = found.expect("a search").to_json();
        assert_eq!(
            found["matches"],
            json!([{"path": "docs/guides/a.md", "line": 1, "text": "TODO a"}])
        );
        assert_ne!(found["skipped"], 0);
        assert_eq!(found["complete"], false);
    }

    #[test]
    fn a_search_whose_directory_cannot_be_listed_is_refused() {
        if privileged() {
            eprintln!("skipped: permission bits do not bind a privileged process");
            return;
        }
        let scratch = scratch();
        let guides = scratch.path().join("root/docs/guides");
        fs::create_dir_all(&guides).unwrap();
        fs::write(guides.join("a.md"), "TODO a").unwrap();
        set_mode(&guides, 0o000);
        let mut reader = reader(scratch.path(), &[scope("recursive-read:docs/guides")]);
        let refused = reader.serve(&search_request("docs/guides", "TODO", None));
        set_mode(&guides, 0o755);
        assert_eq!(refused.err(), Some(Refusal::ResourceUnavailable));
    }
}
