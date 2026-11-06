Issue-Source: trace-sweep-divergence
Issue-Trace: resource-groups-tagging-api-tag-resources-validation-error-paths
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resource-groups-tagging-api-tag-resources-validation-error-paths
Issue-Shape-Id: com.amazonaws.resourcegroupstaggingapi#ResourceGroupsTaggingAPI_20170126
Issue-Key: resourcegroupstaggingapi|TagResources|2xx->2xx|$.FailedResourcesMap.arn:aws:sns:us-west-2:000000000000:sim2real-probe-rgta-missing-a7d5c72d
Issue-Commit: 45af2140f358870cdf193287cbc1a2501641d010

# TagResources omits the per-resource FailedResourcesMap entry for an absent SNS topic ARN

> _Note: this issue was identified against the code base at commit `45af2140f358870cdf193287cbc1a2501641d010`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResources

## Description
One cause (Cause 1 of 1).

Cause 1 of 1: TagResources reports no per-resource failure for a resource ARN that does not exist, so a batch containing an unresolvable ARN is reported as fully tagged.

AWS: Smithy (com-amazonaws-resourcegroupstaggingapi, TagResources POST /TagResources via X-Amz-Target, RPC protocol) models TagResourcesInput{ResourceARNList @required @length 1..20, Tags @required @length 1..50} -> TagResourcesOutput{FailedResourcesMap: map<ResourceARN, FailureInfo>} with HTTP 200. The operation is partial-failure by design: every ARN that could NOT be tagged appears as a FailedResourcesMap entry keyed by that exact ARN from the request, carrying FailureInfo{StatusCode: integer, ErrorCode, ErrorMessage}; ARNs that were tagged do not appear. The model's FailureInfo documentation states ErrorCode may be "any valid error code returned by the Amazon Web Services service that hosts the resource that you want to tag", i.e. the owning service's refusal is relayed per resource rather than failing the whole call. For an SNS topic ARN in the caller's own account/region that names no existing topic, the recorded response carries StatusCode 404, ErrorCode "ResourceNotFound" and ErrorMessage "Resource does not exist" (recorded behavior only for the exact code/message; the model constrains only the shape). In that message the entire text "Resource does not exist" is fixed text with no variable parts; the variable part of the entry is its KEY, which is the request's ARN reproduced verbatim, and the 404 status is SNS's not-found status for the resource it hosts.

Emulator: the call answers HTTP 200 with FailedResourcesMap containing no entry for the absent ARN at all (the key for the requested topic ARN is missing / the map is empty), so the unresolvable resource is reported as successfully tagged. Replay detail: "$.FailedResourcesMap.<absent sns topic arn>: missing (expected {'ErrorCode': 'ResourceNotFound', 'ErrorMessage': 'Resource does not exist', 'StatusCode': 404}, got None)".

Why: partial failure is the whole contract of this API, and FailedResourcesMap is the documented and only way a client learns what was not tagged. A client that tags a batch assembled from an inventory that has drifted (typos, deleted topics, cross-account ARNs) is told everything succeeded, so tag-driven automation — cost allocation, backup selection, policy enforcement, cleanup — believes it covered resources it never touched and never retries or reports them.

Fix: the resourcegroupstaggingapi TagResources handler must resolve each ResourceARNList entry against the owning service's state and, for every ARN it cannot resolve, emit a FailedResourcesMap entry keyed by that ARN with the owning service's status/code/message (here SNS's 404 / ResourceNotFound / "Resource does not exist"), while still applying the tags for the ARNs that do resolve.

Related OPEN issues that do NOT cover this step (neither carries this replay key): 20261023T205437Z-resource-groups-tagging-api-tagresources-silently-succeeds-for.md describes the same empty-FailedResourcesMap symptom for an absent ELBv2 target group, and 20261102T093929Z-lambda-resourcegroupstaggingapi-lambda-tag-plane-and-the-resource-groups-tagging.md records it (Cause 4) for an absent Lambda function; both are behavioral findings against other owning services, and neither names the SNS-hosted resource path or this wire fixture step. The fixed issue 20261022T081518Z-resource-groups-tagging-api-tagresources-is-over-permissive.md does not cover it either, a resolved issue never covering a live failure.

Traces: resource-groups-tagging-api-tag-resources-validation-error-paths#task#5.

## Evidence
Fixture resource-groups-tagging-api-tag-resources-validation-error-paths, step task#5, operation TagResources (service resourcegroupstaggingapi).

Expected (recorded AWS response): HTTP 200 with FailedResourcesMap carrying exactly one entry, keyed by the requested-but-absent SNS topic ARN (arn:aws:sns:<region>:<account>:<topic-name>), whose value is {"StatusCode": 404, "ErrorCode": "ResourceNotFound", "ErrorMessage": "Resource does not exist"}.

Actual (emulator): HTTP 200, status matched, but the FailedResourcesMap entry for that ARN is absent — replay reports "$.FailedResourcesMap.arn:aws:sns:us-west-2:000000000000:sim2real-probe-rgta-missing-<suffix>: missing (expected {'ErrorCode': 'ResourceNotFound', 'ErrorMessage': 'Resource does not exist', 'StatusCode': 404}, got None)". The response therefore reports total success for a batch in which one resource could not be tagged. (The account id, region and generated topic name above are the recording's values, not part of the requirement; the requirement is that the key is the request's ARN verbatim and the value carries 404 / ResourceNotFound / "Resource does not exist".)

## Steps to Reproduce
1. Build an SNS topic ARN in the caller's own account and region that names a topic which does not exist (e.g. arn:aws:sns:<region>:<account>:<unused-name>).
2. Call resourcegroupstaggingapi TagResources with ResourceARNList=[that absent topic ARN] and a valid Tags map (1..50 pairs, e.g. {"sim2real-probe-run": "probe"}).
3. Read the 200 response's FailedResourcesMap. Observe that it contains no entry for the requested ARN, instead of one entry keyed by that ARN with {"StatusCode": 404, "ErrorCode": "ResourceNotFound", "ErrorMessage": "Resource does not exist"}.
4. Replay fixture resource-groups-tagging-api-tag-resources-validation-error-paths, step task#5, to confirm.
