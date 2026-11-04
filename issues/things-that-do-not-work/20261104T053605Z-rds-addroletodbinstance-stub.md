Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.add-role-to-instance-contract rds.add-role-to-instance-quota-exceeded-rejected rds.concurrent-add-role-to-instance-duplicate-race rds__s3.cli-rds-associate-role-s3-import-workflow rds__s3.cli-rds-s3-import-role-attach-workflow rds__s3.iam-role-trust-s3-service-for-rds-integration-positive rds__s3.rds-associate-iam-role-for-s3-export-positive rds__s3.rds-instance-multiple-roles-different-features-positive rds__s3.rds-remove-iam-role-association-stops-s3-access-negative rds__s3.rds-remove-role-unassociated-feature-name-not-found rds__s3.rds-s3-import-role-association-positive rds__s3.rds-s3-import-role-missing-trust-negative
Issue-Key: rds|AddRoleToDBInstance|NotImplemented
Issue-Key: rds__s3.cli-rds-associate-role-s3-import-workflow|_run_aws#6|AssertionError|raise AssertionError(f"aws CLI command failed: {' '.join(cmd)}\nstderr: {result.stderr}")
Issue-Key: rds__s3.cli-rds-s3-import-role-attach-workflow|_run_shell#7|AssertionError|raise AssertionError(
Issue-Key: rds__s3.rds-s3-import-role-missing-trust-negative|run#8|AssertionError|assert code == "InvalidParameterValue", (
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# Implement rds AddRoleToDBInstance: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddRoleToDBInstance

## Description
`AddRoleToDBInstance` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-rds-amazonrdsv19/src/ops/add_role_to_db_instance.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

12 recorded unit(s) fail on their `AddRoleToDBInstance` call, so each one unblocks only when `AddRoleToDBInstance` works: `rds.add-role-to-instance-contract`, `rds.add-role-to-instance-quota-exceeded-rejected`, `rds.concurrent-add-role-to-instance-duplicate-race`, `rds__s3.cli-rds-associate-role-s3-import-workflow`, `rds__s3.cli-rds-s3-import-role-attach-workflow`, `rds__s3.iam-role-trust-s3-service-for-rds-integration-positive`, `rds__s3.rds-associate-iam-role-for-s3-export-positive`, `rds__s3.rds-instance-multiple-roles-different-features-positive`, `rds__s3.rds-remove-iam-role-association-stops-s3-access-negative`, `rds__s3.rds-remove-role-unassociated-feature-name-not-found`, `rds__s3.rds-s3-import-role-association-positive`, `rds__s3.rds-s3-import-role-missing-trust-negative`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-rds-amazonrdsv19/src/ops/add_role_to_db_instance.rs`. Replaying the corpus, the emulator refused the `AddRoleToDBInstance` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the AddRoleToDBInstance operation: operation AddRoleToDBInstance is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the rds emulator
Call AddRoleToDBInstance and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
