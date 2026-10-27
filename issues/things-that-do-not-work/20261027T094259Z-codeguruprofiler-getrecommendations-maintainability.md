Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 9dfd5bd5800ceb89967be182e813f239a03c5430

# GetRecommendations' module header states the opposite rule from the handler, the response codec and the test that pin it

_Filed: 2026-10-27T09:42:59Z_
_Operation: GetRecommendations | Class: maintainability_

> _Note: this issue was identified against the code base at commit `9dfd5bd5800ceb89967be182e813f239a03c5430`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

How an empty analysis is reported is written twice in this crate, and the two copies contradict each other.

The handler's module header says the two `@required` analysis lists are sent as present-and-empty. The inline comment in the same file's `handle` body, the doc on `GetRecommendationsOutput`, and the codec's `req_unless_empty` presence all say the opposite: the lists are OMITTED from the body while they are empty — which is what the service actually answers, and what tests/all/wire_response_shape.rs pins.

So a reader (or a future fixer) of get_recommendations.rs is told a rule the code under it breaks, and the two statements cannot both be used as the reason for a later change: one of them would send a fixer to "restore" present-and-empty lists and break the pinned wire shape. Only the stale header needs to change — the responses and every existing test stay exactly as they are.

## Evidence

Copy 1, the module header — src/ops/get_recommendations.rs:22-29:
```
//! `recommendations` and `anomalies` are reported EMPTY. This emulator stores an agent
...
//! Inventing `Recommendation`/`Anomaly` entries would be output synthesised to look
//! computed, so the modelled lists are present and empty — the honest answer for an
//! analysis that found nothing.
```

Copy 2, the handler body of the same file — src/ops/get_recommendations.rs:257-265:
```
            // No anti-pattern matcher and no anomaly detector run over the stored agent
            // profiles (see this module's header), so the analysis reports no findings
            // rather than invented ones. An analysis with nothing to report OMITS the two
            // lists from the body rather than sending them empty — that is what the
            // `GetRecommendationsResponse` serializer does with an empty one (see
            // `ops/types/date_time_wire.rs`), so a client can tell "no analysis to report"
            // from "analysis ran and found nothing" by the key's presence.
            recommendations: Vec::new(),
            anomalies: Vec::new(),
```

The code that decides it — src/ops/types/get_recommendations_types.rs:26-28 and :70-85:
```
/// `profileStartTime`/`profileEndTime` carry `@timestampFormat("date-time")`, and the two
/// analysis lists — `@required` in the model — are OMITTED from the body by the service
/// when the analysis has nothing to report, rather than reported empty
...
    (M_RECOMMENDATIONS, recommendations, "recommendations", 3, req_unless_empty, list_of(Recommendation)),
    (M_ANOMALIES, anomalies, "anomalies", 4, req_unless_empty, list_of(Anomaly)),
```
and src/ops/types/date_time_wire.rs:267-269:
```
    ($s:ident, $this:ident, $m:path, $field:ident, req_unless_empty, $($kind:tt)+) => {
        if !$this.$field.is_empty() {
```

Tests that pin the omission and must stay green unchanged: tests/all/wire_response_shape.rs:17 ("`recommendations`/`anomalies` are left out until an analysis...") and its assertions at lines 876-890.

## Steps to Reproduce

1. Open src/ops/get_recommendations.rs:22-29 — the header states the modelled lists are "present and empty".
2. Read on to src/ops/get_recommendations.rs:257-263 in the same file — the inline comment states the lists are OMITTED rather than sent empty.
3. Confirm which one the code implements: src/ops/types/get_recommendations_types.rs:70-85 declares both members `req_unless_empty`, and src/ops/types/date_time_wire.rs:267-269 writes such a member only when it is non-empty; tests/all/wire_response_shape.rs:876-890 asserts the keys are absent.
4. Correct the stale header so the file states one rule; no response and no test changes.
