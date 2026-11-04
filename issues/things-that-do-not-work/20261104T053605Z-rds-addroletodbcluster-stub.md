Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.add-role-to-cluster-contract rds.add-role-to-cluster-quota-exceeded-rejected rds.add-role-without-iam-trust-denied
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|AddRoleToDBCluster|NotImplemented
Issue-Key: rds.add-role-without-iam-trust-denied|run#11|AssertionError|assert code == "InvalidParameterValue", f"unexpected error code: {code!r}: {message!r}"
Issue-Commit: f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7

# Implement rds AddRoleToDBCluster: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `f099a2d3bf6bcdb075a9bf115e0dc86bb75f03d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddRoleToDBCluster

## Description
`AddRoleToDBCluster` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-rds-amazonrdsv19/src/ops/add_role_to_db_cluster.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `AddRoleToDBCluster` call, so each one unblocks only when `AddRoleToDBCluster` works: `rds.add-role-to-cluster-contract`, `rds.add-role-to-cluster-quota-exceeded-rejected`, `rds.add-role-without-iam-trust-denied`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-rds-amazonrdsv19/src/ops/add_role_to_db_cluster.rs`. Replaying the corpus, the emulator refused the `AddRoleToDBCluster` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the AddRoleToDBCluster operation: operation AddRoleToDBCluster is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the rds emulator
Call AddRoleToDBCluster and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
