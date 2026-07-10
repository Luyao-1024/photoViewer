#[test]
fn sidebar_refresh_trace_logs_stay_debug() {
    let source = include_str!("../refresh.rs");
    let production_source = source
        .split("\n#[cfg(test)]")
        .next()
        .expect("refresh.rs must contain production code");

    let mut search_from = 0;
    while let Some(relative_index) = production_source[search_from..].find("SIDEBAR_TRACE") {
        let message_index = search_from + relative_index;
        let before = &production_source[..message_index];
        let actual_macro = ["tracing::debug!(", "tracing::info!(", "tracing::warn!("]
            .iter()
            .filter_map(|candidate| before.rfind(candidate).map(|index| (index, *candidate)))
            .max_by_key(|(index, _)| *index)
            .map(|(_, candidate)| candidate)
            .expect("SIDEBAR_TRACE message should be inside a tracing macro");
        assert_eq!(
            actual_macro, "tracing::debug!(",
            "SIDEBAR_TRACE refresh diagnostics should stay out of default INFO logs"
        );
        search_from = message_index + "SIDEBAR_TRACE".len();
    }
}
