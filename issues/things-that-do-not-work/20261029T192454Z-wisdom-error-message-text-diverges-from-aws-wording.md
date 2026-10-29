Issue-Source: trace-sweep-divergence
Issue-Trace: wisdom-quick-response-error-paths
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: wisdom-quick-response-error-paths wisdom-session-recommendations-query-workflow
Issue-Shape-Id: com.amazonaws.wisdom#WisdomService
Issue-Key: wisdom|GetQuickResponse|4xx->4xx|$.message
Issue-Key: wisdom|NotifyRecommendationsReceived|2xx->2xx|$.errors[*].message
Issue-Commit: 8d5a4f803a514b21e009b483e221bcf8b3c8087d

# Wisdom error message text diverges from AWS wording for GetQuickResponse and NotifyRecommendationsReceived

> _Note: this issue was identified against the code base at commit `8d5a4f803a514b21e009b483e221bcf8b3c8087d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQuickResponse

## Description
Two wisdom failure messages are worded by the emulator rather than taken from AWS. Status codes and error types match; only the message strings diverge, and both are pure fixed text with no request- or run-derived part. 2 causes, both in the wisdom service.

Cause 1 of 2: GetQuickResponse not-found message.
AWS: GetQuickResponse for a quickResponseId that does not exist in the knowledge base answers 404 ResourceNotFoundException (Smithy: GetQuickResponse errors -> ResourceNotFoundException, @error client, @httpError 404, doc 'The specified resource does not exist.') with body message exactly 'QuickResponse does not exist.' — the whole string is fixed text; the resource name does not appear in it, so nothing is substituted from the request.
Emulator: same 404 and same x-amzn-errortype, but message 'Quick Response does not exist.' (the resource token spelled as two words).
Why: clients and test suites that branch on, assert against, or surface the exception message see a string AWS never emits; message-matching error handling for a missing quick response fails against the emulator while the status and type look correct.
Fix: the error-construction site for ResourceNotFoundException in the wisdom quick-response handlers (the message literal), not the status mapping.
Traces: wisdom-quick-response-error-paths#task#6.

Cause 2 of 2: NotifyRecommendationsReceived partial-failure message.
AWS: NotifyRecommendationsReceived answers 200 and reports per-recommendation failures in the errors list; for a recommendationId that is not one of the session's recommendations the entry is NotifyRecommendationsReceivedError with recommendationId echoed from the request and message exactly 'Could not process recommendation' (Smithy: NotifyRecommendationsReceivedResponse.errors -> NotifyRecommendationsReceivedErrorList; NotifyRecommendationsReceivedError members recommendationId -> String 'The identifier of the recommendation that is in error', message -> NotifyRecommendationsReceivedErrorMessage 'A recommendation is causing an error'). The message is fixed text — AWS does not vary it per reason; only recommendationId comes from the request.
Emulator: answers 200 with the error entry present but message 'The recommendation is not an available recommendation of this session.'
Why: the partial-failure contract is message-based (one 200 carrying per-id errors), so a client that classifies entries by message cannot recognize a failed notification; it looks like an unknown error class.
Fix: the NotifyRecommendationsReceived handler's error-entry construction (the message literal).
Traces: wisdom-session-recommendations-query-workflow#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, score 65.9%).

Cause 1 — wisdom-quick-response-error-paths#task#6, GetQuickResponse, expected 404 / actual 404:
  $.message: expected 'QuickResponse does not exist.', got 'Quick Response does not exist.'
  (status and x-amzn-errortype matched; the only divergence is the fixed message text.)

Cause 2 — wisdom-session-recommendations-query-workflow#task#5, NotifyRecommendationsReceived, expected 200 / actual 200:
  $.errors[0].message: expected 'Could not process recommendation', got 'The recommendation is not an available recommendation of this session.'
  (the errors list and its recommendationId matched; only the fixed message text diverges.)

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: GET /knowledgeBases/{knowledgeBaseId}/quickResponses/{quickResponseId} with a quickResponseId that does not exist — the 404 body reads 'Quick Response does not exist.' instead of AWS's 'QuickResponse does not exist.'. Fixture wisdom-quick-response-error-paths step task#6.
3. Cause 2: POST /assistants/{assistantId}/sessions/{sessionId}/recommendations/notify with a recommendationId that is not one of the session's recommendations — the 200 body's errors[0].message reads 'The recommendation is not an available recommendation of this session.' instead of AWS's 'Could not process recommendation'. Fixture wisdom-session-recommendations-query-workflow step task#5.
