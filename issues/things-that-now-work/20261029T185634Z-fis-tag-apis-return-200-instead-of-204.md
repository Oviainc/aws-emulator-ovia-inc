Issue-Source: trace-sweep-divergence
Issue-Trace: fis-tag-experiment-template-lifecycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: fis-tag-experiment-template-lifecycle fis-tag-multiple-templates-isolation fis-tag-resource-without-initial-tags fis-untag-all-tags-then-verify-empty fis-list-tags-for-nonexistent-arn fis-tag-operations-resource-not-found fis-tag-operations-on-deleted-template
Issue-Shape-Id: com.amazonaws.fis#FaultInjectionSimulator
Issue-Key: fis|TagResource|2xx->2xx|status
Issue-Key: fis|UntagResource|2xx->2xx|status
Issue-Key: fis|ListTagsForResource|4xx->4xx|$.message
Issue-Key: fis|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: fis|ListTagsForResource|4xx->4xx|status
Issue-Key: fis|UntagResource|4xx->4xx|$.message
Issue-Key: fis|UntagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: fis|UntagResource|4xx->4xx|status
Issue-Key: fis|TagResource|4xx->4xx|$.message
Issue-Key: fis|TagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: fis|TagResource|4xx->4xx|status
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# FIS tag APIs return 200 instead of 204 and misclassify invalid-ARN and deleted-resource failures

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource, UntagResource, ListTagsForResource

## Description
The FIS tagging surface diverges on the response envelope rather than on tag state: successful mutations answer with the wrong HTTP status, and every failing tag call is funnelled through one ResourceNotFoundException instead of the two distinct outcomes FIS has (a 400 BadRequestException for an ARN the service will not accept, and a 404 whose message is 'Resource was not found' for a well-formed ARN naming a resource that is gone). 7 causes.

Cause 1 of 7 (TagResource returns 200, AWS returns 204).
AWS: Smithy: TagResource POST /tags/{resourceArn}, output shape TagResourceResponse with NO members (the model's default success code is 200, but the recorded wire response is 204 No Content, and the empty output shape is consistent with a bodyless 204). A successful tag therefore completes with status 204.
Emulator: answers 200 (recorded: status expected 204, got 200).
Why: a client that branches on the exact status — or an SDK/middleware layer that treats 200 as carrying a body and attempts to parse one — sees a different envelope than the service produces; strict status assertions fail even though the tags were stored.
Fix: the TagResource handler's success response code.
Traces: fis-tag-experiment-template-lifecycle#task#4 (also fis-tag-multiple-templates-isolation#task#4, #task#5, fis-tag-resource-without-initial-tags#task#4).

Cause 2 of 7 (UntagResource returns 200, AWS returns 204).
AWS: Smithy: UntagResource DELETE /tags/{resourceArn}, output shape UntagResourceResponse with NO members; the recorded success is 204 No Content.
Emulator: answers 200 (recorded: status expected 204, got 200).
Why: same as cause 1, on the untag path.
Fix: the UntagResource handler's success response code.
Traces: fis-tag-experiment-template-lifecycle#task#6 (also fis-tag-multiple-templates-isolation#task#8, fis-untag-all-tags-then-verify-empty#task#4, #task#6).

Cause 3 of 7 (ListTagsForResource on an ARN FIS rejects: must be 400 BadRequestException 'Invalid input resource arn').
AWS: Smithy: ListTagsForResource GET /tags/{resourceArn}, resourceArn @length 20..2048 @pattern ^[\S]+$; the operation models NO errors, so this is recorded behavior only: FIS answers 400 with x-amzn-errortype BadRequestException and the message 'Invalid input resource arn' — entirely fixed text, with no part taken from the request.
Emulator: answers 404 with x-amzn-errortype ResourceNotFoundException and the message 'ExperimentTemplate not found.' (recorded: status expected 400, got 404).
Why: a client distinguishing a malformed/unacceptable ARN (a caller bug, not retryable, 4xx-bad-request) from a missing resource routes the failure to the wrong branch, and error-code-based handling never sees BadRequestException at all.
Fix: resourceArn validation in the tagging handlers, ahead of the resource lookup, plus the error mapping that turns it into BadRequestException/400.
Traces: fis-list-tags-for-nonexistent-arn#task#1 (also fis-tag-operations-resource-not-found#task#2).

Cause 4 of 7 (UntagResource on an ARN FIS rejects: same 400 BadRequestException).
AWS: recorded behavior only (UntagResource models no errors): 400, x-amzn-errortype BadRequestException, message 'Invalid input resource arn' (fixed text).
Emulator: 404 ResourceNotFoundException 'ExperimentTemplate not found.' (recorded: status expected 400, got 404).
Why: as cause 3, on the untag path — an un-retryable caller error is reported as a missing resource.
Fix: resourceArn validation in the UntagResource handler, before the lookup.
Traces: fis-list-tags-for-nonexistent-arn#task#2 (also fis-tag-operations-resource-not-found#task#3).

Cause 5 of 7 (TagResource on an ARN FIS rejects: same 400 BadRequestException).
AWS: recorded behavior only (TagResource models no errors): 400, x-amzn-errortype BadRequestException, message 'Invalid input resource arn' (fixed text).
Emulator: 404 ResourceNotFoundException 'ExperimentTemplate not found.' (recorded: status expected 400, got 404).
Why: as cause 3, on the tag path.
Fix: resourceArn validation in the TagResource handler, before the lookup.
Traces: fis-tag-operations-resource-not-found#task#1.

Cause 6 of 7 (TagResource on a deleted resource: 404 message must be 'Resource was not found').
AWS: recorded behavior only: for a well-formed FIS ARN whose resource no longer exists, FIS answers 404 with the message 'Resource was not found' — fixed text that names no resource type and interpolates nothing from the request (the recording does not assert the errortype header for this case).
Emulator: 404 with the message 'ExperimentTemplate not found.' and errortype ResourceNotFoundException (recorded: $.message value_mismatch; $headers.x-amzn-errortype shape_mismatch, expected not asserted, got 'ResourceNotFoundException').
Why: a client that matches on the service's message text, or surfaces it to an operator, sees wording FIS does not produce for tag operations — and a resource-type-specific message where the service deliberately emits a generic one.
Fix: the not-found message emitted by the tagging handlers' resource lookup.
Traces: fis-tag-operations-on-deleted-template#task#4.

Cause 7 of 7 (ListTagsForResource on a deleted resource: same 404 'Resource was not found').
AWS: recorded behavior only: 404, message 'Resource was not found' (fixed text).
Emulator: 404, message 'ExperimentTemplate not found.' with errortype ResourceNotFoundException (recorded: $.message value_mismatch; $headers.x-amzn-errortype shape_mismatch).
Why: as cause 6, on the read path.
Fix: the not-found message emitted by the ListTagsForResource handler's resource lookup.
Traces: fis-tag-operations-on-deleted-template#task#5.

## Evidence
Cause 1 — fis-tag-experiment-template-lifecycle#task#4, TagResource: status expected 204, got 200 (4 steps affected).
Cause 2 — fis-tag-experiment-template-lifecycle#task#6, UntagResource: status expected 204, got 200 (4 steps affected).
Cause 3 — fis-list-tags-for-nonexistent-arn#task#1, ListTagsForResource: status expected 400, got 404; $.message expected 'Invalid input resource arn', got 'ExperimentTemplate not found.'; $headers.x-amzn-errortype expected 'BadRequestException', got 'ResourceNotFoundException' (2 steps affected).
Cause 4 — fis-list-tags-for-nonexistent-arn#task#2, UntagResource: identical triple (status 400->404, 'Invalid input resource arn' -> 'ExperimentTemplate not found.', BadRequestException -> ResourceNotFoundException) (2 steps affected).
Cause 5 — fis-tag-operations-resource-not-found#task#1, TagResource: identical triple.
Cause 6 — fis-tag-operations-on-deleted-template#task#4, TagResource 404/404: $.message expected 'Resource was not found', got 'ExperimentTemplate not found.'; x-amzn-errortype shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException').
Cause 7 — fis-tag-operations-on-deleted-template#task#5, ListTagsForResource 404/404: same message divergence.
Smithy cited: TagResource POST /tags/{resourceArn} output TagResourceResponse {} ; UntagResource DELETE /tags/{resourceArn} output UntagResourceResponse {} ; ListTagsForResource GET /tags/{resourceArn}; resourceArn -> ResourceArn @required @httpLabel @length 20..2048 @pattern ^[\S]+$; all three model NO errors (so BadRequestException and the 404 wording are recorded behavior only).

## Steps to Reproduce
1. Replay the wire corpus against the emulator (replay_wire_corpus).
2. Causes 1-2: create an experiment template, POST /tags/{arn} with a tag, then DELETE /tags/{arn}?tagKeys=... — both answer 200 where 204 is expected (fis-tag-experiment-template-lifecycle#task#4 and #task#6).
3. Causes 3-5: call GET, DELETE and POST on /tags/{arn} with an ARN FIS will not accept — each answers 404/ResourceNotFoundException/'ExperimentTemplate not found.' instead of 400/BadRequestException/'Invalid input resource arn' (fis-list-tags-for-nonexistent-arn#task#1, #task#2, fis-tag-operations-resource-not-found#task#1).
4. Causes 6-7: create a template, delete it, then POST /tags/{arn} and GET /tags/{arn} with its still-well-formed ARN — the 404 message is 'ExperimentTemplate not found.' instead of 'Resource was not found' (fis-tag-operations-on-deleted-template#task#4 and #task#5).
