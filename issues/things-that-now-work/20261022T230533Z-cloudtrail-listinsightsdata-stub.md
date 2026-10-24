Issue-Source: trace-sweep-divergence
Issue-Trace: cloudtrail-insights-data-lookup-on-unprovisioned-trail
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudtrail-insights-data-lookup-on-unprovisioned-trail cloudtrail-insights-data-on-synthetic-trail cloudtrail-list-insights-data-invalid-params cloudtrail-list-insights-data-not-found-errors cloudtrail-list-insights-data-time-range-rejected-on-live-trail
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail|stub:ListInsightsData
Issue-Commit: 2d1586b45d80f7c9b7f4b7e099aeb4390c46a491

# Implement cloudtrail ListInsightsData: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2d1586b45d80f7c9b7f4b7e099aeb4390c46a491`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInsightsData

## Description
`ListInsightsData` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/list_insights_data.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

10 recorded unit(s) fail on their `ListInsightsData` call, so each one unblocks only when `ListInsightsData` works: `cloudtrail-insights-data-lookup-on-unprovisioned-trail#task#1`, `cloudtrail-insights-data-lookup-on-unprovisioned-trail#task#2`, `cloudtrail-insights-data-on-synthetic-trail#task#1`, `cloudtrail-list-insights-data-invalid-params#task#1`, `cloudtrail-list-insights-data-invalid-params#task#2`, `cloudtrail-list-insights-data-invalid-params#task#3`, `cloudtrail-list-insights-data-not-found-errors#task#1`, `cloudtrail-list-insights-data-not-found-errors#task#2`, `cloudtrail-list-insights-data-time-range-rejected-on-live-trail#task#5`, `cloudtrail-list-insights-data-time-range-rejected-on-live-trail#task#6`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/list_insights_data.rs`. Replaying the corpus, the emulator refused the `ListInsightsData` call: status: expected 400, got 501
$.__type: value_mismatch (expected 'ValidationException', got 'NotImplemented')
$.message: value_mismatch (expected 'The Insight source arn:aws:cloudtrail:us-west-2:000000000000:trail/sim2real-probe-cloudtrail-ghost-trail-42e404c5 is not a valid trail ARN.', got 'operation amazonaws.cloudtrail.v20131101.CloudTrail_20131101.ListInsightsData is not implemented in this emulator')

## Steps to Reproduce
Boot the cloudtrail emulator
Call ListInsightsData and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
