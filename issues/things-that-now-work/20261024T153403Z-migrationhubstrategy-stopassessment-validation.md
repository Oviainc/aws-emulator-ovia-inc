Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# A malformed assessmentId is answered 404 ResourceNotFoundException instead of the modeled 400 ValidationException

_Filed: 2026-10-24T15:34:03Z_
_Operation: StopAssessment | Class: validation_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StopAssessment's assessmentId targets AsyncTaskId, which carries @length(0,52) and @pattern [0-9a-z-:]+ . Pattern constraints are not enforced anywhere in this crate, so a value that cannot be an assessment id at all (uppercase letters, spaces, '!' and other characters outside the catalog) reaches the handler, which simply fails to find it in the store and answers HTTP 404 ResourceNotFoundException — an error shape this operation's own error list does not even declare (it is widened in via extra_errors for the legitimate well-formed-but-unknown id case). Expected: a value outside the member's @pattern is refused at the boundary with the modeled HTTP 400 ValidationException ('The request body isn't valid.') before any lookup, exactly as the emulator already does for a length violation on a target-shape constraint. Actual: HTTP 404 ResourceNotFoundException, so a client cannot distinguish 'you sent an impossible id' from 'that assessment does not exist'.

Same root cause, same divergence, on this service's other id-bearing members: GET /get-recommendation-report-details/{id} and GET /get-import-file-task/{id} (patterns [0-9a-z-:]+ and .*\S.*) and serverId on GET /get-server-details/{serverId}, GET /get-server-strategies/{serverId} and POST /update-server-config (pattern .*\S.*) each answer 404 for a value whose spelling the model forbids. (GetAssessment happens to coincide with AWS here because its unknown-id answer is already 400.)

## Evidence

Spec: smithy_query(query="operation", operation="StopAssessment") — input assessmentId {target AsyncTaskId, required, length 0..52, pattern [0-9a-z-:]+}; errors include ValidationException (httpError 400) but NOT ResourceNotFoundException.
Handler: src/ops/stop_assessment.rs:161-174 goes straight to store.settled_assessment_mut(&input.assessment_id, now) and maps the miss to ResourceNotFoundException (widened in at lines 85-88). No pattern check exists in the handler or the generated schema.
Test coverage: tests/all/assessment_error_paths.rs:37-60 pins only 'a legal, never-issued' id ('the id the recorded request used: a legal, never-issued one') -> StopAssessment 404 / GetAssessment 400; nothing exercises a value outside the @pattern. tests/all/refusal_messages.rs:103-118 records that '@pattern is not enforced at all' in this service's generated schema, so the malformed case is unvalidated by construction.

## Steps to Reproduce

1. POST /stop-assessment with body {"assessmentId": "ASSESSMENT!!"} — uppercase and '!' are outside AsyncTaskId's @pattern [0-9a-z-:]+. Expected HTTP 400 ValidationException; emulator answers HTTP 404 ResourceNotFoundException.
2. POST /stop-assessment with {"assessmentId": "has spaces"} — same expectation, same 404.
3. GET /get-recommendation-report-details/ASSESSMENT!! and GET /get-import-file-task/%20 — expected HTTP 400 ValidationException for the pattern violation; emulator answers HTTP 404 ResourceNotFoundException.
4. GET /get-server-details/%20 (a single space: inside ServerId's @length 1..27 but outside its @pattern .*\S.*) — expected HTTP 400 ValidationException; emulator answers HTTP 404 ResourceNotFoundException.
