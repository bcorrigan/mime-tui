/// A parsed `.desktop` application. `id` is the file basename (e.g.
/// `firefox.desktop`) because that is what `mimeapps.list` references; `name`
/// is the human-readable `Name=` field used for display.
#[derive(Debug, Clone)]
#[allow(dead_code)] // exec/terminal feed into Phase 2 storage + future launch hooks.
pub struct DesktopApp {
    pub id: String,
    pub name: String,
    pub comment: String,
    pub exec: String,
    pub terminal: bool,
    pub mime_types: Vec<String>,
    /// Single-bucket category derived from the `.desktop` `Categories=`
    /// field — drives the per-app icon. One of: Utilities, Development,
    /// Network, Audio/Video, Graphics, System, Office, Games, Education,
    /// Settings, or a passthrough if the raw value doesn't match a known
    /// keyword.
    pub category: String,
}

impl DesktopApp {
    #[allow(dead_code)]
    pub fn handles(&self, mime: &str) -> bool {
        self.mime_types.iter().any(|m| m == mime)
    }
}

/// A MIME type pulled from shared-mime-info. The set of `.desktop` files that
/// declare support is derived on demand via `App::associations_for`, not
/// pre-cached here.
#[derive(Debug, Clone)]
pub struct MimeType {
    pub id: String,
    pub description: String,
}

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// Resolved on-disk association state, merged across all readable
/// `mimeapps.list` files per XDG precedence (first file mentioning a given
/// mime wins, per spec).
#[derive(Debug, Clone, Default)]
pub struct OnDiskAssoc {
    pub defaults: HashMap<String, String>,
    pub added: HashMap<String, HashSet<String>>,
    pub removed: HashMap<String, HashSet<String>>,
    /// Provenance: which file each `defaults` entry was read from. Used to
    /// (a) surface "from gnome-mimeapps.list" in the UI and (b) warn when a
    /// save would be silently shadowed by a higher-priority file.
    pub default_sources: HashMap<String, PathBuf>,
}

impl OnDiskAssoc {
    /// True if this app is explicitly removed from this mime's associations.
    pub fn is_removed(&self, mime: &str, app_id: &str) -> bool {
        self.removed
            .get(mime)
            .map(|s| s.contains(app_id))
            .unwrap_or(false)
    }
}

/// User edits accumulated since the last save. Layered on top of
/// [`OnDiskAssoc`] by `App::effective_*` to render a live preview, and folded
/// into the user `mimeapps.list` by `storage::mimeapps::save_user_file` on
/// `Ctrl+S`.
///
/// Invariants kept by `App::*` mutators:
///   * for any `(mime, id)`, at most one of `add[mime]` / `remove[mime]`
///     contains `id` — the inverse op clears the other.
#[derive(Debug, Clone, Default)]
pub struct PendingEdits {
    /// `Some(id)` sets a new default; `None` explicitly clears it.
    pub set_default: HashMap<String, Option<String>>,
    pub add: HashMap<String, HashSet<String>>,
    pub remove: HashMap<String, HashSet<String>>,
    /// For each `(mime, app_id)` pair currently in `remove`, a snapshot of
    /// what `set_default[mime]` looked like *before* `App::action_remove_assoc`
    /// ran its default-clearing cascade. `undo_remove` restores from here so
    /// the user's prior default intent (whether on-disk or pending) isn't
    /// silently lost when they undo a remove.
    ///
    /// Inner value:
    ///   * `None`           — `set_default[mime]` had no entry; restore by
    ///                        dropping the cascade's `Some(None)` insert.
    ///   * `Some(None)`     — `set_default[mime]` was `Some(None)` (pending
    ///                        explicit clear); restore that.
    ///   * `Some(Some(x))`  — `set_default[mime]` was `Some(Some(x))` (the
    ///                        user had pending-set an app as default); restore.
    ///
    /// Cleared by [`PendingEdits::clear`] and invalidated for a given mime
    /// when the user takes any explicit default action on that mime (their
    /// intent supersedes the cascade).
    pub remove_default_snapshot: HashMap<(String, String), Option<Option<String>>>,
}

impl PendingEdits {
    pub fn is_empty(&self) -> bool {
        self.set_default.is_empty() && self.add.is_empty() && self.remove.is_empty()
    }

    pub fn clear(&mut self) {
        self.set_default.clear();
        self.add.clear();
        self.remove.clear();
        self.remove_default_snapshot.clear();
    }

    pub fn count(&self) -> usize {
        self.set_default.len()
            + self.add.values().map(|s| s.len()).sum::<usize>()
            + self.remove.values().map(|s| s.len()).sum::<usize>()
    }

    pub fn set_default(&mut self, mime: &str, id: Option<&str>) {
        self.set_default
            .insert(mime.to_string(), id.map(|s| s.to_string()));
    }

    pub fn add_assoc(&mut self, mime: &str, id: &str) {
        if let Some(s) = self.remove.get_mut(mime) {
            s.remove(id);
            if s.is_empty() {
                self.remove.remove(mime);
            }
        }
        self.add
            .entry(mime.to_string())
            .or_default()
            .insert(id.to_string());
    }

    pub fn remove_assoc(&mut self, mime: &str, id: &str) {
        if let Some(s) = self.add.get_mut(mime) {
            s.remove(id);
            if s.is_empty() {
                self.add.remove(mime);
            }
        }
        self.remove
            .entry(mime.to_string())
            .or_default()
            .insert(id.to_string());
    }

    /// Undo a pending removal without otherwise touching the edit set. Used
    /// by the `r`-toggle: pressing `r` on a pending-removed row should bring
    /// the row back to its on-disk state, *not* coerce it into pending.add
    /// (which is what `add_assoc` would do).
    ///
    /// Also restores any `set_default[mime]` value that was clobbered by the
    /// matching `action_remove_assoc` cascade — see
    /// [`remove_default_snapshot`](Self::remove_default_snapshot).
    pub fn undo_remove(&mut self, mime: &str, id: &str) {
        if let Some(s) = self.remove.get_mut(mime) {
            s.remove(id);
            if s.is_empty() {
                self.remove.remove(mime);
            }
        }
        if let Some(prior) = self
            .remove_default_snapshot
            .remove(&(mime.to_string(), id.to_string()))
        {
            match prior {
                Some(v) => {
                    self.set_default.insert(mime.to_string(), v);
                }
                None => {
                    self.set_default.remove(mime);
                }
            }
        }
    }

    /// Drop a pending add for (mime, id) without writing anything to the
    /// remove set. Used by `App::action_remove_assoc` to express
    /// "add-then-remove → no edit" when the row never existed on disk.
    pub fn cancel_add(&mut self, mime: &str, id: &str) {
        if let Some(s) = self.add.get_mut(mime) {
            s.remove(id);
            if s.is_empty() {
                self.add.remove(mime);
            }
        }
    }
}
