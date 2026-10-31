//! Tests for the crash-left launch-folder reaper (coven#1607). Every test
//! works beneath its own scratch folder standing in for the system's
//! temporary folder; none touches the real one.

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::PathBuf;
use std::time::Duration;

use super::*;
use crate::automations::scoped_read::admission::IsolatedLaunch;

const FULL: Budget = Budget {
    folders: 64,
    entries: 1_000_000,
    time: Duration::from_secs(60),
};

/// A scratch temporary folder, a Coven home beside it, and a canary folder
/// whose contents no sweep may touch.
struct Scene {
    _scratch: tempfile::TempDir,
    base: PathBuf,
    home: PathBuf,
    canary: PathBuf,
}

impl Scene {
    fn new() -> Self {
        let scratch = tempfile::Builder::new()
            .prefix("reaper-")
            .tempdir_in("/tmp")
            .unwrap();
        let base = scratch.path().join("tmp");
        let home = scratch.path().join("coven");
        let canary = scratch.path().join("owner-files");
        for folder in [&base, &home, &canary] {
            fs::create_dir(folder).unwrap();
        }
        fs::write(canary.join("keep.txt"), "the owner's file").unwrap();
        fs::create_dir(canary.join("nested")).unwrap();
        fs::write(canary.join("nested/keep.txt"), "also kept").unwrap();
        Self {
            _scratch: scratch,
            base,
            home,
            canary,
        }
    }

    /// A launch folder whose daemon crashed: its harness home holds a tree,
    /// a symlink to the canary folder, a symlink to a canary file, and a hard
    /// link to a canary file.
    fn crashed_launch(&self) -> PathBuf {
        let launch = IsolatedLaunch::create_in(&self.base, &self.home).unwrap();
        let home = launch.home();
        fs::create_dir_all(home.join(".claude/projects/p")).unwrap();
        fs::write(home.join(".claude/projects/p/transcript.jsonl"), "secret").unwrap();
        fs::write(home.join(".claude.json"), "{}").unwrap();
        symlink(&self.canary, home.join("link-to-folder")).unwrap();
        symlink(self.canary.join("keep.txt"), home.join("link-to-file")).unwrap();
        fs::hard_link(self.canary.join("keep.txt"), home.join("hard-link")).unwrap();
        fs::write(launch.cwd().join("scratch.txt"), "work").unwrap();
        launch.abandon()
    }

    fn reap(&self, budget: Budget) -> Report {
        settled(|| reap_stale_in(&self.base, &self.home, budget, &mut |_| {}).unwrap())
    }

    fn assert_canary_intact(&self) {
        assert_eq!(
            fs::read_to_string(self.canary.join("keep.txt")).unwrap(),
            "the owner's file"
        );
        assert_eq!(
            fs::read_to_string(self.canary.join("nested/keep.txt")).unwrap(),
            "also kept"
        );
    }

    fn entries(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.base)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn prefix(&self) -> String {
        folder_prefix(&home_digest(&self.home))
    }
}

/// Runs `sweep` again while it sees a crash-left folder as live. Tests run in
/// parallel, and another test's `fork` duplicates every open descriptor until
/// its child calls `exec`, so a marker just released can read as locked for
/// that moment. The reaper then retains the folder, which is the safe
/// direction; the test waits that moment out.
fn settled(mut sweep: impl FnMut() -> Report) -> Report {
    for _ in 0..200 {
        let report = sweep();
        if report.live == 0 {
            return report;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    sweep()
}

#[test]
fn a_crash_left_folder_is_removed_without_following_links() {
    let scene = Scene::new();
    let folder = scene.crashed_launch();
    assert!(folder.join(MARKER).is_file());
    let report = scene.reap(FULL);
    assert_eq!(
        report,
        Report {
            removed: 1,
            ..Report::default()
        }
    );
    assert!(!folder.exists());
    assert!(scene.entries().is_empty());
    scene.assert_canary_intact();
}

#[test]
fn a_live_launch_is_retained() {
    let scene = Scene::new();
    let live = IsolatedLaunch::create_in(&scene.base, &scene.home).unwrap();
    fs::write(live.home().join("note"), "in use").unwrap();
    let report = reap_stale_in(&scene.base, &scene.home, FULL, &mut |_| {}).unwrap();
    assert_eq!(
        report,
        Report {
            live: 1,
            ..Report::default()
        }
    );
    assert_eq!(
        fs::read_to_string(live.home().join("note")).unwrap(),
        "in use"
    );
    // The live launch still removes its own folder when it ends.
    let root = live.root().to_path_buf();
    drop(live);
    assert!(!root.exists());
}

#[test]
fn unattributed_entries_are_retained() {
    let scene = Scene::new();
    let prefix = scene.prefix();
    let digest = home_digest(&scene.home);

    // No marker, as a crash before the marker was written leaves it.
    let bare = scene.base.join(format!("{prefix}bare"));
    fs::create_dir(&bare).unwrap();
    fs::set_permissions(&bare, fs::Permissions::from_mode(0o700)).unwrap();
    // A marker naming another home.
    let other = scene.base.join(format!("{prefix}other"));
    fs::create_dir(&other).unwrap();
    fs::set_permissions(&other, fs::Permissions::from_mode(0o700)).unwrap();
    drop(mark(&other, &"0".repeat(64)).unwrap());
    fs::write(other.join("data"), "kept").unwrap();
    // Open to group and others.
    let open = scene.base.join(format!("{prefix}open"));
    fs::create_dir(&open).unwrap();
    fs::set_permissions(&open, fs::Permissions::from_mode(0o755)).unwrap();
    drop(mark(&open, &digest).unwrap());
    // A symlink with our name, pointing at a folder with a valid marker.
    let decoy = scene.canary.join("decoy");
    fs::create_dir(&decoy).unwrap();
    fs::set_permissions(&decoy, fs::Permissions::from_mode(0o700)).unwrap();
    drop(mark(&decoy, &digest).unwrap());
    fs::write(decoy.join("data"), "kept").unwrap();
    symlink(&decoy, scene.base.join(format!("{prefix}link"))).unwrap();
    // A regular file with our name.
    fs::write(scene.base.join(format!("{prefix}file")), "kept").unwrap();
    // A marker that is a symlink to a valid marker.
    let linked = scene.base.join(format!("{prefix}linked-marker"));
    fs::create_dir(&linked).unwrap();
    fs::set_permissions(&linked, fs::Permissions::from_mode(0o700)).unwrap();
    symlink(decoy.join(MARKER), linked.join(MARKER)).unwrap();
    // Not our prefix at all, with a valid marker.
    let foreign = scene.base.join("coven-launch-unrelated");
    fs::create_dir(&foreign).unwrap();
    fs::set_permissions(&foreign, fs::Permissions::from_mode(0o700)).unwrap();
    drop(mark(&foreign, &digest).unwrap());

    let before = scene.entries();
    let report = scene.reap(FULL);
    assert_eq!(
        report,
        Report {
            unattributed: 6,
            ..Report::default()
        }
    );
    assert_eq!(scene.entries(), before);
    assert_eq!(fs::read_to_string(other.join("data")).unwrap(), "kept");
    assert_eq!(fs::read_to_string(decoy.join("data")).unwrap(), "kept");
    assert!(decoy.join(MARKER).is_file());
    scene.assert_canary_intact();
}

#[test]
fn another_homes_folders_are_not_considered() {
    let scene = Scene::new();
    let other_home = scene.home.with_file_name("other-coven");
    fs::create_dir(&other_home).unwrap();
    let theirs = IsolatedLaunch::create_in(&scene.base, &other_home)
        .unwrap()
        .abandon();
    assert_eq!(scene.reap(FULL), Report::default());
    assert!(theirs.join(MARKER).is_file());
    // Their own daemon does remove it.
    let report = settled(|| reap_stale_in(&scene.base, &other_home, FULL, &mut |_| {}).unwrap());
    assert_eq!(report.removed, 1);
    assert!(!theirs.exists());
}

#[test]
fn an_exhausted_budget_resumes_and_repeated_sweeps_are_idempotent() {
    let scene = Scene::new();
    let folder = scene.crashed_launch();
    let small = Budget { entries: 2, ..FULL };
    let first = scene.reap(small);
    assert_eq!(first.incomplete, 1);
    assert!(first.budget_exhausted);
    assert!(
        folder.join(MARKER).is_file(),
        "the marker stays until the end"
    );

    let second = scene.reap(FULL);
    assert_eq!(second.removed, 1);
    assert!(!folder.exists());
    assert_eq!(scene.reap(FULL), Report::default());
    scene.assert_canary_intact();

    // The folder budget stops before examining more folders.
    let a = scene.crashed_launch();
    let b = scene.crashed_launch();
    let one = Budget { folders: 1, ..FULL };
    let report = scene.reap(one);
    assert_eq!(report.removed, 1);
    assert!(report.budget_exhausted);
    assert_eq!(usize::from(a.exists()) + usize::from(b.exists()), 1);
    assert_eq!(scene.reap(FULL).removed, 1);
    assert!(scene.entries().is_empty());
}

#[test]
fn a_permission_failure_is_left_without_widening_access() {
    // SAFETY: geteuid() only reads the calling process's effective uid.
    if unsafe { libc::geteuid() } == 0 {
        // Root ignores directory modes, so the failure cannot be staged.
        return;
    }
    let scene = Scene::new();
    let folder = scene.crashed_launch();
    let sealed = folder.join("home/sealed");
    fs::create_dir(&sealed).unwrap();
    fs::write(sealed.join("inside"), "x").unwrap();
    fs::set_permissions(&sealed, fs::Permissions::from_mode(0o000)).unwrap();

    let report = scene.reap(FULL);
    assert_eq!(report.failed, 1);
    assert_eq!(report.removed, 0);
    assert!(
        folder.join(MARKER).is_file(),
        "a failed folder keeps its marker"
    );
    assert_eq!(
        fs::metadata(&sealed).unwrap().permissions().mode() & 0o777,
        0,
        "the sweep never changes a mode"
    );
    scene.assert_canary_intact();

    // Once the owner restores access, the next sweep finishes it.
    fs::set_permissions(&sealed, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(scene.reap(FULL).removed, 1);
    assert!(!folder.exists());
}

#[test]
fn a_folder_replaced_after_verification_cannot_redirect_removal() {
    // The adversarial order coven#924 describes: after the folder is
    // verified and locked, a same-user process renames it away and puts an
    // unrelated folder with content under its name. Removal goes through the
    // handle verified first, so the replacement is untouched and the final
    // `rmdir` refuses the name.
    let scene = Scene::new();
    let folder = scene.crashed_launch();
    let moved = scene.canary.join("moved-away");
    let mut replaced = false;
    let report = settled(|| {
        reap_stale_in(&scene.base, &scene.home, FULL, &mut |name| {
            let path = scene.base.join(name);
            fs::rename(&path, &moved).unwrap();
            fs::create_dir(&path).unwrap();
            fs::write(path.join("unrelated.txt"), "not the launch's").unwrap();
            replaced = true;
        })
        .unwrap()
    });
    assert!(replaced);
    assert_eq!(report.failed, 1);
    assert_eq!(report.removed, 0);
    assert_eq!(
        fs::read_to_string(folder.join("unrelated.txt")).unwrap(),
        "not the launch's"
    );
    // The verified folder, wherever it went, was emptied; nothing it linked
    // to was touched.
    assert_eq!(fs::read_dir(&moved).unwrap().count(), 0);
    scene.assert_canary_intact();
}

#[test]
fn a_new_launch_folder_is_named_marked_and_locked() {
    let scene = Scene::new();
    let launch = IsolatedLaunch::create_in(&scene.base, &scene.home).unwrap();
    let name = launch
        .root()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(name.starts_with(&scene.prefix()), "{name}");
    let marker = launch.root().join(MARKER);
    assert_eq!(
        fs::read_to_string(&marker).unwrap(),
        format!("coven-scoped-launch/1\n{}\n", home_digest(&scene.home))
    );
    assert_eq!(
        fs::metadata(&marker).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let other = fs::File::open(&marker).unwrap();
    assert_eq!(
        rustix::fs::flock(&other, FlockOperation::NonBlockingLockExclusive),
        Err(Errno::WOULDBLOCK),
        "the launch holds the marker's lock"
    );
}

#[test]
fn the_report_carries_counts_only() {
    let report = Report {
        removed: 2,
        live: 1,
        unattributed: 3,
        incomplete: 0,
        failed: 1,
        budget_exhausted: true,
    };
    assert_eq!(
        report.to_string(),
        "scoped launch folders: removed=2 live=1 unattributed=3 incomplete=0 failed=1 budget_exhausted"
    );
    assert!(!report.is_quiet());
    assert!(Report::default().is_quiet());
}
