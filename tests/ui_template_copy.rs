//! Blueprint templates must not carry user-visible copy.
//!
//! Every string a user reads comes from `i18n/*.json` through `tr()`/`trf()` in
//! Rust. A literal in a `.blp` stays in whatever language it was written in no
//! matter what the session locale resolves to — the Search page shipped
//! `label: "全部"`/`"文件名"`/`"日期"` that way (P0-5).

use std::fs;
use std::path::{Path, PathBuf};

const COPY_PROPERTIES: &[&str] = &[
    "label",
    "title",
    "text",
    "placeholder-text",
    "tooltip-text",
    "subtitle",
    "description",
];

fn templates() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("data/ui");
    let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|err| panic!("{dir:?} should be readable: {err}"))
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "blp"))
        .collect();
    paths.sort();
    assert!(
        !paths.is_empty(),
        "expected Blueprint templates under {dir:?}"
    );
    paths
}

/// `prop: "value";` → `Some(("prop", "value"))`. Comments and every other
/// property (icon names, action names, css classes) are not user copy.
fn copy_assignment(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with("//") {
        return None;
    }
    let (property, rest) = trimmed.split_once(':')?;
    let property = property.trim();
    if !COPY_PROPERTIES.contains(&property) {
        return None;
    }
    let value = rest.trim().strip_suffix(';').unwrap_or(rest).trim();
    let value = value.strip_prefix('"')?;
    Some((property, value.trim_end_matches('"')))
}

#[test]
fn templates_leave_copy_to_the_i18n_catalogues() {
    let mut offenders = Vec::new();

    for path in templates() {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("{path:?} should be readable: {err}"));
        for (index, line) in source.lines().enumerate() {
            if let Some((property, value)) = copy_assignment(line) {
                if !value.is_empty() {
                    offenders.push(format!(
                        "{}:{} — {property}: \"{value}\"",
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("?"),
                        index + 1
                    ));
                }
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "user-visible copy belongs in i18n/*.json and tr(), not in a template: {offenders:#?}"
    );
}

#[test]
fn the_search_page_template_defers_its_segment_labels_to_rust() {
    let source =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("data/ui/search-page.blp"))
            .expect("search page template should be readable");

    // The AT-SPI probe resolves these three keys, so an empty template label here
    // is what keeps a screen reader announcing the same words the user sees.
    for key in ["search.field.all", "search.field.name", "search.field.date"] {
        assert!(
            source.contains(key),
            "search-page.blp should document that its segment labels come from {key:?}"
        );
    }
}
