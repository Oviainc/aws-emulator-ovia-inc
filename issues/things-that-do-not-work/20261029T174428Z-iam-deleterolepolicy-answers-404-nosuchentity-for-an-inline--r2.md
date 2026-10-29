Issue-Source: trace-sweep-divergence
Issue-Trace: mediapackage-vod-list-assets-filtered-by-packaging-group
Issue-Step: teardown#14
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediapackage-vod-list-assets-filtered-by-packaging-group
Issue-Shape-Id: com.amazonaws.mediapackagevod#MediaPackageVod
Issue-Key: iam|DeleteRolePolicy|2xx->4xx|status
Issue-Commit: c5b0e480c5d08ce7c34e019fd56b1aa2c0c8407c

# IAM DeleteRolePolicy answers 404 NoSuchEntity for an inline role policy that was just created

> _Note: this issue was identified against the code base at commit `c5b0e480c5d08ce7c34e019fd56b1aa2c0c8407c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteRolePolicy

## Description
One cause on the IAM (awsQuery, sdkId "IAM") control plane: deleting an inline role policy that exists fails with a not-found error instead of succeeding.

=== Cause 1 of 1: DeleteRolePolicy on an existing inline policy returns 404 instead of 200 ===
AWS: Smithy (services/service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json): DeleteRolePolicy, input DeleteRolePolicyRequest {RoleName @required roleNameType @length(1,64) @pattern ^[\w+=,.@-]+$, PolicyName @required policyNameType @length(1,128) @pattern ^[\w+=,.@-]+$}, output Unit, modeled errors NoSuchEntityException(404, awsQueryError NoSuchEntity), LimitExceededException(409, LimitExceeded), UnmodifiableEntityException(400, UnmodifiableEntity), ServiceFailureException(500, ServiceFailure). DeleteRolePolicy removes the named inline policy from the named role: when that role exists and carries an inline policy of that name — which is exactly the state PutRolePolicy leaves behind — the call SUCCEEDS with HTTP 200 and an awsQuery <DeleteRolePolicyResponse> envelope whose only content is ResponseMetadata/RequestId. NoSuchEntityException is reserved for the case where the role or the inline policy genuinely does not exist; its message is AWS's fixed text for that class with the requested name substituted in ("The role policy with name <PolicyName> cannot be found." when the policy is absent from an existing role, "The role with name <RoleName> cannot be found." when the role itself is absent), where the name is taken from the request and the rest of the sentence is fixed.
Emulator: answers HTTP 404 for the inline policy the same fixture attached to the same role earlier (the PutRolePolicy/role-setup steps passed), so the delete of an existing policy is rejected as not-found. Replay detail: `status: expected 200, got 404`. Hypothesis: the body is the modeled NoSuchEntityException with awsQuery error code NoSuchEntity, since 404 is the only status the model maps to this operation and that error carries httpResponseCode 404 — the replay asserts the status, so only the status is certain.
Why: a client that attaches an inline policy to a role and later detaches it gets a hard failure on a call AWS answers successfully. Teardown and reconciliation paths are where this bites hardest: the SDK raises NoSuchEntity, the caller cannot delete the policy, and the dependent DeleteRole that follows fails too because a role carrying inline policies cannot be deleted — so a provisioning run leaks the role and every cleanup retry fails identically. Code that treats NoSuchEntity as idempotent-success masks the leak instead; code that does not, aborts the whole teardown.
Fix: the DeleteRolePolicy handler in the iam service crate and the inline role-policy state it reads. The lookup must resolve the policy that PutRolePolicy stored for that role — i.e. keyed by (RoleName, PolicyName) in the same store and with the same key normalization PutRolePolicy writes — then remove it and return the empty 200 awsQuery success envelope. NoSuchEntity must be returned only when no such role, or no inline policy of that name on that role, is present. Worth verifying the two handlers agree on the key, since PutRolePolicy succeeding while DeleteRolePolicy 404s on the same pair points at a write/read key or state-container mismatch rather than at the delete logic itself.
Traces: fixture mediapackage-vod-list-assets-filtered-by-packaging-group step teardown#14 (DeleteRolePolicy, service iam), verdict mismatch, expected_status 200 / actual_status 404, diverging key status.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the booted emulator (emulator_up true, bootstrap_signal false): 79 steps total, 66 passed, 5 mismatched, 0 errored, 8 blocked, score 83.5%.

Cause 1 — fixture mediapackage-vod-list-assets-filtered-by-packaging-group, step teardown#14, operation DeleteRolePolicy, service iam, verdict mismatch, affected_count 1
  divergence_key: service=iam|DeleteRolePolicy|status=200->404|keys=status
  expected: HTTP 200 with the awsQuery <DeleteRolePolicyResponse> success envelope (ResponseMetadata/RequestId only; the request id is a per-run value and not asserted)
  actual:   HTTP 404 — `status: expected 200, got 404`, i.e. the operation is rejected as not-found
  The role and its inline policy were created by earlier steps of the SAME fixture and those steps passed, so the policy existed when teardown#14 ran. The role name, policy name and account id in the fixture are per-run environment values, not requirements; the requirement is that a DeleteRolePolicy for a (RoleName, PolicyName) pair a prior PutRolePolicy created succeeds with 200.

Model citations (smithy_query): DeleteRolePolicy input {RoleName @required, PolicyName @required}, output Unit, errors NoSuchEntityException -> httpError 404 / awsQueryError NoSuchEntity, LimitExceededException 409, UnmodifiableEntityException 400, ServiceFailureException 500. No modeled error applies to deleting a policy that exists.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from that checkout.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus) and look at fixture mediapackage-vod-list-assets-filtered-by-packaging-group.
3. The fixture's setup creates an IAM role and attaches an inline policy to it with PutRolePolicy (those steps pass). Its teardown then unwinds that state; at step teardown#14 it issues DeleteRolePolicy for the same RoleName/PolicyName pair.
4. By hand against the emulator's IAM endpoint with sigv4 credentials it accepts: CreateRole, then PutRolePolicy(RoleName=<role>, PolicyName=<p>, PolicyDocument=<doc>) -> 200, then DeleteRolePolicy(RoleName=<role>, PolicyName=<p>).
5. Observe HTTP 404 (NoSuchEntity) instead of the HTTP 200 <DeleteRolePolicyResponse> envelope AWS returns. Optionally call ListRolePolicies/GetRolePolicy for the same pair to see whether the inline policy is readable at all, which localizes the mismatch to the delete lookup or to the stored state.
6. After fixing, re-replay and confirm step teardown#14 of that fixture passes with status 200, and that DeleteRolePolicy for a role/policy pair that does NOT exist still returns 404 NoSuchEntity with AWS's message text for that class.
