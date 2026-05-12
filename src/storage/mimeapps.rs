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

/// Apply `pending` to the user's `mimeapps.list`. Atomic: writes a tempfile
/// then renames into place. Keeps one rolling `.bak` alongside. Best-effort
/// triggers `update-desktop-database` on `$XDG_DATA_HOME/applications`
/// afterwards so other apps pick up the change without re-login.
pub fn save_user_file(pending: &PendingEdits) -> Result<()> {
    let path = user_mimeapps_path()
        .ok_or_else(|| eyre::eyre!("could not resolve XDG_CONFIG_HOME"))?;
    save_user_file_at(&path, pending)?;
    let _ = run_update_desktop_database();
    Ok(())
}

/// Best-effort: refresh `$XDG_DATA_HOME/applications/mimeinfo.cache` so DEs see
/// the change immediately. Failures are intentionally ignored — the binary
/// isn't always installed, and the user might not have write access to the
/// target.
fn run_update_desktop_database() -> Option<()> {
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
}
