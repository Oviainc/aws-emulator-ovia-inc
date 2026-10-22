Issue-Source: trace-sweep-divergence
Issue-Trace: rds-db-proxy-lifecycle
Issue-Step: teardown#23
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-db-proxy-lifecycle
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: iam|DeleteRole|2xx->4xx|status
Issue-Commit: 0b8884c4ab978d23b4fe746f795a1277a2a36df3

# IAM DeleteRole answers 409 for a role AWS deletes successfully

> _Note: this issue was identified against the code base at commit `0b8884c4ab978d23b4fe746f795a1277a2a36df3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteRole

## Description
One cause. AWS: DeleteRole on an existing role whose dependents have already been removed succeeds with HTTP 200 and an empty DeleteRoleResponse; 409 is reserved for the modeled conflict errors DeleteConflictException (awsQueryError DeleteConflict, httpResponseCode 409), ConcurrentModificationException (ConcurrentModification, 409) and LimitExceededException (LimitExceeded, 409), and a missing role is NoSuchEntityException (NoSuchEntity, 404) — Smithy: IAM DeleteRole, input RoleName (roleNameType, pattern ^[\w+=,.@-]+$). The recorded response for this teardown step is 200. Emulator: answers HTTP 409, so the role is never deleted. Hypothesis: the handler keeps a dependent attachment (the RDS proxy's assume-role / attached-policy bookkeeping created earlier in the fixture) registered against the role after the dependent has been removed, and the DeleteConflict guard then fires for a role AWS considers free. Why: the standard create-use-detach-delete teardown cannot complete; the role leaks and a retry loop that treats 409 as retryable never converges. Fix: the DeleteRole handler's conflict guard and the attachment bookkeeping it reads in IAM state — the guard must only fire while a dependent (attached managed policy, instance profile, permissions boundary) is still present. Traces: rds-db-proxy-lifecycle#teardown#23.

## Evidence
rds-db-proxy-lifecycle#teardown#23 — DeleteRole: 'status: expected 200, got 409' (divergence_key service=iam|DeleteRole|status=200->409|keys=status). No error code/message was asserted by the fixture, so only the status is a stated requirement here.

## Steps to Reproduce
1) Replay the wire corpus (replay_wire_corpus).
2) Fixture rds-db-proxy-lifecycle: run setup (CreateRole plus the proxy that assumes it) and the task steps, then let teardown delete the proxy and its policies.
3) teardown#23 calls IAM DeleteRole for that role — expect 200 with an empty DeleteRoleResponse, observe 409.
