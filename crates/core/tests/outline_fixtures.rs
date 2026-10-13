//! Outline parsing + surgical write-back tests.
//!
//! The fixtures under `tests/fixtures/blog` are *synthetic*: they reproduce every
//! irregularity the parser must survive (mixed list markers, linked/unlinked
//! 📖/📺, duplicate problem ids, full-width brackets in titles, trailing
//! whitespace) without carrying any real blog content.
//!
//! Guarantees under test:
//! * parsing copes with the messy real-world grammar;
//! * editing one line changes only that line, byte for byte;
//! * parsing then rendering is lossless.

use astro_publisher_core::outline::{HeadingLevel, ListMarker, Marker, Outline, OutlineEntry};
use std::path::{Path, PathBuf};

const BASIC_INDEX: &str = "blog/group/group-index.md";
const ADVANCED_INDEX: &str = "blog/advanced/advanced-index.md";

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

fn read(name: &str) -> (Outline, String) {
    let path = fixture(name);
    let text = std::fs::read_to_string(&path).expect("fixture readable");
    let outline = Outline::parse(&text, &path);
    (outline, text)
}

#[test]
fn parses_the_basic_book_index() {
    let (outline, _) = read(BASIC_INDEX);

    assert!(
        outline.entries.len() > 12,
        "expected the full outline, got {} entries",
        outline.entries.len()
    );

    // Part/Chapter/Section headings are all present.
    assert!(outline.sections.iter().any(|s| s.level == HeadingLevel::Part));
    assert!(outline.sections.iter().any(|s| s.level == HeadingLevel::Chapter));
    assert!(outline.sections.iter().any(|s| s.level == HeadingLevel::Section));

    // A written entry keeps its problem id, title, and linked solution.
    let written = outline
        .entries_for_problem("B1000")
        .into_iter()
        .next()
        .expect("B1000 present");
    assert!(written.checked);
    assert_eq!(written.title, "【样例1.例1】打印问候");
    assert_eq!(
        written.solution,
        Marker::Linked {
            label: "题解".into(),
            href: "/posts/group/_0x0001".into()
        }
    );
    // This book lists solutions but leaves the video marker as a placeholder.
    assert_eq!(written.video, Marker::Placeholder);
}

#[test]
fn tolerates_both_list_markers() {
    let (outline, _) = read(BASIC_INDEX);

    // The fixture mixes `-` and `*`; both must parse.
    assert!(outline.entries.iter().any(|e| e.marker == ListMarker::Dash));
    assert!(outline.entries.iter().any(|e| e.marker == ListMarker::Star));
}

#[test]
fn an_unwritten_entry_has_placeholder_markers_and_unchecked_box() {
    let (outline, _) = read(BASIC_INDEX);

    let unwritten = outline
        .entries_for_problem("P2002")
        .into_iter()
        .next()
        .expect("P2002 present");

    assert!(!unwritten.checked, "P2002 is still unwritten");
    assert_eq!(unwritten.solution, Marker::Placeholder);
    assert_eq!(unwritten.video, Marker::Placeholder);
}

#[test]
fn advanced_book_has_checked_entries_with_linked_videos() {
    // This book marks progress with videos: checked, 题解 unlinked, 视频 linked.
    let (outline, _) = read(ADVANCED_INDEX);

    let entry = outline
        .entries_for_problem("P3001")
        .into_iter()
        .next()
        .expect("P3001 present");

    assert!(entry.checked);
    assert_eq!(entry.solution, Marker::Placeholder);
    match &entry.video {
        Marker::Linked { href, .. } => assert!(
            href.starts_with("https://video.example.invalid/"),
            "unexpected video href: {href}"
        ),
        other => panic!("expected a linked video, got {other:?}"),
    }
}

#[test]
fn duplicate_problem_ids_are_all_kept() {
    // P2009 appears both under "分支结构" and in the extra-practice section.
    let (outline, _) = read(BASIC_INDEX);
    let occurrences = outline.entries_for_problem("P2009");
    assert!(
        occurrences.len() >= 2,
        "expected duplicate entries for P2009, got {}",
        occurrences.len()
    );

    // They sit on different lines, which is what makes line-indexed edits safe.
    let mut lines: Vec<usize> = occurrences.iter().map(|e| e.line).collect();
    lines.sort_unstable();
    lines.dedup();
    assert_eq!(lines.len(), occurrences.len());
}

#[test]
fn editing_one_entry_changes_exactly_one_line() {
    let (mut outline, original) = read(BASIC_INDEX);

    let target_line = outline
        .entries_for_problem("P2002")
        .into_iter()
        .next()
        .expect("P2002 present")
        .line;

    let mut updated: OutlineEntry = outline
        .entries
        .iter()
        .find(|e| e.line == target_line)
        .expect("entry present")
        .clone();
    updated.solution = Marker::Linked {
        label: "题解".into(),
        href: "/posts/group/_0x0013".into(),
    };
    updated.checked = true;

    outline
        .replace_entry(target_line, &updated)
        .expect("edit succeeds");

    let rendered = outline.render();

    // Exactly one line differs from the original.
    let before: Vec<&str> = original.lines().collect();
    let after: Vec<&str> = rendered.lines().collect();
    assert_eq!(before.len(), after.len(), "line count must not change");

    let changed: Vec<usize> = before
        .iter()
        .zip(after.iter())
        .enumerate()
        .filter(|(_, (b, a))| b != a)
        .map(|(i, _)| i)
        .collect();

    assert_eq!(changed, vec![target_line], "only the target line may change");

    // The rewritten line carries the new link and a checked box, keeping order.
    let line = after[target_line];
    assert!(line.contains("📖[题解](/posts/group/_0x0013)"), "line: {line}");
    assert!(line.starts_with("- [x] ") || line.starts_with("* [x] "), "line: {line}");
    assert!(line.contains("📺视频"), "video marker preserved: {line}");
}

#[test]
fn editing_preserves_untouched_lines_byte_for_byte() {
    // Trailing whitespace elsewhere in the file must survive.
    let (mut outline, original) = read(BASIC_INDEX);

    let target = outline.entries.first().expect("entries exist").line;
    let mut updated: OutlineEntry = outline.entries[0].clone();
    updated.checked = !updated.checked;
    outline.replace_entry(target, &updated).expect("edit");

    let rendered = outline.render();
    let before: Vec<&str> = original.lines().collect();
    let after: Vec<&str> = rendered.lines().collect();

    for (i, (b, a)) in before.iter().zip(after.iter()).enumerate() {
        if i == target {
            continue;
        }
        assert_eq!(b, a, "line {i} changed unexpectedly");
    }
}

#[test]
fn round_trip_without_edits_is_identical() {
    // Parsing then rendering must be lossless — otherwise every save would
    // create spurious diffs.
    for name in [BASIC_INDEX, ADVANCED_INDEX] {
        let (outline, original) = read(name);
        let rendered = outline.render();
        assert_eq!(
            normalize(&original),
            normalize(&rendered),
            "round-trip changed {name}"
        );
    }
}

fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n")
}

#[test]
fn sections_are_addressable_by_heading_text() {
    let (outline, _) = read(BASIC_INDEX);

    // Sections are addressable by their visible heading text.
    let sections = outline.find_sections("1. 输出练习");
    assert_eq!(sections.len(), 1, "expected exactly one matching section");

    let section = sections[0];
    assert_eq!(section.level, HeadingLevel::Chapter);
    assert!(
        section.entries.len() >= 3,
        "section should hold its entries, got {}",
        section.entries.len()
    );
}
