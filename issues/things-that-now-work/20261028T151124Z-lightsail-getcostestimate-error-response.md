Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: d2be18544689ad5807caafb5cad50952f5ef0fe4

# GetCostEstimate hand-rolls its own inverted-window refusal instead of the crate's single owner of that rule

_Filed: 2026-10-28T15:11:24Z_
_Operation: GetCostEstimate | Class: error-response_

> _Note: this issue was identified against the code base at commit `d2be18544689ad5807caafb5cad50952f5ef0fe4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other handler in this crate that takes a `startTime`/`endTime` pair refuses an inverted window through `crate::state::inverted_window_refusal`, whose doc declares itself "the one sentence this service answers that pair with wherever it takes one" and whose eight call sites repeat that claim ("this service answers that pair the same way wherever it takes one"). `GetCostEstimate` takes exactly such a pair and answers a free-form sentence of its own instead, so one service answers the same malformed window with two different messages depending on which operation received it.

The divergence is not cosmetic for a caller: the shared refusal names both bounds as the instants they are ("startTime must be earlier than endTime, got startTime <s> and endTime <s> (seconds since the epoch)"), which is the form this crate already went back and fixed for the metric readers, while GetCostEstimate's sentence names neither bound at all, so a caller with an inverted window is told nothing about the two instants it sent.

## Evidence

The crate's declared single owner of the rule — services/service-com-amazonaws-lightsail-lightsail-20161128/src/state.rs:15964-15983:
```
/// The refusal a time window whose end does not follow its start earns, and
/// `None` while the pair is a window — the one sentence this service answers
/// that pair with wherever it takes one.
///
/// Both bounds are named as the INSTANTS they are, sub-second part and sign
/// included, so a window inverted inside one second — or one lying before the
/// epoch — is reported as what it is rather than as two equal whole seconds.
pub fn inverted_window_refusal(
    start: ::aws_smithy_types::DateTime,
    end: ::aws_smithy_types::DateTime,
) -> Option<String> {
    (timestamp_key(start) >= timestamp_key(end)).then(|| {
        format!(
            "startTime must be earlier than endTime, got startTime {} and endTime {} \
             (seconds since the epoch)",
```
A sibling that goes through it, restating the rule — src/ops/get_load_balancer_metric_data.rs:223-233:
```
        // "The start time of the period" / "The end time of the period": a period
        // whose end does not follow its start is no period at all, so no window
        // of it could be reported. The refusal is
        // [`crate::state::inverted_window_refusal`], which names both bounds as
        // the instants they are — this service answers that pair the same way
        // wherever it takes one.
        if let Some(message) =
            crate::state::inverted_window_refusal(input.start_time, input.end_time)
        {
            return Err(invalid(message));
        }
```
(the same call at get_instance_metric_data.rs:226, get_bucket_metric_data.rs:281, get_distribution_metric_data.rs:273, get_container_service_metric_data.rs:215, get_relational_database_metric_data.rs:277, get_container_log.rs:216 and get_relational_database_log_events.rs:203).

The one handler that does not — src/ops/get_cost_estimate.rs:197-208:
```
        let start_ms = epoch_millis(input.start_time, "startTime")?;
        let end_ms = epoch_millis(input.end_time, "endTime")?;
        // The modelled `TimePeriod` is "start date is inclusive, but the end date
        // is exclusive", so a window whose end is not after its start holds no
        // instant at all and there is nothing to estimate over it.
        if end_ms <= start_ms {
            return Err(invalid(
                "endTime must name an instant after startTime: a cost estimate window \
                 includes startTime and excludes endTime"
                    .to_string(),
            ));
        }
```
The condition is the same one the shared helper applies (`start >= end`), so only the wording differs.

Test coverage: tests/all/inverted_time_window.rs exercises the shared refusal for the window readers; tests/all/cost_estimate.rs covers GetCostEstimate, and no case there asserts the shared sentence, so switching the call would only need that suite's expectation for this one message updated.

## Steps to Reproduce

1. Call GetInstanceMetricData with an inverted window (startTime later than endTime) and record the refusal message: "startTime must be earlier than endTime, got startTime <s> and endTime <s> (seconds since the epoch)".
2. Call GetCostEstimate with resourceName of an existing resource and the SAME inverted window.
3. Observe a different sentence — "endTime must name an instant after startTime: a cost estimate window includes startTime and excludes endTime" — which names neither of the two instants the request carried.
4. Read src/state.rs:15964-15967 ("the one sentence this service answers that pair with wherever it takes one") beside src/ops/get_cost_estimate.rs:202-208, which answers that pair with a second sentence.
