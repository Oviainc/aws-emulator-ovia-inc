Issue-Source: trace-sweep-divergence
Issue-Trace: drs-resource-not-found-and-tag-arn-errors
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: drs-resource-not-found-and-tag-arn-errors
Issue-Shape-Id: com.amazonaws.drs#ElasticDisasterRecoveryService
Issue-Key: drs|stub:TagResource
Issue-Commit: 563433aa8ea88fa1a8e8ad3e10fc27d869dea4a3

# Implement drs TagResource: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `563433aa8ea88fa1a8e8ad3e10fc27d869dea4a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
`TagResource` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-drs-elasticdisasterrecoveryservice/src/ops/tag_resource.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `TagResource` call, so each one unblocks only when `TagResource` works: `drs-resource-not-found-and-tag-arn-errors#task#4`, `drs-resource-not-found-and-tag-arn-errors#task#5`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-drs-elasticdisasterrecoveryservice/src/ops/tag_resource.rs`. Replaying the corpus, the emulator refused the `TagResource` call: status: expected 403, got 501
$.message: value_mismatch (expected 'Unable to determine service/operation name to be authorized', got 'operation POST /TagResource is not implemented in this emulator')
$headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'NotImplemented')

## Steps to Reproduce
Boot the drs emulator
Call TagResource and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
