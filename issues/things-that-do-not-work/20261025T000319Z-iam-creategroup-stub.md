Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.attach-list-group-policies iam.delete-with-dependents-returns-deleteconflict iam.duplicate-create-returns-entityalreadyexists iam.get-group-lists-members iam.get-nonexistent-inline-policy-returns-nosuchentity iam.group-inline-policy-lifecycle iam.group-lifecycle-crud iam.group-policy-explicit-deny-overrides-the-users-own-allow iam.iam-bound-deep-iam-3-group-inline-policy-over-5120-significant-chars-rejected iam.invalid-policy-arn-on-attach-detach-rejected iam.limit-exceeded-on-add-and-attach-operations iam.list-groups-includes-created iam.minted-identifier-prefixes-and-arn-paths iam.op-contract-deleteGroup iam.op-contract-deleteGroupPolicy iam.op-contract-listGroups-path-prefix iam.op-contract-listGroupsForUser-two-groups iam.simulate-principal-policy-group-inherited-permission iam.update-user-rename-carries-access-keys-and-group-membership iam.user-identity-policy-inventory-distinguishes-granted-from-bare-users
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|CreateGroup|NotImplemented
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# Implement iam CreateGroup: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGroup

## Description
`CreateGroup` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/create_group.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

20 recorded unit(s) fail on their `CreateGroup` call, so each one unblocks only when `CreateGroup` works: `iam.attach-list-group-policies`, `iam.delete-with-dependents-returns-deleteconflict`, `iam.duplicate-create-returns-entityalreadyexists`, `iam.get-group-lists-members`, `iam.get-nonexistent-inline-policy-returns-nosuchentity`, `iam.group-inline-policy-lifecycle`, `iam.group-lifecycle-crud`, `iam.group-policy-explicit-deny-overrides-the-users-own-allow`, `iam.iam-bound-deep-iam-3-group-inline-policy-over-5120-significant-chars-rejected`, `iam.invalid-policy-arn-on-attach-detach-rejected`, `iam.limit-exceeded-on-add-and-attach-operations`, `iam.list-groups-includes-created`, `iam.minted-identifier-prefixes-and-arn-paths`, `iam.op-contract-deleteGroup`, `iam.op-contract-deleteGroupPolicy`, `iam.op-contract-listGroups-path-prefix`, `iam.op-contract-listGroupsForUser-two-groups`, `iam.simulate-principal-policy-group-inherited-permission`, `iam.update-user-rename-carries-access-keys-and-group-membership`, `iam.user-identity-policy-inventory-distinguishes-granted-from-bare-users`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/create_group.rs`. Replaying the corpus, the emulator refused the `CreateGroup` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateGroup operation: operation CreateGroup is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the iam emulator
Call CreateGroup and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
