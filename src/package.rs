//! Portable project packaging (PROMPT.md §48).
//!
//! A shareable project is a directory: the project file plus every file it
//! references by relative path (firmware) plus the conventional `dbc/` and
//! `assets/` companions when present. [`package_project`] copies exactly
//! that closure into a fresh output directory, preserving relative layout
//! so the packaged project validates and simulates from its new home.
//!
//! Rules (explicit, §48 + §21):
//!
//! - The project must validate first; invalid projects never package.
//! - A firmware file missing for a node that actually executes it
//!   (`backend: renode`) is a hard error naming the node: such a package
//!   could never run. Virtual nodes merely reference firmware, so a
//!   missing file is skipped with a printed warning — matching
//!   `validate`/`simulate`, which treat it the same way.
//! - Nothing executes, nothing is transformed: byte copies plus the
//!   project file rewritten from the parsed model (normalizes formatting,
//!   never content).
//! - Refusing to overwrite: the output directory must not exist.

use crate::project::{validate_project, Project};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Actionable packaging errors (§68).
#[derive(Debug, Error)]
pub enum PackageError {
    #[error("cannot package {path}: {reasons}")]
    InvalidProject { path: String, reasons: String },
    #[error("node \"{node}\": firmware \"{firmware}\" not found — packages must be complete (build it first, then repackage)")]
    MissingFirmware { node: String, firmware: String },
    #[error("output directory {path} already exists — remove it or pick another --out")]
    OutputExists { path: String },
    #[error("I/O error while packaging: {0}")]
    Io(String),
}

/// What one `package_project` run produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageReport {
    /// Files copied, relative to the output directory (project file last).
    pub files: Vec<String>,
}

/// Copy the project closure into `out_dir` (which must not exist).
pub fn package_project(project_path: &Path, out_dir: &Path) -> Result<PackageReport, PackageError> {
    if out_dir.exists() {
        return Err(PackageError::OutputExists {
            path: out_dir.display().to_string(),
        });
    }
    let proj = Project::load_from_file(project_path).map_err(|e| PackageError::InvalidProject {
        path: project_path.display().to_string(),
        reasons: e.to_string(),
    })?;
    let base = project_path.parent().unwrap_or(Path::new("."));
    let rep = validate_project(&proj, base);
    if !rep.is_ok() {
        let reasons = rep
            .errors
            .iter()
            .map(|e| format!("[{}] {}", e.path, e.message))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(PackageError::InvalidProject {
            path: project_path.display().to_string(),
            reasons,
        });
    }

    let mut files = Vec::new();
    // Pre-check firmware for nodes that execute it, before copying
    // anything, so a fatal miss leaves no partial output behind. Virtual
    // nodes only reference firmware — skip those files with a warning.
    for n in &proj.nodes {
        if let Some(fw) = &n.firmware {
            if n.backend != "virtual" && !base.join(fw).is_file() {
                return Err(PackageError::MissingFirmware {
                    node: n.id.clone(),
                    firmware: fw.clone(),
                });
            }
        }
    }
    for n in &proj.nodes {
        if let Some(fw) = &n.firmware {
            let rel = clean_relative(fw);
            if !base.join(&rel).is_file() {
                eprintln!(
                    "warning: node \"{}\" references missing firmware \"{fw}\" — skipped (virtual nodes never execute it)",
                    n.id
                );
                continue;
            }
            let dst = out_dir.join(&rel);
            copy_file(&base.join(&rel), &dst)?;
            files.push(rel.display().to_string());
        }
    }
    // Conventional companions travel when present (never required).
    for companion in ["dbc", "assets"] {
        let src = base.join(companion);
        if src.is_dir() {
            for entry in walk_files(&src) {
                let rel = entry
                    .strip_prefix(base)
                    .expect("entry under base")
                    .to_path_buf();
                copy_file(&entry, &out_dir.join(&rel))?;
                files.push(rel.display().to_string());
            }
        }
    }
    // Project file last: presence of every expected file is signaled by a
    // complete report, and the file itself round-trips through the parser.
    let yaml = proj
        .to_yaml()
        .map_err(|e| PackageError::Io(e.to_string()))?;
    let name = project_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project.canlab".into());
    let dst = out_dir.join(&name);
    std::fs::create_dir_all(dst.parent().expect("file has parent"))
        .map_err(|e| PackageError::Io(e.to_string()))?;
    std::fs::write(&dst, yaml).map_err(|e| PackageError::Io(e.to_string()))?;
    files.push(name);

    Ok(PackageReport { files })
}

/// Collapse lexical `.` from a project-relative reference (`..` is rejected
/// earlier by validation, so only `.` needs handling) for clean reports.
fn clean_relative(rel: &str) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in Path::new(rel).components() {
        if c == Component::CurDir {
            continue;
        }
        out.push(c);
    }
    out
}

fn copy_file(src: &Path, dst: &Path) -> Result<(), PackageError> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| PackageError::Io(e.to_string()))?;
    }
    std::fs::copy(src, dst).map_err(|e| PackageError::Io(e.to_string()))?;
    Ok(())
}

/// All files under `dir`, recursively, sorted for deterministic reports.
fn walk_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&d)
            .map(|r| r.filter_map(|e| e.ok()).map(|e| e.path()).collect())
            .unwrap_or_default();
        entries.sort();
        for e in entries {
            if e.is_dir() {
                stack.push(e);
            } else if e.is_file() {
                out.push(e);
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, content: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn project(dir: &Path) -> PathBuf {
        write(&dir.join("firmware/ecu.elf"), b"\x7fELF-fake");
        write(&dir.join("dbc/vehicle.dbc"), b"VERSION \"x\"\n");
        let p = dir.join("project.canlab");
        write(
            &p,
            b"version: 1\nsimulation: {mode: deterministic}\nbuses:\n  - {id: b, type: can, bitrate: 500000, fd: false}\nnodes:\n  - {id: n1, device: stm32f103, backend: virtual, firmware: ./firmware/ecu.elf, can: {bus: b}}\n  - {id: n2, device: arduino_uno, backend: virtual, can: {bus: b}}\nmessages:\n  - {sender: n1, id: 0x100, data: [1]}\n",
        );
        p
    }

    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("canlab-pkg-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn packages_closure_and_revalidates_from_new_home() {
        let dir = tmp("ok");
        let src = project(&dir.join("src"));
        let out = dir.join("pkg");
        let rep = package_project(&src, &out).unwrap();
        assert_eq!(
            rep.files,
            vec!["firmware/ecu.elf", "dbc/vehicle.dbc", "project.canlab"]
        );
        // Byte-identical firmware, and the packaged project validates with
        // the output dir as its new base.
        assert_eq!(
            std::fs::read(out.join("firmware/ecu.elf")).unwrap(),
            b"\x7fELF-fake"
        );
        let back = Project::load_from_file(&out.join("project.canlab")).unwrap();
        assert!(validate_project(&back, &out).is_ok());
        // Scripted traffic survived the round trip.
        assert_eq!(back.messages.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn virtual_nodes_skip_missing_firmware_renode_nodes_fail() {
        let dir = tmp("lenient");
        let src = project(&dir.join("src"));
        // Virtual nodes merely reference firmware: the dangling file is
        // skipped with success, not failure.
        std::fs::remove_file(dir.join("src/firmware/ecu.elf")).unwrap();
        let rep = package_project(&src, &dir.join("pkg")).unwrap();
        assert_eq!(rep.files, vec!["dbc/vehicle.dbc", "project.canlab"]);
        assert!(!dir.join("pkg").join("firmware/ecu.elf").exists());
        // The packaged tree still validates from its new home.
        let back = Project::load_from_file(&dir.join("pkg/project.canlab")).unwrap();
        assert!(validate_project(&back, &dir.join("pkg")).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_missing_firmware_invalid_and_existing_out() {
        let dir = tmp("rej");
        let src = project(&dir.join("src"));
        // A renode node truly executes firmware: dangling file is fatal,
        // with no partial output left behind.
        let renode_yaml = std::fs::read_to_string(&src).unwrap().replace(
            "- {id: n1, device: stm32f103, backend: virtual, firmware: ./firmware/ecu.elf, can: {bus: b}}",
            "- {id: n1, device: stm32f103, backend: renode, firmware: ./firmware/ecu.elf, can: {bus: b}}",
        );
        std::fs::write(&src, renode_yaml).unwrap();
        std::fs::remove_file(dir.join("src/firmware/ecu.elf")).unwrap();
        let err = package_project(&src, &dir.join("pkg")).unwrap_err();
        assert!(matches!(err, PackageError::MissingFirmware { .. }), "{err}");
        assert!(
            !dir.join("pkg").exists(),
            "no partial output on pre-check failure"
        );
        // Invalid projects never package.
        std::fs::write(
            &src,
            b"version: 1\nsimulation: {mode: nope}\nbuses: []\nnodes: []\n",
        )
        .unwrap();
        assert!(matches!(
            package_project(&src, &dir.join("pkg2")).unwrap_err(),
            PackageError::InvalidProject { .. }
        ));
        // Never overwrite.
        std::fs::create_dir_all(dir.join("taken")).unwrap();
        assert!(matches!(
            package_project(&src, &dir.join("taken")).unwrap_err(),
            PackageError::OutputExists { .. }
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
