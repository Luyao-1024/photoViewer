//! Reusable operation logging and selective performance tracing.
//!
//! Long-running work often crosses the GTK main thread, a blocking worker and
//! the single-writer DB actor. A normal [`tracing::Span`] cannot retain its
//! parent across those hand-offs, so [`OperationTrace`] carries a small stable
//! operation id explicitly. It is deliberately domain-neutral: callers pick a
//! [`TraceChain`] and annotate generic stages instead of inventing a bespoke
//! `*_PERF` logging format for every feature.

use std::collections::BTreeSet;
use std::fmt::Display;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use crate::core::log_targets;

/// Environment switch used by the Flatpak runner and direct launches.
pub const TRACE_CHAINS_ENV: &str = "PHOTOVIEWER_TRACE_CHAINS";
/// Existing all-span Chrome trace switch. Kept for backwards-compatible,
/// whole-process captures.
pub const CHROME_TRACE_ENV: &str = "PHOTOVIEWER_CHROME_TRACE";

/// Stable, documented groups for independently selectable trace chains.
///
/// Keep these broad enough to remain useful when new features are added. A
/// feature should use its semantic work class rather than add its own chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceChain {
    Startup,
    Database,
    Scan,
    Filesystem,
    Thumbnail,
    Mutation,
}

impl TraceChain {
    pub const ALL: [Self; 6] = [
        Self::Startup,
        Self::Database,
        Self::Scan,
        Self::Filesystem,
        Self::Thumbnail,
        Self::Mutation,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::Database => "database",
            Self::Scan => "scan",
            Self::Filesystem => "filesystem",
            Self::Thumbnail => "thumbnail",
            Self::Mutation => "mutation",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "startup" => Some(Self::Startup),
            "database" => Some(Self::Database),
            "scan" => Some(Self::Scan),
            "filesystem" => Some(Self::Filesystem),
            "thumbnail" => Some(Self::Thumbnail),
            "mutation" => Some(Self::Mutation),
            _ => None,
        }
    }

    pub fn names() -> impl Iterator<Item = &'static str> {
        Self::ALL.into_iter().map(Self::as_str)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TraceChainSelection {
    all: bool,
    chains: BTreeSet<String>,
    invalid: Vec<String>,
    requested: bool,
}

impl TraceChainSelection {
    pub fn parse(raw: Option<&str>) -> Self {
        let mut selection = Self::default();
        let Some(raw) = raw else {
            return selection;
        };

        for value in raw
            .split(|c: char| c == ',' || c.is_ascii_whitespace())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            selection.requested = true;
            if value.eq_ignore_ascii_case("all") {
                selection.all = true;
            } else if let Some(chain) = TraceChain::parse(&value.to_ascii_lowercase()) {
                selection.chains.insert(chain.as_str().to_string());
            } else {
                selection.invalid.push(value.to_string());
            }
        }
        selection
    }

    pub fn requested(&self) -> bool {
        self.requested
    }

    pub fn includes(&self, chain: TraceChain) -> bool {
        self.all || self.chains.contains(chain.as_str())
    }

    pub fn invalid(&self) -> &[String] {
        &self.invalid
    }

    pub fn display(&self) -> String {
        if self.all {
            return "all".to_string();
        }
        self.chains.iter().cloned().collect::<Vec<_>>().join(",")
    }
}

static TRACE_SELECTION: OnceLock<TraceChainSelection> = OnceLock::new();
static CHROME_TRACE_REQUESTED: OnceLock<bool> = OnceLock::new();
static NEXT_OPERATION_ID: AtomicU64 = AtomicU64::new(1);

pub fn trace_selection() -> &'static TraceChainSelection {
    TRACE_SELECTION
        .get_or_init(|| TraceChainSelection::parse(std::env::var(TRACE_CHAINS_ENV).ok().as_deref()))
}

/// The global trace switch remains valid; a chain selector also enables the
/// Chrome layer so `--trace-chain` is useful on its own.
pub fn chrome_trace_requested() -> bool {
    *CHROME_TRACE_REQUESTED
        .get_or_init(|| env_truthy(CHROME_TRACE_ENV) || trace_selection().requested())
}

/// A non-empty selector changes Chrome capture from process-wide spans to the
/// explicit `OperationTrace` target only. The individual operation is further
/// gated by [`OperationTrace::start`], so a `scan` capture cannot be polluted
/// by database, thumbnail, or mutation work.
pub fn selective_trace_requested() -> bool {
    trace_selection().requested()
}

pub fn emit_configuration_log() {
    let selection = trace_selection();
    if !selection.requested() {
        return;
    }

    if selection.display().is_empty() {
        tracing::warn!(
            target: log_targets::FLOW,
            env = TRACE_CHAINS_ENV,
            valid_chains = %TraceChain::names().collect::<Vec<_>>().join(","),
            "FLOW_TRACE no valid trace chains were selected"
        );
    } else {
        tracing::info!(
            target: log_targets::FLOW,
            env = TRACE_CHAINS_ENV,
            chains = %selection.display(),
            "FLOW_TRACE selective trace capture enabled"
        );
    }

    if !selection.invalid().is_empty() {
        tracing::warn!(
            target: log_targets::FLOW,
            env = TRACE_CHAINS_ENV,
            invalid = %selection.invalid().join(","),
            valid_chains = %TraceChain::names().collect::<Vec<_>>().join(","),
            "FLOW_TRACE ignored unknown trace chains"
        );
    }
}

/// Correlation context for one user-visible or background operation.
///
/// When no Chrome trace is requested this is a cheap no-op. It only creates a
/// correlation id when the selected chain is actively captured.
#[derive(Debug, Clone)]
pub struct OperationTrace {
    chain: TraceChain,
    operation: &'static str,
    state: Option<OperationTraceState>,
}

#[derive(Debug, Clone)]
struct OperationTraceState {
    operation_id: u64,
}

impl OperationTrace {
    pub fn start(chain: TraceChain, operation: &'static str) -> Self {
        let selection = trace_selection();
        let enabled = if selection.requested() {
            selection.includes(chain)
        } else {
            chrome_trace_requested()
        };
        Self {
            chain,
            operation,
            // Keep normal production work allocation-, clock-, and atomic-free.
            // The id is only needed when a Chrome trace is actually active.
            state: enabled.then(|| OperationTraceState {
                operation_id: NEXT_OPERATION_ID.fetch_add(1, Ordering::Relaxed),
            }),
        }
    }

    pub const fn chain(&self) -> TraceChain {
        self.chain
    }

    pub const fn operation(&self) -> &'static str {
        self.operation
    }

    /// Enters one timing stage and exits it automatically when the returned
    /// guard leaves scope. Do not hold this guard across an `await`; enter a
    /// fresh stage inside the worker or synchronous section that does the work.
    #[must_use = "keep the stage guard in a local binding so its duration is recorded"]
    pub fn stage(&self, stage: &'static str) -> TraceStage {
        let span = match &self.state {
            Some(state) => tracing::info_span!(
                target: log_targets::FLOW,
                "flow:stage",
                chain = self.chain.as_str(),
                operation = self.operation,
                operation_id = state.operation_id,
                stage,
                detail = tracing::field::Empty,
                item_count = tracing::field::Empty,
                bytes = tracing::field::Empty,
                queue_wait_ms = tracing::field::Empty,
            ),
            None => tracing::Span::none(),
        };
        let entered = span.clone().entered();
        TraceStage {
            span,
            _entered: entered,
        }
    }
}

/// A scoped timing stage. Dropping this guard exits the span, which is the
/// Chrome/Perfetto end event; callers never need a matching success/finish API.
#[must_use = "keep the stage guard in a local binding so its duration is recorded"]
pub struct TraceStage {
    span: tracing::Span,
    _entered: tracing::span::EnteredSpan,
}

impl TraceStage {
    pub fn record(&self, field: &'static str, value: impl tracing::field::Value) {
        self.span.record(field, value);
    }
}

/// Error logging is independent from performance tracing. Use this at a
/// critical operation boundary; it always reaches `app.log` even when tracing
/// is off, while the trace itself remains timing-only.
pub fn log_error(trace: &OperationTrace, stage: &'static str, error: impl Display) {
    tracing::error!(
        target: log_targets::APP,
        chain = trace.chain.as_str(),
        operation = trace.operation,
        operation_id = ?trace.state.as_ref().map(|state| state.operation_id),
        stage,
        error = %error,
        "operation failed"
    );
}

/// Warning counterpart to [`log_error`], for degraded but recoverable work.
pub fn log_warning(trace: &OperationTrace, stage: &'static str, error: impl Display) {
    tracing::warn!(
        target: log_targets::APP,
        chain = trace.chain.as_str(),
        operation = trace.operation,
        operation_id = ?trace.state.as_ref().map(|state| state.operation_id),
        stage,
        error = %error,
        "operation warning"
    );
}

fn env_truthy(name: &str) -> bool {
    match std::env::var(name) {
        Ok(value) => {
            let value = value.trim();
            !value.is_empty()
                && !value.eq_ignore_ascii_case("0")
                && !value.eq_ignore_ascii_case("false")
                && !value.eq_ignore_ascii_case("off")
                && !value.eq_ignore_ascii_case("no")
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests;
