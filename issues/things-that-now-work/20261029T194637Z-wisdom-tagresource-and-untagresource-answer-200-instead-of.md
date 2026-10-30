Issue-Source: trace-sweep-divergence
Issue-Trace: qconnect-assistant-lifecycle-and-tags
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: qconnect-assistant-lifecycle-and-tags
Issue-Shape-Id: com.amazonaws.qconnect#WisdomService
Issue-Key: wisdom|TagResource|2xx->2xx|status
Issue-Key: wisdom|UntagResource|2xx->2xx|status
Issue-Commit: 5b71589754f95f05f88c1a0ee9ccd691bbc5f701

# Wisdom TagResource and UntagResource answer 200 instead of 204

> _Note: this issue was identified against the code base at commit `5b71589754f95f05f88c1a0ee9ccd691bbc5f701`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
The wisdom (Amazon Q in Connect) tagging operations return the wrong HTTP success status for their empty response: the emulator answers 200 where the service answers 204 No Content. 2 causes, both in the wisdom service, both the same missing empty-response status mapping in the REST-JSON success path.

Cause 1 of 2: TagResource responds 200 instead of 204.
AWS: TagResource is a @idempotent POST /tags/{resourceArn} whose output structure TagResourceResponse has NO members, and the recorded behavior for a successful tag application is HTTP 204 with an empty body (no JSON object at all). Smithy states no explicit @http code for the operation — the model's code is the protocol default 200 (codeDefaulted) — so the 204 is recorded behavior only; its modeled errors are ResourceNotFoundException (404) and TooManyTagsException (400), neither of which is involved here. The requirement is the status alone: 204, empty body, no error code and no message.
Emulator: answers HTTP 200 for the same successful request. The body is otherwise acceptable (the response carries no members); only the status line diverges.
Why: a client or SDK middleware that distinguishes 204 No Content from 200 (treating 200 as 'a document follows') tries to deserialize a body that is not there, and conformance tests or proxies that assert the no-content status on the tagging path report the emulator as a different API than AWS.
Fix: the success-status mapping for TagResource in the wisdom/qconnect operation handler's response construction (the REST-JSON serializer path that chooses the status for an empty output shape) — not validation and not state; the tag write itself succeeds.
Traces: qconnect-assistant-lifecycle-and-tags#task#3.

Cause 2 of 2: UntagResource responds 200 instead of 204.
AWS: UntagResource is an @idempotent DELETE /tags/{resourceArn} with tagKeys bound to the query string (@length 1..50) and an output structure UntagResourceResponse with NO members; a successful untag is recorded as HTTP 204 with an empty body. Smithy again gives no explicit @http code (defaulted 200), so the 204 is recorded behavior only; the modeled error is ResourceNotFoundException (404), not involved here.
Emulator: answers HTTP 200 for the same successful request.
Why: same as cause 1 — the no-content contract of the tag-removal call is not reproduced, so clients that branch on 204 take the wrong path.
Fix: same success-status mapping, applied to the UntagResource handler's response construction.
Traces: qconnect-assistant-lifecycle-and-tags#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, score 35.1%).

Cause 1 — qconnect-assistant-lifecycle-and-tags#task#3, TagResource, verdict mismatch, detail 'status: expected 204, got 200'. Expected status 204 (empty body); actual status 200. No error code and no error message are involved on either side — the divergence is the success status only.

Cause 2 — qconnect-assistant-lifecycle-and-tags#task#5, UntagResource, verdict mismatch, detail 'status: expected 204, got 200'. Expected status 204 (empty body); actual status 200.

Both operations' output shapes are empty in the Smithy model (TagResourceResponse / UntagResourceResponse have no members) and the model carries no explicit @http code, so the 204 is taken from the recorded responses.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: create an assistant, then POST /tags/{assistantArn} (TagResource) with a tag map and inspect the status line — the emulator returns 200 where AWS returns 204 with an empty body. Fixture qconnect-assistant-lifecycle-and-tags step task#3.
3. Cause 2: DELETE /tags/{assistantArn}?tagKeys=<key> (UntagResource) against the same resource and inspect the status line — the emulator returns 200 instead of 204. Fixture qconnect-assistant-lifecycle-and-tags step task#5.
