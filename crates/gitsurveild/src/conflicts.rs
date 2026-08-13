//! Pure conflict-marker parsing (`specs/conflict-resolver.md`).
//!
//! Everything in here is a pure function over text: no I/O, no git. It turns a
//! conflicted file's raw bytes into ordered [`ConflictSegment`]s (context vs
//! hunk), which is what the three-pane editor renders and what the daemon
//! re-parses after each `conflicts.save` to know how many hunks remain.
//!
//! The invariants that make the feature safe are all here and are all tested:
//!
//! - **Byte-exact round trip.** Lines are kept verbatim, terminator included,
//!   so serializing an unmodified parse reproduces the file exactly — CRLF,
//!   missing final newline, and all (AC-3.1, AC-3.4).
//! - **Malformed input degrades, never panics.** Truncated hunks, nested
//!   markers, or a source line that merely *starts* with `<<<<<<<` fall back
//!   to context text (AC-3.3). The daemon never fails a request because a
//!   file confused the parser; it just stops treating it as resolved.
//!
//! `parse_conflicts` is also the commit guard: `conflicts.commit` serializes
//! the working tree and refuses if any `<<<<<<<` marker survived (AC-4.4).
//!
//! The public functions feed the socket handlers in the next step of the
//! feature; until they're wired up the module is exercised only by tests.

#![allow(dead_code)]

use gitsurveil_proto::{ConflictHunk, ConflictSegment};

/// Parses `text` into ordered segments. Context and conflict blocks are
/// returned in file order, so the UI can render all three panes by walking
/// them once.
pub fn parse_conflicts(text: &str) -> Vec<ConflictSegment> {
    let lines: Vec<String> = split_keeping_terminators(text);
    let mut segments = Vec::new();
    let mut context = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        if is_marker(&lines[i], "<<<<<<<", true) {
            match parse_hunk(&lines, i) {
                Some((hunk, next)) => {
                    if !context.is_empty() {
                        segments.push(ConflictSegment::Context {
                            lines: std::mem::take(&mut context),
                        });
                    }
                    segments.push(ConflictSegment::Conflict { hunk });
                    i = next;
                }
                None => {
                    // Malformed block: keep the marker line, then swallow the
                    // rest of the block through the next `>>>>>>>` (or EOF) as
                    // context. Without this, a nested `<<<<<<<` in the
                    // poisoned region would re-enter the parser as a bogus
                    // hunk.
                    context.push(lines[i].clone());
                    i += 1;
                    while i < lines.len() {
                        let ends_block = is_marker(&lines[i], ">>>>>>>", true);
                        context.push(lines[i].clone());
                        i += 1;
                        if ends_block {
                            break;
                        }
                    }
                }
            }
        } else {
            context.push(lines[i].clone());
            i += 1;
        }
    }
    if !context.is_empty() {
        segments.push(ConflictSegment::Context {
            lines: context,
        });
    }
    segments
}

/// Serializes segments back to text. Byte-exact inverse of [`parse_conflicts`]
/// for unchanged input: context lines and each hunk's verbatim block are
/// concatenated as-is.
pub fn serialize(segments: &[ConflictSegment]) -> String {
    let mut out = String::new();
    for segment in segments {
        match segment {
            ConflictSegment::Context { lines } => out.push_str(&lines.concat()),
            ConflictSegment::Conflict { hunk } => out.push_str(&hunk.raw.concat()),
        }
    }
    out
}

/// Counts conflict hunks in `text` — the "3 of 5 resolved" progress number.
pub fn conflict_count(text: &str) -> usize {
    parse_conflicts(text)
        .iter()
        .filter(|s| matches!(s, ConflictSegment::Conflict { .. }))
        .count()
}

/// Splits text into lines *keeping each line's terminator*. A file that
/// doesn't end with a newline gets an unterminated final element. This is the
/// representation that makes round-tripping exact.
fn split_keeping_terminators(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text.split_inclusive('\n').map(str::to_owned).collect();
    if text.is_empty() {
        lines.clear();
    }
    lines
}

/// The marker comparison form of a line: the trailing `\n` (and `\r`, only as
/// part of `\r\n`) removed, everything else untouched.
fn marker_text(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

/// Whether `line` is a git conflict marker. `<<<<<<<`, `|||||||` and
/// `>>>>>>>` may carry a label after them; `=======` must be bare, so a
/// content line like `========` (markdown header) isn't mistaken for the
/// separator.
fn is_marker(line: &str, marker: &str, allow_suffix: bool) -> bool {
    let text = marker_text(line);
    text.starts_with(marker) && (allow_suffix || text == marker)
}

/// The text after a labeled marker, e.g. `HEAD` for `<<<<<<< HEAD`.
fn label(text: &str, marker: &str) -> Option<String> {
    let rest = marker_text(text).strip_prefix(marker)?.trim();
    if rest.is_empty() { None } else { Some(rest.to_string()) }
}

/// Attempts to parse a conflict hunk starting at the `<<<<<<<` line
/// `lines[start]`. Returns the hunk and the index just past its `>>>>>>>`
/// line. Returns `None` on any malformed structure (nested markers, missing
/// separator, missing close); the caller falls back to context.
fn parse_hunk(lines: &[String], start: usize) -> Option<(ConflictHunk, usize)> {
    let ours_label = label(&lines[start], "<<<<<<<");
    let mut ours: Vec<String> = Vec::new();
    let mut base: Option<Vec<String>> = None;
    let mut base_label: Option<String> = None;
    let mut theirs: Vec<String> = Vec::new();
    let mut theirs_label: Option<String> = None;
    let mut stage = 0; // 0: ours, 1: base, 2: theirs
    let mut i = start + 1;
    let mut end: Option<usize> = None;

    while i < lines.len() {
        let line = &lines[i];
        if is_marker(line, "<<<<<<<", true) {
            return None; // nested conflict: not a hunk we can resolve safely
        }
        match stage {
            0 if is_marker(line, "|||||||", true) => {
                base_label = label(line, "|||||||");
                stage = 1;
            }
            0 if is_marker(line, "=======", false) => stage = 2,
            0 => {
                if is_marker(line, ">>>>>>>", true) {
                    return None; // no theirs section
                }
                ours.push(line.clone());
            }
            1 if is_marker(line, "=======", false) => stage = 2,
            1 if is_marker(line, "|||||||", true) => return None,
            1 => {
                if is_marker(line, ">>>>>>>", true) {
                    return None; // diff3 section never closed
                }
                base.get_or_insert_with(Vec::new).push(line.clone());
            }
            _ => {
                if is_marker(line, ">>>>>>>", true) {
                    theirs_label = label(line, ">>>>>>>");
                    end = Some(i);
                    break;
                }
                if is_marker(line, "=======", false) || is_marker(line, "|||||||", true) {
                    return None; // a second separator
                }
                theirs.push(line.clone());
            }
        }
        i += 1;
    }

    let end = end?;
    let raw = lines[start..=end].to_vec();
    Some((
        ConflictHunk {
            start_line: start + 1,
            end_line: end + 1,
            raw,
            ours_label,
            ours,
            base,
            base_label,
            theirs,
            theirs_label,
        },
        end + 1,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conflict_hunks(text: &str) -> Vec<ConflictHunk> {
        parse_conflicts(text)
            .into_iter()
            .filter_map(|s| match s {
                ConflictSegment::Conflict { hunk } => Some(hunk),
                ConflictSegment::Context { .. } => None,
            })
            .collect()
    }

    #[test]
    fn round_trips_unmodified_conflicts_byte_exactly() {
        // No trailing newline on the file, so the round trip covers both the
        // terminated and unterminated cases at once.
        let text = "a\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> feature\nb";
        assert_eq!(serialize(&parse_conflicts(text)), text);
    }

    #[test]
    fn round_trips_with_trailing_newline() {
        let text = "<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> feature\n";
        assert_eq!(serialize(&parse_conflicts(text)), text);
    }

    #[test]
    fn round_trips_crlf() {
        let text = "a\r\n<<<<<<< HEAD\r\nours\r\n=======\r\ntheirs\r\n>>>>>>> feature\r\nb\r\n";
        assert_eq!(serialize(&parse_conflicts(text)), text);
    }

    #[test]
    fn parses_diff3_base_section() {
        let text = "a\n<<<<<<< HEAD\nours\n||||||| merged common ancestor\nbase\n=======\ntheirs\n>>>>>>> feature\nb\n";
        let hunks = conflict_hunks(text);
        assert_eq!(hunks.len(), 1);
        let hunk = &hunks[0];
        assert_eq!(hunk.ours, vec!["ours\n"]);
        assert_eq!(hunk.base.as_deref(), Some(&["base\n".to_string()][..]));
        assert_eq!(hunk.theirs, vec!["theirs\n"]);
        assert_eq!(hunk.ours_label.as_deref(), Some("HEAD"));
        assert_eq!(hunk.theirs_label.as_deref(), Some("feature"));
        assert_eq!(hunk.base_label.as_deref(), Some("merged common ancestor"));
    }

    #[test]
    fn truncated_hunk_degrades_to_context_without_panicking() {
        // `>>>>>>>` never comes; the whole thing must stay as context text.
        let text = "a\n<<<<<<< HEAD\nours\n=======\ntheirs\n";
        let segments = parse_conflicts(text);
        assert_eq!(conflict_hunks(text).len(), 0);
        assert!(segments.iter().all(|s| matches!(s, ConflictSegment::Context { .. })));
        assert_eq!(serialize(&segments), text);
    }

    #[test]
    fn nested_markers_degrades_to_context() {
        let text = "<<<<<<< HEAD\n<<<<<<< HEAD\ninner\n=======\n>>>>>>> feature\n=======\ntheirs\n>>>>>>> feature\n";
        assert_eq!(conflict_hunks(text).len(), 0);
        assert_eq!(serialize(&parse_conflicts(text)), text);
    }

    #[test]
    fn marker_like_line_inside_source_is_not_a_hunk() {
        // A line that merely starts with `<<<<<<<` (e.g. in a string literal)
        // with no valid structure must degrade, not panic.
        let text = "let s = \"not a conflict\";\n<<<<<<< placeholder\nstill just code\n";
        let segments = parse_conflicts(text);
        assert_eq!(conflict_hunks(text).len(), 0);
        assert_eq!(serialize(&segments), text);
    }

    #[test]
    fn multiple_conflicts_stay_ordered_and_distinct() {
        let text = "head\n<<<<<<< HEAD\none\n=======\none-theirs\n>>>>>>> f\nmid\n<<<<<<< HEAD\ntwo\n||||||| base\nb\n=======\ntwo-theirs\n>>>>>>> f\ntail\n";
        let hunks = conflict_hunks(text);
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].ours, vec!["one\n"]);
        assert_eq!(hunks[1].theirs, vec!["two-theirs\n"]);
        assert_eq!(serialize(&parse_conflicts(text)), text);
    }

    #[test]
    fn content_line_of_eight_equals_is_not_a_separator() {
        // `========` (a markdown header) starts with `=======` but must be
        // treated as content, or the theirs section would be cut short.
        let text = "a\n<<<<<<< HEAD\n========\n=======\ntheirs\n>>>>>>> f\nb\n";
        let hunks = conflict_hunks(text);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].ours, vec!["========\n"]);
        assert_eq!(hunks[0].theirs, vec!["theirs\n"]);
    }

    #[test]
    fn count_reports_hunks_and_clears_once_resolved() {
        let text = "a\n<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> f\n<<<<<<< HEAD\np\n=======\nq\n>>>>>>> g\n";
        assert_eq!(conflict_count(text), 2);
        let resolved = "a\nx\ny\n";
        assert_eq!(conflict_count(resolved), 0);
    }

    #[test]
    fn empty_file_parses_to_no_segments() {
        assert_eq!(parse_conflicts(""), Vec::<ConflictSegment>::new());
    }
}
