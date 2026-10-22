Issue-Source: trace-sweep-divergence
Issue-Trace: cloudtrail-channel-and-eventdatastore-arn-format-errors
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudtrail-channel-and-eventdatastore-arn-format-errors cloudtrail-lake-mutation-error-contracts
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail|stub:CreateChannel
Issue-Commit: 2d1586b45d80f7c9b7f4b7e099aeb4390c46a491

# Implement cloudtrail CreateChannel: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2d1586b45d80f7c9b7f4b7e099aeb4390c46a491`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannel

## Description
`CreateChannel` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/create_channel.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `CreateChannel` call, so each one unblocks only when `CreateChannel` works: `cloudtrail-channel-and-eventdatastore-arn-format-errors#task#3`, `cloudtrail-channel-and-eventdatastore-arn-format-errors#task#4`, `cloudtrail-lake-mutation-error-contracts#task#5`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/create_channel.rs`. Replaying the corpus, the emulator refused the `CreateChannel` call: status: expected 400, got 501
$.__type: value_mismatch (expected 'EventDataStoreARNInvalidException', got 'NotImplemented')
$.Message: missing (expected 'The event data store ARN arn:aws:cloudtrail:us-west-2:000000000000:eventdatastorewrong/85998324-19c3-4197-86cf-3a0a481d480a format is not valid.', got None)

## Steps to Reproduce
Boot the cloudtrail emulator
Call CreateChannel and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
