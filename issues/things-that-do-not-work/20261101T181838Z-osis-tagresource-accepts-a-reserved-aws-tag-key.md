Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: osis.tag-resource-reserved-prefix-rejected
Issue-Shape-Id: com.amazonaws.osis#AmazonOpenSearchIngestionService
Issue-Key: osis.tag-resource-reserved-prefix-rejected|_client_error#8|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Commit: 64ba80be2f956a355eead737f51c939a3b404043

# osis TagResource accepts a reserved `aws:` tag key with HTTP 200 instead of refusing it with ValidationException

> _Note: this issue was identified against the code base at commit `64ba80be2f956a355eead737f51c939a3b404043`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
One net-new root cause in the osis service.

Cause 1 of 1: TagResource performs no tag-KEY validation, so a tag whose key uses the reserved `aws:` prefix is accepted and attached instead of being refused.
Failing check: osis.tag-resource-reserved-prefix-rejected.

AWS: Smithy TagResource (POST /2022-01-01/osis/tagResource, success code 200) takes Arn (@required, httpQuery `arn`, target PipelineArn @length 46..76, pattern ^arn:(aws|aws\-cn|aws\-us\-gov|aws\-iso|aws\-iso\-b):osis:.+:pipeline\/.+$) and Tags (@required TagList), and models ValidationException (errorKind client, httpError 400, member `message`) — documented as "An exception for missing or invalid input fields" — alongside ResourceNotFoundException(404), LimitExceededException(409), DisabledOperationException(409), AccessDeniedException(403), InternalException(500). The model states no tag-key charset rule, so the reserved-namespace rule itself is recorded behavior only: a tag key in the reserved `aws:` namespace is refused 400 ValidationException with the message `Invalid System Tag: <tagKey> is missing Vendor Id.` — the FIXED text is `Invalid System Tag: ` ... ` is missing Vendor Id.`, and `<tagKey>` is the offending Key copied verbatim from the request's Tags entry (in the replayed check it is `aws:reserved`; that literal is a request value filling the template, not part of it). The refusal is atomic: no part of the call is applied, so a following ListTagsForResource does not show the rejected key, while a normal-key tag on the same pipeline is accepted 200 and IS visible via ListTagsForResource.

Emulator: the call is accepted — HTTP 200 with an empty JSON body (`content-length: 2`) and no error at all — and the reserved key is merged into the pipeline's tag set. services/service-com-amazonaws-osis-amazonopensearchingestionservice/src/ops/tag_resource.rs validates only the tag-list size (MAX_TAGS_PER_PIPELINE, before and after the merge) and the ARN's existence (404 `The pipeline could not be found`); there is no tag-key inspection whatsoever between the pipeline lookup and `crate::state::merge_tags`, and the handler constructs its ValidationException variant nowhere.

Why: a client relying on AWS's behavior cannot distinguish a tag it is allowed to set from one the service reserves. IaC and tag-reconciliation tooling that probes the reserved namespace (or that accidentally submits an `aws:`-prefixed key, e.g. by echoing back system tags read from another resource) gets a success here and a permanent 400 ValidationException against real AWS, so the error path is never exercised and the emulator's pipeline state ends up carrying a system tag AWS would never let a caller create — which then leaks into later GetPipeline/ListTagsForResource reads and into any tag-driven logic (cost allocation, resource-group membership) built on them.

Fix: the TagResource handler's input validation (services/service-com-amazonaws-osis-amazonopensearchingestionservice/src/ops/tag_resource.rs) — reject any submitted Tags entry whose Key is in the reserved `aws:` namespace with 400 ValidationException carrying AWS's wording, doing so BEFORE any merge into the stored record so nothing is partially applied; the same rule belongs on the sibling tag-write paths of this crate (tag-on-create and UntagResource) so one bad key is refused the same way everywhere.

Traces: osis.tag-resource-reserved-prefix-rejected.

Related filed issues (neither covers this cause): 20261029T191455Z-osis-refuses-implemented-path-calls-with-notimplemented-putresourcepolicy.md (still open) describes this check's EARLIER symptom — TagResource answering 501 NotImplemented — in its Cause 2; the handler has since been implemented (the fixed issue 20261029T191455Z-osis-tagresource-stub.md stood it up) and now answers 200, so the surviving divergence is the missing reserved-prefix guard described here, not a NotImplemented refusal. This is NOT a reopen of a refusal: the call now succeeds where AWS refuses.

## Evidence
run_behavioral_suite (suite simparity-behavioral-aws-v1): total 9/21 passed (42.9%), 6 mismatched, 6 errored, 0 skipped, bootstrap_signal false — the emulator is up and serving osis. The check below is verdict=mismatch and is marked neither "ALREADY FILED" nor "OWNED by" in the report.

[mismatch] osis.tag-resource-reserved-prefix-rejected (GROUP overly-permissive; tags axis:rejection, concern:negative-classes, concern:representation, plane:control):
  File "check_osis_tag_resource_reserved_prefix_rejected.py", line 29, in run
    error = _client_error(
  File "check_osis_tag_resource_reserved_prefix_rejected.py", line 57, in _client_error
    raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
  AssertionError: expected the call to be rejected, but it succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '2', ...}, 'RetryAttempts': 0}}

The refused call was osis.tag_resource(Arn=<the pipeline's PipelineArn>, Tags=[{"Key": "aws:reserved", "Value": "nope"}]).

Expected (recorded AWS behavior, asserted by the check):
  Error.Code    = "ValidationException"  (HTTP 400)
  Error.Message = "Invalid System Tag: aws:reserved is missing Vendor Id."
                  fixed text: "Invalid System Tag: " + "<Key>" + " is missing Vendor Id.";
                  variable part: the offending tag Key from the request (here "aws:reserved").
  then ListTagsForResource(Arn=<pipeline arn>) must NOT contain key "aws:reserved",
  and a subsequent tag_resource with Tags=[{"Key":"sim2real-normal","Value":"ok"}] must be
  accepted and visible as sim2real-normal=ok in ListTagsForResource.
Actual: HTTP 200, empty body, no error; the reserved key is merged into the pipeline's tags.

Code grounding: services/service-com-amazonaws-osis-amazonopensearchingestionservice/src/ops/tag_resource.rs handle() lines 142-182 — the only guards are `input.tags.len() > crate::state::MAX_TAGS_PER_PIPELINE` (line 154), the ARN lookup `store.pipeline_by_arn_mut(&input.arn).ok_or_else(not_found)` (lines 164-166) and the post-merge ceiling re-check (line 174); `crate::state::merge_tags(&mut tags, input.tags)` (line 173) upserts every submitted key unconditionally. The file's helper set is not_found()/limit_exceeded()/store_unavailable() only — no ValidationException constructor exists even though the variant is declared at line 92.

Model grounding (smithy_query on services/service-com-amazonaws-osis-amazonopensearchingestionservice/smithy-model/com-amazonaws-osis-amazonopensearchingestionservice.json): TagResource POST /2022-01-01/osis/tagResource code 200, in {Arn httpQuery arn @required PipelineArn len 46..76, Tags @required TagList}, out {} (no members), errors ValidationException(400)/ResourceNotFoundException(404)/LimitExceededException(409)/DisabledOperationException(409)/AccessDeniedException(403)/InternalException(500); ListTagsForResource GET /2022-01-01/osis/listTagsForResource out {Tags}.

## Steps to Reproduce
Boot the emulator from this checkout and replay the osis behavioral checks of suite simparity-behavioral-aws-v1 (run_behavioral_suite), or by hand:

1. CreatePipeline with any valid configuration body (the check uses an http source on path /ingest and an s3 sink) and wait for the pipeline to reach ACTIVE.
2. Read its ARN: pipeline_arn = GetPipeline(PipelineName=<name>)["Pipeline"]["PipelineArn"].
3. Call TagResource(Arn=pipeline_arn, Tags=[{"Key": "aws:reserved", "Value": "nope"}]).
   Expect HTTP 400 ValidationException with message "Invalid System Tag: aws:reserved is missing Vendor Id." (fixed text around the request's own tag Key).
   Observed: HTTP 200 with an empty body and no error.
4. Call ListTagsForResource(Arn=pipeline_arn). Expect the key "aws:reserved" to be ABSENT (the rejected call must not mutate state).
5. Call TagResource(Arn=pipeline_arn, Tags=[{"Key": "sim2real-normal", "Value": "ok"}]) and expect ListTagsForResource to report sim2real-normal=ok — a normal key on the same pipeline must still be accepted.

Re-replay check osis.tag-resource-reserved-prefix-rejected to confirm the cause is closed.
