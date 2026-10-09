//! Safe startup migration for the first Windows Desktop data layout.

use std::fmt;
use std::path::{Path, PathBuf};

/// What a successful migration copied, and what an existing destination kept.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MigrationReport {
    pub copied_files: usize,
    pub skipped_files: usize,
}

/// Why legacy data could not be inspected or copied.
#[derive(Debug)]
pub struct MigrationError {
    pub path: PathBuf,
    pub reason: std::io::Error,
}

impl fmt::Display for MigrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "legacy data migration failed at {}: {}",
            self.path.display(),
            self.reason
        )
    }
}

impl std::error::Error for MigrationError {}

fn migration_error(path: &Path, reason: std::io::Error) -> MigrationError {
    MigrationError {
        path: path.to_path_buf(),
        reason,
    }
}

/// Copy an old Desktop data tree into an empty new root, if migration is safe.
///
/// `Ok(None)` means "nothing to do": either the old tree does not exist or the
/// new tree already owns data. The old tree is only read, and an existing
/// destination file is skipped rather than replaced.
pub fn migrate_windows_legacy_data(
    old: &Path,
    new: &Path,
) -> Result<Option<MigrationReport>, MigrationError> {
    if !old.exists() {
        return Ok(None);
    }

    let new_was_empty = match std::fs::read_dir(new) {
        Ok(mut entries) => entries.next().is_none(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => return Err(migration_error(new, error)),
    };
    if !new_was_empty {
        return Ok(None);
    }

    std::fs::create_dir_all(new).map_err(|error| migration_error(new, error))?;
    let mut report = MigrationReport {
        copied_files: 0,
        skipped_files: 0,
    };
    copy_tree_if_absent(old, new, &mut report)?;
    Ok(Some(report))
}

pub(crate) fn copy_tree_if_absent(
    source: &Path,
    destination: &Path,
    report: &mut MigrationReport,
) -> Result<(), MigrationError> {
    let entries = std::fs::read_dir(source)
        .map_err(|error| migration_error(source, error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| migration_error(source, error))?;

    for entry in entries {
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|error| migration_error(&source_path, error))?;

        if file_type.is_dir() {
            if destination_path.exists() {
                report.skipped_files += 1;
                continue;
            }
            std::fs::create_dir(&destination_path)
                .map_err(|error| migration_error(&destination_path, error))?;
            copy_tree_if_absent(&source_path, &destination_path, report)?;
        } else if destination_path.exists() || !file_type.is_file() {
            report.skipped_files += 1;
        } else {
            std::fs::copy(&source_path, &destination_path)
                .map_err(|error| migration_error(&destination_path, error))?;
            report.copied_files += 1;
        }
    }
    Ok(())
}
