//! `mimeapps.list` discovery, parsing, and writing per the freedesktop "MIME
//! Applications associations" spec. We resolve the full priority-ordered chain
//! of files into a single [`OnDiskAssoc`] for display, and write back **only**
//! to the user's `$XDG_CONFIG_HOME/mimeapps.list` — never to system files.

use std::collections::{BTreeMap, HashMap, HashSet};
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
    normalize_added_against_removed(&mut assoc);
    assoc
}

/// Strip any `(mime, id)` from `assoc.added` that's also in `assoc.removed`.
/// Per the XDG spec, a [Removed Associations] entry overrides any matching
/// [Added Associations] entry — keeping both in memory does nothing except
/// mislead `compute_phantoms` into surfacing apps whose every relation has
/// already been suppressed. Pre-existing contradictions in the user's file
/// (e.g. from mime-tui versions before the apply_pending fix) are healed
/// here so the UI behaves like the spec says it should.
fn normalize_added_against_removed(assoc: &mut OnDiskAssoc) {
    for (mime, removed_ids) in &assoc.removed {
        if let Some(added_ids) = assoc.added.get_mut(mime) {
            added_ids.retain(|id| !removed_ids.contains(id));
        }
    }
    assoc.added.retain(|_, v| !v.is_empty());
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

/// Remove every (mime, id) listed in `pairs` from `map`, dropping mime keys
/// whose value list goes empty. Used by `apply_pending` to keep the
/// `[Added]` / `[Removed]` sections from contradicting each other after a
/// remove cancels an earlier add (or vice versa).
fn strip_pending(
    map: &mut BTreeMap<String, Vec<String>>,
    pairs: &HashMap<String, HashSet<String>>,
) {
    for (mime, ids) in pairs {
        let Some(entry) = map.get_mut(mime) else {
            continue;
        };
        entry.retain(|existing| !ids.contains(existing));
        if entry.is_empty() {
            map.remove(mime);
        }
    }
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
                // A pending remove must strip the matching entry from
                // [Added Associations] — otherwise re-loading mimeapps.list
                // still surfaces the (now unwanted) app via assoc.added,
                // including as a phantom in the by-app list.
                strip_pending(map, &pending.remove);
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
                // Symmetric: a pending add must lift any matching
                // [Removed Associations] entry so the two sections don't
                // contradict each other after the round-trip.
                strip_pending(map, &pending.add);
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

    heal_added_against_removed(file);
}

/// Final pass over the parsed file: drop any `(mime, id)` from
/// `[Added Associations]` that also appears in `[Removed Associations]`.
/// Pending-driven cases (add cancels remove, remove cancels add) are
/// handled earlier by [`strip_pending`]; this pass exists to clean up
/// pre-existing contradictions left in the file by earlier mime-tui
/// versions or by hand-edits. Any save then writes the spec-consistent
/// form even if the user's pending edits don't touch the offending mime.
fn heal_added_against_removed(file: &mut UserFile) {
    let mut removed_pairs: HashMap<String, HashSet<String>> = HashMap::new();
    for s in file.sections.iter() {
        if let SectionEntry::Removed(map) = s {
            for (mime, ids) in map {
                let entry = removed_pairs.entry(mime.clone()).or_default();
                for id in ids {
                    entry.insert(id.clone());
                }
            }
        }
    }
    if removed_pairs.is_empty() {
        return;
    }
    for s in file.sections.iter_mut() {
        if let SectionEntry::Added(map) = s {
            strip_pending(map, &removed_pairs);
        }
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
    normalize_added_against_removed(&mut assoc);
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
    normalize_added_against_removed(&mut current);

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
                // The dropped default change is the very thing the cascade
                // snapshot would restore on undo; without it, the snapshot
                // is orphaned. Drop snapshots for this mime to match.
                pending
                    .remove_default_snapshot
                    .retain(|(m, _), _| m != &c.mime);
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
                    // The matching remove just got dropped — its snapshot
                    // would never fire again. Drop it too.
                    pending
                        .remove_default_snapshot
                        .remove(&(c.mime.clone(), app_id.clone()));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
