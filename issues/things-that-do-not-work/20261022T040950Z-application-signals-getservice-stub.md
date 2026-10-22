Issue-Source: trace-sweep-divergence
Issue-Trace: application-signals-discovery-and-service-topology
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: application-signals-discovery-and-service-topology
Issue-Shape-Id: com.amazonaws.applicationsignals#ApplicationSignals
Issue-Key: application-signals|stub:GetService
Issue-Commit: 6d3a954d57304c873e4e6a3ccc977c1ac1664bbd

# Implement application-signals GetService: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6d3a954d57304c873e4e6a3ccc977c1ac1664bbd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetService

## Description
`GetService` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-applicationsignals-applicationsignals/src/ops/get_service.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetService` call, so each one unblocks only when `GetService` works: `application-signals-discovery-and-service-topology#task#8`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-applicationsignals-applicationsignals/src/ops/get_service.rs`. Replaying the corpus, the emulator refused the `GetService` call: status: expected 400, got 501
$.message: value_mismatch (expected "1 validation error detected: Value at 'endTime' failed to satisfy constraint: Member must not be null", got 'operation POST /service is not implemented in this emulator')
$headers.x-amzn-errortype: value_mismatch (expected 'ValidationException:http://internal.amazon.com/coral/com.amazon.coral.validate/', got 'NotImplemented')

## Steps to Reproduce
Boot the application-signals emulator
Call GetService and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
