use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn sample_world() -> (Vec<DesktopApp>, Vec<MimeType>, OnDiskAssoc) {
    let apps = vec![
        DesktopApp {
            id: "firefox.desktop".into(),
            name: "Firefox".into(),
            comment: "".into(),
            exec: "firefox".into(),
            terminal: false,
            mime_types: vec!["text/html".into()],
            category: "Network".into(),
        },
        DesktopApp {
            id: "chromium.desktop".into(),
            name: "Chromium".into(),
            comment: "".into(),
            exec: "chromium".into(),
            terminal: false,
            mime_types: vec!["text/html".into()],
            category: "Network".into(),
        },
    ];
    let mimes = vec![MimeType {
        id: "text/html".into(),
        description: "HTML document".into(),
    }];
    (apps, mimes, OnDiskAssoc::default())
}

#[test]
fn set_default_records_pending_and_marks_dirty() {
    let (apps, mimes, assoc) = sample_world();
    let mut app = App::for_test(apps, mimes, assoc);
    app.action_set_default("text/html", "firefox.desktop");
    assert!(app.is_dirty());
    assert_eq!(app.pending.count(), 1);
    // effective_default_for reflects it immediately.
    assert_eq!(
        app.effective_default_for("text/html").map(|a| a.id.as_str()),
        Some("firefox.desktop"),
    );
}

#[test]
fn remove_assoc_filters_from_effective_view() {
    let (apps, mimes, assoc) = sample_world();
    let mut app = App::for_test(apps, mimes, assoc);
    assert_eq!(app.effective_associations_for("text/html").len(), 2);
    app.action_remove_assoc("text/html", "firefox.desktop");
    let remaining: Vec<&str> = app
        .effective_associations_for("text/html")
        .iter()
        .map(|a| a.id.as_str())
        .collect();
    assert_eq!(remaining, vec!["chromium.desktop"]);
}

#[test]
fn is_pending_row_flags_pending_add() {
    let (apps, mimes, mut assoc) = sample_world();
    // Treat firefox as a fresh app that *doesn't* declare text/html on
    // disk, so action_add_assoc actually lands in pending.add (not a
    // no-op against the declared-only baseline).
    assoc
        .added
        .entry("text/html".into())
        .or_default()
        .insert("chromium.desktop".into());
    let mut app = App::for_test(apps, mimes, assoc);
    // Remove firefox from declarations so add is unambiguous.
    app.apps[0].mime_types.clear();
    app.action_add_assoc("text/html", "firefox.desktop");
    assert!(app.is_pending_row("text/html", "firefox.desktop"));
    // chromium is on disk, no pending edit → not pending.
    assert!(!app.is_pending_row("text/html", "chromium.desktop"));
}

#[test]
fn is_pending_row_flags_new_and_old_default_on_default_change() {
    let (apps, mimes, mut assoc) = sample_world();
    // On-disk default: firefox.
    assoc
        .defaults
        .insert("text/html".into(), "firefox.desktop".into());
    let mut app = App::for_test(apps, mimes, assoc);
    app.action_set_default("text/html", "chromium.desktop");
    // chromium becomes the new default → pending.
    assert!(app.is_pending_row("text/html", "chromium.desktop"));
    // firefox loses default → also pending (its star is going away).
    assert!(app.is_pending_row("text/html", "firefox.desktop"));
}

#[test]
fn is_pending_row_flags_old_default_on_clear() {
    let (apps, mimes, mut assoc) = sample_world();
    assoc
        .defaults
        .insert("text/html".into(), "firefox.desktop".into());
    let mut app = App::for_test(apps, mimes, assoc);
    app.action_clear_default("text/html");
    assert!(app.is_pending_row("text/html", "firefox.desktop"));
    // Unrelated app is unaffected.
    assert!(!app.is_pending_row("text/html", "chromium.desktop"));
}

#[test]
fn is_pending_row_flags_pending_remove() {
    let (apps, mimes, mut assoc) = sample_world();
    assoc
        .added
        .entry("text/html".into())
        .or_default()
        .insert("chromium.desktop".into());
    let mut app = App::for_test(apps, mimes, assoc);
    app.action_remove_assoc("text/html", "chromium.desktop");
    assert!(app.is_pending_row("text/html", "chromium.desktop"));
    assert!(app.is_pending_removed_row("text/html", "chromium.desktop"));
}

#[test]
fn displayable_assoc_keeps_pending_removed_rows() {
    let (apps, mimes, mut assoc) = sample_world();
    // Both apps declare text/html via .desktop, so both appear in the
    // effective list. After removing one, the effective list drops it
    // but the displayable list keeps it.
    assoc
        .defaults
        .insert("text/html".into(), "firefox.desktop".into());
    let mut app = App::for_test(apps, mimes, assoc);
    assert_eq!(app.effective_associations_for("text/html").len(), 2);

    app.action_remove_assoc("text/html", "chromium.desktop");
    assert_eq!(
        app.effective_associations_for("text/html").len(),
        1,
        "effective list drops the pending-removed row"
    );

    let displayable = app.displayable_associations_for("text/html");
    assert_eq!(
        displayable.len(),
        2,
        "displayable list keeps the pending-removed row visible"
    );
    let removed_flags: Vec<(String, bool)> = displayable
        .iter()
        .map(|(a, r)| (a.id.clone(), *r))
        .collect();
    assert!(removed_flags.contains(&("chromium.desktop".into(), true)));
    assert!(removed_flags.contains(&("firefox.desktop".into(), false)));
}

#[test]
fn displayable_mime_list_for_app_keeps_pending_removed_rows() {
    let (apps, mimes, mut assoc) = sample_world();
    assoc
        .defaults
        .insert("text/html".into(), "firefox.desktop".into());
    let mut app = App::for_test(apps, mimes, assoc);

    // Baseline: chromium has Associated relation for text/html (it
    // declares the mime in its .desktop).
    let baseline = app.displayable_mime_list_for_app("chromium.desktop");
    assert_eq!(baseline.len(), 1);
    assert_eq!(baseline[0].1, Relation::Associated);
    assert!(!baseline[0].2, "no pending edits → not flagged removed");

    app.action_remove_assoc("text/html", "chromium.desktop");

    // Displayable view keeps the row with its *pre-remove* relation
    // (Associated, not the post-remove DeclaredOnly fallthrough), and
    // sets the pending-removed flag so the UI can strikethrough it.
    let after = app.displayable_mime_list_for_app("chromium.desktop");
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].0.id, "text/html");
    assert_eq!(after[0].1, Relation::Associated);
    assert!(after[0].2, "expected pending-removed flag true");
}

#[test]
fn removing_the_default_cascades_to_clear_it() {
    // firefox is the on-disk default for text/html. Removing it
    // should also clear the default so we don't leave a dangling
    // `[Default Applications]` entry pointing at a row that's
    // simultaneously in `[Removed Associations]`.
    let (apps, mimes, mut assoc) = sample_world();
    assoc
        .defaults
        .insert("text/html".into(), "firefox.desktop".into());
    let mut app = App::for_test(apps, mimes, assoc);

    let cleared = app.action_remove_assoc("text/html", "firefox.desktop");
    assert!(cleared, "should signal that the default was also cleared");

    // pending.set_default reflects the cascade: Some(None) means
    // "explicitly clear the default on save".
    let slot = app
        .pending
        .set_default
        .get("text/html")
        .expect("default change should be staged");
    assert!(slot.is_none(), "expected Some(None), got {:?}", slot);

    // The displayable view *keeps* the ★ on the strikethrough row so it
    // stays in the Default bucket — otherwise the row would jump from the
    // top of the list into Associated, which is disorienting and breaks
    // "repeated `r` walks down the list". The cascade still applies on
    // save (`pending.set_default` is Some(None)); this only affects
    // display grouping until commit.
    let displayable = app.displayable_mime_list_for_app("firefox.desktop");
    let row = displayable
        .iter()
        .find(|(m, _, _)| m.id == "text/html")
        .expect("firefox should still appear in displayable list");
    assert!(row.2, "pending-removed flag should be true");
    assert_eq!(row.1, Relation::Default);
}

#[test]
fn removing_default_keeps_row_in_default_bucket_for_display() {
    // Two-mime regression test for the ordering fix: when the cursor sits
    // on a Default row and the user presses `r`, the row must stay in the
    // Default bucket (top of the list) rather than dropping into
    // Associated. Otherwise it visually jumps down and breaks "tap `r` to
    // walk down the list".
    let (apps, mut mimes, mut assoc) = sample_world();
    // Add a second mime so we can verify *position*, not just relation.
    mimes.push(MimeType {
        id: "text/plain".into(),
        description: "Plain text".into(),
    });
    // firefox is default for *both* mimes. text/html is alphabetically
    // first, so it should be at index 0, text/plain at index 1.
    assoc
        .defaults
        .insert("text/html".into(), "firefox.desktop".into());
    assoc
        .defaults
        .insert("text/plain".into(), "firefox.desktop".into());
    // Make firefox handle both mimes so it shows up at all.
    let mut apps = apps;
    apps[0].mime_types.push("text/plain".into());
    let mut app = App::for_test(apps, mimes, assoc);

    let before = app.displayable_mime_list_for_app("firefox.desktop");
    assert_eq!(before.len(), 2);
    assert_eq!(before[0].0.id, "text/html");
    assert_eq!(before[0].1, Relation::Default);
    assert_eq!(before[1].0.id, "text/plain");
    assert_eq!(before[1].1, Relation::Default);

    // Remove firefox from text/html (the top row).
    app.action_remove_assoc("text/html", "firefox.desktop");

    let after = app.displayable_mime_list_for_app("firefox.desktop");
    assert_eq!(after.len(), 2, "row stays visible with strikethrough");
    // Critical: text/html is still at index 0 and still in the Default
    // bucket. Without the fix it would have dropped to Associated and
    // moved below text/plain.
    assert_eq!(after[0].0.id, "text/html");
    assert_eq!(after[0].1, Relation::Default);
    assert!(after[0].2, "pending-removed flag should be set");
    assert_eq!(after[1].0.id, "text/plain");
    assert_eq!(after[1].1, Relation::Default);
}

#[test]
fn repeated_r_walks_cursor_down_in_by_app_view() {
    // End-to-end: with the cursor on row 0 of the by-app right pane,
    // pressing `r` should mark the row removed AND advance the cursor to
    // row 1, so a second `r` removes the next row. Without the cursor
    // bump, the second `r` would un-remove row 0 (the toggle).
    let (mut apps, mut mimes, mut assoc) = sample_world();
    // Extend the world so firefox handles three mimes — gives us room to
    // verify the cursor walks through them.
    mimes.push(MimeType {
        id: "text/plain".into(),
        description: "Plain text".into(),
    });
    mimes.push(MimeType {
        id: "text/xml".into(),
        description: "XML".into(),
    });
    apps[0].mime_types = vec![
        "text/html".into(),
        "text/plain".into(),
        "text/xml".into(),
    ];
    assoc
        .defaults
        .insert("text/html".into(), "firefox.desktop".into());
    let mut app = App::for_test(apps, mimes, assoc);
    app.view = View::ByApp;
    app.focus = Focus::Right;
    app.selected_left = 0; // firefox
    app.selected_right = 0; // text/html (Default bucket, alphabetically first)

    let r = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE);

    // First r: removes text/html, cursor advances to row 1 (text/plain).
    crate::events::handle_key(&mut app, r).unwrap();
    assert_eq!(app.selected_right, 1);
    assert!(app.is_pending_removed_row("text/html", "firefox.desktop"));
    assert!(!app.is_pending_removed_row("text/plain", "firefox.desktop"));

    // Second r: removes text/plain, cursor advances to row 2 (text/xml).
    crate::events::handle_key(&mut app, r).unwrap();
    assert_eq!(app.selected_right, 2);
    assert!(app.is_pending_removed_row("text/plain", "firefox.desktop"));

    // Third r: removes text/xml, cursor clamps at row 2 (last row).
    crate::events::handle_key(&mut app, r).unwrap();
    assert_eq!(app.selected_right, 2);
    assert!(app.is_pending_removed_row("text/xml", "firefox.desktop"));
}

#[test]
fn r_on_already_removed_row_undoes_and_stays_put() {
    // The undo branch must NOT advance the cursor — the user is
    // correcting their last action, not progressing through the list.
    let (apps, mimes, assoc) = sample_world();
    let mut app = App::for_test(apps, mimes, assoc);
    app.view = View::ByApp;
    app.focus = Focus::Right;
    app.selected_left = 0; // firefox
    app.selected_right = 0;

    let r = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE);

    // First r: removes text/html, cursor would advance — but there's only
    // one row, so it clamps back to 0.
    crate::events::handle_key(&mut app, r).unwrap();
    assert!(app.is_pending_removed_row("text/html", "firefox.desktop"));
    assert_eq!(app.selected_right, 0);

    // Reset to row 0 explicitly (in case the one-row world is masking
    // movement) and undo.
    app.selected_right = 0;
    crate::events::handle_key(&mut app, r).unwrap();
    assert!(!app.is_pending_removed_row("text/html", "firefox.desktop"));
    assert_eq!(app.selected_right, 0, "undo must not advance the cursor");
}

#[test]
fn undo_remove_restores_on_disk_default() {
    // The original fix-3 scenario. firefox is on-disk default for text/html.
    // `r` cascades and stages `set_default[mime] = None`. `r` again toggles
    // the remove off — and must also drop the cascade'd clear, otherwise
    // the user would silently lose the default on save despite "undoing"
    // their remove.
    let (apps, mimes, mut assoc) = sample_world();
    assoc
        .defaults
        .insert("text/html".into(), "firefox.desktop".into());
    let mut app = App::for_test(apps, mimes, assoc);

    app.action_remove_assoc("text/html", "firefox.desktop");
    assert_eq!(
        app.pending.set_default.get("text/html"),
        Some(&None),
        "cascade should stage a clear",
    );

    app.action_undo_remove("text/html", "firefox.desktop");
    assert!(
        !app.pending.set_default.contains_key("text/html"),
        "undo must drop the cascade'd clear so the on-disk default stands",
    );
    assert_eq!(
        app.effective_default_for("text/html").map(|a| a.id.as_str()),
        Some("firefox.desktop"),
        "on-disk default should be effective again after undo",
    );
}

#[test]
fn undo_remove_restores_pending_default_when_d_preceded_r() {
    // chromium is on-disk default; firefox is not. User pending-sets
    // firefox via `d`, then removes firefox via `r`, then undoes. The
    // restored state should bring back the pending `d` (firefox), not
    // silently leave chromium effective.
    let (apps, mimes, mut assoc) = sample_world();
    assoc
        .defaults
        .insert("text/html".into(), "chromium.desktop".into());
    let mut app = App::for_test(apps, mimes, assoc);

    app.action_set_default("text/html", "firefox.desktop");
    assert_eq!(
        app.pending.set_default.get("text/html"),
        Some(&Some("firefox.desktop".to_string())),
    );

    app.action_remove_assoc("text/html", "firefox.desktop");
    assert!(
        !app.pending.set_default.contains_key("text/html"),
        "cascade should have dropped the pending default-change entry",
    );

    app.action_undo_remove("text/html", "firefox.desktop");
    assert_eq!(
        app.pending.set_default.get("text/html"),
        Some(&Some("firefox.desktop".to_string())),
        "undo should restore the user's prior `d` intent",
    );
}

#[test]
fn explicit_clear_after_remove_supersedes_snapshot() {
    // r firefox (default) → c (explicit clear) → r undo. The explicit
    // clear should win — the undo must not restore the on-disk default,
    // because the user has separately and explicitly cleared it.
    let (apps, mimes, mut assoc) = sample_world();
    assoc
        .defaults
        .insert("text/html".into(), "firefox.desktop".into());
    let mut app = App::for_test(apps, mimes, assoc);

    app.action_remove_assoc("text/html", "firefox.desktop");
    app.action_clear_default("text/html");
    assert_eq!(app.pending.set_default.get("text/html"), Some(&None));

    app.action_undo_remove("text/html", "firefox.desktop");
    assert_eq!(
        app.pending.set_default.get("text/html"),
        Some(&None),
        "user's explicit clear must outlive the undo-remove restore",
    );
}

#[test]
fn explicit_d_after_remove_supersedes_snapshot() {
    // r firefox (default) → d chromium → r undo. The user's `d` is the
    // active intent; undo must not roll it back to firefox.
    let (apps, mimes, mut assoc) = sample_world();
    assoc
        .defaults
        .insert("text/html".into(), "firefox.desktop".into());
    let mut app = App::for_test(apps, mimes, assoc);

    app.action_remove_assoc("text/html", "firefox.desktop");
    app.action_set_default("text/html", "chromium.desktop");
    app.action_undo_remove("text/html", "firefox.desktop");

    assert_eq!(
        app.pending.set_default.get("text/html"),
        Some(&Some("chromium.desktop".to_string())),
        "the later `d` chromium wins",
    );
}

#[test]
fn remove_non_default_takes_no_snapshot() {
    // Sanity check: removing a non-default row must not stash a snapshot,
    // otherwise an unrelated `c` followed by undo would inadvertently
    // restore a default that the user never had.
    let (apps, mimes, mut assoc) = sample_world();
    assoc
        .defaults
        .insert("text/html".into(), "firefox.desktop".into());
    let mut app = App::for_test(apps, mimes, assoc);

    app.action_remove_assoc("text/html", "chromium.desktop");
    assert!(
        app.pending
            .remove_default_snapshot
            .get(&("text/html".to_string(), "chromium.desktop".to_string()))
            .is_none(),
        "non-default remove shouldn't capture a snapshot",
    );
}

#[test]
fn removing_non_default_does_not_clear_default() {
    // chromium is associated but not the default — removing it
    // mustn't touch the default for the mime.
    let (apps, mimes, mut assoc) = sample_world();
    assoc
        .defaults
        .insert("text/html".into(), "firefox.desktop".into());
    let mut app = App::for_test(apps, mimes, assoc);

    let cleared = app.action_remove_assoc("text/html", "chromium.desktop");
    assert!(!cleared);
    assert!(
        !app.pending.set_default.contains_key("text/html"),
        "default state must be untouched when removing a non-default row"
    );
}

#[test]
fn removing_phantom_default_cascades_clear_too() {
    // Loupe is the on-disk default for image/jxl but isn't installed
    // — the phantom-default case the user surfaced. Removing it
    // should clear the default so saving doesn't leave an orphan
    // `[Default Applications]` line behind.
    let (apps, mimes, mut assoc) = sample_world();
    assoc
        .defaults
        .insert("image/jxl".into(), "org.gnome.Loupe.desktop".into());
    let mut mimes = mimes;
    mimes.push(MimeType {
        id: "image/jxl".into(),
        description: "JPEG XL image".into(),
    });
    let mut app = App::for_test(apps, mimes, assoc);

    let cleared = app.action_remove_assoc("image/jxl", "org.gnome.Loupe.desktop");
    assert!(cleared);
    let slot = app.pending.set_default.get("image/jxl").unwrap();
    assert!(slot.is_none(), "phantom default should be cleared too");
}

#[test]
fn remove_cancels_pending_add_for_off_disk_row() {
    // Build a world where firefox is the only declared handler for
    // text/html, so adding a third app means a pure pending.add row
    // with no on-disk anchor.
    let (mut apps, mimes, assoc) = sample_world();
    apps.push(DesktopApp {
        id: "elinks.desktop".into(),
        name: "ELinks".into(),
        comment: "".into(),
        exec: "elinks".into(),
        terminal: true,
        mime_types: vec![], // doesn't declare text/html
        category: "Network".into(),
    });
    let mut app = App::for_test(apps, mimes, assoc);

    // Add an off-disk app via the picker path.
    app.action_add_assoc("text/html", "elinks.desktop");
    assert!(app.is_dirty());
    assert_eq!(app.pending.count(), 1);

    // Pressing `r` on this newly-added row should fully undo the edit —
    // no pending.remove entry, no lingering pending.add.
    app.action_remove_assoc("text/html", "elinks.desktop");
    assert_eq!(
        app.pending.count(),
        0,
        "added-then-removed off-disk row should leave pending edits empty"
    );
    assert!(!app.is_dirty());
    // And it disappears from the displayable list (no on-disk anchor,
    // no pending edit to keep it visible).
    let displayable: Vec<&str> = app
        .displayable_associations_for("text/html")
        .iter()
        .map(|(a, _)| a.id.as_str())
        .collect();
    assert!(
        !displayable.contains(&"elinks.desktop"),
        "row should vanish, not stay as strikethrough"
    );
}

#[test]
fn remove_cancels_pending_default_for_off_disk_row() {
    // User adds an off-disk app, sets it as default, then removes it.
    // The pending default should be cleaned up too — otherwise we'd
    // leave a dangling default pointing at an unassociated app.
    let (mut apps, mimes, assoc) = sample_world();
    apps.push(DesktopApp {
        id: "elinks.desktop".into(),
        name: "ELinks".into(),
        comment: "".into(),
        exec: "elinks".into(),
        terminal: true,
        mime_types: vec![],
        category: "Network".into(),
    });
    let mut app = App::for_test(apps, mimes, assoc);

    app.action_add_assoc("text/html", "elinks.desktop");
    app.action_set_default("text/html", "elinks.desktop");
    assert!(app.pending.set_default.contains_key("text/html"));

    app.action_remove_assoc("text/html", "elinks.desktop");
    assert!(
        !app.pending.set_default.contains_key("text/html"),
        "pending.set_default should be cleaned up when its target is cancelled"
    );
    assert_eq!(app.pending.count(), 0);
}

#[test]
fn remove_of_on_disk_row_still_uses_pending_remove() {
    // Sanity: the off-disk shortcut must not change the behaviour of
    // removing a row that *does* have an on-disk anchor.
    let (apps, mimes, mut assoc) = sample_world();
    // chromium is declared via .desktop already; also put it in
    // [Added Associations] to make sure both paths are exercised.
    assoc
        .added
        .entry("text/html".into())
        .or_default()
        .insert("chromium.desktop".into());
    let mut app = App::for_test(apps, mimes, assoc);

    app.action_remove_assoc("text/html", "chromium.desktop");
    assert!(app.is_pending_removed_row("text/html", "chromium.desktop"));
    assert_eq!(app.pending.count(), 1);
}

#[test]
fn current_target_is_phantom_in_by_app_when_phantom_selected() {
    let (apps, mimes, mut assoc) = sample_world();
    assoc
        .defaults
        .insert("image/png".into(), "org.gnome.Loupe.desktop".into());
    let mut app = App::for_test(apps, mimes, assoc);

    // visible_apps order: phantoms first, then installed.
    let visible: Vec<String> =
        app.visible_apps().iter().map(|a| a.id.clone()).collect();
    let loupe_idx = visible
        .iter()
        .position(|id| id == "org.gnome.Loupe.desktop")
        .expect("phantom should appear in visible_apps");

    app.view = View::ByApp;
    app.selected_left = loupe_idx;
    assert!(
        app.current_target_is_phantom(),
        "by-app + phantom highlighted → target is phantom"
    );

    // Pick an installed app instead → not phantom.
    let firefox_idx = visible
        .iter()
        .position(|id| id == "firefox.desktop")
        .expect("firefox should be in visible_apps");
    app.selected_left = firefox_idx;
    assert!(!app.current_target_is_phantom());
}

#[test]
fn current_target_is_phantom_in_by_mime_when_right_row_is_phantom() {
    let (apps, mimes, mut assoc) = sample_world();
    assoc
        .defaults
        .insert("image/png".into(), "org.gnome.Loupe.desktop".into());
    let mut mimes = mimes;
    mimes.push(MimeType {
        id: "image/png".into(),
        description: "PNG image".into(),
    });
    let mut app = App::for_test(apps, mimes, assoc);

    app.view = View::ByMime;
    // Position the left pane on image/png.
    let vis: Vec<String> =
        app.visible_mimes().iter().map(|m| m.id.clone()).collect();
    let png_idx = vis.iter().position(|id| id == "image/png").unwrap();
    app.selected_left = png_idx;

    // displayable_associations_for image/png includes 0 installed
    // apps (neither firefox nor chromium declares image/png) plus
    // the phantom Loupe. So selected_right=0 → the phantom row.
    let displayable = app.displayable_associations_for("image/png");
    let installed_count = displayable.len();
    app.selected_right = installed_count; // first phantom row
    assert!(app.current_target_is_phantom());
}

#[test]
fn phantom_apps_synthesised_from_uninstalled_default() {
    let (apps, mimes, mut assoc) = sample_world();
    assoc
        .defaults
        .insert("image/png".into(), "org.gnome.Loupe.desktop".into());
    let app = App::for_test(apps, mimes, assoc);

    assert!(app.is_phantom_app("org.gnome.Loupe.desktop"));
    assert!(!app.is_phantom_app("firefox.desktop"));
    // Phantom is materialised as a DesktopApp with id=name and a
    // (not installed) comment so the right-pane summary lands clean.
    let phantom = app
        .phantom_apps
        .iter()
        .find(|a| a.id == "org.gnome.Loupe.desktop")
        .expect("phantom record should be synthesised");
    assert_eq!(phantom.name, "org.gnome.Loupe.desktop");
    assert!(phantom.comment.contains("not installed"));
}

#[test]
fn visible_apps_lists_phantoms_before_installed() {
    let (apps, mimes, mut assoc) = sample_world();
    assoc
        .defaults
        .insert("image/png".into(), "org.gnome.Loupe.desktop".into());
    let app = App::for_test(apps, mimes, assoc);

    let visible = app.visible_apps();
    // The two installed apps (firefox, chromium) plus the phantom Loupe.
    assert_eq!(visible.len(), 3);
    // Phantoms are surfaced first — they're the cleanup items the
    // user is most likely looking for in this view.
    assert_eq!(visible[0].id, "org.gnome.Loupe.desktop");
    let installed_ids: Vec<&str> =
        visible[1..].iter().map(|a| a.id.as_str()).collect();
    assert!(installed_ids.contains(&"firefox.desktop"));
    assert!(installed_ids.contains(&"chromium.desktop"));
}

#[test]
fn picker_apps_matching_excludes_phantoms() {
    // The picker offers apps for new associations — phantoms must
    // not show up there since associating a non-installed app makes
    // no sense.
    let (apps, mimes, mut assoc) = sample_world();
    assoc
        .defaults
        .insert("image/png".into(), "org.gnome.Loupe.desktop".into());
    let app = App::for_test(apps, mimes, assoc);

    let picker = app.apps_matching("loupe");
    assert!(
        picker.iter().all(|a| a.id != "org.gnome.Loupe.desktop"),
        "phantom Loupe leaked into the picker results"
    );
}

#[test]
fn mime_has_missing_is_false_when_all_assocs_are_installed() {
    let (apps, mimes, mut assoc) = sample_world();
    // firefox is the default for text/html and it's installed.
    assoc
        .defaults
        .insert("text/html".into(), "firefox.desktop".into());
    assoc
        .added
        .entry("text/html".into())
        .or_default()
        .insert("chromium.desktop".into());
    let app = App::for_test(apps, mimes, assoc);
    assert!(!app.mime_has_missing("text/html"));
}

#[test]
fn mime_has_missing_is_true_when_default_app_is_phantom() {
    // The Loupe case: default points at a non-installed .desktop.
    let (apps, mimes, mut assoc) = sample_world();
    assoc
        .defaults
        .insert("image/png".into(), "org.gnome.Loupe.desktop".into());
    let mut mimes = mimes;
    mimes.push(MimeType {
        id: "image/png".into(),
        description: "PNG image".into(),
    });
    let app = App::for_test(apps, mimes, assoc);
    assert!(app.mime_has_missing("image/png"));
}

#[test]
fn mime_has_missing_is_true_when_added_app_is_phantom() {
    // Default is fine, but [Added Associations] points at a phantom.
    let (apps, mimes, mut assoc) = sample_world();
    assoc
        .defaults
        .insert("image/png".into(), "firefox.desktop".into());
    assoc
        .added
        .entry("image/png".into())
        .or_default()
        .insert("org.gnome.Loupe.desktop".into());
    let mut mimes = mimes;
    mimes.push(MimeType {
        id: "image/png".into(),
        description: "PNG image".into(),
    });
    let app = App::for_test(apps, mimes, assoc);
    assert!(app.mime_has_missing("image/png"));
}

#[test]
fn missing_associations_surface_uninstalled_default_and_added() {
    // image/png has firefox as default (installed) AND a phantom
    // org.gnome.Loupe.desktop in added (not in the installed apps
    // index). image/jxl has only the phantom Loupe as the default.
    let (apps, mimes, mut assoc) = sample_world();
    assoc
        .defaults
        .insert("image/png".into(), "firefox.desktop".into());
    assoc
        .added
        .entry("image/png".into())
        .or_default()
        .insert("org.gnome.Loupe.desktop".into());
    assoc
        .defaults
        .insert("image/jxl".into(), "org.gnome.Loupe.desktop".into());

    // Ensure the test world has the mimes too.
    let mut mimes = mimes;
    mimes.push(MimeType {
        id: "image/png".into(),
        description: "PNG image".into(),
    });
    mimes.push(MimeType {
        id: "image/jxl".into(),
        description: "JPEG XL image".into(),
    });

    let app = App::for_test(apps, mimes, assoc);

    // image/png: firefox is installed → not missing. Loupe is the
    // phantom → surfaces as missing, not the default for this mime.
    let png_missing = app.missing_associations_for("image/png");
    assert_eq!(png_missing.len(), 1);
    assert_eq!(png_missing[0].app_id, "org.gnome.Loupe.desktop");
    assert!(!png_missing[0].is_default);
    assert!(!png_missing[0].is_pending_removed);
    // Default for image/png resolves cleanly (firefox is installed).
    assert!(app.missing_default_for("image/png").is_none());

    // image/jxl: Loupe is the would-be default, but it's missing.
    let jxl_missing = app.missing_associations_for("image/jxl");
    assert_eq!(jxl_missing.len(), 1);
    assert_eq!(jxl_missing[0].app_id, "org.gnome.Loupe.desktop");
    assert!(jxl_missing[0].is_default);
    assert_eq!(
        app.missing_default_for("image/jxl").as_deref(),
        Some("org.gnome.Loupe.desktop"),
    );
}

#[test]
fn missing_associations_reflect_pending_remove_of_phantom() {
    let (apps, mimes, mut assoc) = sample_world();
    assoc
        .added
        .entry("image/png".into())
        .or_default()
        .insert("org.gnome.Loupe.desktop".into());
    let mut mimes = mimes;
    mimes.push(MimeType {
        id: "image/png".into(),
        description: "PNG image".into(),
    });
    let mut app = App::for_test(apps, mimes, assoc);

    // User stages a cleanup of the phantom.
    app.action_remove_assoc("image/png", "org.gnome.Loupe.desktop");

    let missing = app.missing_associations_for("image/png");
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].app_id, "org.gnome.Loupe.desktop");
    assert!(missing[0].is_pending_removed);
}

#[test]
fn missing_default_returns_none_when_pending_clears_it() {
    // Default on disk is the phantom, but user has staged a clear.
    // missing_default_for should return None (nothing to flag).
    let (apps, mimes, mut assoc) = sample_world();
    assoc
        .defaults
        .insert("image/png".into(), "org.gnome.Loupe.desktop".into());
    let mut mimes = mimes;
    mimes.push(MimeType {
        id: "image/png".into(),
        description: "PNG image".into(),
    });
    let mut app = App::for_test(apps, mimes, assoc);

    app.action_clear_default("image/png");
    assert!(app.missing_default_for("image/png").is_none());
}

#[test]
fn pending_change_summary_is_empty_when_clean() {
    let (apps, mimes, assoc) = sample_world();
    let app = App::for_test(apps, mimes, assoc);
    assert!(app.pending_change_summary().is_empty());
}

#[test]
fn pending_change_summary_groups_per_mime_and_sorts() {
    let (apps, mimes, mut assoc) = sample_world();
    assoc
        .defaults
        .insert("text/html".into(), "firefox.desktop".into());
    assoc
        .added
        .entry("text/html".into())
        .or_default()
        .insert("chromium.desktop".into());
    let mut app = App::for_test(apps, mimes, assoc);

    // Mixed edits across two mimes — verify grouping, sort order, and
    // that DefaultChange::Set carries the on-disk previous value.
    app.action_set_default("text/html", "chromium.desktop");
    app.action_remove_assoc("text/html", "firefox.desktop");
    app.apps[0].mime_types.clear();
    app.apps[1].mime_types.clear();
    app.action_add_assoc("application/pdf", "chromium.desktop");

    let summary = app.pending_change_summary();
    assert_eq!(summary.len(), 2);
    // Sorted by mime id: application/pdf before text/html.
    assert_eq!(summary[0].mime, "application/pdf");
    assert_eq!(summary[1].mime, "text/html");

    // application/pdf: just an add.
    assert_eq!(summary[0].default_change, None);
    assert_eq!(summary[0].adds, vec!["chromium.desktop"]);
    assert!(summary[0].removes.is_empty());

    // text/html: default change AND a remove. The default change must
    // carry the on-disk previous default in `old`.
    match &summary[1].default_change {
        Some(DefaultChange::Set { new, old }) => {
            assert_eq!(new, "chromium.desktop");
            assert_eq!(old.as_deref(), Some("firefox.desktop"));
        }
        other => panic!("expected Set default change, got {:?}", other),
    }
    assert_eq!(summary[1].removes, vec!["firefox.desktop"]);
}

#[test]
fn pending_change_summary_captures_default_cleared() {
    let (apps, mimes, mut assoc) = sample_world();
    assoc
        .defaults
        .insert("text/html".into(), "firefox.desktop".into());
    let mut app = App::for_test(apps, mimes, assoc);
    app.action_clear_default("text/html");

    let summary = app.pending_change_summary();
    assert_eq!(summary.len(), 1);
    match &summary[0].default_change {
        Some(DefaultChange::Cleared { old }) => {
            assert_eq!(old.as_deref(), Some("firefox.desktop"));
        }
        other => panic!("expected Cleared default change, got {:?}", other),
    }
}

#[test]
fn pending_change_summary_sorts_adds_and_removes_within_mime() {
    let (apps, mimes, mut assoc) = sample_world();
    // Ensure both apps have an on-disk anchor so action_remove_assoc
    // writes pending.remove rather than cancelling an add.
    assoc
        .added
        .entry("text/html".into())
        .or_default()
        .insert("chromium.desktop".into());
    assoc
        .added
        .entry("text/html".into())
        .or_default()
        .insert("firefox.desktop".into());
    let mut app = App::for_test(apps, mimes, assoc);

    // Remove in non-alphabetical order — summary must sort them.
    app.action_remove_assoc("text/html", "firefox.desktop");
    app.action_remove_assoc("text/html", "chromium.desktop");

    let summary = app.pending_change_summary();
    assert_eq!(summary.len(), 1);
    assert_eq!(
        summary[0].removes,
        vec!["chromium.desktop", "firefox.desktop"]
    );
}

#[test]
fn undo_remove_returns_row_to_clean_state() {
    let (apps, mimes, mut assoc) = sample_world();
    assoc
        .added
        .entry("text/html".into())
        .or_default()
        .insert("firefox.desktop".into());
    let mut app = App::for_test(apps, mimes, assoc);

    app.action_remove_assoc("text/html", "firefox.desktop");
    assert!(app.is_pending_removed_row("text/html", "firefox.desktop"));
    assert_eq!(app.pending.count(), 1);

    app.action_undo_remove("text/html", "firefox.desktop");

    // pending.remove is gone, and crucially pending.add is NOT populated
    // (which is what add_assoc would have done) — so the row is back to
    // its clean on-disk state with no net edit.
    assert!(!app.is_pending_removed_row("text/html", "firefox.desktop"));
    assert!(!app.is_pending_row("text/html", "firefox.desktop"));
    assert_eq!(app.pending.count(), 0);
    assert!(!app.is_dirty());
}

#[test]
fn is_pending_row_false_for_clean_state() {
    let (apps, mimes, mut assoc) = sample_world();
    assoc
        .defaults
        .insert("text/html".into(), "firefox.desktop".into());
    let app = App::for_test(apps, mimes, assoc);
    assert!(!app.is_pending_row("text/html", "firefox.desktop"));
    assert!(!app.is_pending_row("text/html", "chromium.desktop"));
}

#[test]
fn remove_then_add_clears_remove() {
    let (apps, mimes, assoc) = sample_world();
    let mut app = App::for_test(apps, mimes, assoc);
    app.action_remove_assoc("text/html", "firefox.desktop");
    app.action_add_assoc("text/html", "firefox.desktop");
    // Both add and remove for the same (mime, id) is an inconsistent
    // pending state; mutators must keep only one.
    assert!(
        !app.pending
            .remove
            .get("text/html")
            .map(|s| s.contains("firefox.desktop"))
            .unwrap_or(false),
        "add should clear an inverse remove"
    );
    assert_eq!(app.effective_associations_for("text/html").len(), 2);
}

#[test]
fn set_default_includes_app_in_associations() {
    // Even if the app doesn't declare MimeType=text/html, setting it as
    // the default should make it appear in the effective associations.
    let mut apps = vec![DesktopApp {
        id: "outlier.desktop".into(),
        name: "Outlier".into(),
        comment: "".into(),
        exec: "outlier".into(),
        terminal: false,
        mime_types: vec![], // doesn't declare text/html
        category: "Utilities".into(),
    }];
    apps.extend(sample_world().0);
    let (_, mimes, assoc) = sample_world();
    let mut app = App::for_test(apps, mimes, assoc);
    app.action_set_default("text/html", "outlier.desktop");
    assert_eq!(
        app.effective_default_for("text/html").map(|a| a.id.as_str()),
        Some("outlier.desktop"),
    );
    let assoc_ids: Vec<&str> = app
        .effective_associations_for("text/html")
        .iter()
        .map(|a| a.id.as_str())
        .collect();
    assert!(assoc_ids.contains(&"outlier.desktop"));
}

#[test]
fn save_end_to_end() {
    use std::fs;

    let dir = std::env::temp_dir().join(format!(
        "mime_tui_app_save_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mimeapps.list");

    let (apps, mimes, assoc) = sample_world();
    let mut app = App::for_test(apps, mimes, assoc);
    app.action_set_default("text/html", "firefox.desktop");
    app.action_add_assoc("text/html", "chromium.desktop"); // already declared, dedup

    // Drive the save explicitly through the storage layer so we can target
    // the temp path. (App::save() uses XDG_CONFIG_HOME which we don't
    // want to mutate during tests.)
    crate::storage::mimeapps::save_user_file_at(&path, &app.pending).unwrap();

    let content = fs::read_to_string(&path).unwrap();
    assert!(content.contains("[Default Applications]"));
    assert!(content.contains("text/html=firefox.desktop;"));
    // chromium.desktop is already-declared, but we did add it to pending —
    // it will land in [Added Associations]. Either presence (or none) is
    // acceptable; this assertion captures current behaviour.
    assert!(content.contains("text/html=chromium.desktop;"));

    let _ = fs::remove_dir_all(&dir);
}
