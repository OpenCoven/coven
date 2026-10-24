//! The scheduler's fair batch: which pending proposals one pass visits, in what
//! order, and how the cursor that rotates passes through a backlog persists.
use super::*;

const LIMIT: usize = crate::proposal_store::PROPOSAL_SCHEDULER_BATCH_LIMIT;

fn proposal(index: usize) -> String {
    format!("p{index:03}.json")
}

fn home_with(names: &[String]) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    let pending = home.path().join("pending");
    std::fs::create_dir_all(&pending).unwrap();
    for name in names {
        std::fs::write(pending.join(name), b"{}").unwrap();
    }
    home
}

fn batch(home: &tempfile::TempDir) -> Vec<String> {
    scheduler_candidate_batch(home.path())
        .unwrap()
        .into_iter()
        .map(|(name, path)| {
            assert_eq!(path, home.path().join("pending").join(&name));
            name
        })
        .collect()
}

fn cursor_file(home: &tempfile::TempDir) -> std::path::PathBuf {
    home.path().join("pending").join(".scheduler-cursor")
}

#[test]
fn scheduler_batch_is_empty_without_a_pending_directory() {
    let home = tempfile::tempdir().unwrap();
    assert!(scheduler_candidate_batch(home.path()).unwrap().is_empty());
}

#[test]
fn scheduler_batch_refuses_a_pending_store_that_is_not_a_directory() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("pending"), b"").unwrap();
    let error = scheduler_candidate_batch(home.path()).unwrap_err();
    assert!(
        format!("{error:#}").contains("pending proposal store is not a directory"),
        "{error:#}"
    );
}

#[test]
fn scheduler_batch_visits_only_active_proposals_in_name_order() {
    let home = home_with(&[
        "b.json".into(),
        "c.json.reject.deciding".into(),
        "a.json.approve.deciding".into(),
        "notes.txt".into(),
        "d.json.tmp".into(),
        "e.json.approve.decided".into(),
    ]);
    assert_eq!(
        batch(&home),
        [
            "a.json.approve.deciding",
            "b.json",
            "c.json.reject.deciding"
        ]
    );
}

#[test]
fn scheduler_batch_is_bounded_to_the_smallest_names() {
    let home = home_with(&(0..LIMIT + 5).rev().map(proposal).collect::<Vec<_>>());
    assert_eq!(batch(&home), (0..LIMIT).map(proposal).collect::<Vec<_>>());
}

#[test]
fn scheduler_batch_resumes_after_the_cursor_and_wraps_to_the_start() {
    let home = home_with(&(0..LIMIT + 4).map(proposal).collect::<Vec<_>>());
    write_scheduler_cursor(home.path(), &proposal(LIMIT)).unwrap();
    // Names strictly after the cursor come first; the cursor's own name and
    // everything before it fill the rest of the batch from the start.
    let mut expected: Vec<_> = (LIMIT + 1..LIMIT + 4).map(proposal).collect();
    expected.extend((0..LIMIT - 3).map(proposal));
    assert_eq!(batch(&home), expected);

    write_scheduler_cursor(home.path(), "z.json").unwrap();
    assert_eq!(batch(&home), (0..LIMIT).map(proposal).collect::<Vec<_>>());
}

#[test]
fn scheduler_cursor_ignores_unsafe_or_inactive_content() {
    let home = home_with(&[proposal(0), proposal(1)]);
    for unusable in [
        "",
        " \n",
        "../p000.json",
        "dir/p000.json",
        "dir\\p000.json",
        "p000.txt",
    ] {
        std::fs::write(cursor_file(&home), unusable).unwrap();
        assert_eq!(read_scheduler_cursor(home.path()), None, "{unusable:?}");
        assert_eq!(batch(&home), [proposal(0), proposal(1)], "{unusable:?}");
    }
    std::fs::write(cursor_file(&home), " p000.json\n").unwrap();
    assert_eq!(
        read_scheduler_cursor(home.path()).as_deref(),
        Some("p000.json")
    );
    assert_eq!(batch(&home), [proposal(1), proposal(0)]);
}

#[test]
fn scheduler_cursor_is_replaced_atomically_without_staged_leftovers() {
    let home = tempfile::tempdir().unwrap();
    write_scheduler_cursor(home.path(), "p001.json").unwrap();
    write_scheduler_cursor(home.path(), "p002.json").unwrap();
    assert_eq!(
        std::fs::read_to_string(home.path().join("pending/.scheduler-cursor")).unwrap(),
        "p002.json"
    );
    let leftovers: Vec<_> = std::fs::read_dir(home.path().join("pending"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.ends_with(".staged"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}
