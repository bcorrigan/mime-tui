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

/// Re-read the file via the public reader so tests can inspect the
/// resulting `[Added]` / `[Removed]` sets section-aware, instead of
/// substring-matching across the whole text.
fn read_back(path: &Path) -> OnDiskAssoc {
    let mut assoc = OnDiskAssoc::default();
    let raw = fs::read_to_string(path).unwrap();
    merge_one(&raw, path, &mut assoc);
    assoc
}

#[test]
fn remove_strips_matching_added_entry() {
    // The bug: an app in [Added Associations] that the user removes should
    // disappear from that section on save. Before the fix, it stayed and
    // also appeared in [Removed Associations], leaving a phantom alive on
    // the next load.
    let dir = tempdir("mime_tui_remove_strips_added");
    let path = dir.join("mimeapps.list");
    fs::write(
        &path,
        "[Added Associations]\n\
         x-scheme-handler/tg=userapp-Telegram.desktop;\n\
         image/png=gimp.desktop;\n",
    )
    .unwrap();

    let mut pending = PendingEdits::default();
    pending.remove_assoc("x-scheme-handler/tg", "userapp-Telegram.desktop");

    save_user_file_at(&path, &pending).unwrap();
    let assoc = read_back(&path);

    // The whole mime key drops from [Added] since it had only one entry.
    assert!(assoc.added.get("x-scheme-handler/tg").is_none());
    // Unrelated entries stay put.
    let png = assoc.added.get("image/png").unwrap();
    assert!(png.contains("gimp.desktop"));
    // The remove is still recorded so any .desktop declaration is suppressed.
    let removed = assoc.removed.get("x-scheme-handler/tg").unwrap();
    assert!(removed.contains("userapp-Telegram.desktop"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn remove_keeps_other_ids_in_added_for_same_mime() {
    // When the same mime has multiple [Added] entries and only one is
    // being removed, the others survive.
    let dir = tempdir("mime_tui_remove_partial_added");
    let path = dir.join("mimeapps.list");
    fs::write(
        &path,
        "[Added Associations]\nimage/png=gimp.desktop;krita.desktop;\n",
    )
    .unwrap();

    let mut pending = PendingEdits::default();
    pending.remove_assoc("image/png", "gimp.desktop");

    save_user_file_at(&path, &pending).unwrap();
    let assoc = read_back(&path);

    let added = assoc.added.get("image/png").unwrap();
    assert!(added.contains("krita.desktop"));
    assert!(!added.contains("gimp.desktop"));
    let removed = assoc.removed.get("image/png").unwrap();
    assert!(removed.contains("gimp.desktop"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn load_normalizes_added_against_removed() {
    // Pre-existing contradictory state (the file produced by older
    // mime-tui versions before the apply_pending strip): same (mime, id)
    // in both [Added] and [Removed]. Load-time normalization must drop
    // the [Added] entry so phantom-detection doesn't surface an app whose
    // every relation has already been suppressed.
    let dir = tempdir("mime_tui_load_normalize");
    let path = dir.join("mimeapps.list");
    fs::write(
        &path,
        "[Added Associations]\n\
         x-scheme-handler/tg=userapp-Telegram.desktop;\n\
         image/png=gimp.desktop;\n\
         \n\
         [Removed Associations]\n\
         x-scheme-handler/tg=userapp-Telegram.desktop;\n",
    )
    .unwrap();

    let baseline = read_user_file_baseline_at(&path);
    let assoc = baseline.assoc;

    // The contradicting entry is gone from [Added]; the unrelated one stays.
    assert!(assoc.added.get("x-scheme-handler/tg").is_none());
    assert!(
        assoc
            .added
            .get("image/png")
            .unwrap()
            .contains("gimp.desktop")
    );
    // [Removed] is untouched — it remains the source of truth for the
    // suppression.
    assert!(
        assoc
            .removed
            .get("x-scheme-handler/tg")
            .unwrap()
            .contains("userapp-Telegram.desktop")
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn save_heals_existing_contradictions_even_without_pending_for_that_mime() {
    // The user's file has a stale (mime, id) in both [Added] and [Removed].
    // They save an unrelated edit. The save should *also* clean up the
    // contradiction so the file converges to a spec-consistent shape.
    let dir = tempdir("mime_tui_save_heal");
    let path = dir.join("mimeapps.list");
    fs::write(
        &path,
        "[Added Associations]\n\
         x-scheme-handler/tg=userapp-Telegram.desktop;\n\
         \n\
         [Removed Associations]\n\
         x-scheme-handler/tg=userapp-Telegram.desktop;\n",
    )
    .unwrap();

    // An unrelated pending edit — has nothing to say about x-scheme-handler/tg.
    let mut pending = PendingEdits::default();
    pending.set_default("text/html", Some("firefox.desktop"));

    save_user_file_at(&path, &pending).unwrap();
    let assoc = read_back(&path);

    // The stale [Added] entry is gone.
    assert!(assoc.added.get("x-scheme-handler/tg").is_none());
    // The [Removed] entry stays.
    assert!(
        assoc
            .removed
            .get("x-scheme-handler/tg")
            .unwrap()
            .contains("userapp-Telegram.desktop")
    );
    // The unrelated edit went through.
    assert_eq!(
        assoc.defaults.get("text/html"),
        Some(&"firefox.desktop".to_string())
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn add_strips_matching_removed_entry() {
    // Symmetric case: re-associating an app that was previously in
    // [Removed Associations] should lift it from there, not leave both
    // sections contradicting each other.
    let dir = tempdir("mime_tui_add_strips_removed");
    let path = dir.join("mimeapps.list");
    fs::write(
        &path,
        "[Removed Associations]\nimage/png=gimp.desktop;\n",
    )
    .unwrap();

    let mut pending = PendingEdits::default();
    pending.add_assoc("image/png", "gimp.desktop");

    save_user_file_at(&path, &pending).unwrap();
    let assoc = read_back(&path);

    assert!(assoc.added.get("image/png").unwrap().contains("gimp.desktop"));
    assert!(assoc.removed.get("image/png").is_none());

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
