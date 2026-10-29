// SPDX-License-Identifier: CC0-1.0

//! The ONE `tracing` capture this crate's test binary installs, shared by every suite
//! that reads back the records a state-changing operation leaves.
//!
//! A `tracing` subscriber written against `tracing`'s own `Subscriber` trait rather than
//! pulling in a formatting subscriber crate — the same harness this tree's other audit
//! suites use (e.g.
//! `service-com-amazonaws-ssmguiconnect-ssmguiconnect/tests/all/connection_recording_preferences.rs`)
//! — so capturing records adds no dependency.
//!
//! Global rather than scoped on purpose: `tracing` caches per callsite whether any
//! subscriber is interested, the cache is process-wide, and sibling tests run in parallel
//! on other threads. A test binary admits exactly ONE global default, so this module owns
//! it: a suite that wants captured records calls [`audit_capture`] rather than installing
//! another, which `set_global_default` would refuse. What keeps one suite's records out of
//! another's assertions — and out of another's drain, since suites run in parallel — is the
//! access key id each suite signs with ([`CapturedRecords::drain_for`]).

// Each suite uses the part of this harness its own rules need.
#![allow(dead_code)]

use std::sync::Arc;

/// The dedicated target a record carrying the caller's identity is emitted on.
pub const AUDIT_TARGET: &str = "emulator::audit";

/// A `tracing` subscriber that renders each event as one `target field=value …` line and
/// keeps the lines so a test can read them back.
#[derive(Clone, Default)]
pub struct CapturedRecords(Arc<std::sync::Mutex<Vec<String>>>);

impl CapturedRecords {
    /// The records captured so far on the [`AUDIT_TARGET`] that calls signing with `akid`
    /// wrote, TAKEN from the buffer and leaving every other suite's records in it.
    ///
    /// A suite signs with an access key id no other suite uses, so the records of a sibling
    /// suite running in parallel over the same routes are neither counted here nor
    /// discarded from under it — which is what lets an assertion demand EXACTLY one record
    /// per call.
    pub fn drain_for(&self, akid: &str) -> Vec<String> {
        let mut held = self.0.lock().expect("the audit capture lock");
        let (mine, theirs) = std::mem::take(&mut *held)
            .into_iter()
            .partition(|record| is_for(record, akid));
        *held = theirs;
        mine
    }
}

/// Whether `record` is an audit record written by a call signing with `akid`.
fn is_for(record: &str, akid: &str) -> bool {
    record.starts_with(&format!("{AUDIT_TARGET} ")) && record.contains(&format!("akid={akid}"))
}

/// Renders an event's fields as ` name=value` onto one line.
struct RecordLine(String);

impl tracing::field::Visit for RecordLine {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write as _;
        let _ = write!(self.0, " {}={value:?}", field.name());
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        use std::fmt::Write as _;
        let _ = write!(self.0, " {}={value}", field.name());
    }
}

impl tracing::Subscriber for CapturedRecords {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        *metadata.level() <= tracing::Level::INFO
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        // The service emits events, never spans; an id is still required by the trait.
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut line = RecordLine(format!("{} ", event.metadata().target()));
        event.record(&mut line);
        self.0.lock().expect("the audit capture lock").push(line.0);
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

/// The capture for audit records, installed ONCE as this test binary's global `tracing`
/// subscriber.
static AUDIT_CAPTURE: std::sync::OnceLock<CapturedRecords> = std::sync::OnceLock::new();

/// This binary's capture, installing it on first use.
pub fn audit_capture() -> &'static CapturedRecords {
    AUDIT_CAPTURE.get_or_init(|| {
        let captured = CapturedRecords::default();
        tracing::subscriber::set_global_default(captured.clone())
            .expect("this module is the only installer of a global tracing subscriber");
        captured
    })
}
