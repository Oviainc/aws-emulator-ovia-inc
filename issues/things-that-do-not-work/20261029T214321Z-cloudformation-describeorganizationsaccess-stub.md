Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.invalid-operation-error-org-and-stackset-rejected cloudformation.invalid-parameter-combination-stackset-ops-rejected cloudformation.organizations-access-op-not-found
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.invalid-operation-error-org-and-stackset-rejected|run#20|AssertionError|assert error2.get("Code") == "InvalidOperationException", (
Issue-Key: cloudformation.invalid-parameter-combination-stackset-ops-rejected|run#12|AssertionError|assert error2.get("Code") == "InvalidOperationException", (
Issue-Key: cloudformation|DescribeOrganizationsAccess|NotImplemented
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# Implement cloudformation DescribeOrganizationsAccess: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeOrganizationsAccess

## Description
`DescribeOrganizationsAccess` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudformation-cloudformation/src/ops/describe_organizations_access.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `DescribeOrganizationsAccess` call, so each one unblocks only when `DescribeOrganizationsAccess` works: `cloudformation.invalid-operation-error-org-and-stackset-rejected`, `cloudformation.invalid-parameter-combination-stackset-ops-rejected`, `cloudformation.organizations-access-op-not-found`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudformation-cloudformation/src/ops/describe_organizations_access.rs`. Replaying the corpus, the emulator refused the `DescribeOrganizationsAccess` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_cloudformation_invalid_operation_error_org_and_stackset_rejected.py", line 81, in run
    assert error2.get("Code") == "InvalidOperationException", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: DescribeOrganizationsAccess(CallAs=DELEGATED_ADMIN) from a non-delegated-admin account: expected InvalidOperationException, got {'Type': 'Receiver', 'Code': 'NotImplemented', 'Message': 'operation DescribeOrganizationsAccess is scaffolded but not implemented in this emulator'}

## Steps to Reproduce
Boot the cloudformation emulator
Call DescribeOrganizationsAccess and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
