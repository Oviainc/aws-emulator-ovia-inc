Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 6b193e67e38a36506ad31c413899507363d5dc88

# The "an empty or whitespace DestinationArn is the same input as an absent one" rule is implemented six times, once per metrics handler

_Filed: 2026-10-28T05:58:57Z_
_Operation: PutRumMetricsDestination | Class: maintainability_

> _Note: this issue was identified against the code base at commit `6b193e67e38a36506ad31c413899507363d5dc88`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation of this crate that accepts the `(Destination, DestinationArn)` pair re-implements the same normalisation rule in its own handler body — trim the ARN, then read the empty string as "no ARN" — rather than reading it from one place. There are six copies of the expression and of the comment that justifies it (`PutRumMetricsDestination` carries a seventh, for `IamRoleArn`).

The rule is not one operation's: it decides which stored destination a request addresses, and the crate already states exactly that where the destinations map is keyed (`crate::state::metric_destination_key`, which normalises "an absent or empty ARN … to the same key, so one destination cannot be stored twice under two spellings of 'no ARN'"). With the normalisation sitting in six handler bodies instead of beside that key, the put and the delete, or the batch create and the batch read, can drift apart on what "no ARN" means — which is the one thing the shared key exists to prevent. The crate has already made this exact move once, for the page-size rule: `crate::state::page_size` (state.rs:2293) is documented as "ONE page-size rule for every such operation of this service … so their page sizes cannot drift".

The fix is a single shared helper in `crate::state` beside `metric_destination_key`, called by the six handlers. No response and no existing test changes: each copy is byte-identical today, so hoisting it computes the same `Option<&str>` every handler computes now.

## Evidence

Six identical copies, each with the same justifying comment:

`src/ops/put_rum_metrics_destination.rs:179-186`
```
        // `DestinationArn` is modelled `length 0..2048`, so an empty ARN can reach here;
        // it names no experiment, so it is the same input as an absent one and must not
        // become a second destination.
        let destination_arn = input
            .destination_arn
            .as_deref()
            .map(str::trim)
            .filter(|arn| !arn.is_empty());
```
`src/ops/delete_rum_metrics_destination.rs:165-172` — "exactly as the put reads it, which is what keeps the two addressing one destination", then the same four lines.
`src/ops/batch_create_rum_metric_definitions.rs:210-216` — "An empty `DestinationArn` (the member is modelled `length 0..2048`) names no experiment, so it is the same input as an absent one.", then the same four lines.
`src/ops/batch_delete_rum_metric_definitions.rs:290-296` — same comment, same four lines.
`src/ops/batch_get_rum_metric_definitions.rs:173-179` — same comment, same four lines.
`src/ops/update_rum_metric_definition.rs:172-179` — same comment, same four lines.
(and the seventh copy, for the role, at `src/ops/put_rum_metrics_destination.rs:187-191`.)

The place the crate says this rule belongs, `src/state.rs:384-395`:
```
/// The [`MetricDestinationKey`] for `destination` and `destination_arn`, with an absent
/// or empty ARN normalised to the same key, so one destination cannot be stored twice
/// under two spellings of "no ARN".
pub fn metric_destination_key(
```
and the precedent for hoisting a per-handler rule into it, `src/state.rs:2282-2293`:
```
/// ONE page-size rule for every such operation of this service (`ListAppMonitors`,
/// `ListRumMetricsDestinations`, `BatchGetRumMetricDefinitions`), beside the token codec
/// those same operations share, so their page sizes cannot drift.
pub fn page_size(requested: Option<i32>) -> usize {
```

Tests that cover the behaviour and must stay green unchanged: `tests/all/batch_get_metric_definitions_reads.rs` (header line 29, "an absent versus an empty `DestinationArn`", with the rows "CloudWatch with an empty ARN" at line 213 and "Evidently with an empty ARN" at line 234), `tests/all/metrics_destinations.rs`, `tests/all/metrics_destination_removal.rs`, `tests/all/metric_destination_summary_members.rs`, `tests/all/metric_definitions.rs` and `tests/all/batch_delete_metric_definitions.rs`.

## Steps to Reproduce

1. Open the six handler files listed in the evidence and compare the four-line `destination_arn` expression in each: they are identical, and each carries its own copy of the comment explaining the rule
2. Note that the rule decides which stored destination a request addresses, and that `crate::state::metric_destination_key` (state.rs:387) already owns the matching normalisation for the map key
3. Hoist the expression into one `crate::state` helper beside `metric_destination_key` and call it from the six handlers; `cargo test -p service-com-amazonaws-rum-rum` must pass unchanged
