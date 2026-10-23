Issue-Source: trace-sweep-divergence
Issue-Trace: cloudtrail-lake-mutation-error-contracts
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudtrail-lake-mutation-error-contracts cloudtrail-list-insights-data-time-range-rejected-on-live-trail
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail|stub:PutInsightSelectors
Issue-Commit: 2d1586b45d80f7c9b7f4b7e099aeb4390c46a491

# Implement cloudtrail PutInsightSelectors: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2d1586b45d80f7c9b7f4b7e099aeb4390c46a491`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutInsightSelectors

## Description
`PutInsightSelectors` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/put_insight_selectors.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `PutInsightSelectors` call, so each one unblocks only when `PutInsightSelectors` works: `cloudtrail-lake-mutation-error-contracts#task#7`, `cloudtrail-list-insights-data-time-range-rejected-on-live-trail#setup#4`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/put_insight_selectors.rs`. Replaying the corpus, the emulator refused the `PutInsightSelectors` call: status: expected 400, got 501
$.__type: value_mismatch (expected 'InvalidInsightSelectorsException', got 'NotImplemented')
$.Message: missing (expected 'The provided value is not a valid Insights type: NotARealInsightType. Valid Insight types are: ApiCallRateInsight and ApiErrorRateInsight. Verify your input and then try again.', got None)

## Steps to Reproduce
Boot the cloudtrail emulator
Call PutInsightSelectors and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
