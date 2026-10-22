Issue-Source: trace-sweep-divergence
Issue-Trace: mediapackage-vod-list-assets-filtered-by-packaging-group
Issue-Step: teardown#14
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediapackage-vod-list-assets-filtered-by-packaging-group
Issue-Shape-Id: com.amazonaws.mediapackagevod#MediaPackageVod
Issue-Key: iam|DeleteRolePolicy|2xx->4xx|status
Issue-Commit: adba6aa61f0ba2f78a6b2d7c8f4b487f05884eaf

# IAM DeleteRolePolicy answers 404 NoSuchEntity for an inline policy that was successfully put

> _Note: this issue was identified against the code base at commit `adba6aa61f0ba2f78a6b2d7c8f4b487f05884eaf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteRolePolicy

## Description
One cause.

=== Cause 1 of 1: DeleteRolePolicy cannot find an inline role policy the emulator already accepted via PutRolePolicy ===
AWS: Smithy (com.amazonaws.iam, AWSIdentityManagementV20100508, awsQuery): DeleteRolePolicy takes required RoleName (roleNameType, length 1-64, pattern ^[\w+=,.@-]+$) and required PolicyName (policyNameType, length 1-128, pattern ^[\w+=,.@-]+$), returns Unit, and raises NoSuchEntityException (errorKind client, httpError 404, awsQueryError code NoSuchEntity / httpResponseCode 404) only when the role or the named inline policy does not exist. After a successful PutRolePolicy (required RoleName, PolicyName, PolicyDocument) for the same RoleName/PolicyName pair, AWS deletes the inline policy and answers HTTP 200 with an empty awsQuery response envelope — which is what the recording shows for this teardown step.
Emulator: answers HTTP 404 — the inline policy put earlier in the same fixture is not found at delete time, so a delete AWS performs successfully is refused as a missing entity.
Why: the inline role-policy attachment is write-only in the emulator: a client that attaches an inline policy to a role and later detaches it (every IAM-role teardown, and every policy-rotation flow that deletes before re-putting) gets 404 NoSuchEntity. A teardown that treats 404 as fatal aborts and leaks the role; one that re-puts after the failed delete accumulates state. It also means an inline policy cannot be observed or removed once attached, so nothing downstream can assert on a role's effective inline policy set.
Hypothesis: PutRolePolicy (recently implemented — see the resolved issue 'Implement iam PutRolePolicy: unimplemented scaffold stub') records the inline policy under a key the DeleteRolePolicy lookup does not read, or does not record it at all.
Fix: the inline role-policy state shared by the PutRolePolicy and DeleteRolePolicy handlers in the iam service crate — PutRolePolicy must persist the (RoleName, PolicyName, PolicyDocument) entry into the store that DeleteRolePolicy looks up, and DeleteRolePolicy must answer 200 with the empty awsQuery envelope when it finds it, reserving NoSuchEntity/404 for a genuinely absent role or policy name.
Traces: mediapackage-vod-list-assets-filtered-by-packaging-group#teardown#14 (an iam step inside a mediapackage-vod fixture: the fixture creates an IAM role with an inline S3 access policy for MediaPackage VOD asset ingest, then tears it down).

Note: this is not covered by the open 'Implement iam GetRole' stub issue (a different operation) nor by the resolved 'IAM DeleteRole answers 409 for a role AWS deletes successfully' (DeleteRole, not DeleteRolePolicy).

## Evidence
Wire-replay divergence from trace set simparity-wire-aws-v1 (emulator up, 0 state_reset_failures):

  fixture_id: mediapackage-vod-list-assets-filtered-by-packaging-group
  step_key:   teardown#14
  service:    iam
  operation:  DeleteRolePolicy
  verdict:    mismatch
  divergence_key: service=iam|DeleteRolePolicy|status=200->404|keys=status
  detail:     status: expected 200, got 404
  affected_count: 1

Earlier in the same fixture the role is created and PutRolePolicy succeeds (the role ARN it produces is consumed by CreateAsset's sourceRoleArn, e.g. arn:aws:iam::000000000000:role/sim2real-mpvod-role-... — an example value, not a requirement), so the inline policy demonstrably existed from the client's point of view before the delete was refused.

Model citations from services/service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json: DeleteRolePolicy input {RoleName required, PolicyName required}, output Unit, errors LimitExceededException(409/LimitExceeded), NoSuchEntityException(404/NoSuchEntity), ServiceFailureException(500/ServiceFailure), UnmodifiableEntityException(400/UnmodifiableEntity); PutRolePolicy input {RoleName, PolicyName, PolicyDocument all required}, output Unit. The required fact here is the HTTP status: 200 for a delete of an existing inline policy, 404/NoSuchEntity only when it is absent.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from the checkout.
2. Replay trace set simparity-wire-aws-v1 against it (replay_wire_corpus).
3. Replay fixture mediapackage-vod-list-assets-filtered-by-packaging-group: its setup calls iam CreateRole and then PutRolePolicy to attach an inline policy to that role (both succeed).
4. Reach step teardown#14, which calls iam DeleteRolePolicy with the same RoleName and PolicyName.
5. Expect HTTP 200 with the empty awsQuery DeleteRolePolicy response; observe HTTP 404 (NoSuchEntity).
6. Equivalent minimal check: PutRolePolicy(RoleName=r, PolicyName=p, PolicyDocument=d) then DeleteRolePolicy(RoleName=r, PolicyName=p) against the emulator's iam endpoint.
