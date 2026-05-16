use super::*;

/// Build a chunk from a single string of `width` chars. Saves boilerplate
/// in the layout tests below where the styling and action are irrelevant.
fn chunk(text: &str) -> Chunk {
    Chunk {
        spans: vec![Span::raw(text.to_string())],
        action: None,
    }
}

fn clickable_chunk(text: &str) -> Chunk {
    Chunk {
        spans: vec![Span::raw(text.to_string())],
        action: Some(key_press(KeyCode::Char('x'))),
    }
}

/// Concatenate a Line's spans into a plain String so tests can assert on
/// the visible content.
fn line_text(line: &Line<'_>) -> String {
    line.iter().map(|s| s.content.as_ref()).collect()
}

#[test]
fn single_chunk_fits_on_one_line() {
    let (lines, _) = layout_chunks(vec![chunk("hello")], 80);
    assert_eq!(lines.len(), 1);
    assert_eq!(line_text(&lines[0]), "hello");
}

#[test]
fn multiple_chunks_join_with_two_space_separator() {
    let (lines, _) = layout_chunks(
        vec![chunk("[h]"), chunk("Tab"), chunk("Esc")],
        80,
    );
    assert_eq!(lines.len(), 1);
    assert_eq!(line_text(&lines[0]), "[h]  Tab  Esc");
}

#[test]
fn wraps_when_next_chunk_would_overflow() {
    // Header 6 wide; each hint 3 wide; sep 2 wide. indent_w = 6+2 = 8.
    // width=30 → indent_w*3=24 ≤ 30, so indent IS applied on wrap.
    // line 1: "[hdr]   xxx  yyy  zzz  www" = 6+2+3+2+3+2+3+2+3 = 26 (fits)
    //   adding "vvv" needs +2+3 = +5 → 31 > 30 → wrap before vvv.
    // line 2: 8 spaces + "vvv  uuu" = 8 + 3+2+3 = 16.
    let (lines, _) = layout_chunks(
        vec![
            chunk("[hdr] "),
            chunk("xxx"),
            chunk("yyy"),
            chunk("zzz"),
            chunk("www"),
            chunk("vvv"),
            chunk("uuu"),
        ],
        30,
    );
    assert_eq!(lines.len(), 2, "expected exactly 2 lines, got {:?}",
        lines.iter().map(line_text).collect::<Vec<_>>());
    assert_eq!(line_text(&lines[0]), "[hdr]   xxx  yyy  zzz  www");
    assert_eq!(line_text(&lines[1]), "        vvv  uuu");
}

#[test]
fn wrap_never_breaks_inside_a_chunk() {
    // A chunk like ["a", ":add mime"] is one atomic unit — wrap must
    // keep it whole even if `a:add` would fit on the previous line.
    let (lines, _) = layout_chunks(
        vec![
            chunk("[h] "),
            chunk("filler"),
            Chunk {
                spans: vec![Span::raw("a"), Span::raw(":add mime")],
                action: None,
            },
        ],
        14,
    );
    assert!(lines.len() >= 2);
    let joined: String = lines.iter().map(line_text).collect();
    assert!(joined.contains("a:add mime"));
}

#[test]
fn wrapped_lines_indent_to_align_with_first_hint() {
    // Header chunk is 9 wide ("[by-mime]"). indent_w = 9 + 2 = 11.
    // With width 50, indent_w*3 = 33 ≤ 50 → indent applied.
    let (lines, _) = layout_chunks(
        vec![
            chunk("[by-mime]"),
            chunk("Tab: view"),
            chunk("d:set default"),
            chunk("r:remove app"),
            chunk("c:clear default"),
            chunk("a:add app"),
            chunk("Ctrl-S:save"),
        ],
        50,
    );
    assert!(lines.len() >= 2);
    for (i, line) in lines.iter().enumerate().skip(1) {
        let text = line_text(line);
        assert!(
            text.starts_with("           "), // 11 spaces
            "line {} should start with 11-space indent: {:?}",
            i,
            text
        );
        assert_ne!(text.chars().nth(11), Some(' '),
            "line {} should have non-space after the indent: {:?}", i, text);
    }
}

#[test]
fn very_narrow_terminal_drops_the_indent() {
    let (lines, _) = layout_chunks(
        vec![
            chunk("[long-header] "),
            chunk("a"),
            chunk("b"),
            chunk("c"),
        ],
        18,
    );
    for (i, line) in lines.iter().enumerate().skip(1) {
        let text = line_text(line);
        assert!(
            !text.starts_with(' '),
            "line {} should NOT be indented on a narrow terminal: {:?}",
            i,
            text
        );
    }
}

#[test]
fn chunk_wider_than_width_still_renders() {
    let (lines, _) = layout_chunks(
        vec![chunk("this header is wider than the width"), chunk("x")],
        10,
    );
    assert!(line_text(&lines[0]).contains("wider than"));
    assert!(lines.len() >= 2);
}

#[test]
fn empty_chunks_yields_one_blank_line() {
    let (lines, _) = layout_chunks(vec![], 80);
    assert_eq!(lines.len(), 1);
    assert_eq!(line_text(&lines[0]), "");
}

#[test]
fn zero_width_does_not_panic() {
    let _ = layout_chunks(vec![chunk("a"), chunk("b")], 0);
}

#[test]
fn first_chunk_never_gets_leading_separator() {
    let (lines, _) = layout_chunks(
        vec![chunk("[h] "), chunk("aaaaaaaaaa"), chunk("bbbbbbbbbb")],
        14,
    );
    for line in &lines {
        let text = line_text(line);
        assert!(
            !text.trim_start().starts_with(' '),
            "line should not begin with the chunk separator: {:?}",
            text
        );
    }
}

// ── click positions ────────────────────────────────────────────────

#[test]
fn clickable_chunks_report_their_starting_column() {
    // "[h]" 3 wide, then sep 2 wide, then "abc" at col 5.
    // Only the second chunk is clickable.
    let (_, positions) = layout_chunks(
        vec![chunk("[h]"), clickable_chunk("abc")],
        80,
    );
    assert_eq!(positions.len(), 1);
    let (line_idx, col, w, _) = positions[0];
    assert_eq!(line_idx, 0);
    assert_eq!(col, 5);
    assert_eq!(w, 3);
}

#[test]
fn non_clickable_chunks_do_not_emit_positions() {
    let (_, positions) = layout_chunks(
        vec![chunk("[h]"), chunk("not-clickable")],
        80,
    );
    assert!(positions.is_empty());
}

#[test]
fn wrapped_clickable_chunks_report_correct_line_and_indent() {
    // header 11 cells → indent_w = 13. Pack so the second clickable
    // wraps to line 1 sitting at column 13.
    let (lines, positions) = layout_chunks(
        vec![
            chunk("[hdr-eleven]"),     // 12 wide
            clickable_chunk("first"),
            clickable_chunk("second"),
        ],
        22,
    );
    assert!(lines.len() >= 2);
    assert_eq!(positions.len(), 2);
    // First clickable lands on line 0 right after the header + sep.
    assert_eq!(positions[0].0, 0);
    // Second wraps to line 1 with the indent prepended.
    assert_eq!(positions[1].0, 1);
}
