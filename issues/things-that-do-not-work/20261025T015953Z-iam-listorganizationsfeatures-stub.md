Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.list-organizations-features
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|ListOrganizationsFeatures|NotImplemented
Issue-Commit: 8b134aa8d1570c43dbe434e21aa8020747d2a419

# Implement iam ListOrganizationsFeatures: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `8b134aa8d1570c43dbe434e21aa8020747d2a419`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListOrganizationsFeatures

## Description
`ListOrganizationsFeatures` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/list_organizations_features.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListOrganizationsFeatures` call, so each one unblocks only when `ListOrganizationsFeatures` works: `iam.list-organizations-features`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/list_organizations_features.rs`. Replaying the corpus, the emulator refused the `ListOrganizationsFeatures` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_iam_list_organizations_features.py", line 98, in run
    assert code in ORG_LIST_FEATURES_PRECONDITION_CODES, (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected a documented Organizations-membership precondition, got 'NotImplemented'

## Steps to Reproduce
Boot the iam emulator
Call ListOrganizationsFeatures and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
