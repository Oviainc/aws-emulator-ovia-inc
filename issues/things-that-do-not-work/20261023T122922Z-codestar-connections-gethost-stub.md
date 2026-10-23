Issue-Source: trace-sweep-divergence
Issue-Trace: codestar-connections-host-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codestar-connections-host-error-paths
Issue-Shape-Id: com.amazonaws.codestarconnections#CodeStar_connections_20191201
Issue-Key: codestar-connections|stub:GetHost
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# Implement codestar-connections GetHost: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetHost

## Description
`GetHost` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codestarconnections-codestar-connections-20191201/src/ops/get_host.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetHost` call, so each one unblocks only when `GetHost` works: `codestar-connections-host-error-paths#task#1`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codestarconnections-codestar-connections-20191201/src/ops/get_host.rs`. Replaying the corpus, the emulator refused the `GetHost` call: status: expected 400, got 501
$.__type: value_mismatch (expected 'com.amazonaws.codestar.connections#ResourceNotFoundException', got 'NotImplemented')
$.Message: missing (expected 'No host exists with the ARN: arn:aws:codestar-connections:us-west-2:000000000000:host/00000000-0000-0000-0000-000000000000', got None)

## Steps to Reproduce
Boot the codestar-connections emulator
Call GetHost and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
