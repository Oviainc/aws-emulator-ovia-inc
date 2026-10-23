Issue-Source: trace-sweep-divergence
Issue-Trace: qconnect-error-paths
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: qconnect-error-paths
Issue-Shape-Id: com.amazonaws.qconnect#WisdomService
Issue-Key: wisdom|CreateSession|4xx->4xx|$.message
Issue-Key: wisdom|CreateSession|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: wisdom|CreateSession|4xx->4xx|status
Issue-Commit: 1d15d4851343e3a31be69cd8cea36918aab666fe

# QConnect CreateSession validates the assistantId before the auth check, answering 400 ValidationException instead of 401 UnauthorizedException

> _Note: this issue was identified against the code base at commit `1d15d4851343e3a31be69cd8cea36918aab666fe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSession

## Description
This issue carries 1 cause on service `wisdom`, net-new (stub_path empty, owned_by empty in the replay report).

Cause 1 of 1 — CreateSession, rejection-class precedence on an unauthorized request that also carries a malformed assistantId.

AWS: Smithy services/service-com-amazonaws-qconnect-wisdomservice/smithy-model/com-amazonaws-qconnect-wisdomservice.json — CreateSession POST /assistants/{assistantId}/sessions, input member `assistantId` is @required @httpLabel targeting `UuidOrArn` with pattern `^[a-f0-9]{8}-...-[a-f0-9]{12}$|^arn:[a-z-]*?:wisdom:...$`; modeled errors include `UnauthorizedException` (httpError 401, doc 'You do not have permission to perform this action.') and `ValidationException` (httpError 400). Which check runs first is not stated by the model, so this is recorded behavior only: the recording of this step shows AWS answering HTTP 401 with `x-amzn-errortype: UnauthorizedException` for a request whose caller is not authorized, even though the path label would also fail the UuidOrArn pattern — i.e. the authorization decision precedes input validation. The requirements asserted here are the HTTP status 401 and the error code `UnauthorizedException`; the recorded body message 'Unauthorized' is quoted as an example only.

Emulator: HTTP 400 with `x-amzn-errortype: ValidationException` and the request-derived message "1 validation error detected: Value 'not-a-valid-id' at 'assistantId' failed to satisfy constraint: Member must satisfy regular expression pattern: <UuidOrArn pattern>" — the httpLabel pattern check runs before any authorization check, so the unauthorized condition is never reported.

Why: 401 and 400 drive different client behavior. An SDK/credential provider treats 401 UnauthorizedException as a credentials/permissions problem (refresh or surface an auth failure), while 400 ValidationException is a non-retryable caller bug. A client whose request is actually refused for permissions is told its input is malformed, so it never re-authenticates and permission misconfiguration is misdiagnosed; conversely an emulator-based test cannot exercise the auth-failure branch of CreateSession at all.

Fix: the CreateSession request pipeline ordering — the authorization/identity check must be evaluated before httpLabel/member constraint validation for this operation (the shared restJson1 request pipeline for the qconnect service, not the validation rule itself, which is otherwise correct in form).

Traces: qconnect-error-paths#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline: emulator_up=true ('/__admin/health answered 200 and the :33037 data listener accepts'), bootstrap_signal=false, state_reset_failures=0, fixtures_replayed=12; steps_total=114, passed=26, mismatched=42, blocked=46, divergence_count=6.

Cluster: fixture qconnect-error-paths, step task#4, operation CreateSession, service wisdom, verdict mismatch, affected_count=1, divergence_key service=wisdom|CreateSession|status=401->400|keys=$.message,$headers.x-amzn-errortype,status.
  status: expected 401, got 400
  $headers.x-amzn-errortype: expected 'UnauthorizedException', got 'ValidationException'
  $.message: expected 'Unauthorized' (example text), got "1 validation error detected: Value 'not-a-valid-id' at 'assistantId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$|^arn:[a-z-]*?:wisdom:[a-z0-9-]*?:[0-9]{12}:[a-z-]*?/[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}(?:/[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}){0,2}$"

Both UnauthorizedException (401) and ValidationException (400) are modeled on CreateSession, so the emulator has the right shape available and merely applies it at the wrong point in the pipeline. The asserted requirements are the status and the error code; the message difference is reported only as corroborating detail. The service is bootstrapped in this checkout (resolved bootstrap issues for service-com-amazonaws-wisdom-wisdomservice and service-com-amazonaws-qconnect-wisdomservice).

## Steps to Reproduce
1. Clone mainline and build the emulator; replay the simparity-wire-aws-v1 wire corpus.
2. Step qconnect-error-paths#task#4: POST /assistants/not-a-valid-id/sessions (CreateSession) as the fixture records it — an unauthorized caller with a malformed assistantId path label.
3. Expected wire outcome: HTTP 401 with header `x-amzn-errortype: UnauthorizedException` (body message is not asserted).
4. Observed: HTTP 401 is never produced; the emulator answers HTTP 400 with `x-amzn-errortype: ValidationException` and a pattern-constraint message for 'assistantId'.
5. Equivalently with boto3 `qconnect` against the emulator, call create_session(assistantId='not-a-valid-id', name='x') under the fixture's unauthorized identity: the raised ClientError reports ValidationException/400 instead of UnauthorizedException/401.
