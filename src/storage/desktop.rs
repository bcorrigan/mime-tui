//! `.desktop` file parsing, copied from bstl/storage.rs and adapted:
//! - cache PK is the desktop-file basename (e.g. `firefox.desktop`) rather
//!   than the `Name=` field, because that is what `mimeapps.list` references;
//! - `MimeType=` is captured as a raw semicolon-separated string;
//! - NoDisplay / Hidden filter still applies, but OnlyShowIn / NotShowIn
//!   filtering is intentionally relaxed — a user editing MIME associations
//!   may legitimately want to see apps from other desktops.

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
                    "INSERT OR REPLACE INTO apps(id, name, exec, terminal, comment, raw_mime_types, raw_categories, source_path, file_mtime)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    params![
                        app.id,
                        app.name,
                        app.exec,
                        app.terminal as i32,
                        app.comment,
                        app.raw_mime_types,
                        app.raw_categories,
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
pub fn load_apps(conn: &Connection) -> Result<Vec<DesktopApp>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, exec, terminal, comment, raw_mime_types, raw_categories
         FROM apps ORDER BY name COLLATE NOCASE",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i32>(3)? != 0,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, String>(6)?,
        ))
    })?;
    let mut out = Vec::new();
    for r in rows {
        let (id, name, exec, terminal, comment, raw_mime_types, raw_categories) = r?;
        let category = group_category(&raw_categories, &name);
        out.push(DesktopApp {
            id,
            name,
            comment,
            exec,
            terminal,
            mime_types: split_semicolons(&raw_mime_types),
            category,
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

/// Parse a single `.desktop` file. Returns `None` for entries we deliberately
/// skip (Hidden / NoDisplay / missing required fields).
pub fn parse_desktop_file(path: &Path) -> Option<ParsedDesktop> {
    let content = fs::read_to_string(path).ok()?;

    let mut name: Option<String> = None;
    let mut generic_name: Option<String> = None;
    let mut exec: Option<String> = None;
    let mut comment: Option<String> = None;
    let mut mime_types: Option<String> = None;
    let mut categories: Option<String> = None;
    let mut no_display = false;
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
            "NoDisplay" => no_display = value == "true",
            "Hidden" => no_display = no_display || value == "true",
            "Terminal" => terminal = value == "true",
            _ => {}
        }
    }

    if no_display {
        return None;
    }

    let name = name.or(generic_name)?;
    let exec_raw = exec?;

    let id = path
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())?;

    Some(ParsedDesktop {
        id,
        name,
        comment: comment.unwrap_or_default(),
        exec: clean_exec(&exec_raw),
        terminal,
        raw_mime_types: mime_types.unwrap_or_default(),
        raw_categories: categories.unwrap_or_default(),
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
    fn skips_no_display() {
        let dir = tempdir("mime_tui_parse_nodisplay");
        let path = dir.join("hidden.desktop");
        fs::write(
            &path,
            "[Desktop Entry]\nName=Hidden\nExec=hidden\nNoDisplay=true\n",
        )
        .unwrap();
        assert!(parse_desktop_file(&path).is_none());
        let _ = fs::remove_dir_all(&dir);
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
