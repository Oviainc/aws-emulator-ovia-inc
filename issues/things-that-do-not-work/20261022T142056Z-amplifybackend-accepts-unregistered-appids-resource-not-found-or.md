Issue-Source: trace-sweep-divergence
Issue-Trace: amplifybackend-errors-unregistered-appid
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: amplifybackend-errors-unregistered-appid
Issue-Shape-Id: com.amazonaws.amplifybackend#AmplifyBackend
Issue-Key: amplifybackend|GetBackend|4xx->4xx|$.code
Issue-Key: amplifybackend|GetBackend|4xx->4xx|$.message
Issue-Key: amplifybackend|GetBackend|4xx->4xx|$headers.content-type
Issue-Key: amplifybackend|GetBackend|4xx->4xx|$headers.etag
Issue-Key: amplifybackend|GetBackend|4xx->4xx|status
Issue-Key: amplifybackend|ListBackendJobs|4xx->4xx|$.code
Issue-Key: amplifybackend|ListBackendJobs|4xx->4xx|$.message
Issue-Key: amplifybackend|ListBackendJobs|4xx->4xx|$headers.content-type
Issue-Key: amplifybackend|ListBackendJobs|4xx->4xx|$headers.etag
Issue-Key: amplifybackend|ListBackendJobs|4xx->4xx|status
Issue-Key: amplifybackend|GetBackendJob|4xx->4xx|$.code
Issue-Key: amplifybackend|GetBackendJob|4xx->4xx|$.message
Issue-Key: amplifybackend|GetBackendJob|4xx->4xx|$headers.content-type
Issue-Key: amplifybackend|GetBackendJob|4xx->4xx|$headers.etag
Issue-Key: amplifybackend|GetBackendJob|4xx->4xx|status
Issue-Key: amplifybackend|DeleteBackend|4xx->4xx|$.code
Issue-Key: amplifybackend|DeleteBackend|4xx->4xx|$.message
Issue-Key: amplifybackend|DeleteBackend|4xx->4xx|$headers.content-type
Issue-Key: amplifybackend|DeleteBackend|4xx->4xx|$headers.etag
Issue-Key: amplifybackend|DeleteBackend|4xx->4xx|status
Issue-Key: amplifybackend|GetBackendAuth|4xx->4xx|$.code
Issue-Key: amplifybackend|GetBackendAuth|4xx->4xx|$.message
Issue-Key: amplifybackend|GetBackendAuth|4xx->4xx|$headers.content-type
Issue-Key: amplifybackend|GetBackendAuth|4xx->4xx|$headers.etag
Issue-Key: amplifybackend|GetBackendAuth|4xx->4xx|status
Issue-Key: amplifybackend|CreateToken|4xx->2xx|$.code
Issue-Key: amplifybackend|CreateToken|4xx->2xx|$.message
Issue-Key: amplifybackend|CreateToken|4xx->2xx|$headers.content-type
Issue-Key: amplifybackend|CreateToken|4xx->2xx|$headers.etag
Issue-Key: amplifybackend|CreateToken|4xx->2xx|status
Issue-Key: amplifybackend|GetToken|4xx->4xx|$.code
Issue-Key: amplifybackend|GetToken|4xx->4xx|$.message
Issue-Key: amplifybackend|GetToken|4xx->4xx|$headers.content-type
Issue-Key: amplifybackend|GetToken|4xx->4xx|$headers.etag
Issue-Key: amplifybackend|GetToken|4xx->4xx|status
Issue-Key: amplifybackend|DeleteToken|4xx->4xx|$.code
Issue-Key: amplifybackend|DeleteToken|4xx->4xx|$.message
Issue-Key: amplifybackend|DeleteToken|4xx->4xx|$headers.content-type
Issue-Key: amplifybackend|DeleteToken|4xx->4xx|$headers.etag
Issue-Key: amplifybackend|DeleteToken|4xx->4xx|status
Issue-Commit: c73ebf67a23d923b166243bec836d38257c16c68

# AmplifyBackend accepts unregistered appIds: resource-not-found or success instead of 400 ValidationException

> _Note: this issue was identified against the code base at commit `c73ebf67a23d923b166243bec836d38257c16c68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBackend, ListBackendJobs, GetBackendJob, DeleteBackend, GetBackendAuth, CreateToken, GetToken, DeleteToken

## Description
Three root causes, 8 wire divergence clusters, all from replaying AmplifyBackend calls that carry an appId which is not a registered Amplify app. The emulator has no app-registration gate at all: it treats "unknown appId" as "this resource does not exist" (404) or, on CreateToken, as a perfectly ordinary success (200). It must be fixed as THREE causes; closing one does not close the others.

NOTE ON AN EXISTING ISSUE: the open behavioral issue 20261022T052247Z-amplifybackend-control-plane-7-fidelity-divergences-no-environment.md describes neighbouring app-identity problems (its Cause 2, CreateBackend auto-creating apps) but it is a behavioral-corpus issue (Issue-Corpus: simparity-behavioral-aws-v1) and carries NO wire replay key, and the replay reported owned_by=[] for every cluster below. It does not cover these fixture steps; the unregistered-appId gate on the read/delete/token paths and the error envelope are not described there at all.

=== Cause 1 of 3: no app-registration gate on the read/delete/token-read paths — 404 resource-not-found instead of 400 ValidationException ===
AWS: recorded behavior only for the error code and message. Smithy binds every one of these operations to BadRequestException (HTTP 400), GatewayTimeoutException (504), NotFoundException (404) and TooManyRequestsException (429), and types AppId as a bare __string httpLabel with no @pattern, so the model does not state this rule. The recording shows real AmplifyBackend validating app IDENTITY before any per-resource lookup: a well-formed but unregistered appId is refused with HTTP 400 and error code `ValidationException`, with a message built from that rule and naming the broken input, e.g. "An error occurred while processing your request: Invalid appId passed." (example text; the requirement is the 400 + ValidationException + the service's fixed prefix plus an appId-invalid sentence). This holds uniformly across GetBackend, ListBackendJobs, GetBackendJob, DeleteBackend, GetBackendAuth, GetToken and DeleteToken.
Emulator: every handler goes straight to a state lookup and reports the MISSING RESOURCE instead of the invalid app. GetBackend/ListBackendJobs/GetBackendJob/DeleteBackend/GetBackendAuth answer HTTP 404 with message 'no backend exists for app <appId>' (see services/service-com-amazonaws-amplifybackend-amplifybackend/src/ops/get_backend.rs:160-167, the `store.app(&input.app_id)` -> `crate::state::not_found(RESOURCE_TYPE_BACKEND, ...)` branch, mirrored in the sibling ops); GetToken/DeleteToken answer HTTP 404 with 'no challenge token with session id sess-1 exists for app <appId>'. Status is 404 where 400 is required, and no error code is emitted at all (see Cause 3).
Why: a client cannot tell a typo'd/deleted appId from a resource that was simply never created. Code that branches on ValidationException to surface "you passed a bad appId" never runs; code that treats 404 as "not created yet, create it" proceeds to create against an app that does not exist, and a retry/poll loop keeps polling an appId that can never resolve. The status class is also wrong (client-input error vs resource-absent), so generic SDK error handling mis-routes.
Fix: the app-resolution step shared by these operations' handlers (each op's `store.app(&input.app_id)` branch in src/ops/*.rs, backed by crate::state's app lookup in src/state.rs) must first decide whether the appId identifies a registered app and, when it does not, refuse with the modeled BadRequestException (400) carrying the ValidationException code, BEFORE any environment/job/token lookup.
Traces: amplifybackend-errors-unregistered-appid#task#1 (GetBackend), #task#2 (ListBackendJobs), #task#3 (GetBackendJob), #task#5 (DeleteBackend), #task#8 (GetBackendAuth), #task#14 (GetToken), #task#15 (DeleteToken).

=== Cause 2 of 3: CreateToken SUCCEEDS for an unregistered appId ===
AWS: recorded behavior only. Smithy: CreateToken POST /backend/{AppId}/challenge, input is AppId only, output AppId/ChallengeCode/SessionId/Ttl, errors BadRequestException(400)/GatewayTimeoutException(504)/NotFoundException(404)/TooManyRequestsException(429). The recording shows the same app-identity gate as Cause 1 applying here: HTTP 400, error code `ValidationException`, message e.g. "An error occurred while processing your request: Invalid appId passed." No challenge token is minted.
Emulator: returns HTTP 200 and mints a challenge token for an appId that was never registered (expected 400, got 200; no message/code in the body at all). This is a different defect from Cause 1 — here the handler does not even perform a lookup that could fail, so adding the identity gate is the whole fix and nothing in the handler currently rejects anything.
Why: an overly permissive success. A caller gets a ChallengeCode/SessionId for a non-existent app, so an Amplify Studio pairing flow appears to work against the emulator and fails outright against AWS; worse, the emulator's token state now holds a token bound to an app that does not exist, which is why the follow-up GetToken/DeleteToken steps then diverge on a token-shaped 404 rather than the appId error.
Fix: CreateToken's handler (src/ops/create_token.rs) — apply the same app-registration gate as Cause 1 before minting anything, refusing with 400 + ValidationException and storing no token.
Traces: amplifybackend-errors-unregistered-appid#task#13.

=== Cause 3 of 3: the error response envelope omits the `code` member and the service's content-type/ETag headers ===
AWS: recorded behavior only — the model's error shapes carry only Message (BadRequestException: Message; NotFoundException: Message + ResourceType, both jsonName lowercase), so the model does not state this. Every recorded AmplifyBackend error response body carries BOTH a `message` and a `code` member (the `code` values observed include ValidationException, ResourceNotFoundException and NotFoundException — codes the model does not even declare), and the response carries `content-type: application/json; charset=utf-8` plus an ETag header.
Emulator: the error body carries no `code` member at all ($.code: missing, got None on all 8 clusters) and the response's content-type is 'application/json' without the charset parameter; no ETag header is sent. The error code is the ONLY machine-readable discriminator AmplifyBackend gives a client, so its absence from the body makes every one of these errors unclassifiable from the payload.
Why: a client that reads the error code out of the response body (as the recorded service contract allows) gets nothing, so it cannot distinguish ValidationException from NotFoundException and falls back to treating all failures alike; a content-type match on the exact recorded value fails. The recorded ETag VALUE is derived from the response bytes and is environment-specific (example only: 'W/"71-AM6iFcHGTs+SaUzU+dk7lHa7j2M"') — only the header's presence is the recorded behavior, not its value. Likewise the exact message texts above are examples; the requirements are the status, the code, and the service's fixed "An error occurred while processing your request: " prefix rule.
Fix: the service's error channel/serializer — crate::state's error constructors (src/state.rs: not_found and siblings) plus the restJson1 error serialization path for this service, so an error response emits the `code` member alongside `message` and sets the service's content-type/ETag headers.
Traces: all 8 clusters — amplifybackend-errors-unregistered-appid#task#1, #task#2, #task#3, #task#5, #task#8, #task#13, #task#14, #task#15.

## Evidence
Replay of trace set simparity-wire-aws-v1 (summary: 33 steps, 2 passed, 31 mismatched, score 6.1%). All 8 clusters below are from fixture amplifybackend-errors-unregistered-appid; none carried stub_path or owned_by.

Cause 1 — task#1 GetBackend: status expected 400, got 404; $.message expected 'An error occurred while processing your request: Invalid appId passed.', got 'no backend exists for app d12440e142099'; $.code expected 'ValidationException', got None.
Cause 1 — task#2 ListBackendJobs: status expected 400, got 404; same message/code divergence.
Cause 1 — task#3 GetBackendJob: status expected 400, got 404; same message/code divergence.
Cause 1 — task#5 DeleteBackend: status expected 400, got 404; same message/code divergence.
Cause 1 — task#8 GetBackendAuth: status expected 400, got 404; same message/code divergence.
Cause 1 — task#14 GetToken: status expected 400, got 404; $.message got 'no challenge token with session id sess-1 exists for app d12440e142099'; $.code expected 'ValidationException', got None.
Cause 1 — task#15 DeleteToken: status expected 400, got 404; same as task#14.

Cause 2 — task#13 CreateToken: status expected 400, got 200; $.message expected 'An error occurred while processing your request: Invalid appId passed.', got None; $.code expected 'ValidationException', got None.

Cause 3 — all 8 clusters: $.code missing (expected a code, got None); $headers.content-type value_mismatch (expected 'application/json; charset=utf-8', got 'application/json'); $headers.etag missing (recorded present, e.g. 'W/"71-AM6iFcHGTs+SaUzU+dk7lHa7j2M"' — value is body-derived and is an example only).

Model citations: Smithy AppId is __string httpLabel with no @pattern on all these ops; errors bound are BadRequestException(400)/GatewayTimeoutException(504)/NotFoundException(404)/TooManyRequestsException(429); BadRequestException has only Message (jsonName 'message'), NotFoundException has Message + ResourceType — no `code` member is modeled, so the body's code is recorded behavior only.

## Steps to Reproduce
Boot the emulator and replay fixture amplifybackend-errors-unregistered-appid against it (all steps use a well-formed appId that was never registered as an Amplify app, e.g. 'd12440e142099'):

1. task#1 — POST /backend/{appId}/details (GetBackend). Observe HTTP 404 with body {"message": "no backend exists for app <appId>"} instead of HTTP 400 with code ValidationException and the 'Invalid appId passed.' message (Cause 1).
2. task#2 / task#3 / task#5 / task#8 — ListBackendJobs, GetBackendJob, DeleteBackend, GetBackendAuth for the same appId: each answers 404 instead of 400 ValidationException (Cause 1).
3. task#13 — POST /backend/{appId}/challenge (CreateToken). Observe HTTP 200 with a minted ChallengeCode/SessionId instead of HTTP 400 ValidationException (Cause 2).
4. task#14 / task#15 — GET /backend/{appId}/challenge/sess-1 and POST .../remove (GetToken, DeleteToken). Observe HTTP 404 'no challenge token with session id sess-1 exists for app <appId>' instead of 400 ValidationException (Cause 1).
5. On every one of those responses, inspect the body and headers: there is no `code` member, content-type is 'application/json' rather than 'application/json; charset=utf-8', and no ETag header is set (Cause 3).
