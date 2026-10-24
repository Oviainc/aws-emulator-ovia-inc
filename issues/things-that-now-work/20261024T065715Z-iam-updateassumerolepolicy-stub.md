Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sts.assume-role-trust-policy-mfa-condition-refuses-non-mfa-assume sts.assume-role-trust-policy-principal-arn-condition-enforced sts.assume-role-trust-policy-session-name-condition-enforced sts.concurrent-role-trust-policy-update-and-assume-role-race-consistent sts.the-assume-role-allow-decision-is-served-from-cache-after-the-trust-policy-tightens sts.tightening-the-trust-policy-does-not-invalidate-an-already-vended-session
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: iam|UpdateAssumeRolePolicy|NotImplemented
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# Implement iam UpdateAssumeRolePolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAssumeRolePolicy

## Description
`UpdateAssumeRolePolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/update_assume_role_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `UpdateAssumeRolePolicy` call, so each one unblocks only when `UpdateAssumeRolePolicy` works: `sts.assume-role-trust-policy-mfa-condition-refuses-non-mfa-assume`, `sts.assume-role-trust-policy-principal-arn-condition-enforced`, `sts.assume-role-trust-policy-session-name-condition-enforced`, `sts.concurrent-role-trust-policy-update-and-assume-role-race-consistent`, `sts.the-assume-role-allow-decision-is-served-from-cache-after-the-trust-policy-tightens`, `sts.tightening-the-trust-policy-does-not-invalidate-an-already-vended-session`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/update_assume_role_policy.rs`. Replaying the corpus, the emulator refused the `UpdateAssumeRolePolicy` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UpdateAssumeRolePolicy operation: operation UpdateAssumeRolePolicy is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the iam emulator
Call UpdateAssumeRolePolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
