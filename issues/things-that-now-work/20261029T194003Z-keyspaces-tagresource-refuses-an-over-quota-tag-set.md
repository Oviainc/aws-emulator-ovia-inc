Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: keyspaces.tag-resource-quota-exceeded-rejected
Issue-Shape-Id: com.amazonaws.keyspaces#KeyspacesService
Issue-Key: keyspaces.tag-resource-quota-exceeded-rejected|run#9|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Commit: baf896d7efe27c876c690dc5952f8e38980dff3b

# keyspaces TagResource refuses an over-quota tag set with 402 ServiceQuotaExceededException instead of AWS's 400 ValidationException, and words the message itself

> _Note: this issue was identified against the code base at commit `baf896d7efe27c876c690dc5952f8e38980dff3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
One root cause (1 of 1).

AWS: Smithy (services/service-com-amazonaws-keyspaces-keyspacesservice/smithy-model/com-amazonaws-keyspaces-keyspacesservice.json) declares TagResource with BOTH ValidationException (errorKind client, httpError 400 — 'The operation failed due to an invalid or malformed request') and ServiceQuotaExceededException (errorKind client, httpError 402 — 'The operation exceeded the service quota for this resource'), and models TagResource.tags as TagList @length min 1 max 60, i.e. the per-REQUEST bound, which cannot see the tags the resource already holds. The 50-tags-per-resource limit is therefore judged in the handler. Which of the two modeled errors AWS picks for it is not stated by the model; recorded behavior only: a TagResource whose resulting tag set would exceed 50 user tags is refused with 400 ValidationException and message `Tag count exceed the imposed limit, The resultant tag set must not have more than 50 user tags.` That sentence is entirely FIXED text (note the service's own grammar, 'Tag count exceed'): it interpolates neither the resource ARN, nor the current tag count, nor the resulting count. Only the limit 50 is a value, and it is the service's published per-resource tag quota, not a per-request value.

Emulator: refuses the same call with 402 ServiceQuotaExceededException and an emulator-authored message of the form `the resource would hold <n> tags, which exceeds the limit of 50 tags per resource` — a different error class, a different HTTP status (402 vs 400), and a sentence that interpolates the resulting tag count AWS does not report.

Why: the error class is the contract a client branches on. ServiceQuotaExceededException reads as 'quota reached, request a quota increase / retry later' while ValidationException reads as 'this request is malformed, fix the request' — a client that catches ValidationException to trim its tag set instead sees an unhandled 402 and fails, and an SDK retry/throttling policy keyed on the 402 quota class can retry a call that will never succeed. The status code difference (402, which few HTTP stacks expect from a tagging call) compounds it. The message also diverges in full.

Fix: the TagResource handler's over-quota branch, and the shared tag-rejection classifier it delegates to — the classifier maps the per-resource-count rejection onto the quota error, and that mapping plus the sentence it renders are both wrong for this condition. The per-request/reserved-prefix classifications it also serves must keep their current classes; only the resulting-tag-count condition changes class to ValidationException and must carry AWS's fixed sentence. Note the handler's acceptance behavior is correct: no overflow tag landed and the in-bounds control tagging up to exactly 50 succeeded, so only the refusal's class/status/message need changing.

Traces: keyspaces.tag-resource-quota-exceeded-rejected. The classifier lives at services/service-com-amazonaws-keyspaces-keyspacesservice/src/state.rs:647-683 (`TagRejection`, `TagRejection::message`, `TagRejection::is_quota`, with MAX_TAGS_PER_RESOURCE at state.rs:628); the handler branch that consumes `is_quota()` to choose the exception is the same shape as src/ops/create_keyspace.rs:189-199.

## Evidence
keyspaces.tag-resource-quota-exceeded-rejected:
  AssertionError: expected ValidationException, got ServiceQuotaExceededException

Request sequence: CreateKeyspace, then TagResource with 48 tags (accepted), then TagResource with 3 further tags (resulting set would be 51).

Expected (AWS): HTTP 400, Error.Code `ValidationException`, Error.Message `Tag count exceed the imposed limit, The resultant tag set must not have more than 50 user tags.` — fixed text in full; `50` is the service's per-resource tag quota.
Actual (emulator): HTTP 402, Error.Code `ServiceQuotaExceededException`, Error.Message of the form `the resource would hold 51 tags, which exceeds the limit of 50 tags per resource` (state.rs:673), with the resulting count interpolated from request+state.

The check failed on the code assertion before reaching its message assertion, so the message divergence is read from the emulator's own rendering site rather than from the replay line. The effect assertions that follow (exactly the original 48 tags present, no `overflow*` key landed, and a subsequent 2-tag call reaching exactly 50 succeeding) are consistent with correct accept/reject behavior — only the refusal's class, status and sentence diverge.

## Steps to Reproduce
Against the emulator's keyspaces endpoint:
1. CreateKeyspace(keyspaceName=K) and take resourceArn from the response.
2. TagResource(resourceArn=ARN, tags=[{key:k0..k47, value:v}]) — 48 tags; succeeds.
3. TagResource(resourceArn=ARN, tags=[{key:overflow0..overflow2, value:v}]) — the resulting set would hold 51 user tags.
4. Inspect the refusal: expect HTTP 400 / ValidationException / `Tag count exceed the imposed limit, The resultant tag set must not have more than 50 user tags.`; the emulator answers HTTP 402 / ServiceQuotaExceededException with its own sentence.
5. ListTagsForResource(ARN) still shows exactly the 48 original keys (correct), and a follow-up TagResource of 2 more tags reaching exactly 50 succeeds (correct).

Replay keyspaces.tag-resource-quota-exceeded-rejected to confirm.
