Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: securityagent.batch-create-security-requirements-duplicate-partial-failure securityagent.batch-create-security-requirements-round-trip securityagent.batch-delete-security-requirements-partial-not-found securityagent.batch-update-security-requirements-round-trip securityagent.concurrent-pack-creation-same-name-no-corruption securityagent.duplicate-security-requirement-pack-name-allowed securityagent.security-requirement-pack-disabled-status-at-creation securityagent.security-requirement-pack-lifecycle-crud securityagent.update-security-requirement-pack-persists-changes
Issue-Shape-Id: com.amazonaws.securityagent#SecurityAgent
Issue-Key: securityagent|CreateSecurityRequirementPack|NotImplemented
Issue-Key: securityagent.concurrent-pack-creation-same-name-no-corruption|run#8|AssertionError|assert len(successes) >= 2, (
Issue-Commit: 00577afa9aa0eead33f7dafc8adac2dd3659e1fc

# Implement securityagent CreateSecurityRequirementPack: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `00577afa9aa0eead33f7dafc8adac2dd3659e1fc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSecurityRequirementPack

## Description
`CreateSecurityRequirementPack` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-securityagent-securityagent/src/ops/create_security_requirement_pack.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

9 recorded unit(s) fail on their `CreateSecurityRequirementPack` call, so each one unblocks only when `CreateSecurityRequirementPack` works: `securityagent.batch-create-security-requirements-duplicate-partial-failure`, `securityagent.batch-create-security-requirements-round-trip`, `securityagent.batch-delete-security-requirements-partial-not-found`, `securityagent.batch-update-security-requirements-round-trip`, `securityagent.concurrent-pack-creation-same-name-no-corruption`, `securityagent.duplicate-security-requirement-pack-name-allowed`, `securityagent.security-requirement-pack-disabled-status-at-creation`, `securityagent.security-requirement-pack-lifecycle-crud`, `securityagent.update-security-requirement-pack-persists-changes`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-securityagent-securityagent/src/ops/create_security_requirement_pack.rs`. Replaying the corpus, the emulator refused the `CreateSecurityRequirementPack` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateSecurityRequirementPack operation: operation CreateSecurityRequirementPack is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the securityagent emulator
Call CreateSecurityRequirementPack and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
