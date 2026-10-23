Issue-Source: trace-sweep-divergence
Issue-Trace: cloudtrail-event-selectors-missing-trail-and-conflicting-config
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudtrail-event-selectors-missing-trail-and-conflicting-config cloudtrail-trail-error-paths cloudtrail-trail-lifecycle-error-paths cloudtrail-trail-operations-notfound-and-validation-errors
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail|stub:PutEventSelectors
Issue-Commit: 2d1586b45d80f7c9b7f4b7e099aeb4390c46a491

# Implement cloudtrail PutEventSelectors: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2d1586b45d80f7c9b7f4b7e099aeb4390c46a491`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEventSelectors

## Description
`PutEventSelectors` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/put_event_selectors.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `PutEventSelectors` call, so each one unblocks only when `PutEventSelectors` works: `cloudtrail-event-selectors-missing-trail-and-conflicting-config#task#2`, `cloudtrail-event-selectors-missing-trail-and-conflicting-config#task#3`, `cloudtrail-trail-error-paths#task#7`, `cloudtrail-trail-lifecycle-error-paths#task#7`, `cloudtrail-trail-operations-notfound-and-validation-errors#task#7`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/put_event_selectors.rs`. Replaying the corpus, the emulator refused the `PutEventSelectors` call: status: expected 400, got 501
$.__type: value_mismatch (expected 'TrailNotFoundException', got 'NotImplemented')
$.Message: missing (expected 'Unknown trail: arn:aws:cloudtrail:us-west-2:000000000000:trail/sim2real-probe-cloudtrail-evtsel-missing-7092f210 for the user: 000000000000', got None)

## Steps to Reproduce
Boot the cloudtrail emulator
Call PutEventSelectors and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
