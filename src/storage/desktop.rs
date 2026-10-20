//! `.desktop` file parsing, copied from bstl/storage.rs and adapted:
//! - cache PK is the desktop-file basename (e.g. `firefox.desktop`) rather
//!   than the `Name=` field, because that is what `mimeapps.list` references;
//! - `MimeType=` is captured as a raw semicolon-separated string;
//! - only `Hidden=true` (the spec's "deleted") removes an app. `NoDisplay=true`
//!   just means "keep out of launcher menus" — such apps (zathura plugins,
//!   imv, ...) are still installed MIME handlers, so they are kept. OnlyShowIn
//!   / NotShowIn filtering is likewise relaxed — a user editing MIME
//!   associations may legitimately want to see apps from other desktops;
//! - rows are cached per file (keyed by `source_path`) and XDG precedence is
//!   resolved in [`load_apps`]: the earliest dir providing an id wins, and if
//!   that file is `Hidden` the id is treated as not installed.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::SystemTime;

use eyre::Result;
use rusqlite::{Connection, params};

use crate::model::DesktopApp;

#[derive(Debug, Clone)]
pub struct ParsedDesktop {
    pub id: String,
    pub name: String,
    pub comment: String,
    pub exec: String,
    pub terminal: bool,
    pub raw_mime_types: String,
    pub raw_categories: String,
    pub hidden: bool,
}

/// Refresh the cached `apps` table by walking the given directories. Uses
/// directory mtime to skip unchanged dirs; within a changed dir, only re-parses
/// `.desktop` files whose own mtime is newer than the cached row's mtime.
pub fn refresh_app_cache(conn: &mut Connection, dirs: &[String]) -> Result<()> {
    let tx = conn.transaction()?;

    let mut seen_paths: Vec<String> = Vec::new();

    for dir in dirs {
        let dir_path = Path::new(dir);
        let dir_mtime = match mtime_secs(dir_path) {
            Some(m) => m,
            None => continue,
        };

        let cached_dir_mtime: Option<i64> = tx
            .query_row(
                "SELECT dir_mtime FROM scan_meta WHERE directory = ?",
                params![dir],
                |r| r.get(0),
            )
            .ok();

        if cached_dir_mtime == Some(dir_mtime) {
            // Trust the cache; just collect existing source_paths so we don't
            // mistakenly prune them.
            let mut stmt = tx.prepare(
                "SELECT source_path FROM apps WHERE source_path LIKE ?",
            )?;
            let pat = format!("{}/%", dir.trim_end_matches('/'));
            let rows = stmt.query_map(params![pat], |r| r.get::<_, String>(0))?;
            for r in rows {
                seen_paths.push(r?);
            }
            continue;
        }

        let entries = match fs::read_dir(dir_path) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("desktop") {
                continue;
            }
            let path_str = path.to_string_lossy().into_owned();
            seen_paths.push(path_str.clone());

            let file_mtime = mtime_secs(&path).unwrap_or(0);
            let cached_file_mtime: Option<i64> = tx
                .query_row(
                    "SELECT file_mtime FROM apps WHERE source_path = ?",
                    params![path_str],
                    |r| r.get(0),
                )
                .ok();

            if cached_file_mtime == Some(file_mtime) {
                continue;
            }

            let parsed = parse_desktop_file(&path);
            tx.execute("DELETE FROM apps WHERE source_path = ?", params![path_str])?;
            if let Some(app) = parsed {
                tx.execute(
                    "INSERT OR REPLACE INTO apps(id, name, exec, terminal, comment, raw_mime_types, raw_categories, hidden, source_path, file_mtime)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    params![
                        app.id,
                        app.name,
                        app.exec,
                        app.terminal as i32,
                        app.comment,
                        app.raw_mime_types,
                        app.raw_categories,
                        app.hidden as i32,
                        path_str,
                        file_mtime,
                    ],
                )?;
            }
        }

        tx.execute(
            "INSERT INTO scan_meta(directory, dir_mtime, scanned_at)
             VALUES (?, ?, strftime('%s','now'))
             ON CONFLICT(directory) DO UPDATE SET
                dir_mtime = excluded.dir_mtime,
                scanned_at = excluded.scanned_at",
            params![dir, dir_mtime],
        )?;
    }

    if !seen_paths.is_empty() {
        tx.execute("CREATE TEMP TABLE seen(path TEXT PRIMARY KEY)", [])?;
        {
            let mut ins = tx.prepare("INSERT OR IGNORE INTO seen(path) VALUES (?)")?;
            for p in &seen_paths {
                ins.execute(params![p])?;
            }
        }
        for dir in dirs {
            let pat = format!("{}/%", dir.trim_end_matches('/'));
            tx.execute(
                "DELETE FROM apps
                 WHERE source_path LIKE ?
                   AND source_path NOT IN (SELECT path FROM seen)",
                params![pat],
            )?;
        }
        tx.execute("DROP TABLE seen", [])?;
    }

    tx.commit()?;
    Ok(())
}

/// Load all cached apps as `DesktopApp`s, with `mime_types` parsed into a
/// trimmed list and `category` derived from `raw_categories` via
/// [`group_category`].
///
/// `dirs` must be in XDG precedence order (as from [`xdg_application_dirs`]):
/// when several dirs provide the same id the earliest wins, and a winning
/// `Hidden=true` file masks the id entirely. Rows from dirs no longer in
/// `dirs` are ignored.
pub fn load_apps(conn: &Connection, dirs: &[String]) -> Result<Vec<DesktopApp>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, exec, terminal, comment, raw_mime_types, raw_categories, hidden, source_path
         FROM apps ORDER BY name COLLATE NOCASE",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i32>(3)? != 0,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, i32>(7)? != 0,
                r.get::<_, String>(8)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let rank_of = |source_path: &str| {
        let parent = Path::new(source_path).parent()?;
        dirs.iter()
            .position(|d| Path::new(d.trim_end_matches('/')) == parent)
    };

    // id -> (rank, source_path) of the highest-precedence file.
    let mut winners: HashMap<&str, (usize, &str)> = HashMap::new();
    for row in &rows {
        let (id, source_path) = (row.0.as_str(), row.8.as_str());
        let Some(rank) = rank_of(source_path) else {
            continue;
        };
        match winners.get(id) {
            Some(&(best, _)) if best <= rank => {}
            _ => {
                winners.insert(id, (rank, source_path));
            }
        }
    }

    let mut out = Vec::new();
    for (id, name, exec, terminal, comment, raw_mime_types, raw_categories, hidden, source_path) in
        &rows
    {
        if *hidden || winners.get(id.as_str()).map(|w| w.1) != Some(source_path.as_str()) {
            continue;
        }
        out.push(DesktopApp {
            id: id.clone(),
            name: name.clone(),
            comment: comment.clone(),
            exec: exec.clone(),
            terminal: *terminal,
            mime_types: split_semicolons(raw_mime_types),
            category: group_category(raw_categories, name),
        });
    }
    Ok(out)
}

/// Map a `Categories=` raw string + app name into a single display bucket.
/// First matching keyword wins; falls back to "Utilities" when nothing
/// matches. Ported from bstl, where the same buckets drive the launcher's
/// category-list ordering.
pub fn group_category(raw: &str, _app_name: &str) -> String {
    let raw = raw.to_lowercase();
    if raw.contains("game") {
        "Games".into()
    } else if raw.contains("utility") {
        "Utilities".into()
    } else if raw.contains("development") {
        "Development".into()
    } else if raw.contains("network") {
        "Network".into()
    } else if raw.contains("audio") || raw.contains("video") {
        "Audio/Video".into()
    } else if raw.contains("graphics")
        || raw.contains("2dgraphics")
        || raw.contains("3dgraphics")
    {
        "Graphics".into()
    } else if raw.contains("system") {
        "System".into()
    } else if raw.contains("office") {
        "Office".into()
    } else if raw.contains("education") {
        "Education".into()
    } else if raw.contains("settings") {
        "Settings".into()
    } else {
        "Utilities".into()
    }
}

fn mtime_secs(p: &Path) -> Option<i64> {
    let md = fs::metadata(p).ok()?;
    let mt = md.modified().ok()?;
    let dur = mt.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    Some(dur.as_secs() as i64)
}

/// Parse a single `.desktop` file. Returns `None` for entries missing required
/// fields. `Hidden=true` entries are returned (with `hidden` set, and possibly
/// empty name/exec) so they can mask lower-precedence copies of the same id;
/// `NoDisplay` is deliberately ignored.
pub fn parse_desktop_file(path: &Path) -> Option<ParsedDesktop> {
    let content = fs::read_to_string(path).ok()?;

    let mut name: Option<String> = None;
    let mut generic_name: Option<String> = None;
    let mut exec: Option<String> = None;
    let mut comment: Option<String> = None;
    let mut mime_types: Option<String> = None;
    let mut categories: Option<String> = None;
    let mut hidden = false;
    let mut terminal = false;
    let mut in_desktop_entry = false;

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_desktop_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_desktop_entry {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.contains('[') {
            // Skip localized variants like Name[de]=...
            continue;
        }
        let key = key.trim();
        let value = value.trim();
        match key {
            "Name" => name = Some(value.to_string()),
            "GenericName" => generic_name = Some(value.to_string()),
            "Exec" => exec = Some(value.to_string()),
            "Comment" => comment = Some(value.to_string()),
            "MimeType" => mime_types = Some(value.to_string()),
            "Categories" => categories = Some(value.to_string()),
            "Hidden" => hidden = value == "true",
            "Terminal" => terminal = value == "true",
            _ => {}
        }
    }

    let id = path
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())?;

    if hidden {
        return Some(ParsedDesktop {
            id,
            name: name.or(generic_name).unwrap_or_default(),
            comment: String::new(),
            exec: String::new(),
            terminal: false,
            raw_mime_types: String::new(),
            raw_categories: String::new(),
            hidden: true,
        });
    }

    let name = name.or(generic_name)?;
    let exec_raw = exec?;

    Some(ParsedDesktop {
        id,
        name,
        comment: comment.unwrap_or_default(),
        exec: clean_exec(&exec_raw),
        terminal,
        raw_mime_types: mime_types.unwrap_or_default(),
        raw_categories: categories.unwrap_or_default(),
        hidden: false,
    })
}

fn split_semicolons(raw: &str) -> Vec<String> {
    raw.split(';')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn clean_exec(exec: &str) -> String {
    let mut result = exec.to_string();
    let field_codes = [
        "%f", "%F", "%u", "%U", "%d", "%D", "%n", "%N", "%i", "%c", "%k", "%v", "%m",
    ];
    for code in &field_codes {
        result = result.replace(code, "");
    }
    result.trim().to_string()
}

/// Resolve the ordered list of XDG application directories.
pub fn xdg_application_dirs() -> Vec<String> {
    let mut paths = Vec::new();

    let data_home = std::env::var("XDG_DATA_HOME").ok().filter(|s| !s.is_empty());
    if let Some(home) = data_home {
        paths.push(format!("{}/applications", home));
    } else if let Ok(home) = std::env::var("HOME") {
        paths.push(format!("{}/.local/share/applications", home));
    }

    let data_dirs = std::env::var("XDG_DATA_DIRS").ok().filter(|s| !s.is_empty());
    match data_dirs {
        Some(dirs) => {
            for dir in dirs.split(':') {
                if !dir.is_empty() {
                    paths.push(format!("{}/applications", dir));
                }
            }
        }
        None => {
            paths.push("/usr/local/share/applications".to_string());
            paths.push("/usr/share/applications".to_string());
        }
    }

    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tempdir(prefix: &str) -> std::path::PathBuf {
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{}_{}_{}", prefix, pid, nanos));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parses_mime_type_and_id() {
        let dir = tempdir("mime_tui_parse_mime");
        let path = dir.join("firefox.desktop");
        fs::write(
            &path,
            "[Desktop Entry]\nName=Firefox\nExec=firefox %u\nMimeType=text/html;application/pdf;\n",
        )
        .unwrap();
        let p = parse_desktop_file(&path).expect("should parse");
        assert_eq!(p.id, "firefox.desktop");
        assert_eq!(p.name, "Firefox");
        assert_eq!(p.exec, "firefox");
        assert_eq!(p.raw_mime_types, "text/html;application/pdf;");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn keeps_no_display() {
        // NoDisplay only keeps an app out of menus; it's still a MIME handler
        // (e.g. org.pwmt.zathura-pdf-mupdf.desktop) — issue #3.
        let dir = tempdir("mime_tui_parse_nodisplay");
        let path = dir.join("zathura.desktop");
        fs::write(
            &path,
            "[Desktop Entry]\nName=Zathura\nExec=zathura %U\nMimeType=application/pdf;\nNoDisplay=true\n",
        )
        .unwrap();
        let p = parse_desktop_file(&path).expect("NoDisplay apps should parse");
        assert!(!p.hidden);
        assert_eq!(p.raw_mime_types, "application/pdf;");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hidden_parses_as_tombstone() {
        // Hidden=true is the spec's "deleted"; it may omit Name/Exec but must
        // still parse so it can mask lower-precedence copies.
        let dir = tempdir("mime_tui_parse_hidden");
        let path = dir.join("hidden.desktop");
        fs::write(&path, "[Desktop Entry]\nHidden=true\n").unwrap();
        let p = parse_desktop_file(&path).expect("Hidden entries should parse");
        assert!(p.hidden);
        assert_eq!(p.id, "hidden.desktop");
        let _ = fs::remove_dir_all(&dir);
    }

    fn load_from(user: &Path, system: &Path) -> Vec<DesktopApp> {
        let mut storage = crate::storage::Storage::in_memory();
        let dirs = vec![
            user.to_string_lossy().into_owned(),
            system.to_string_lossy().into_owned(),
        ];
        refresh_app_cache(&mut storage.conn, &dirs).unwrap();
        load_apps(&storage.conn, &dirs).unwrap()
    }

    #[test]
    fn user_dir_overrides_system_dir() {
        let root = tempdir("mime_tui_precedence");
        let (user, system) = (root.join("user"), root.join("system"));
        fs::create_dir_all(&user).unwrap();
        fs::create_dir_all(&system).unwrap();
        fs::write(
            system.join("foo.desktop"),
            "[Desktop Entry]\nName=System Foo\nExec=foo\n",
        )
        .unwrap();
        fs::write(
            user.join("foo.desktop"),
            "[Desktop Entry]\nName=User Foo\nExec=foo --user\n",
        )
        .unwrap();
        let apps = load_from(&user, &system);
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].name, "User Foo");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn user_hidden_masks_system_copy() {
        let root = tempdir("mime_tui_hidden_mask");
        let (user, system) = (root.join("user"), root.join("system"));
        fs::create_dir_all(&user).unwrap();
        fs::create_dir_all(&system).unwrap();
        fs::write(
            system.join("foo.desktop"),
            "[Desktop Entry]\nName=Foo\nExec=foo\n",
        )
        .unwrap();
        fs::write(
            system.join("bar.desktop"),
            "[Desktop Entry]\nName=Bar\nExec=bar\nNoDisplay=true\n",
        )
        .unwrap();
        fs::write(user.join("foo.desktop"), "[Desktop Entry]\nHidden=true\n").unwrap();
        let apps = load_from(&user, &system);
        let ids: Vec<&str> = apps.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["bar.desktop"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn split_semicolons_strips_empty_and_whitespace() {
        assert_eq!(
            split_semicolons("text/html;application/pdf;"),
            vec!["text/html".to_string(), "application/pdf".to_string()],
        );
        assert_eq!(split_semicolons("  ; ; "), Vec::<String>::new());
    }
}
