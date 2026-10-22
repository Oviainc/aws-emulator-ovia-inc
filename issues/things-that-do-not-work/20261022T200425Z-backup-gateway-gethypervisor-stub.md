Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup-gateway.get-hypervisor-arn-validation-and-not-found
Issue-Shape-Id: com.amazonaws.backupgateway#BackupOnPremises_v20210101
Issue-Key: backup-gateway.get-hypervisor-arn-validation-and-not-found|run#8|AssertionError|assert not_found == ("ResourceNotFoundException", HYPERVISOR_NOT_FOUND), (
Issue-Commit: dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c

# Implement backup-gateway GetHypervisor: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetHypervisor

## Description
`GetHypervisor` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-backupgateway-backuponpremises-v20210101/src/ops/get_hypervisor.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetHypervisor` call, so each one unblocks only when `GetHypervisor` works: `backup-gateway.get-hypervisor-arn-validation-and-not-found`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-backupgateway-backuponpremises-v20210101/src/ops/get_hypervisor.rs`. Replaying the corpus, the emulator refused the `GetHypervisor` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_backup_gateway_get_hypervisor_arn_validation_and_not_found.py", line 43, in run
    assert not_found == ("ResourceNotFoundException", HYPERVISOR_NOT_FOUND), (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ResourceNotFoundException for a well-formed but nonexistent HypervisorArn, got ('NotImplemented', 'operation GetHypervisor is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the backup-gateway emulator
Call GetHypervisor and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
