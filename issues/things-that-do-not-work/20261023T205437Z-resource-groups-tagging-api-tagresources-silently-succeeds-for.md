Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2__resourcegroupstaggingapi.tag-resources-nonexistent-target-group-rejected-per-resource
Issue-Key: elbv2__resourcegroupstaggingapi.tag-resources-nonexistent-target-group-rejected-per-resource|run#16|AssertionError|assert set(failed) == {ghost_arn}, (
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# Resource Groups Tagging API TagResources silently succeeds for a nonexistent ARN: FailedResourcesMap comes back empty

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResources

## Description
One cause (Cause 1 of 1).

Cause 1 of 1: TagResources reports no per-resource failure for an ARN that does not exist.

AWS: Smithy (com-amazonaws-resourcegroupstaggingapi) models TagResources(ResourceARNList required @length 1..20, Tags required @length 1..50) -> TagResourcesOutput{FailedResourcesMap: map<ResourceARN, FailedInfo>} with HTTP 200. The operation is partial-failure by design: resources that could not be tagged appear in FailedResourcesMap with their ErrorCode/ErrorMessage/StatusCode, and resources that were tagged do not. A mixed request naming one live ELBv2 target group and one absent target-group ARN therefore returns 200 with exactly the absent ARN in FailedResourcesMap, and the live group does carry the new tag.

Emulator: FailedResourcesMap comes back empty ([]) — the absent target-group ARN is reported as tagged. The live resource's tag is applied, so only the per-resource failure reporting diverges.

Why: partial failure is the entire contract of this API. A caller that tags a batch and inspects FailedResourcesMap (the documented way to find out what did not get tagged) is told everything succeeded, so typos, deleted resources and cross-account ARNs pass silently and tag-driven automation (cost allocation, backup selection, policy enforcement) believes it covered resources it never touched.

Fix: per-resource existence resolution in the TagResources handler — populate FailedResourcesMap (with the resource's error code and status) for every ARN that cannot be resolved, while still applying the tags that succeeded.

Related open issue recording the older symptom (ELBv2 CreateTargetGroup answering InvalidAction made this surface unreachable), which does not carry this replay key: 20261023T025808Z-elbv2-resource-groups-tagging-api-resource-groups-tagging-over-elbv2-resources-is-unreachable.md.

Traces: elbv2__resource-groups-tagging-api.tag-resources-nonexistent-target-group-rejected-per-resource.

## Evidence
Expected: TagResources(ResourceARNList=[<live target group ARN>, <absent target group ARN>], Tags={...}) -> HTTP 200 with FailedResourcesMap naming exactly the absent ARN, and the live target group carrying the tag.
Actual: HTTP 200 with FailedResourcesMap empty — "FailedResourcesMap named [], expected exactly the absent target group 'arn:aws:elasticloadbalancing:us-west-2:...:targetgroup/sim2real-ghost-.../0123456789abcdef' -- the live target group '...' must not fail and the absent one must not pass".

## Steps to Reproduce
1. Create an ELBv2 target group and note its ARN.
2. Fabricate a second target-group ARN that does not exist (same account/region, unknown name/id).
3. Call resourcegroupstaggingapi TagResources(ResourceARNList=[live, absent], Tags={'sim2real:state': 'partial'}).
4. Observe FailedResourcesMap == {} instead of an entry for the absent ARN (the live group's tag is applied either way).
