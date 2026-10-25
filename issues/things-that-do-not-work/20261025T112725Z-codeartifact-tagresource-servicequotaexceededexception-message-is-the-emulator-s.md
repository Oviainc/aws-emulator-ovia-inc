Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeartifact.tag-resource-quota-exceeded-rejected
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact.tag-resource-quota-exceeded-rejected|run#15|AssertionError|assert message == "The resultant tag set must not have more than 50 user tags.", (
Issue-Commit: 7ec46f8e53e7b4a82f0a526dc76e942bfb70fd11

# CodeArtifact TagResource: ServiceQuotaExceededException message is the emulator's own wording, not AWS's "The resultant tag set must not have more than 50 user tags."

> _Note: this issue was identified against the code base at commit `7ec46f8e53e7b4a82f0a526dc76e942bfb70fd11`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
One net-new root cause.

Cause 1 of 1: the per-resource tag-quota refusal on TagResource carries a hand-written message instead of AWS's fixed sentence. Failing check: codeartifact.tag-resource-quota-exceeded-rejected.

AWS: TagResource refuses a request that would leave a CodeArtifact resource holding more tags than the per-resource tag quota with ServiceQuotaExceededException (Smithy: TagResource errors include ServiceQuotaExceededException, httpError 402, documented "The operation did not succeed because it would have exceeded a service limit for your account."; input TagResource.tags -> TagList @length 0..200, resourceArn -> Arn @length 1..1011 pattern ^\\S+$). The model does not fix the message text, so the message is recorded behavior only: for this error class AWS answers the fixed sentence "The resultant tag set must not have more than 50 user tags." Its ONLY variable part is the quota number (50 here, the per-resource user-tag quota in effect); the rest — "The resultant tag set must not have more than " and " user tags." — is fixed text and contains no ARN, no resource name and no resulting tag count. AWS also leaves the tag set untouched on refusal, which the emulator does correctly.

Emulator: it returns the right code (ServiceQuotaExceededException) but its own sentence, templated over the request ARN and the would-be count: "Cannot tag resource <resourceArn>: the request would leave it holding <merged tag count> tags, more than the maximum of <MAX_TAGS_PER_RESOURCE> tags per resource." In the replay that rendered as 'Cannot tag resource arn:aws:codeartifact:us-west-2:000000000000:domain/<domain-name>: the request would leave it holding 51 tags, more than the maximum of 50 tags per resource.' (the ARN, domain name and 51 are per-run values filling the emulator's template, not part of the requirement). It additionally populates resourceId/resourceType on the exception, which AWS's quota refusal for this class does not need; the diverging part asserted by the check is the message.

Why: a client that classifies or surfaces quota refusals by the documented message — log scrapers, CLI/SDK wrappers, retry/branching logic, and golden-output tests — sees a sentence that never appears in AWS and either fails to match or prints wording no AWS runbook contains. Because the code and status are right, the mismatch is invisible until a message comparison runs, so it slips into tooling built against the emulator and breaks against real AWS.

Fix: in the TagResource handler's quota branch — services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/tag_resource.rs, the `merged.len() > crate::state::MAX_TAGS_PER_RESOURCE` arm — emit AWS's fixed sentence with the quota value interpolated and nothing else; the quota constant lives in crate::state (MAX_TAGS_PER_RESOURCE). The same wording belongs anywhere else this service refuses on the per-resource tag quota so the two cannot drift.

Traces: codeartifact.tag-resource-quota-exceeded-rejected (fills a fresh domain to exactly 50 tags, then adds one more distinct tag).

Note: this cause is NOT covered by the open issues 20261024T061931Z-codeartifact-cfn-provisioned-domains-absent-resource-policies-never.md (CFN provisioning, resource-policy evaluation, origin-block classification), 20261022T171312Z-codeartifact-no-iam-authorization-on-reads-publishpackageversion-digest.md, or 20261025T024237Z-codeartifact-repository-endpoints-serve-nothing-npm-registry-404.md (npm data-plane routing/token gating); none of them touches the tag-quota refusal message.

## Evidence
Check: codeartifact.tag-resource-quato-exceeded-rejected [codeartifact.tag-resource-quota-exceeded-rejected]
Step: create domain -> TagResource with 50 distinct tags (succeeds) -> ListTagsForResource returns 50 -> TagResource with 1 further distinct tag.

Expected (AWS): HTTP 402, code ServiceQuotaExceededException, message exactly "The resultant tag set must not have more than 50 user tags." (fixed text, with the quota 50 as its only variable part); tag set unchanged at 50.

Actual (emulator): HTTP 402, code ServiceQuotaExceededException (correct), message 'Cannot tag resource arn:aws:codeartifact:us-west-2:000000000000:domain/sim2real-ca-86ba1cd101db0: the request would leave it holding 51 tags, more than the maximum of 50 tags per resource.'

Assertion from the replay:
  File "check_codeartifact_tag_resource_quota_exceeded_rejected.py", line 50, in run
    assert message == "The resultant tag set must not have more than 50 user tags."
  AssertionError: the ServiceQuotaExceededException said 'Cannot tag resource arn:aws:codeartifact:us-west-2:000000000000:domain/sim2real-ca-86ba1cd101db0: the request would leave it holding 51 tags, more than the maximum of 50 tags per resource.', expected AWS's exact 'The resultant tag set must not have more than 50 user tags.'

Source of the diverging text: services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/tag_resource.rs lines 212-227.

The rest of the check's assertions passed: the refusal left the tag set at exactly the quota, the overflow tag was absent afterwards, and freeing one slot let the same tag be added — so only the message diverges.

## Steps to Reproduce
1. Boot the emulator and point a botocore codeartifact client at it.
2. CreateDomain with a fresh lowercase domain name; keep the returned domain.arn.
3. TagResource(resourceArn=<that arn>, tags=[{key: k0..k49, value: v0..v49}]) — 50 distinct keys. Confirm ListTagsForResource returns 50 tags.
4. TagResource(resourceArn=<that arn>, tags=[{key: 'extra-tag', value: 'overflow'}]).
5. Observe the ClientError: Code is ServiceQuotaExceededException (correct) but Message is the emulator's 'Cannot tag resource <arn>: the request would leave it holding 51 tags, more than the maximum of 50 tags per resource.' instead of AWS's 'The resultant tag set must not have more than 50 user tags.'
6. Re-replay codeartifact.tag-resource-quota-exceeded-rejected to confirm the fix.
