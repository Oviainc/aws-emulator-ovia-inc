Issue-Source: trace-sweep-divergence
Issue-Trace: mediapackage-vod-list-assets-filtered-by-packaging-group
Issue-Step: teardown#14
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediapackage-vod-list-assets-filtered-by-packaging-group
Issue-Shape-Id: com.amazonaws.mediapackagevod#MediaPackageVod
Issue-Key: iam|DeleteRolePolicy|2xx->4xx|status
Issue-Commit: 134aa7970ed0c23d5be465e9c36f71c1d4a91758

# IAM DeleteRolePolicy answers 404 NoSuchEntity for an existing inline role policy

> _Note: this issue was identified against the code base at commit `134aa7970ed0c23d5be465e9c36f71c1d4a91758`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteRolePolicy

## Description
DeleteRolePolicy answers HTTP 404 where AWS answers HTTP 200, when deleting an inline role policy that was put on that role earlier in the same session.

Cause 1 of 1: deleting an existing inline role policy is rejected as not-found instead of succeeding.

AWS: Smithy (com-amazonaws-iam-awsidentitymanagementv20100508) models DeleteRolePolicy with required RoleName (roleNameType, @length 1..64, pattern ^[\w+=,.@-]+$) and required PolicyName (policyNameType, @length 1..128, same pattern), and an EMPTY output (output_shape Unit) — i.e. a success is HTTP 200 carrying only the awsQuery response envelope with ResponseMetadata. Its modeled errors are NoSuchEntity (awsQueryError code NoSuchEntity, httpResponseCode 404), LimitExceeded (409), UnmodifiableEntity (400) and ServiceFailure (500). PutRolePolicy is modeled the same way (required RoleName/PolicyName/PolicyDocument, empty output). So after a successful PutRolePolicy(RoleName=R, PolicyName=P), AWS resolves DeleteRolePolicy(RoleName=R, PolicyName=P) to the stored inline policy and returns 200; the only path to 404 NoSuchEntity is a role or inline policy that does not exist. The recorded response for this step is a 200.

Emulator: the replayed teardown step answers HTTP 404 — replay detail "status: expected 200, got 404", divergence_key service=iam|DeleteRolePolicy|status=200->404|keys=status. 404 is the status IAM's awsQuery error mapping reserves for NoSuchEntity, so the emulator is reporting the role's inline policy as absent even though the same fixture created the role and attached the inline policy and those calls succeeded.

Why: inline role policies cannot be removed. Every teardown/cleanup path breaks: a client deleting a role must first delete its inline policies, and DeleteRolePolicy failing with a client-class 404 is not retryable, so the role can never be deleted either and the caller is left with undeletable state. More broadly it indicates the inline-policy store written by PutRolePolicy is not the one DeleteRolePolicy reads, so an inline policy a caller believes it attached is not actually resolvable by name — anything that reads, replaces, or revokes role permissions by policy name operates on a policy the service denies exists. Idempotent infrastructure tooling (CloudFormation-style delete, test fixture teardown) treats NoSuchEntity on delete as a hard failure of the stack rather than as success.

Fix: the iam DeleteRolePolicy handler and the role inline-policy state it looks up — the keying of inline policies by (role name, policy name) in IAM state, and the write performed by PutRolePolicy that the delete must observe. Either the delete lookup keys differently from the put (e.g. by arn/path vs plain name, or case/normalization mismatch), or the inline policies are stored off the role object the delete resolves. On a resolvable role+policy the handler must remove the policy and return 200 with an empty DeleteRolePolicyResult, reserving 404 NoSuchEntity for a role or policy name that genuinely has no entry.

Traces: fixture mediapackage-vod-list-assets-filtered-by-packaging-group, step teardown#14 (verdict mismatch, expected 200 / actual 404). Related but NOT covering: the open stub issue "Implement iam ListRolePolicies" concerns a different, unimplemented operation (and would plausibly share the same inline-policy state), and "IAM has no AWS-managed policy catalog: GetPolicy on arn:aws:iam::aws:policy/* answers 404 NoSuchEntity" concerns managed policies, not inline role policies.

## Evidence
Expected (recorded AWS): HTTP 200 for DeleteRolePolicy(RoleName=<the role created in this fixture>, PolicyName=<the inline policy put on it>), empty result body (Smithy output_shape Unit) plus the awsQuery ResponseMetadata envelope.
Actual (emulator): HTTP 404 — replay detail "status: expected 200, got 404"; divergence_key service=iam|DeleteRolePolicy|status=200->404|keys=status. 404 is IAM's awsQueryError mapping for NoSuchEntity, the only modeled 404 on this operation.
The role and policy names are fixture-generated and environment-specific; the requirement is the status (200, not 404/NoSuchEntity) for an inline policy that was successfully put on the named role earlier in the session. The error message text is not asserted here.

## Steps to Reproduce
1. git_clone() then replay_wire_corpus() against the built emulator.
2. Replay fixture mediapackage-vod-list-assets-filtered-by-packaging-group: its setup creates an IAM role and calls PutRolePolicy to attach an inline policy to it (both succeed).
3. Observe teardown step teardown#14, which calls DeleteRolePolicy with that exact RoleName and PolicyName.
4. Emulator answers HTTP 404 (NoSuchEntity mapping); the recorded AWS response is HTTP 200 with an empty result.
