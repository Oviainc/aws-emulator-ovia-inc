Issue-Source: trace-sweep-divergence
Issue-Trace: chatbot-resource-not-found-error-paths
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chatbot-resource-not-found-error-paths
Issue-Shape-Id: com.amazonaws.chatbot#WheatleyOrchestration_20171011
Issue-Key: chatbot|UntagResource|4xx->4xx|$.Message
Issue-Key: chatbot|UntagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: chatbot|UntagResource|4xx->4xx|status
Issue-Commit: 3a705addac2ff66eb8110aaf4fdbff8de4e78738

# Chatbot UntagResource on a missing resource returns 403 UnauthorizedException instead of 404 ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `3a705addac2ff66eb8110aaf4fdbff8de4e78738`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UntagResource

## Description
One cause, in the chatbot service.

Cause 1 of 1: UntagResource addressed at a chatbot resource ARN that does not exist answers the wrong error class on all three parts of the error response (status, x-amzn-errortype, body message).

AWS: Smithy (services/service-com-amazonaws-chatbot-wheatleyorchestration-20171011): UntagResource is POST /untag-resource, success code 200, and its modeled errors are InternalServiceError (server, 500), ResourceNotFoundException (client, httpError 404, body member `Message`) and ServiceUnavailableException (client, 429). UnauthorizedException is NOT a modeled error of UntagResource. Required input ResourceARN is an AmazonResourceName (@length 1..1011, pattern ^arn:aws:(wheatley|chatbot):...). For an ARN that resolves to no resource, the recorded response is HTTP 404 with x-amzn-errortype ResourceNotFoundException and `Message` = `Resource <ResourceARN> does not exist!` — the fixed text is `Resource ` + ` does not exist!` and the single variable part is the ResourceARN taken verbatim from the request (the same template AWS uses for the sibling tagging operations ListTagsForResource and TagResource).

Emulator: HTTP 403 with x-amzn-errortype `UnauthorizedException` and no `Message` member in the body at all. The replayed step expected 404 and got 403; $.Message was missing; $headers.x-amzn-errortype was 'UnauthorizedException' where 'ResourceNotFoundException' was recorded.

Why: an SDK client maps 403/UnauthorizedException to a credentials or permissions failure, which is not retried and not branched on as "resource absent". Tag-reconciliation code that removes tags from resources it may have already deleted catches ResourceNotFoundException to treat the removal as a no-op; against this emulator it instead surfaces an authorization error and aborts the reconciliation, and credential-refresh/backoff paths keyed on auth errors spin pointlessly. Because the emulator also omits `Message`, a client that logs or matches the error text gets nothing.

Fix: the UntagResource handler's ARN-resolution path in the chatbot service crate (src/ops/) must raise the modeled ResourceNotFoundException rather than an unauthorized/refusal error, and the chatbot error serializer must emit the 404 status, the `ResourceNotFoundException` x-amzn-errortype header, and the `Message` member formatted as `Resource <ResourceARN> does not exist!` from the request's ResourceARN. This is the same defect family already filed for GetCustomAction, ListTagsForResource and TagResource in open issue 20261029T170844Z-chatbot-not-found-and-conflict-errors-use-wrong.md, which does NOT cover UntagResource (the replay reports this step as unowned), so UntagResource must be fixed and re-replayed on its own.

Traces: chatbot-resource-not-found-error-paths#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false).

Divergence cluster service=chatbot|UntagResource|status=404->403|keys=$.Message,$headers.x-amzn-errortype,status
  fixture: chatbot-resource-not-found-error-paths, step: task#6, operation: UntagResource, verdict: mismatch
  - status: expected 404, got 403
  - $.Message: missing (expected 'Resource <ResourceARN> does not exist!' with the request's ARN filling the variable part; the recording's instance used the non-existent custom-action ARN from the fixture)
  - $headers.x-amzn-errortype: value_mismatch (expected 'ResourceNotFoundException', got 'UnauthorizedException')
  affected_count: 1; owned_by: (none) — not covered by any open issue's replay key.

Model grounding: UntagResource errors = InternalServiceError(500) | ResourceNotFoundException(404, member `Message`) | ServiceUnavailableException(429); UnauthorizedException is not among them.

## Steps to Reproduce
1. Clone this repository and boot the emulator built from it; confirm /__admin/health answers 200.
2. Replay trace set simparity-wire-aws-v1, fixture chatbot-resource-not-found-error-paths.
3. Step task#6 sends POST /untag-resource with a ResourceARN for a chatbot custom action that was never created, plus a TagKeys list.
4. Observe the response is 403 with header x-amzn-errortype: UnauthorizedException and a body with no `Message` member.
5. Expected (recorded AWS): 404 with x-amzn-errortype: ResourceNotFoundException and body {"Message": "Resource <the ResourceARN sent in step 3> does not exist!"}.
