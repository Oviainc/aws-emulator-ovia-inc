Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup__kms__s3.refarch-bkp-e2e-review-backup-plan-cross-region-lifecycle
Issue-Key: backup|GetBackupPlan|NotImplemented
Issue-Commit: 8ae1b3d406553f92eef0619abe1571efd9650c6e

# Implement backup GetBackupPlan: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `8ae1b3d406553f92eef0619abe1571efd9650c6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBackupPlan

## Description
`GetBackupPlan` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/get_backup_plan.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetBackupPlan` call, so each one unblocks only when `GetBackupPlan` works: `backup__kms__s3.refarch-bkp-e2e-review-backup-plan-cross-region-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/get_backup_plan.rs`. Replaying the corpus, the emulator refused the `GetBackupPlan` call: File "contextlib.py", line 162, in __exit__
    self.gen.throw(value)
    ~~~~~~~~~~~~~~^^^^^^^
  File "check_backup__kms__s3_refarch_bkp_e2e_review_backup_plan_cross_region_lifecycle.py", line 223, in phase
    raise _PhaseError(
        f"[phase {i}/{len(self.phases)} {name}] {type(exc).__name__}: {exc}"
    ) from exc
_simparity_dyn_check_backup__kms__s3_refarch_bkp_e2e_review_backup_plan_cross_region_lifecycle_61ec1c21._PhaseError: [phase 2/4 solve] ClientError: An error occurred (NotImplemented) when calling the GetBackupPlan operation: operation GetBackupPlan is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the backup emulator
Call GetBackupPlan and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
