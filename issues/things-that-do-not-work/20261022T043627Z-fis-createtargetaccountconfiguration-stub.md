Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: fis.concurrent-target-account-configuration-updates-last-write-wins fis.create-target-account-configuration-duplicate-conflict fis.delete-target-account-configuration-running-experiment-isolated fis.get-experiment-target-account-configuration-cross-experiment-not-found fis.get-experiment-target-account-configuration-not-found-rejected fis.list-target-account-configurations-invalid-token-rejected fis.list-target-account-configurations-pagination-covers-all-accounts fis.target-account-configuration-empty-vs-absent-role-arn-preserved fis.target-account-configuration-lifecycle fis.target-account-configuration-role-arn-round-trip fis.target-account-configuration-unicode-description-round-trip fis.update-target-account-configuration-malformed-role-arn-rejected fis.update-target-account-configuration-nonexistent-rejected
Issue-Shape-Id: com.amazonaws.fis#FaultInjectionSimulator
Issue-Key: fis|CreateTargetAccountConfiguration|NotImplemented
Issue-Commit: bffba5cba440022693dbdf16c8135b8d2c175737

# Implement fis CreateTargetAccountConfiguration: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `bffba5cba440022693dbdf16c8135b8d2c175737`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTargetAccountConfiguration

## Description
`CreateTargetAccountConfiguration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/create_target_account_configuration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

13 recorded unit(s) fail on their `CreateTargetAccountConfiguration` call, so each one unblocks only when `CreateTargetAccountConfiguration` works: `fis.concurrent-target-account-configuration-updates-last-write-wins`, `fis.create-target-account-configuration-duplicate-conflict`, `fis.delete-target-account-configuration-running-experiment-isolated`, `fis.get-experiment-target-account-configuration-cross-experiment-not-found`, `fis.get-experiment-target-account-configuration-not-found-rejected`, `fis.list-target-account-configurations-invalid-token-rejected`, `fis.list-target-account-configurations-pagination-covers-all-accounts`, `fis.target-account-configuration-empty-vs-absent-role-arn-preserved`, `fis.target-account-configuration-lifecycle`, `fis.target-account-configuration-role-arn-round-trip`, `fis.target-account-configuration-unicode-description-round-trip`, `fis.update-target-account-configuration-malformed-role-arn-rejected`, `fis.update-target-account-configuration-nonexistent-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-fis-faultinjectionsimulator/src/ops/create_target_account_configuration.rs`. Replaying the corpus, the emulator refused the `CreateTargetAccountConfiguration` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateTargetAccountConfiguration operation: operation CreateTargetAccountConfiguration is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the fis emulator
Call CreateTargetAccountConfiguration and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
