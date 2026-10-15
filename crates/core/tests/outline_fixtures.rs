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

#[test]
fn unwritten_lists_only_entries_whose_solution_is_a_placeholder() {
    let (outline, _) = read(BASIC_INDEX);
    let unwritten = outline.unwritten();

    // Everything with a linked 题解 is written and must not be offered again.
    for entry in &unwritten {
        let original = outline
            .entries
            .iter()
            .find(|e| e.line == entry.line)
            .expect("line still present");
        assert_eq!(
            original.solution,
            Marker::Placeholder,
            "line {} is not an unwritten entry",
            entry.line
        );
    }

    // Written and unwritten partition the outline exactly: nothing is offered
    // twice and nothing is hidden.
    let linked = outline
        .entries
        .iter()
        .filter(|e| e.solution.is_linked())
        .count();
    assert_eq!(linked + unwritten.len(), outline.entries.len());

    let ids: Vec<&str> = unwritten
        .iter()
        .map(|e| e.problem_id.as_deref().unwrap_or("?"))
        .collect();
    assert!(ids.contains(&"B1002"), "unchecked placeholder: {ids:?}");
    assert!(ids.contains(&"P2003"), "unchecked placeholder: {ids:?}");
    assert!(
        !ids.contains(&"B1000") && !ids.contains(&"B1010"),
        "linked entries must be excluded: {ids:?}"
    );
}

#[test]
fn unwritten_keeps_the_section_of_a_duplicated_problem_id() {
    let (outline, _) = read(BASIC_INDEX);

    // P2009 appears under two different headings, and creation targets one of
    // them — so the section has to travel with the entry.
    let unwritten = outline.unwritten();
    let sections: Vec<&str> = unwritten
        .iter()
        .filter(|e| e.problem_id.as_deref() == Some("P2009"))
        .map(|e| e.section.as_str())
        .collect();

    assert_eq!(sections.len(), 2, "P2009 appears twice: {sections:?}");
    assert!(sections.contains(&"2.4 额外加练"), "{sections:?}");
    assert!(sections.contains(&"3. 分支结构"), "{sections:?}");
}

#[test]
fn the_advanced_book_reports_checked_entries_as_unwritten_solutions() {
    // 进阶篇 tracks progress with videos: `[x]` plus a linked 视频 but a plain
    // 📖题解 placeholder. Those problems still have no solution post, so they
    // must be offered — and linking a solution is not what ticks their box.
    let (outline, _) = read(ADVANCED_INDEX);
    let unwritten = outline.unwritten();

    assert_eq!(unwritten.len(), outline.entries.len());
    assert!(
        outline
            .entries
            .iter()
            .filter(|e| e.checked)
            .all(|e| e.video.is_linked()),
        "in this book a ticked box is driven by a video link"
    );

    let done = unwritten
        .iter()
        .find(|e| e.problem_id.as_deref() == Some("P3001"))
        .expect("P3001 is listed but has no 题解 link");
    assert!(done.checked, "its box is already ticked by the video");
    assert_eq!(done.section, "1.1 双指针");

    let todo = unwritten
        .iter()
        .find(|e| e.problem_id.as_deref() == Some("P3008"))
        .expect("P3008 is listed but unwritten");
    assert!(!todo.checked);
    assert_eq!(todo.section, "2.1 并查集");
}
