Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__s3.asg-instance-role-scoped-to-one-prefix-is-denied-the-other-write autoscaling__ec2__s3.asg-launched-instance-boots-and-writes-its-own-identity
Issue-Key: iam|AddRoleToInstanceProfile|NotImplemented
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# Implement iam AddRoleToInstanceProfile: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddRoleToInstanceProfile

## Description
`AddRoleToInstanceProfile` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/add_role_to_instance_profile.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `AddRoleToInstanceProfile` call, so each one unblocks only when `AddRoleToInstanceProfile` works: `autoscaling__ec2__s3.asg-instance-role-scoped-to-one-prefix-is-denied-the-other-write`, `autoscaling__ec2__s3.asg-launched-instance-boots-and-writes-its-own-identity`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/add_role_to_instance_profile.rs`. Replaying the corpus, the emulator refused the `AddRoleToInstanceProfile` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the AddRoleToInstanceProfile operation: operation AddRoleToInstanceProfile is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the iam emulator
Call AddRoleToInstanceProfile and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
