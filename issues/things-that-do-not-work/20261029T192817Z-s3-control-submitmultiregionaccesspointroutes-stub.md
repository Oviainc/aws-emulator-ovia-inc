Issue-Source: trace-sweep-divergence
Issue-Trace: s3control-multi-region-access-point-absent-envelopes
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3control-multi-region-access-point-absent-envelopes
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control|stub:SubmitMultiRegionAccessPointRoutes
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# Implement s3-control SubmitMultiRegionAccessPointRoutes: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitMultiRegionAccessPointRoutes

## Description
`SubmitMultiRegionAccessPointRoutes` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-s3control-awss3controlservicev20180820/src/ops/submit_multi_region_access_point_routes.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `SubmitMultiRegionAccessPointRoutes` call, so each one unblocks only when `SubmitMultiRegionAccessPointRoutes` works: `s3control-multi-region-access-point-absent-envelopes#task#6`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-s3control-awss3controlservicev20180820/src/ops/submit_multi_region_access_point_routes.rs`. Replaying the corpus, the emulator refused the `SubmitMultiRegionAccessPointRoutes` call: status: expected 400, got 501
$.ErrorResponse.Error.Code: value_mismatch (expected 'InvalidRequest', got 'NotImplemented')
$.ErrorResponse.Error.Message: value_mismatch (expected 'Request invalid', got 'operation PATCH /v20180820/mrap/instances/sim2real-probe-s3c-absent-4d2c9a/routes is not implemented in this emulator')
$.ErrorResponse.RequestId: missing (expected '5QW6RJ2D7NQ8VTRZ', got None)
$.ErrorResponse.HostId: missing (expected 'end7cnnolf49q2bW22lbqW0jRoAAA+tFJc803R3YEiFciJBFtvTQfb8ujpfQt+r0n8kypr2+GHf8HPJmnbW4wICaW5ylJwTl', got None)

## Steps to Reproduce
Boot the s3-control emulator
Call SubmitMultiRegionAccessPointRoutes and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
