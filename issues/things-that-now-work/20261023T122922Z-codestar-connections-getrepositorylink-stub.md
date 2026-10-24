Issue-Source: trace-sweep-divergence
Issue-Trace: codestar-connections-repository-link-error-paths
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codestar-connections-repository-link-error-paths
Issue-Shape-Id: com.amazonaws.codestarconnections#CodeStar_connections_20191201
Issue-Key: codestar-connections|stub:GetRepositoryLink
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# Implement codestar-connections GetRepositoryLink: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRepositoryLink

## Description
`GetRepositoryLink` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codestarconnections-codestar-connections-20191201/src/ops/get_repository_link.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetRepositoryLink` call, so each one unblocks only when `GetRepositoryLink` works: `codestar-connections-repository-link-error-paths#task#4`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codestarconnections-codestar-connections-20191201/src/ops/get_repository_link.rs`. Replaying the corpus, the emulator refused the `GetRepositoryLink` call: status: expected 400, got 501
$.__type: value_mismatch (expected 'com.amazonaws.codestar.connections#ResourceNotFoundException', got 'NotImplemented')
$.Message: missing (expected '<shaped-not-asserted>', got None)

## Steps to Reproduce
Boot the codestar-connections emulator
Call GetRepositoryLink and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
