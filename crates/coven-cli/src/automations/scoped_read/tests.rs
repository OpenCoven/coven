//! The scoped read vectors (`vectors.json`), and the budget, limit and mount
//! cases a vector file cannot express.

use serde_json::{json, Value};

use super::scope::{Grants, ReadRequest, RelativePath};
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
        let got = outcome(
            ReadRequest::parse(&case["request"], &R0_READ_POLICY)
                .and_then(|request| grants.authorize(&request)),
            "authorized",
        );
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
fn the_r0_policy_is_the_designs() {
    assert_eq!(R0_READ_POLICY.file_bytes, 1 << 20);
    assert_eq!(R0_READ_POLICY.attempt_bytes, 8 << 20);
    assert_eq!(R0_READ_POLICY.calls, 1024);
    assert_eq!(R0_READ_POLICY.depth, 32);
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

    use super::super::open::{ScopedReader, Step};
    use super::super::scope::RootKind;
    use super::super::{Refusal, R0_READ_POLICY};
    use super::{cases, scope, scopes, text, vectors};
    use crate::automations::contract::canonical_json::sha256_hex;

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
            let content = match entry.get("size").and_then(Value::as_u64) {
                Some(size) => vec![b'a'; size as usize],
                None => text(entry, "text").as_bytes().to_vec(),
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
            "before_file" => Step::BeforeFile,
            "file" => Step::File,
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

            let request = json!({
                "operation": "read_file",
                "root": case.get("requestRoot").cloned().unwrap_or(json!("project")),
                "path": case["request"],
            });
            let race = case.get("race");
            let race_at = race.map(|race| step(text(race, "at")));
            let mut steps = Vec::new();
            let mut fired = false;
            let result = reader.read_with(&request, &mut |reached| {
                steps.push(reached);
                if Some(reached) == race_at && !fired {
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
                (Ok(read), Some(code)) => panic!(
                    "vector {id} read {} bytes instead of refusing {code}",
                    read.content.len()
                ),
                (Ok(read), None) => {
                    assert_eq!(read.digest, sha256_hex(&read.content), "vector {id} digest");
                    assert!(
                        !String::from_utf8_lossy(&read.content).contains("SENTINEL"),
                        "vector {id} disclosed a sentinel"
                    );
                    if let Some(want) = expect.get("text") {
                        assert_eq!(
                            read.content,
                            want.as_str().unwrap().as_bytes(),
                            "vector {id}"
                        );
                    }
                    if let Some(want) = expect.get("length").and_then(Value::as_u64) {
                        assert_eq!(read.content.len() as u64, want, "vector {id} length");
                    }
                }
            }
        }
    }

    fn reader(base: &Path, scopes: &[Value]) -> ScopedReader {
        let root = base.join("root");
        ScopedReader::new(
            scopes,
            &[(RootKind::Project, root.as_path())],
            &[],
            R0_READ_POLICY,
        )
        .unwrap_or_else(|refusal| panic!("binds: {}", refusal.code()))
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
            let _ = reader.read(&read_request(path));
        }
        assert_eq!(
            reader.read(&read_request("notes/today.md")).err(),
            Some(Refusal::LimitCalls)
        );
        assert_eq!(
            reader.read(&json!("not even a request")).err(),
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
            let read = reader
                .read(&read_request("data.txt"))
                .expect("within the total");
            assert_eq!(read.content.len() as u64, R0_READ_POLICY.file_bytes);
        }
        // Refused before any of the file is read.
        let mut steps = Vec::new();
        assert_eq!(
            reader
                .read_with(&read_request("small.txt"), &mut |step| steps.push(step))
                .err(),
            Some(Refusal::LimitAttemptDisclosure)
        );
        assert_eq!(steps.last(), Some(&Step::File), "stopped at {steps:?}");
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
            reader.read(&read_request("dev/null")).err(),
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
}
