//! What must hold after every interruption of a mixed restore (#1680), however
//! the interruption was produced: a killed child, a returned error or an
//! explicitly reconstructed retained image.
//!
//! The fence is read from the raw journal bytes, not through the installer's
//! reader. Until this operation's journal is retained the destination is the
//! untouched old installation. From then on nothing is usable, no retained
//! floor has moved down, and only the original operation with the original
//! signed source may continue: a different operation and a different authentic
//! export are both refused without changing a byte.
use super::super::super::mixed::{write_files, Files};
use super::*;

/// How far the interrupted operation got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Fence {
    /// This operation's journal is not retained.
    Absent,
    Installing,
    AwaitingHandovers,
}

pub(super) fn fence(files: &Files, operation: Uuid) -> Fence {
    let Some(journal) = files.get(JOURNAL).map(|bytes| json(bytes)) else {
        return Fence::Absent;
    };
    if journal["operationId"] != operation.to_string() {
        return Fence::Absent;
    }
    match journal["phase"].as_str() {
        Some("installing") => Fence::Installing,
        Some("awaitingHandovers") => Fence::AwaitingHandovers,
        other => panic!("unknown restore journal phase {other:?}"),
    }
}

/// Private staging files are never authority, whatever they hold.
pub(super) fn is_staging(name: &str) -> bool {
    name.starts_with(".mobile-state-") && name.ends_with(".tmp")
}

/// The canonical authority namespace: no advisory locks and no staging files.
pub(super) fn canonical(files: &Files) -> Files {
    authority_files(files)
        .into_iter()
        .filter(|(name, _)| !is_staging(name))
        .collect()
}

/// The canonical files of a home whose mobile directory may not exist yet.
pub(super) fn image(home: &Path) -> Files {
    if home.join("mobile").exists() {
        canonical(&read_files(home))
    } else {
        Files::new()
    }
}

/// Refusal classes for a different operation arriving while this one is
/// installing. Which one fires depends on how much identity was copied.
const OTHER_OPERATION_WHILE_INSTALLING: [&str; 2] = [
    "finish the existing restore operation before starting another",
    "partial destination identity requires the original restore operation",
];

pub(super) struct Contract<'a> {
    lineage: &'a Lineage,
    sources: &'a Sources<'a>,
    label: String,
    source: tempfile::TempDir,
    source_before: Files,
    authority_before: Vec<u8>,
    /// A second authentic export of the same lineage point: the same
    /// authority in different signed bytes.
    alternate: std::path::PathBuf,
    _alternate: tempfile::TempDir,
    /// The retained destination's canonical files; empty for an empty home.
    original: Files,
    floors: Option<Known>,
    expected: BTreeMap<Uuid, (u64, u64)>,
}

impl<'a> Contract<'a> {
    pub(super) fn new(lineage: &'a Lineage, sources: &'a Sources<'a>) -> Self {
        let source = selected(sources.snapshot);
        let exporter = tempfile::tempdir().unwrap();
        write_files(exporter.path(), &sources.authority.files);
        let backup = tempfile::tempdir().unwrap();
        let alternate = ensure_private_mobile_dir(backup.path())
            .unwrap()
            .join("alternate.json");
        export::export_authority(exporter.path(), &alternate).unwrap();
        let authority_before = fs::read(&sources.authority.authority).unwrap();
        assert!(
            fs::read(&alternate).unwrap() != authority_before,
            "a second export must differ in its signed bytes"
        );
        Self {
            lineage,
            sources,
            label: sources.label(),
            source_before: read_files(source.path()),
            source,
            authority_before,
            alternate,
            _alternate: backup,
            original: sources
                .destination
                .map(|point| canonical(&point.files))
                .unwrap_or_default(),
            floors: sources.destination.map(|point| Known::image(&point.files)),
            expected: sources.expected().1,
        }
    }
    pub(super) fn label(&self) -> &str {
        &self.label
    }
    pub(super) fn source(&self) -> &Path {
        self.source.path()
    }
    pub(super) fn authority(&self) -> &Path {
        &self.sources.authority.authority
    }
    pub(super) fn devices(&self) -> impl Iterator<Item = &Uuid> {
        self.expected.keys()
    }

    /// Checks the interrupted destination before any retry.
    pub(super) fn interrupted(&self, home: &Path, operation: Uuid, at: &str) -> Fence {
        let label = &self.label;
        let before = image(home);
        let fence = fence(&before, operation);
        if fence == Fence::Absent {
            // Before the fence the retained installation is untouched.
            assert!(
                before == self.original,
                "{label} {at}: the destination changed before the fence"
            );
            return fence;
        }

        // This operation's journal is retained: nothing is usable, and
        // nothing the destination retained has moved down.
        let registry = DeviceRegistry::load(home).unwrap();
        for device in self.expected.keys() {
            assert!(
                check_application(home, &registry, *device).is_err(),
                "{label} {at}: {device} usable under the fence"
            );
            if fence == Fence::Installing {
                assert!(
                    prepare_direct(home, &registry, &self.lineage.host, *device).is_err(),
                    "{label} {at}: {device} offered a handover while installing"
                );
            }
        }
        if let Some(floors) = &self.floors {
            let now = Known::image(&before);
            assert!(now.installation_epoch >= floors.installation_epoch);
            for (device, generation) in &floors.generations {
                assert!(
                    now.generations[device] >= *generation,
                    "{label} {at}: {device} generation floor lowered"
                );
            }
            for (device, epoch) in &floors.epochs {
                assert!(
                    now.epochs[device] >= *epoch,
                    "{label} {at}: {device} epoch floor lowered"
                );
            }
        }

        // A different operation is refused.
        let error = install_from_snapshot(
            home,
            self.source.path(),
            self.authority(),
            self.lineage.pin,
            Uuid::new_v4(),
        )
        .map(|_| ())
        .expect_err(&format!("{label} {at}: a different operation was admitted"));
        let error = format!("{error:#}");
        match fence {
            Fence::Installing => assert!(
                OTHER_OPERATION_WHILE_INSTALLING
                    .iter()
                    .any(|class| error.contains(class)),
                "{label} {at}: unexpected refusal of a different operation: {error}"
            ),
            // The completed operation advanced every epoch, so its own source
            // is now older authority for anyone else.
            _ => assert!(
                error.contains("predates"),
                "{label} {at}: unexpected refusal after completion: {error}"
            ),
        }

        // The original operation with a different authentic source is refused.
        let error = install_from_snapshot(
            home,
            self.source.path(),
            &self.alternate,
            self.lineage.pin,
            operation,
        )
        .map(|_| ())
        .expect_err(&format!("{label} {at}: a different source was admitted"));
        let error = format!("{error:#}");
        assert!(
            error.contains("restore retry requires the exact original authority source"),
            "{label} {at}: unexpected refusal of a different source: {error}"
        );

        assert!(
            image(home) == before,
            "{label} {at}: a refused attempt changed the destination"
        );
        fence
    }

    /// Retries the original operation with the original signed source, then
    /// takes every usable pairing through its own fresh direct handover.
    pub(super) fn retry(&self, home: &Path, operation: Uuid, at: &str) {
        let label = &self.label;
        let state = install_from_snapshot(
            home,
            self.source.path(),
            self.authority(),
            self.lineage.pin,
            operation,
        )
        .unwrap_or_else(|e| panic!("{label} {at}: {e:#}"));
        assert_eq!(state.operation_id, operation, "{label} {at}");
        verify_installed(self.lineage, self.sources, home, &state);
        assert!(
            read_files(self.source.path()) == self.source_before,
            "{label} {at}: the selected snapshot changed"
        );
        assert!(
            fs::read(self.authority()).unwrap() == self.authority_before,
            "{label} {at}: the signed authority changed"
        );
    }
}
