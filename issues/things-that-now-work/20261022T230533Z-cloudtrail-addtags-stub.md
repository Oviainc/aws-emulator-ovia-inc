Issue-Source: trace-sweep-divergence
Issue-Trace: cloudtrail-add-tags-not-found-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudtrail-add-tags-not-found-errors cloudtrail-channel-and-eventdatastore-arn-format-errors cloudtrail-channel-notfound-error-paths cloudtrail-tag-and-channel-arn-validation-errors cloudtrail-tag-and-channel-error-paths cloudtrail-tag-arn-format-and-notfound-errors cloudtrail-tagging-channel-and-policy-not-found-error-paths
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail|stub:AddTags
Issue-Commit: 2d1586b45d80f7c9b7f4b7e099aeb4390c46a491

# Implement cloudtrail AddTags: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2d1586b45d80f7c9b7f4b7e099aeb4390c46a491`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddTags

## Description
`AddTags` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/add_tags.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

11 recorded unit(s) fail on their `AddTags` call, so each one unblocks only when `AddTags` works: `cloudtrail-add-tags-not-found-errors#task#1`, `cloudtrail-add-tags-not-found-errors#task#2`, `cloudtrail-channel-and-eventdatastore-arn-format-errors#task#2`, `cloudtrail-channel-notfound-error-paths#task#3`, `cloudtrail-tag-and-channel-arn-validation-errors#task#1`, `cloudtrail-tag-and-channel-arn-validation-errors#task#2`, `cloudtrail-tag-and-channel-error-paths#task#1`, `cloudtrail-tag-and-channel-error-paths#task#4`, `cloudtrail-tag-arn-format-and-notfound-errors#task#1`, `cloudtrail-tag-arn-format-and-notfound-errors#task#2`, `cloudtrail-tagging-channel-and-policy-not-found-error-paths#task#1`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/add_tags.rs`. Replaying the corpus, the emulator refused the `AddTags` call: status: expected 400, got 501
$.__type: value_mismatch (expected 'EventDataStoreNotFoundException', got 'NotImplemented')
$.Message: missing (expected 'The event data store arn:aws:cloudtrail:us-west-2:000000000000:eventdatastore/a1b9f92c-fa85-40de-9198-dd92e0c55524 was not found.', got None)

## Steps to Reproduce
Boot the cloudtrail emulator
Call AddTags and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
