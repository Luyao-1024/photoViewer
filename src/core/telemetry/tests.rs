use super::*;

#[test]
fn chain_selection_accepts_multiple_types_and_all() {
    let selection = TraceChainSelection::parse(Some("startup, database thumbnail"));
    assert!(selection.requested());
    assert!(selection.includes(TraceChain::Startup));
    assert!(selection.includes(TraceChain::Database));
    assert!(selection.includes(TraceChain::Thumbnail));
    assert!(!selection.includes(TraceChain::Mutation));
    assert!(selection.invalid().is_empty());

    let all = TraceChainSelection::parse(Some("all"));
    assert!(all.includes(TraceChain::Filesystem));
    assert!(all.includes(TraceChain::Mutation));
}

#[test]
fn chain_selection_reports_unknown_type_without_enabling_it() {
    let selection = TraceChainSelection::parse(Some("startup,not-a-chain"));
    assert!(selection.includes(TraceChain::Startup));
    assert!(!selection.includes(TraceChain::Scan));
    assert_eq!(selection.invalid(), ["not-a-chain"]);
}

#[test]
fn falsey_chrome_switches_are_disabled() {
    for value in ["", "0", "false", "off", "no"] {
        std::env::set_var("PHOTOVIEWER_TEST_FALSEY_TRACE", value);
        assert!(!env_truthy("PHOTOVIEWER_TEST_FALSEY_TRACE"));
    }
    std::env::remove_var("PHOTOVIEWER_TEST_FALSEY_TRACE");
}
