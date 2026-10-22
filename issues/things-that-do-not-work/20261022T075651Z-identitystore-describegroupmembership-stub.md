Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: identitystore.delete-group-membership-direct-removes-from-list identitystore.delete-group-membership-repeat-delete-idempotent identitystore.delete-user-cascades-membership-removal identitystore.delete-user-removes-memberships identitystore.describe-group-membership-fields-consistent identitystore.group-membership-create-then-describe identitystore.group-membership-created-and-described
Issue-Shape-Id: com.amazonaws.identitystore#AWSIdentityStore
Issue-Key: identitystore.delete-group-membership-direct-removes-from-list|run#14|AssertionError|assert err == ("ResourceNotFoundException", "Group membership not found for given pool and membershipId."), (
Issue-Key: identitystore.delete-group-membership-repeat-delete-idempotent|run#12|AssertionError|assert describe_err == ("ResourceNotFoundException", "Group membership not found for given pool and membershipId."), (
Issue-Key: identitystore|DescribeGroupMembership|NotImplemented
Issue-Commit: 4cbced68d8f61275ceb5b427ff8478e9aa7bafe5

# Implement identitystore DescribeGroupMembership: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `4cbced68d8f61275ceb5b427ff8478e9aa7bafe5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeGroupMembership

## Description
`DescribeGroupMembership` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-identitystore-awsidentitystore/src/ops/describe_group_membership.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

7 recorded unit(s) fail on their `DescribeGroupMembership` call, so each one unblocks only when `DescribeGroupMembership` works: `identitystore.delete-group-membership-direct-removes-from-list`, `identitystore.delete-group-membership-repeat-delete-idempotent`, `identitystore.delete-user-cascades-membership-removal`, `identitystore.delete-user-removes-memberships`, `identitystore.describe-group-membership-fields-consistent`, `identitystore.group-membership-create-then-describe`, `identitystore.group-membership-created-and-described`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-identitystore-awsidentitystore/src/ops/describe_group_membership.rs`. Replaying the corpus, the emulator refused the `DescribeGroupMembership` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_identitystore_delete_group_membership_direct_removes_from_list.py", line 57, in run
    assert err == ("ResourceNotFoundException", "Group membership not found for given pool and membershipId."), (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected DescribeGroupMembership on deleted membership_a (2a7710d9-7da9-44fe-8477-5a871599c3dd) to raise ResourceNotFoundException, got ('NotImplemented', 'operation DescribeGroupMembership is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the identitystore emulator
Call DescribeGroupMembership and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
