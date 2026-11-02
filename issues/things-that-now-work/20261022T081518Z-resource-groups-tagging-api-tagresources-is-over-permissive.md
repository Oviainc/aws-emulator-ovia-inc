Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__resourcegroupstaggingapi.tag-resources-nonexistent-cluster-rejected-per-resource events__resourcegroupstaggingapi.tag-resources-nonexistent-rule-rejected-per-resource kms__resourcegroupstaggingapi.tag-resources-nonexistent-key-rejected-per-resource resourcegroupstaggingapi.tag-resources-nonexistent-queue-rejected-per-resource resourcegroupstaggingapi__secretsmanager.tag-resources-nonexistent-secret-rejected-per-resource resourcegroupstaggingapi__stepfunctions.tag-resources-nonexistent-state-machine-rejected-per-resource resourcegroupstaggingapi.tag-resources-malformed-arn-rejected resourcegroupstaggingapi.tag-resources-cross-region-arn-rejected resourcegroupstaggingapi.tag-resources-system-tag-prefix-rejected resourcegroupstaggingapi.get-resources-invalid-pagination-token-rejected resourcegroupstaggingapi.get-resources-out-of-range-page-size-rejected resourcegroupstaggingapi.resource-type-filter-narrows-tag-query apigateway__lambda__logs__resourcegroupstaggingapi__sqs.sqs-queue-tag-write-reaches-native-tags-and-discovered-arn-usable ecs__resourcegroupstaggingapi.cluster-and-task-definition-tags-visible-through-get-resources resourcegroupstaggingapi__secretsmanager.secret-tags-visible-and-force-deleted-secret-leaves-the-index
Issue-Key: ecs__resourcegroupstaggingapi.tag-resources-nonexistent-cluster-rejected-per-resource|run#12|AssertionError|assert set(failed) == {ghost_arn}, (
Issue-Key: events__resourcegroupstaggingapi.tag-resources-nonexistent-rule-rejected-per-resource|run#13|AssertionError|assert set(failed) == {ghost_arn}, (
Issue-Key: kms__resourcegroupstaggingapi.tag-resources-nonexistent-key-rejected-per-resource|run#12|AssertionError|assert set(failed) == {ghost_arn}, (
Issue-Key: resourcegroupstaggingapi.tag-resources-nonexistent-queue-rejected-per-resource|run#13|AssertionError|assert set(failed) == {ghost_arn}, (
Issue-Key: resourcegroupstaggingapi__secretsmanager.tag-resources-nonexistent-secret-rejected-per-resource|run#12|AssertionError|assert set(failed) == {ghost_arn}, (
Issue-Key: resourcegroupstaggingapi__stepfunctions.tag-resources-nonexistent-state-machine-rejected-per-resource|run#10|AssertionError|assert set(failed) == {ghost_arn}, (
Issue-Key: resourcegroupstaggingapi.tag-resources-malformed-arn-rejected|_error_code_and_message#9|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: resourcegroupstaggingapi.tag-resources-cross-region-arn-rejected|_error#13|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: resourcegroupstaggingapi.tag-resources-system-tag-prefix-rejected|_error_code_and_message#9|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: resourcegroupstaggingapi.get-resources-invalid-pagination-token-rejected|run#19|AssertionError|assert code == "InvalidParameterException", (
Issue-Key: resourcegroupstaggingapi.get-resources-out-of-range-page-size-rejected|run#14|AssertionError|assert below_message == expected_message, (
Issue-Key: resourcegroupstaggingapi.resource-type-filter-narrows-tag-query|run#20|AssertionError|assert qualified == {q_arn}, (
Issue-Key: apigateway__lambda__logs__resourcegroupstaggingapi__sqs.sqs-queue-tag-write-reaches-native-tags-and-discovered-arn-usable|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: ecs__resourcegroupstaggingapi.cluster-and-task-definition-tags-visible-through-get-resources|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: resourcegroupstaggingapi__secretsmanager.secret-tags-visible-and-force-deleted-secret-leaves-the-index|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Commit: 5ef273b5ec187d9b55db2ccfdad00816d8c5c39a

# Resource Groups Tagging API: TagResources is over-permissive on absent/malformed/cross-region ARNs and system tags, and GetResources type-filter + pagination/range errors diverge (7 causes)

> _Note: this issue was identified against the code base at commit `5ef273b5ec187d9b55db2ccfdad00816d8c5c39a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResources

## Description
The emulator's resourcegroupstaggingapi crate answers every request, but seven distinct rules it implements diverge from AWS. All seven are in the same service and are filed as one bundle; a fixer must close ALL SEVEN before submitting.

=== Cause 1 of 7: TagResources accepts ARNs naming resources that do not exist, returning an empty FailedResourcesMap ===
AWS: TagResources is a 200-with-partial-failure operation. Smithy: TagResources -> TagResourcesOutput.FailedResourcesMap (map of ARN -> FailureInfo{StatusCode,ErrorCode,ErrorMessage}); errors InternalServiceException/InvalidParameterException/ThrottledException only. The FailureInfo doc states the per-resource ErrorCode "can also include any valid error code returned by the Amazon Web Services service that hosts the resource that the ARN key represents". So a batch of one live and one well-formed-but-absent ARN is a 200 in which the live ARN is tagged and the absent ARN appears in FailedResourcesMap carrying the OWNING service's own not-found identity. Recorded behavior across six services: absent SQS queue -> StatusCode 400, ErrorCode AWS.SimpleQueueService.NonExistentQueue (example message "The specified queue does not exist."); absent ECS cluster -> 400 ClusterNotFoundException (example "Cluster not found."); absent EventBridge rule -> 400 ResourceNotFoundException; absent KMS key -> 400 NotFoundException; absent Secrets Manager secret -> ResourceNotFoundException (example "Secrets Manager can't find the specified secret."); absent Step Functions state machine -> 400 ResourceNotFound (example "Resource not found: '<arn>'").
Emulator: every one of those six batches returns 200 with FailedResourcesMap == {}. The absent ARN is accepted exactly as the live one is; nothing in the response distinguishes them.
Why: a client that reads FailedResourcesMap to learn which resources it failed to tag is told it tagged everything. Typos, deleted resources and wrong-account ARNs are all reported as successful tag writes, and tooling that reconciles a desired tag set against the result will believe the estate is tagged when it is not.
Fix: the TagResources handler's per-resource resolution step — each ARN must be resolved against the owning service's state before the tag is written, and an unresolved ARN must be emitted as a FailedResourcesMap entry carrying that service's not-found code/status rather than being treated as a success.
Traces: ecs__resource-groups-tagging-api.tag-resources-nonexistent-cluster-rejected-per-resource, events__resource-groups-tagging-api.tag-resources-nonexistent-rule-rejected-per-resource, kms__resource-groups-tagging-api.tag-resources-nonexistent-key-rejected-per-resource, resource-groups-tagging-api.tag-resources-nonexistent-queue-rejected-per-resource, resource-groups-tagging-api__secretsmanager.tag-resources-nonexistent-secret-rejected-per-resource, resource-groups-tagging-api__sfn.tag-resources-nonexistent-state-machine-rejected-per-resource.

=== Cause 2 of 7: a malformed (non-ARN) entry is reported per-resource instead of refusing the whole call ===
AWS: a value in ResourceARNList that is not an ARN at all is a request-level parameter fault, not a per-resource one: the call fails with HTTP 400 InvalidParameterException (Smithy: TagResources errors include InvalidParameterException, httpError 400) and the request is atomic — no tag is written for any ARN in the batch, including the well-formed live one.
Emulator: returns HTTP 200 with the malformed value placed inside FailedResourcesMap: {'sim2real-behav-not-an-arn': {'StatusCode': 400, 'ErrorCode': 'InvalidParameterException', 'ErrorMessage': 'ResourceARNList contains a value that is not an Amazon Resource Name: ...'}}. It has the right diagnosis at the wrong level — a 200 envelope instead of a 400 refusal.
Why: a client using exceptions for request faults never raises; the batch partially applies, so a request AWS would reject wholesale leaves the other ARNs mutated. The two failure modes must stay distinguishable: well-formed-but-absent is a 200 with FailedResourcesMap (cause 1), malformed is a 400.
Fix: the TagResources input validation, which must run over the whole ResourceARNList before any state write and raise InvalidParameterException instead of synthesising a FailureInfo entry.
Traces: resource-groups-tagging-api.tag-resources-malformed-arn-rejected.

=== Cause 3 of 7: a cross-region ARN is accepted ===
AWS: TagResources is a regional endpoint and only tags resources in its own region; an ARN whose region segment names a different region is refused with HTTP 400 InvalidParameterException (Smithy: TagResources errors, InvalidParameterException httpError 400).
Emulator: the call succeeds — {'FailedResourcesMap': {}, HTTPStatusCode 200} — for a batch containing one in-region ARN and one ARN in a foreign region.
Why: callers that fan a tag write out per region rely on the wrong-region ARN being refused; silently accepting it means tags appear to be applied to resources in regions the endpoint cannot reach, and the emulator's index acquires entries that no region's endpoint should own.
Fix: the TagResources validation of the ARN's region segment against the serving region.
Traces: resource-groups-tagging-api.tag-resources-cross-region-arn-rejected.

=== Cause 4 of 7: a reserved aws:-prefixed tag key is accepted ===
AWS: keys in the reserved aws: namespace are system tags that a caller may not write; TagResources refuses the request with HTTP 400 InvalidParameterException (example message "The caller isn't allowed to modify system tags.").
Emulator: 200 with FailedResourcesMap {} — the reserved key is written.
Why: system tags are how AWS itself attributes resources (e.g. aws:cloudformation:*). A caller able to forge them through the emulator will pass tests that real AWS rejects, and emulated state ends up carrying system tags no real caller could have set.
Fix: the TagResources tag-key validation (reserved-prefix check) ahead of the state write.
Traces: resource-groups-tagging-api.tag-resources-system-tag-prefix-rejected.

=== Cause 5 of 7: an unminted PaginationToken is answered with the wrong error identity ===
AWS: a PaginationToken that this service never minted is a malformed parameter: GetResources (and GetTagValues) refuse it with InvalidParameterException (example message "Pagination token is invalid=<token>"). PaginationTokenExpiredException is a DIFFERENT, separately modeled error reserved for a token that WAS minted and has aged out — Smithy: PaginationTokenExpiredException doc, "the specified PaginationToken has expired. A PaginationToken is valid for a maximum of 15 minutes"; both are bound to GetResources with httpError 400.
Emulator: raises PaginationTokenExpiredException for a token that was never minted.
Why: the two codes drive opposite client behavior. PaginationTokenExpiredException tells a client to restart the scan because its cursor aged out and is retryable; InvalidParameterException tells it the token is garbage and the call will never succeed. A client that retries on expiry will loop forever against a token the emulator calls expired.
Fix: the GetResources/GetTagValues pagination-token decoder, which must distinguish "not a token I issued" from "a token I issued that is past its window".
Traces: resource-groups-tagging-api.get-resources-invalid-pagination-token-rejected.

=== Cause 6 of 7: the ResourcesPerPage range-violation message is built by the wrong rule ===
AWS: ResourcesPerPage has a documented window of 1..100 that is a fixed API constant, not an account quota (Smithy: GetResourcesInput.ResourcesPerPage doc, "You can specify a minimum of 1 and a maximum value of 100"; the member carries no @range, so the check is the handler's). A value outside it is refused with InvalidParameterException and a fixed text naming the member and that window — "ResourcesPerPage is not in the valid range of 1 and 100" — identically for 0 and for 101; the submitted value is not echoed. The rule is therefore known: member name + documented constant window, value-independent.
Emulator: the error code and status are correct, but the text is built by a different rule — "ResourcesPerPage must be between 1 and 100; got 0" — rephrasing the constraint and appending the offending value.
Why: this is the only divergence in the check that is message-only; it is filed because the producing rule is known rather than incidental. Callers and fixtures that match on the service's fixed range text (and recorded-response comparisons) see a mismatch for every out-of-range page size.
Fix: the GetResources validation of ResourcesPerPage — emit the service's fixed range text for the member rather than a generated one that echoes the request value.
Traces: resource-groups-tagging-api.get-resources-out-of-range-page-size-rejected.

=== Cause 7 of 7: GetResources ResourceTypeFilters matches nothing in the qualified service:resourceType form ===
AWS: Smithy: GetResourcesInput.ResourceTypeFilters doc — "The format of each resource type is service[:resourceType]. For example, specifying a service of ec2 returns all Amazon EC2 resources ... Specifying a resource type of ec2:instance returns only EC2 instances", and "The string for each service name and resource type is the same as that embedded in a resource's Amazon Resource Name (ARN)". So the bare token and the qualified token are both valid, the qualified one narrowing to that one type, and several filters are a union intersected with TagFilters.
Emulator: the bare form works — in the filter check, ResourceTypeFilters=['sqs'], ['sns'] and ['sqs','sns'] all returned the right resources — but the qualified form matches nothing: ResourceTypeFilters=['sqs:queue'] returned [] for a queue the unfiltered tag query had already returned in the same check. Three discovery checks that poll GetResources with only qualified filters (['sqs:queue'], ['ecs:cluster','ecs:task-definition'], ['secretsmanager:secret']) consequently never settle and are graded as timeouts.
Why: the qualified form is the form AWS's own documentation and every scripted tagging query uses (e.g. --resource-type-filters lambda:function). A caller that narrows by resource type sees an empty estate and concludes nothing is tagged, while the identical query with a bare service token returns rows — so discovery through the tagging API is unusable for any type-scoped query.
Fix: the GetResources resource-type-filter matcher — parse each filter as service[:resourceType] and compare both segments against the type embedded in the indexed ARN, instead of comparing the whole filter string to the service token.
Traces: resource-groups-tagging-api.resource-type-filter-narrows-tag-query (hard assertion), plus the three polls that never settle: apigateway__lambda__logs__resource-groups-tagging-api__sqs.sqs-queue-tag-write-reaches-native-tags-and-discovered-arn-usable, ecs__resource-groups-tagging-api.cluster-and-task-definition-tags-visible-through-get-resources, resource-groups-tagging-api__secretsmanager.secret-tags-visible-and-force-deleted-secret-leaves-the-index.

## Evidence
Cause 1 (6 checks, identical shape): AssertionError: FailedResourcesMap named [], expected exactly the absent cluster 'arn:aws:ecs:us-west-2:000000000000:cluster/sim2real-behav-ghost-a26afe4ff256' -- the live cluster must not fail and the absent one must not pass. Same for the absent rule 'arn:aws:events:...:rule/sim2real-behav-ghost-...', the absent key 'arn:aws:kms:...:key/4e8ec784-...', the absent queue 'arn:aws:sqs:...:sim2real-behav-ghost-...', the absent secret 'arn:aws:secretsmanager:...:secret:sim2real-behav-ghost-...-AbCdEf' and the absent state machine 'arn:aws:states:...:stateMachine:sim2real-behav-ghost-...'. Expected per-resource details: (400, AWS.SimpleQueueService.NonExistentQueue), (400, ClusterNotFoundException), (400, ResourceNotFoundException), (400, NotFoundException), (ResourceNotFoundException), (400, ResourceNotFound).

Cause 2: AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'FailedResourcesMap': {'sim2real-behav-not-an-arn': {'StatusCode': 400, 'ErrorCode': 'InvalidParameterException', 'ErrorMessage': 'ResourceARNList contains a value that is not an Amazon Resource Name: sim2real-behav-not-an-arn'}}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}} — expected a 400 InvalidParameterException for the whole call.

Cause 3: AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'FailedResourcesMap': {}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}} for TagResources(ResourceARNList=[in-region arn, foreign-region arn]).

Cause 4: AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'FailedResourcesMap': {}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}} for Tags={'aws:<reserved>': 'v'}; expected 400 InvalidParameterException (example text "The caller isn't allowed to modify system tags.").

Cause 5: AssertionError: an unminted PaginationToken raised 'PaginationTokenExpiredException', expected InvalidParameterException.

Cause 6: AssertionError: ResourcesPerPage=0 was refused with 'ResourcesPerPage must be between 1 and 100; got 0', expected 'ResourcesPerPage is not in the valid range of 1 and 100' (code InvalidParameterException matched).

Cause 7: AssertionError: ResourceTypeFilters=['sqs:queue'] returned [], expected exactly 'arn:aws:sqs:us-west-2:000000000000:sim2real-behav-q-5aa2ee601be6'. The same check's earlier assertions on ResourceTypeFilters=['sqs'], ['sns'] and ['sqs','sns'] passed, and its unfiltered TagFilters-only positive control returned both resources. Three further checks that poll only with qualified filters were graded "Check timed out after 60.0s" while GetResources kept returning [].

## Steps to Reproduce
1. Boot the emulator and point a botocore resourcegroupstaggingapi client at it (region us-west-2).
2. Cause 1: create a live SQS queue (or ECS cluster / EventBridge rule / KMS key / Secrets Manager secret / Step Functions state machine), then call TagResources(ResourceARNList=[live_arn, well_formed_but_never_created_arn], Tags={'k':'partial'}). Observe FailedResourcesMap == {}; AWS names the absent ARN with the owning service's not-found code and StatusCode 400.
3. Cause 2: TagResources(ResourceARNList=[live_arn, 'not-an-arn'], Tags={'k':'v'}) -> 200 with a FailedResourcesMap entry; AWS raises 400 InvalidParameterException and writes nothing.
4. Cause 3: TagResources(ResourceARNList=[live_arn, 'arn:aws:sqs:<other-region>:<acct>:q'], Tags={'k':'v'}) -> 200 FailedResourcesMap {}; AWS raises 400 InvalidParameterException.
5. Cause 4: TagResources(ResourceARNList=[live_arn], Tags={'aws:something':'v'}) -> 200; AWS raises 400 InvalidParameterException.
6. Cause 5: GetResources(TagFilters=[{'Key':k}], PaginationToken='<never minted>') -> PaginationTokenExpiredException; AWS raises InvalidParameterException. Same for GetTagValues.
7. Cause 6: GetResources(TagFilters=[{'Key':k}], ResourcesPerPage=0) and =101 -> InvalidParameterException with 'ResourcesPerPage must be between 1 and 100; got 0'; AWS uses the fixed text 'ResourcesPerPage is not in the valid range of 1 and 100' for both.
8. Cause 7: tag an SQS queue and an SNS topic with the same key, then GetResources(TagFilters=[{'Key':k}], ResourceTypeFilters=['sqs']) -> the queue (correct), and ResourceTypeFilters=['sqs:queue'] -> [] (wrong). Repeat with ['ecs:cluster'] / ['secretsmanager:secret'] against natively tagged resources of those types.
