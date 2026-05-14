//! `mimeapps.list` discovery, parsing, and writing per the freedesktop "MIME
//! Applications associations" spec. We resolve the full priority-ordered chain
//! of files into a single [`OnDiskAssoc`] for display, and write back **only**
//! to the user's `$XDG_CONFIG_HOME/mimeapps.list` — never to system files.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use eyre::{Context, Result};

use crate::model::{OnDiskAssoc, PendingEdits};

/// Per-desktop variants (e.g. `gnome-mimeapps.list`) take precedence over the
/// plain `mimeapps.list` at the same level. We read both and treat the
/// per-desktop one as higher priority.
fn current_desktops() -> Vec<String> {
    std::env::var("XDG_CURRENT_DESKTOP")
        .or_else(|_| std::env::var("DESKTOP_SESSION"))
        .unwrap_or_default()
        .split(':')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

fn config_home() -> Option<PathBuf> {
    dirs::config_dir()
}

fn config_dirs() -> Vec<PathBuf> {
    std::env::var("XDG_CONFIG_DIRS")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|s| s.split(':').map(PathBuf::from).collect())
        .unwrap_or_else(|| vec![PathBuf::from("/etc/xdg")])
}

fn data_home() -> Option<PathBuf> {
    dirs::data_dir()
}

fn data_dirs() -> Vec<PathBuf> {
    std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|s| s.split(':').map(PathBuf::from).collect())
        .unwrap_or_else(|| {
            vec![
                PathBuf::from("/usr/local/share"),
                PathBuf::from("/usr/share"),
            ]
        })
}

/// The single file mime-tui writes to on `Ctrl+S`. Always
/// `$XDG_CONFIG_HOME/mimeapps.list` — never a per-desktop variant, never a
/// system file.
pub fn user_mimeapps_path() -> Option<PathBuf> {
    config_home().map(|c| c.join("mimeapps.list"))
}

/// All mimeapps.list files we read, in **priority order** (highest first).
pub fn discover_mimeapps_files() -> Vec<PathBuf> {
    let desktops = current_desktops();
    let mut out: Vec<PathBuf> = Vec::new();

    // 1. $XDG_CONFIG_HOME/$desktop-mimeapps.list
    // 2. $XDG_CONFIG_HOME/mimeapps.list
    if let Some(c) = config_home() {
        for d in &desktops {
            out.push(c.join(format!("{}-mimeapps.list", d)));
        }
        out.push(c.join("mimeapps.list"));
    }

    // 3. $XDG_CONFIG_DIRS/$desktop-mimeapps.list
    // 4. $XDG_CONFIG_DIRS/mimeapps.list
    for c in config_dirs() {
        for d in &desktops {
            out.push(c.join(format!("{}-mimeapps.list", d)));
        }
        out.push(c.join("mimeapps.list"));
    }

    // 5. $XDG_DATA_HOME/applications/{,$desktop-}mimeapps.list (deprecated)
    if let Some(d) = data_home() {
        let apps = d.join("applications");
        for desk in &desktops {
            out.push(apps.join(format!("{}-mimeapps.list", desk)));
        }
        out.push(apps.join("mimeapps.list"));
    }

    // 6. $XDG_DATA_DIRS/applications/{,$desktop-}mimeapps.list (deprecated)
    for dir in data_dirs() {
        let apps = dir.join("applications");
        for desk in &desktops {
            out.push(apps.join(format!("{}-mimeapps.list", desk)));
        }
        out.push(apps.join("mimeapps.list"));
    }

    out.into_iter().filter(|p| p.exists()).collect()
}

/// Read every discovered mimeapps.list and resolve into a single [`OnDiskAssoc`].
/// Rules:
/// - For `[Default Applications]`, the highest-priority file mentioning a mime
///   wins (first-found in the priority-ordered list).
/// - For `[Added Associations]` and `[Removed Associations]`, we union across
///   all files. This is a slight simplification of the spec (which interleaves
///   add/remove rules in priority order) but matches what users will recognise
///   from running `xdg-mime query`.
///
/// Provenance: the path of the winning file for each default is recorded in
/// `OnDiskAssoc::default_sources` so the UI can show "from gnome-mimeapps.list"
/// and the save path can warn when a write will be shadowed.
pub fn read_all() -> OnDiskAssoc {
    let mut assoc = OnDiskAssoc::default();
    for path in discover_mimeapps_files() {
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        merge_one(&content, &path, &mut assoc);
    }
    assoc
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    None,
    Default,
    Added,
    Removed,
}

fn merge_one(content: &str, source: &Path, assoc: &mut OnDiskAssoc) {
    let mut section = Section::None;
    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = match line {
                "[Default Applications]" => Section::Default,
                "[Added Associations]" => Section::Added,
                "[Removed Associations]" => Section::Removed,
                _ => Section::None,
            };
            continue;
        }
        if section == Section::None {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let mime = key.trim().to_string();
        let ids: Vec<String> = value
            .split(';')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if mime.is_empty() {
            continue;
        }
        match section {
            Section::Default => {
                if let Some(first) = ids.into_iter().next() {
                    // First file to mention a mime wins (priority order).
                    // Record provenance only when we actually insert, so the
                    // source matches the winning default.
                    if !assoc.defaults.contains_key(&mime) {
                        assoc
                            .default_sources
                            .insert(mime.clone(), source.to_path_buf());
                        assoc.defaults.insert(mime, first);
                    }
                }
            }
            Section::Added => {
                let set: &mut HashSet<String> = assoc.added.entry(mime).or_default();
                for id in ids {
                    set.insert(id);
                }
            }
            Section::Removed => {
                let set: &mut HashSet<String> = assoc.removed.entry(mime).or_default();
                for id in ids {
                    set.insert(id);
                }
            }
            Section::None => {}
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
//                                Save path
// ───────────────────────────────────────────────────────────────────────────

/// In-memory representation of a single `mimeapps.list`, preserving the file's
/// section order and keeping unknown sections verbatim so forward-compatibility
/// isn't a write-time concern.
#[derive(Debug, Default)]
struct UserFile {
    sections: Vec<SectionEntry>,
}

#[derive(Debug)]
enum SectionEntry {
    Default(BTreeMap<String, Vec<String>>),
    Added(BTreeMap<String, Vec<String>>),
    Removed(BTreeMap<String, Vec<String>>),
    Unknown { header: String, lines: Vec<String> },
}

/// Best-effort: refresh `$XDG_DATA_HOME/applications/mimeinfo.cache` so DEs
/// see the change immediately. Called from `App::save` and `App::save_force`
/// after a successful write. Failures are intentionally ignored — the
/// binary isn't always installed, and the user might not have write access
/// to the target.
pub fn run_update_desktop_database() -> Option<()> {
    let apps = dirs::data_dir()?.join("applications");
    if !apps.exists() {
        return None;
    }
    let _ = std::process::Command::new("update-desktop-database")
        .arg(&apps)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    Some(())
}

/// Same as [`save_user_file`] but with an explicit target path — used by tests
/// (both in this module and at the App-integration layer).
pub(crate) fn save_user_file_at(path: &Path, pending: &PendingEdits) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    let existing = fs::read_to_string(path).unwrap_or_default();
    let mut file = parse_user_file(&existing);
    apply_pending(&mut file, pending);
    let new_content = serialize_user_file(&file);

    // Rolling single backup. Best-effort — failure to back up shouldn't block
    // the save.
    if path.exists() {
        let bak = bak_path(path);
        let _ = fs::copy(path, &bak);
    }

    let tmp = tmp_path(path);
    fs::write(&tmp, new_content)
        .with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

fn bak_path(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".bak");
    PathBuf::from(s)
}

fn tmp_path(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".tmp");
    PathBuf::from(s)
}

fn parse_user_file(content: &str) -> UserFile {
    let mut sections: Vec<SectionEntry> = Vec::new();
    let mut current: Option<SectionEntry> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if let Some(s) = current.take() {
                sections.push(s);
            }
            current = Some(match trimmed {
                "[Default Applications]" => SectionEntry::Default(BTreeMap::new()),
                "[Added Associations]" => SectionEntry::Added(BTreeMap::new()),
                "[Removed Associations]" => SectionEntry::Removed(BTreeMap::new()),
                other => SectionEntry::Unknown {
                    header: other.to_string(),
                    lines: Vec::new(),
                },
            });
            continue;
        }
        let Some(cur) = current.as_mut() else {
            // Lines before any section header are dropped — not part of the
            // spec, and mime-tui will produce a clean file anyway.
            continue;
        };
        match cur {
            SectionEntry::Unknown { lines, .. } => lines.push(line.to_string()),
            SectionEntry::Default(map)
            | SectionEntry::Added(map)
            | SectionEntry::Removed(map) => {
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                if let Some((k, v)) = trimmed.split_once('=') {
                    let key = k.trim().to_string();
                    if key.is_empty() {
                        continue;
                    }
                    let vals: Vec<String> = v
                        .split(';')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    map.insert(key, vals);
                }
            }
        }
    }
    if let Some(s) = current {
        sections.push(s);
    }
    UserFile { sections }
}

fn apply_pending(file: &mut UserFile, pending: &PendingEdits) {
    let mut have_default = false;
    let mut have_added = false;
    let mut have_removed = false;
    for s in file.sections.iter_mut() {
        match s {
            SectionEntry::Default(map) => {
                have_default = true;
                for (mime, slot) in &pending.set_default {
                    match slot {
                        Some(id) => {
                            map.insert(mime.clone(), vec![id.clone()]);
                        }
                        None => {
                            map.remove(mime);
                        }
                    }
                }
            }
            SectionEntry::Added(map) => {
                have_added = true;
                for (mime, ids) in &pending.add {
                    let entry = map.entry(mime.clone()).or_default();
                    for id in ids {
                        if !entry.contains(id) {
                            entry.push(id.clone());
                        }
                    }
                }
            }
            SectionEntry::Removed(map) => {
                have_removed = true;
                for (mime, ids) in &pending.remove {
                    let entry = map.entry(mime.clone()).or_default();
                    for id in ids {
                        if !entry.contains(id) {
                            entry.push(id.clone());
                        }
                    }
                }
            }
            SectionEntry::Unknown { .. } => {}
        }
    }

    if !have_default
        && pending.set_default.values().any(|v| v.is_some())
    {
        let mut map = BTreeMap::new();
        for (mime, slot) in &pending.set_default {
            if let Some(id) = slot {
                map.insert(mime.clone(), vec![id.clone()]);
            }
        }
        file.sections.push(SectionEntry::Default(map));
    }
    if !have_added && !pending.add.is_empty() {
        let mut map = BTreeMap::new();
        for (mime, ids) in &pending.add {
            let mut v: Vec<String> = ids.iter().cloned().collect();
            v.sort();
            map.insert(mime.clone(), v);
        }
        file.sections.push(SectionEntry::Added(map));
    }
    if !have_removed && !pending.remove.is_empty() {
        let mut map = BTreeMap::new();
        for (mime, ids) in &pending.remove {
            let mut v: Vec<String> = ids.iter().cloned().collect();
            v.sort();
            map.insert(mime.clone(), v);
        }
        file.sections.push(SectionEntry::Removed(map));
    }
}

fn serialize_user_file(file: &UserFile) -> String {
    let mut out = String::new();
    let mut first = true;
    for s in &file.sections {
        match s {
            SectionEntry::Default(map) => {
                let body = serialize_kv(map);
                if body.is_empty() {
                    continue;
                }
                if !first {
                    out.push('\n');
                }
                out.push_str("[Default Applications]\n");
                out.push_str(&body);
                first = false;
            }
            SectionEntry::Added(map) => {
                let body = serialize_kv(map);
                if body.is_empty() {
                    continue;
                }
                if !first {
                    out.push('\n');
                }
                out.push_str("[Added Associations]\n");
                out.push_str(&body);
                first = false;
            }
            SectionEntry::Removed(map) => {
                let body = serialize_kv(map);
                if body.is_empty() {
                    continue;
                }
                if !first {
                    out.push('\n');
                }
                out.push_str("[Removed Associations]\n");
                out.push_str(&body);
                first = false;
            }
            SectionEntry::Unknown { header, lines } => {
                if !first {
                    out.push('\n');
                }
                out.push_str(header);
                out.push('\n');
                for line in lines {
                    out.push_str(line);
                    out.push('\n');
                }
                first = false;
            }
        }
    }
    out
}

fn serialize_kv(map: &BTreeMap<String, Vec<String>>) -> String {
    let mut out = String::new();
    for (k, v) in map {
        if v.is_empty() {
            continue;
        }
        // Trailing `;` matches the convention used by `xdg-mime` and most
        // freedesktop tools.
        out.push_str(&format!("{}={};\n", k, v.join(";")));
    }
    out
}

// ───────────────────────────────────────────────────────────────────────────
//                           Conflict-aware save
// ───────────────────────────────────────────────────────────────────────────

/// Snapshot of the user's `mimeapps.list` taken at startup. Used by
/// `save_user_file_safely_at` to detect mid-session external modifications.
#[derive(Debug, Clone, Default)]
pub struct UserFileBaseline {
    /// Parsed state of the file when we read it. Conflict detection compares
    /// per-`(section, mime)` keys against the current on-disk state.
    pub assoc: OnDiskAssoc,
    /// Exact bytes we read. Used as a cheap unchanged-file fast path
    /// (`current_raw == baseline.raw` → no external change, skip the
    /// semantic compare entirely).
    pub raw: String,
}

/// A single per-`(mime, ...)` clash between our pending edits and external
/// changes that landed after we took the baseline. See
/// `detect_conflicts` for what counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MimeConflict {
    pub mime: String,
    pub kind: ConflictKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictKind {
    /// We staged a change to this mime's default; external also changed it to
    /// something different (or we cleared and they set, etc).
    DefaultChanged {
        ours: Option<String>,
        theirs: Option<String>,
    },
    /// We staged add(X); external recorded a removal of X (or vice versa).
    /// Opposite intents.
    AddRemoveOpposed {
        app_id: String,
        /// `true` if our pending was an add, `false` if it was a remove.
        we_added: bool,
    },
}

#[derive(Debug)]
pub enum SaveError {
    /// The user-file changed externally and at least one pending edit
    /// overlaps with that change. Pending state is untouched.
    Conflicts(Vec<MimeConflict>),
    /// An I/O or parse error from the underlying [`save_user_file_at`].
    Io(eyre::Report),
}

#[derive(Debug, Clone, Copy)]
pub struct SaveOutcome {
    /// Number of pending edits that were committed.
    pub written: usize,
    /// `true` if the on-disk file changed since we read the baseline (we
    /// merged non-conflicting external edits in). UI flashes a hint about
    /// this so the user knows their save preserved a foreign change.
    pub merged_external_changes: bool,
}

/// Read the user's `mimeapps.list` once and capture both its parsed state
/// and its raw bytes. Used by `App` at startup to snapshot a baseline that
/// `save_user_file_safely_at` later compares against.
pub fn read_user_file_baseline() -> UserFileBaseline {
    user_mimeapps_path()
        .map(|p| read_user_file_baseline_at(&p))
        .unwrap_or_default()
}

pub fn read_user_file_baseline_at(path: &Path) -> UserFileBaseline {
    let raw = fs::read_to_string(path).unwrap_or_default();
    let mut assoc = OnDiskAssoc::default();
    merge_one(&raw, path, &mut assoc);
    UserFileBaseline { assoc, raw }
}

/// Conflict-aware save. Re-reads the file at `path`, compares with
/// `baseline` (hash compare first, then semantic per-mime if the bytes
/// differ), and either:
///  - writes our pending edits on top of the current disk state (so foreign
///    edits to unrelated mimes are preserved), or
///  - returns the list of conflicting mimes without writing anything.
pub fn save_user_file_safely_at(
    path: &Path,
    pending: &PendingEdits,
    baseline: &UserFileBaseline,
) -> Result<SaveOutcome, SaveError> {
    let current_raw = fs::read_to_string(path).unwrap_or_default();
    let written = pending.count();

    // Fast path: file untouched since baseline → no race possible.
    if current_raw == baseline.raw {
        save_user_file_at(path, pending).map_err(SaveError::Io)?;
        return Ok(SaveOutcome {
            written,
            merged_external_changes: false,
        });
    }

    // File differs — could be external edit, or could be a comment / blank
    // line added (no semantic conflict). Re-parse and check per-mime.
    let mut current = OnDiskAssoc::default();
    merge_one(&current_raw, path, &mut current);

    let conflicts = detect_conflicts(&baseline.assoc, &current, pending);
    if !conflicts.is_empty() {
        return Err(SaveError::Conflicts(conflicts));
    }

    save_user_file_at(path, pending).map_err(SaveError::Io)?;
    Ok(SaveOutcome {
        written,
        merged_external_changes: true,
    })
}

/// Public for the `App::action_force_save` path — bypasses conflict checks
/// and writes pending edits over whatever's on disk. Foreign changes that
/// overlap with the user's pending edits get clobbered.
pub fn save_user_file_force_at(
    path: &Path,
    pending: &PendingEdits,
) -> eyre::Result<()> {
    save_user_file_at(path, pending)
}

/// Compute the conflict set. For each pending edit, ask "did external touch
/// the same `(section, mime)` differently from how we want it?".
///
/// Idempotent overlaps (external set X, we want X) and complementary
/// overlaps (we want to add Y, external also added Y) are *not* conflicts —
/// just unions that compose cleanly at write time.
pub fn detect_conflicts(
    baseline: &OnDiskAssoc,
    current: &OnDiskAssoc,
    pending: &PendingEdits,
) -> Vec<MimeConflict> {
    let mut conflicts: Vec<MimeConflict> = Vec::new();

    // ── default-app conflicts ───────────────────────────────────────────
    for (mime, slot) in &pending.set_default {
        let baseline_d = baseline.defaults.get(mime).cloned();
        let current_d = current.defaults.get(mime).cloned();
        if baseline_d == current_d {
            // External didn't touch this mime's default — safe to apply.
            continue;
        }
        // External touched this mime's default. Compatible with our intent?
        let ours = slot.clone();
        let theirs = current_d.clone();
        match (&ours, &theirs) {
            // Both cleared (we want None, external also has None) — idempotent.
            (None, None) => continue,
            // We want X, external also has X — idempotent.
            (Some(o), Some(t)) if o == t => continue,
            // Otherwise: different intents.
            _ => conflicts.push(MimeConflict {
                mime: mime.clone(),
                kind: ConflictKind::DefaultChanged { ours, theirs },
            }),
        }
    }

    // ── add ↔ remove conflicts ──────────────────────────────────────────
    // We staged "add X to mime"; external recorded "remove X from mime"
    // somewhere between baseline and now.
    for (mime, ids) in &pending.add {
        for id in ids {
            let baseline_removed = baseline
                .removed
                .get(mime)
                .map(|s| s.contains(id))
                .unwrap_or(false);
            let current_removed = current
                .removed
                .get(mime)
                .map(|s| s.contains(id))
                .unwrap_or(false);
            if !baseline_removed && current_removed {
                conflicts.push(MimeConflict {
                    mime: mime.clone(),
                    kind: ConflictKind::AddRemoveOpposed {
                        app_id: id.clone(),
                        we_added: true,
                    },
                });
            }
        }
    }
    // Mirror: we staged "remove"; external added.
    for (mime, ids) in &pending.remove {
        for id in ids {
            let baseline_added = baseline
                .added
                .get(mime)
                .map(|s| s.contains(id))
                .unwrap_or(false);
            let current_added = current
                .added
                .get(mime)
                .map(|s| s.contains(id))
                .unwrap_or(false);
            if !baseline_added && current_added {
                conflicts.push(MimeConflict {
                    mime: mime.clone(),
                    kind: ConflictKind::AddRemoveOpposed {
                        app_id: id.clone(),
                        we_added: false,
                    },
                });
            }
        }
    }

    conflicts
}

/// Mutate `pending` to drop any edits that conflict with `conflicts`. Used
/// by the "merge non-conflicting" path in the conflict-resolve modal.
pub fn drop_conflicting_edits(pending: &mut PendingEdits, conflicts: &[MimeConflict]) {
    for c in conflicts {
        match &c.kind {
            ConflictKind::DefaultChanged { .. } => {
                pending.set_default.remove(&c.mime);
            }
            ConflictKind::AddRemoveOpposed { app_id, we_added } => {
                if *we_added {
                    if let Some(set) = pending.add.get_mut(&c.mime) {
                        set.remove(app_id);
                        if set.is_empty() {
                            pending.add.remove(&c.mime);
                        }
                    }
                } else if let Some(set) = pending.remove.get_mut(&c.mime) {
                    set.remove(app_id);
                    if set.is_empty() {
                        pending.remove.remove(&c.mime);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_path(name: &str) -> PathBuf {
        PathBuf::from(format!("/tmp/{}-mimeapps.list", name))
    }

    #[test]
    fn parses_all_three_sections() {
        let mut assoc = OnDiskAssoc::default();
        merge_one(
            "# comment\n\
             [Default Applications]\n\
             text/html=firefox.desktop\n\
             application/pdf=evince.desktop;\n\
             \n\
             [Added Associations]\n\
             image/png=gimp.desktop;krita.desktop;\n\
             \n\
             [Removed Associations]\n\
             text/plain=nano.desktop\n",
            &dummy_path("user"),
            &mut assoc,
        );
        assert_eq!(assoc.defaults.get("text/html"), Some(&"firefox.desktop".to_string()));
        assert_eq!(assoc.defaults.get("application/pdf"), Some(&"evince.desktop".to_string()));
        let added = assoc.added.get("image/png").unwrap();
        assert!(added.contains("gimp.desktop"));
        assert!(added.contains("krita.desktop"));
        let removed = assoc.removed.get("text/plain").unwrap();
        assert!(removed.contains("nano.desktop"));
    }

    #[test]
    fn higher_priority_default_wins() {
        let mut assoc = OnDiskAssoc::default();
        let gnome = dummy_path("gnome");
        merge_one(
            "[Default Applications]\ntext/html=firefox.desktop\n",
            &gnome,
            &mut assoc,
        );
        // Lower-priority file would overwrite if we weren't careful.
        merge_one(
            "[Default Applications]\ntext/html=chromium.desktop\n",
            &dummy_path("user"),
            &mut assoc,
        );
        assert_eq!(
            assoc.defaults.get("text/html"),
            Some(&"firefox.desktop".to_string())
        );
        // Provenance points at the winning (higher-priority) file.
        assert_eq!(assoc.default_sources.get("text/html"), Some(&gnome));
    }

    #[test]
    fn unknown_sections_are_ignored() {
        let mut assoc = OnDiskAssoc::default();
        merge_one(
            "[Future Use]\ntext/html=foo.desktop\n[Default Applications]\nimage/png=gimp.desktop\n",
            &dummy_path("user"),
            &mut assoc,
        );
        assert!(assoc.defaults.get("text/html").is_none());
        assert!(assoc.defaults.get("image/png").is_some());
    }

    fn tempdir(prefix: &str) -> PathBuf {
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d = std::env::temp_dir().join(format!("{}_{}_{}", prefix, pid, nanos));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn save_creates_file_with_pending_edits() {
        let dir = tempdir("mime_tui_save_create");
        let path = dir.join("mimeapps.list");

        let mut pending = PendingEdits::default();
        pending.set_default("text/html", Some("firefox.desktop"));
        pending.add_assoc("image/png", "gimp.desktop");
        pending.remove_assoc("application/pdf", "evince.desktop");

        save_user_file_at(&path, &pending).unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("[Default Applications]"));
        assert!(content.contains("text/html=firefox.desktop;"));
        assert!(content.contains("[Added Associations]"));
        assert!(content.contains("image/png=gimp.desktop;"));
        assert!(content.contains("[Removed Associations]"));
        assert!(content.contains("application/pdf=evince.desktop;"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_preserves_unknown_sections_and_existing_entries() {
        let dir = tempdir("mime_tui_save_preserve");
        let path = dir.join("mimeapps.list");
        fs::write(
            &path,
            "[Added Associations]\n\
             image/png=krita.desktop;\n\
             \n\
             [Custom-Vendor-Section]\n\
             unrelated=keep-me\n",
        )
        .unwrap();

        let mut pending = PendingEdits::default();
        pending.add_assoc("image/png", "gimp.desktop");

        save_user_file_at(&path, &pending).unwrap();
        let content = fs::read_to_string(&path).unwrap();

        // Pre-existing entry preserved + new id appended.
        assert!(content.contains("image/png=krita.desktop;gimp.desktop;"));
        // Unknown section preserved verbatim.
        assert!(content.contains("[Custom-Vendor-Section]"));
        assert!(content.contains("unrelated=keep-me"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_writes_bak_and_is_atomic() {
        let dir = tempdir("mime_tui_save_bak");
        let path = dir.join("mimeapps.list");
        fs::write(&path, "[Default Applications]\ntext/html=firefox.desktop;\n").unwrap();

        let mut pending = PendingEdits::default();
        pending.set_default("text/html", Some("chromium.desktop"));

        save_user_file_at(&path, &pending).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("text/html=chromium.desktop;"));

        let bak = bak_path(&path);
        assert!(bak.exists(), ".bak should exist after a save");
        let bak_content = fs::read_to_string(&bak).unwrap();
        assert!(bak_content.contains("firefox.desktop"));

        // No tempfile leftover.
        assert!(!tmp_path(&path).exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn clear_default_drops_entry() {
        let dir = tempdir("mime_tui_save_clear");
        let path = dir.join("mimeapps.list");
        fs::write(
            &path,
            "[Default Applications]\ntext/html=firefox.desktop;\nimage/png=gimp.desktop;\n",
        )
        .unwrap();

        let mut pending = PendingEdits::default();
        pending.set_default("text/html", None); // explicit clear

        save_user_file_at(&path, &pending).unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert!(!content.contains("text/html="));
        assert!(content.contains("image/png=gimp.desktop;"));

        let _ = fs::remove_dir_all(&dir);
    }

    // ── Conflict-aware save tests ──────────────────────────────────────

    /// Helper: write a file, snapshot the baseline, return both.
    fn setup_baseline(prefix: &str, content: &str) -> (PathBuf, UserFileBaseline) {
        let dir = tempdir(prefix);
        let path = dir.join("mimeapps.list");
        fs::write(&path, content).unwrap();
        let baseline = read_user_file_baseline_at(&path);
        (path, baseline)
    }

    #[test]
    fn safely_save_succeeds_when_disk_unchanged_since_baseline() {
        let (path, baseline) = setup_baseline(
            "mime_tui_safe_unchanged",
            "[Default Applications]\ntext/html=firefox.desktop;\n",
        );
        let mut pending = PendingEdits::default();
        pending.set_default("text/html", Some("chromium.desktop"));

        let result = save_user_file_safely_at(&path, &pending, &baseline);
        let outcome = result.expect("save should succeed when disk hasn't moved");
        assert!(!outcome.merged_external_changes);

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("text/html=chromium.desktop;"));

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn safely_save_merges_when_external_touched_unrelated_mime() {
        // Baseline has text/html only. Externally somebody added image/png
        // while we were running. Our edit changes text/html. Save should
        // succeed AND preserve image/png.
        let (path, baseline) = setup_baseline(
            "mime_tui_safe_merge",
            "[Default Applications]\ntext/html=firefox.desktop;\n",
        );
        let mut pending = PendingEdits::default();
        pending.set_default("text/html", Some("chromium.desktop"));

        // Simulate external write.
        fs::write(
            &path,
            "[Default Applications]\n\
             text/html=firefox.desktop;\n\
             image/png=gimp.desktop;\n",
        )
        .unwrap();

        let outcome = save_user_file_safely_at(&path, &pending, &baseline)
            .expect("non-conflicting external change should merge");
        assert!(outcome.merged_external_changes,
            "outcome should flag that we merged with an external change");

        let content = fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("text/html=chromium.desktop;"),
            "our pending edit must be applied"
        );
        assert!(
            content.contains("image/png=gimp.desktop;"),
            "external image/png entry must be preserved"
        );

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn safely_save_detects_conflict_on_overlapping_default() {
        let (path, baseline) = setup_baseline(
            "mime_tui_safe_conflict_default",
            "[Default Applications]\ntext/html=firefox.desktop;\n",
        );
        let mut pending = PendingEdits::default();
        pending.set_default("text/html", Some("chromium.desktop"));

        // External raced us to a different value.
        fs::write(
            &path,
            "[Default Applications]\ntext/html=opera.desktop;\n",
        )
        .unwrap();

        let result = save_user_file_safely_at(&path, &pending, &baseline);
        let conflicts = match result {
            Err(SaveError::Conflicts(c)) => c,
            other => panic!("expected SaveError::Conflicts, got {:?}", other),
        };
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].mime, "text/html");
        match &conflicts[0].kind {
            ConflictKind::DefaultChanged { ours, theirs } => {
                assert_eq!(ours.as_deref(), Some("chromium.desktop"));
                assert_eq!(theirs.as_deref(), Some("opera.desktop"));
            }
            k => panic!("expected DefaultChanged, got {:?}", k),
        }

        // Critically: the file should NOT have been overwritten.
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("opera.desktop"));
        assert!(!content.contains("chromium.desktop"));

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn safely_save_idempotent_when_external_matches_pending() {
        // Baseline: no default. Pending: set firefox. External: also set
        // firefox first. Should be a no-conflict idempotent merge.
        let (path, baseline) = setup_baseline(
            "mime_tui_safe_idempotent",
            "[Default Applications]\n",
        );
        let mut pending = PendingEdits::default();
        pending.set_default("text/html", Some("firefox.desktop"));

        fs::write(
            &path,
            "[Default Applications]\ntext/html=firefox.desktop;\n",
        )
        .unwrap();

        let outcome = save_user_file_safely_at(&path, &pending, &baseline)
            .expect("idempotent match should be no-conflict");
        assert!(outcome.merged_external_changes);

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("text/html=firefox.desktop;"));

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn safely_save_conflict_on_add_vs_remove() {
        // We're trying to ADD krita to image/png's associations. Externally
        // someone REMOVED krita from image/png. Opposing intents → conflict.
        let (path, baseline) = setup_baseline(
            "mime_tui_safe_add_remove",
            "[Added Associations]\nimage/png=other.desktop;\n",
        );
        let mut pending = PendingEdits::default();
        pending.add_assoc("image/png", "krita.desktop");

        fs::write(
            &path,
            "[Added Associations]\n\
             image/png=other.desktop;\n\
             \n\
             [Removed Associations]\n\
             image/png=krita.desktop;\n",
        )
        .unwrap();

        let result = save_user_file_safely_at(&path, &pending, &baseline);
        let conflicts = match result {
            Err(SaveError::Conflicts(c)) => c,
            other => panic!("expected SaveError::Conflicts, got {:?}", other),
        };
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].mime, "image/png");
        match &conflicts[0].kind {
            ConflictKind::AddRemoveOpposed { app_id, we_added } => {
                assert_eq!(app_id, "krita.desktop");
                assert!(we_added);
            }
            k => panic!("expected AddRemoveOpposed, got {:?}", k),
        }

        let _ = fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn drop_conflicting_edits_removes_just_the_conflicting_keys() {
        // Pending has two default edits + one add. Conflict list mentions
        // only one of the defaults. The other two pending edits should
        // survive the drop.
        let mut pending = PendingEdits::default();
        pending.set_default("text/html", Some("firefox.desktop"));
        pending.set_default("image/png", Some("gimp.desktop"));
        pending.add_assoc("video/mp4", "vlc.desktop");

        let conflicts = vec![MimeConflict {
            mime: "text/html".into(),
            kind: ConflictKind::DefaultChanged {
                ours: Some("firefox.desktop".into()),
                theirs: Some("opera.desktop".into()),
            },
        }];

        drop_conflicting_edits(&mut pending, &conflicts);
        assert!(!pending.set_default.contains_key("text/html"));
        assert!(pending.set_default.contains_key("image/png"));
        assert!(pending.add.contains_key("video/mp4"));
    }
}
