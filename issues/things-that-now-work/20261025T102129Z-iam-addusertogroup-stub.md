Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.delete-with-dependents-returns-deleteconflict iam.get-group-lists-members iam.group-policy-explicit-deny-overrides-the-users-own-allow iam.limit-exceeded-on-add-and-attach-operations iam.op-contract-listGroupsForUser-two-groups iam.update-user-rename-carries-access-keys-and-group-membership iam.user-identity-policy-inventory-distinguishes-granted-from-bare-users
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|AddUserToGroup|NotImplemented
Issue-Commit: ea7cea25a1e6705e0f44642ae0567838786a0067

# Implement iam AddUserToGroup: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ea7cea25a1e6705e0f44642ae0567838786a0067`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddUserToGroup

## Description
`AddUserToGroup` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/add_user_to_group.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

7 recorded unit(s) fail on their `AddUserToGroup` call, so each one unblocks only when `AddUserToGroup` works: `iam.delete-with-dependents-returns-deleteconflict`, `iam.get-group-lists-members`, `iam.group-policy-explicit-deny-overrides-the-users-own-allow`, `iam.limit-exceeded-on-add-and-attach-operations`, `iam.op-contract-listGroupsForUser-two-groups`, `iam.update-user-rename-carries-access-keys-and-group-membership`, `iam.user-identity-policy-inventory-distinguishes-granted-from-bare-users`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/add_user_to_group.rs`. Replaying the corpus, the emulator refused the `AddUserToGroup` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the AddUserToGroup operation: operation AddUserToGroup is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the iam emulator
Call AddUserToGroup and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
