Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: imagebuilder.ibp-update-distribution-configuration-rejects-an-unknown-region imagebuilder.update-distribution-configuration-persists-new-region
Issue-Shape-Id: com.amazonaws.imagebuilder#imagebuilder
Issue-Key: imagebuilder.ibp-update-distribution-configuration-rejects-an-unknown-region|run#9|AssertionError|assert got == expected, f"unknown-region update: expected {expected!r}, got {got!r}"
Issue-Key: imagebuilder|UpdateDistributionConfiguration|NotImplemented
Issue-Commit: ed195bdda9ffdbdf051f799fc5087e4de9084b05

# Implement imagebuilder UpdateDistributionConfiguration: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ed195bdda9ffdbdf051f799fc5087e4de9084b05`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDistributionConfiguration

## Description
`UpdateDistributionConfiguration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-imagebuilder-imagebuilder/src/ops/update_distribution_configuration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `UpdateDistributionConfiguration` call, so each one unblocks only when `UpdateDistributionConfiguration` works: `imagebuilder.ibp-update-distribution-configuration-rejects-an-unknown-region`, `imagebuilder.update-distribution-configuration-persists-new-region`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-imagebuilder-imagebuilder/src/ops/update_distribution_configuration.rs`. Replaying the corpus, the emulator refused the `UpdateDistributionConfiguration` call: File "run.py", line 353, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_imagebuilder_ibp_update_distribution_configuration_rejects_an_unknown_region.py", line 35, in run
    assert got == expected, f"unknown-region update: expected {expected!r}, got {got!r}"
           ^^^^^^^^^^^^^^^
AssertionError: unknown-region update: expected ('InvalidParameterCombinationException', "The value supplied for parameter 'distributions[0].amiDistributionConfiguration' is not valid. AMI Distribution is not available in 'us-nowhere-1'."), got ('NotImplemented', 'operation UpdateDistributionConfiguration is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the imagebuilder emulator
Call UpdateDistributionConfiguration and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
