Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__lambda__logs__resource-groups-tagging-api__sqs.tag-resources-nonexistent-api-key-arn-rejected-per-resource
Issue-Key: apigateway__lambda__logs__resource-groups-tagging-api__sqs.tag-resources-nonexistent-api-key-arn-rejected-per-resource|run#15|AssertionError|assert fake_arn in failed, (
Issue-Commit: e2dfe022d58b39493003b3d90859f84840a12f00

# resource-groups-tagging-api TagResources accepts a well-formed ARN naming a resource that does not exist and reports an empty FailedResourcesMap

> _Note: this issue was identified against the code base at commit `e2dfe022d58b39493003b3d90859f84840a12f00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResources

## Description
One root cause.

Cause 1 of 1: TagResources does not resolve the ARNs it is given, so a syntactically valid ARN for a resource that was never created is reported as successfully tagged.
AWS: recorded behavior only for the per-resource outcome (the Smithy model types FailedResourcesMap as ResourceARN -> FailedResourceInfo{ErrorCode, ErrorMessage, StatusCode}, and TagResources returns 200 with the per-resource failures rather than a top-level error). AWS resolves each ARN and, for one that names no existing resource, returns that ARN in FailedResourcesMap with a per-resource error instead of silently succeeding.
Emulator: TagResources answered 200 with FailedResourcesMap == {} for a well-formed API Gateway api-key ARN that was never created — the call is accepted as a complete success.
Why: the response is over-permissive. A client that tags a batch of ARNs and checks FailedResourcesMap to find the ones that did not take is told everything succeeded, so typo'd, deleted or cross-account ARNs are never surfaced, and tag-driven inventory/cost allocation silently diverges from reality.
Fix: the TagResources handler's per-ARN resolution in services/service-com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126 — each ARN must be resolved against the owning service's state and unresolved ARNs returned in FailedResourcesMap.
Traces: apigateway__lambda__logs__resource-groups-tagging-api__sqs.tag-resources-nonexistent-api-key-arn-rejected-per-resource. Related open issue, which carries a DIFFERENT check's replay key and not this one: 20261022T081518Z-resource-groups-tagging-api-tagresources-is-over-permissive.md.

## Evidence
From the behavioral replay (suite simparity-behavioral-aws-v1):
- apigateway__lambda__logs__resource-groups-tagging-api__sqs.tag-resources-nonexistent-api-key-arn-rejected-per-resource -> AssertionError: TagResources accepted a well-formed ARN naming an api key that was never created; FailedResourcesMap was {}
Expected (AWS): HTTP 200 with that ARN present in FailedResourcesMap carrying a per-resource ErrorCode/StatusCode.
Actual (emulator): HTTP 200 with FailedResourcesMap == {} (reported as fully successful).

## Steps to Reproduce
1. Boot the emulator; point botocore at it (us-west-2).
2. Build a well-formed but never-created ARN, e.g. arn:aws:apigateway:us-west-2::/apikeys/<random-id>.
3. Call resourcegroupstaggingapi.TagResources(ResourceARNList=[that ARN], Tags={'sim2real':'x'}).
4. Observe FailedResourcesMap == {} instead of an entry for that ARN.
5. Re-replay apigateway__lambda__logs__resource-groups-tagging-api__sqs.tag-resources-nonexistent-api-key-arn-rejected-per-resource to confirm.
