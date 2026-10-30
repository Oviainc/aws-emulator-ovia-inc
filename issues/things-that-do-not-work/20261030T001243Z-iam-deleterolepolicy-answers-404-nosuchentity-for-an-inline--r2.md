Issue-Source: trace-sweep-divergence
Issue-Trace: mediapackage-vod-list-assets-filtered-by-packaging-group
Issue-Step: teardown#14
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediapackage-vod-list-assets-filtered-by-packaging-group
Issue-Shape-Id: com.amazonaws.mediapackagevod#MediaPackageVod
Issue-Key: iam|DeleteRolePolicy|2xx->4xx|status
Issue-Commit: 67ed68e040d396203b765d4dc2b0964aebb19c3c

# iam DeleteRolePolicy answers 404 NoSuchEntity for an inline policy PutRolePolicy accepted on an existing role

> _Note: this issue was identified against the code base at commit `67ed68e040d396203b765d4dc2b0964aebb19c3c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteRolePolicy

## Description
Cause 1 of 1: DeleteRolePolicy cannot delete an inline role policy that the same fixture created with PutRolePolicy on a role that still exists, so the delete answers the NoSuchEntity class (HTTP 404) where AWS answers an empty 200 success.

AWS: Smithy (services/service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json): DeleteRolePolicy input DeleteRolePolicyRequest {RoleName @required -> roleNameType @length(1,64) @pattern ^[\w+=,.@-]+$, PolicyName @required -> policyNameType @length(1,128) @pattern ^[\w+=,.@-]+$}, output shape Unit (awsQuery: an empty DeleteRolePolicyResponse with ResponseMetadata, HTTP 200). Documentation: "Deletes the specified inline policy that is embedded in the specified IAM role." Modeled errors: NoSuchEntityException (httpError 404, awsQueryError code NoSuchEntity / 404), LimitExceededException (409, LimitExceeded), UnmodifiableEntityException (400, UnmodifiableEntity), ServiceFailureException (500, ServiceFailure). NoSuchEntity is reserved for a RoleName that names no role in the account or a PolicyName that is not embedded in that role; an inline policy written by a preceding successful PutRolePolicy (same required RoleName/PolicyName pair, output Unit / HTTP 200) on a role that has not been deleted MUST delete with HTTP 200. The error message for the genuine NoSuchEntity case is AWS's fixed sentence for a missing inline role policy with the request's PolicyName and RoleName interpolated ("The role policy with name <PolicyName> cannot be found."); the names are request values, not fixed text. The recorded response for this step is HTTP 200 and asserts only the status, so the requirement here is the success status and the empty success body, not an error message.

Emulator: answers HTTP 404 — the NoSuchEntity error class — for that same RoleName/PolicyName pair. Replay detail: `status: expected 200, got 404` (divergence_key service=iam|DeleteRolePolicy|status=200->404|keys=status). The role's creation and the PutRolePolicy that embedded the policy are earlier steps of the same fixture and both passed, so the emulator accepted the write and then reported the policy as absent on delete. Only the status/error class diverges; no body member is asserted on the step.

Why: a client that embeds an inline policy on a role and later removes it — every teardown, cleanup and reconciliation path, and any CloudFormation/Terraform-style delete — gets a not-found it cannot act on while the policy is still attached. Retries keep failing, the role can then not be deleted either (AWS refuses DeleteRole while inline policies remain), and the 404 misleads the client into believing the policy was never created, so idempotent cleanup loops either abort or leave the account dirty. It also hides the real fault: the inline-policy write is not readable back.

Hypothesis (neither the model nor the recording states the mechanism): the inline role-policy store is keyed differently on write than on read — e.g. PutRolePolicy records the document under a role identity (arn, or name+path such as a path-qualified role) that DeleteRolePolicy's lookup derives differently, or the write is accepted without being persisted at all and only GetRolePolicy/ListRolePolicies would reveal it.

Fix: the iam service crate's DeleteRolePolicy handler and the inline-role-policy state it reads — resolve the (RoleName, PolicyName) pair under exactly the key PutRolePolicy wrote, remove the embedded document, and answer HTTP 200 with the empty DeleteRolePolicyResponse. Keep 404 NoSuchEntity only when the RoleName resolves to no role or the PolicyName is not embedded in it. Because the same keying is shared with GetRolePolicy and ListRolePolicies, fix it in the shared inline-policy state rather than in the delete handler alone.

Traces: mediapackage-vod-list-assets-filtered-by-packaging-group#teardown#14 (DeleteRolePolicy, verdict mismatch, expected_status 200, actual_status 404, only the status asserted).

Related open issues, none a duplicate: "iam DeleteServiceLinkedRole answers 404 NoSuchEntity for an existing service-linked role" (20261029T234938Z) is the same wrong-404-on-delete shape but on DeleteServiceLinkedRole and the role store, not inline role policies, and carries a different fixture step's replay key. "Role templates are not provisioned so dependent role operations answer 404" (20261029T184359Z) is about unprovisioned role templates, not an inline policy written in-session. "IAM NoSuchEntity messages use emulator prose instead of AWS wording" (20261029T184359Z) concerns the message text of genuine NoSuchEntity refusals; here the refusal itself is wrong — AWS answers 200.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the booted emulator (emulator_up true, bootstrap_signal false, /__admin/health answered 200): 79 steps total, 66 passed, 5 mismatched, 0 errored, 8 blocked, score 83.5%, divergence_count 5. Four of the five divergences are already owned by open issues (mediapackage-vod ListAssets / TagResource / UntagResource / DeletePackagingGroup); this is the one net-new divergence.

Cause 1 — service iam, operation DeleteRolePolicy, fixture mediapackage-vod-list-assets-filtered-by-packaging-group, step teardown#14, verdict mismatch, affected_count 1, affected [mediapackage-vod-list-assets-filtered-by-packaging-group#teardown#14]
  divergence_key: service=iam|DeleteRolePolicy|status=200->404|keys=status
  expected: HTTP 200, empty DeleteRolePolicyResponse (output shape Unit) — the inline policy embedded earlier in the fixture by PutRolePolicy is removed
  actual:   HTTP 404 — the NoSuchEntityException class (awsQueryError code NoSuchEntity, httpResponseCode 404)
  detail:   `status: expected 200, got 404`
  No body member is asserted on this step, so the finding is the status / error class. The role creation and PutRolePolicy steps of the same fixture passed, so the policy was accepted and is not deleted.

This divergence belongs to the iam service even though it was exercised from a mediapackage-vod fixture's teardown (the fixture creates an IAM role and inline policy for MediaPackage VOD's sourceRoleArn and removes them at teardown); its `service` field is "iam".

Model citations (smithy_query, operation query): DeleteRolePolicy input {RoleName @required roleNameType @length(1,64) @pattern ^[\w+=,.@-]+$, PolicyName @required policyNameType @length(1,128)}, output_shape Unit, errors NoSuchEntityException(404/NoSuchEntity), LimitExceededException(409/LimitExceeded), UnmodifiableEntityException(400/UnmodifiableEntity), ServiceFailureException(500/ServiceFailure); PutRolePolicy input {RoleName, PolicyName, PolicyDocument @required policyDocumentType @length(1,131072)}, output_shape Unit; GetRolePolicy/ListRolePolicies share the same NoSuchEntityException(404).

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from that checkout.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus).
3. Look at fixture mediapackage-vod-list-assets-filtered-by-packaging-group. Its setup creates an IAM role (CreateRole) and embeds an inline policy on it (PutRolePolicy) for use as the MediaPackage VOD sourceRoleArn; both steps pass. At teardown#14 it calls iam DeleteRolePolicy with that same RoleName and PolicyName.
4. Equivalent by hand against the emulator's data listener with sigv4 credentials it accepts:
   - POST Action=CreateRole&RoleName=<role>&AssumeRolePolicyDocument=<trust-doc>
   - POST Action=PutRolePolicy&RoleName=<role>&PolicyName=<policy>&PolicyDocument=<doc>  -> observe HTTP 200
   - POST Action=DeleteRolePolicy&RoleName=<role>&PolicyName=<policy>
5. Observe HTTP 404 with the NoSuchEntity error class. AWS answers HTTP 200 with an empty DeleteRolePolicyResponse.
6. Also call Action=GetRolePolicy and Action=ListRolePolicies for the same role to see whether the inline policy is readable back at all — that distinguishes a write that never persisted from a delete-path lookup that derives a different key.
7. After fixing, re-replay and confirm mediapackage-vod-list-assets-filtered-by-packaging-group#teardown#14 passes.
