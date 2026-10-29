Issue-Source: trace-sweep-divergence
Issue-Trace: mpa-session-notfound-errors
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mpa-session-notfound-errors
Issue-Shape-Id: com.amazonaws.mpa#AWSFluffyCoreService
Issue-Key: mpa|GetSession|4xx->4xx|$.Message
Issue-Key: mpa|GetSession|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: mpa|GetSession|4xx->4xx|status
Issue-Key: mpa|CancelSession|4xx->4xx|$.Message
Issue-Key: mpa|CancelSession|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: mpa|CancelSession|4xx->4xx|status
Issue-Commit: 92c3b140342b1bc1fab2a232b8cee3f25ea78ff5

# mpa session reads refuse an unknown session ARN with 404 ResourceNotFoundException instead of AWS's 403 AccessDeniedException

> _Note: this issue was identified against the code base at commit `92c3b140342b1bc1fab2a232b8cee3f25ea78ff5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSession

## Description
Two net-new root causes sharing one fault: every session-scoped operation classifies a session ARN that names no stored session as "resource not found" and answers 404 ResourceNotFoundException, where AWS refuses the caller 403 AccessDeniedException. MPA does not disclose the existence of a session a caller is not authorized for, so a well-formed session ARN with no matching session is an authorization refusal, not a not-found.

Cause 1 of 2 — GetSession. Fixture mpa-session-notfound-errors, step task#1.
AWS: Smithy: GetSession, http GET /sessions/{SessionArn}, input SessionArn @required @httpLabel with @length(min 20, max 2048) and pattern ^arn:aws(-[^:]+)?:mpa:[a-z0-9-]{1,20}:[0-9]{12}:session/[a-zA-Z0-9._-]+/[a-zA-Z0-9_-]+$. Modelled errors include both AccessDeniedException (@error client, @httpError 403, doc "You do not have sufficient access to perform this action. Check your permissions, and try again.") and ResourceNotFoundException (@error client, @httpError 404). The recording shows which of the two AWS picks for a well-formed ARN naming no session the caller can see: HTTP 403, body member Message = "Access Denied." (fixed text, no variable part), response header x-amzn-errortype = "AccessDeniedException:http://internal.amazon.com/coral/com.amazon.awsfluffycoreservice/" — the error shape's name followed by the service's Coral namespace, both fixed for this service.
Emulator: HTTP 404 with x-amzn-errortype = "ResourceNotFoundException" and Message = "The requested resource does not exist." The status, the error code and the message text all diverge, and the errortype header is additionally emitted without the service's Coral namespace suffix that AWS appends.
Why: a client distinguishes the two classes. 403 is a terminal credential/permission failure that an SDK surfaces as AccessDenied and that retry and alarm logic must not retry or treat as "the session finished and was reaped"; 404 reads as "this session no longer exists", so a poller stops waiting on a session it is merely not authorized to read, and an error-handling branch keyed on AccessDeniedException never runs. The error code also leaks that no such session exists, which is the disclosure AWS's 403 exists to avoid.
Fix: the session-lookup refusal in GetSession's handler (and the shared session resolution it calls in the service's state), which must raise AccessDeniedException/403 rather than ResourceNotFoundException/404 for a session ARN that resolves to nothing; plus the error serializer, so x-amzn-errortype carries the shape name with this service's Coral namespace suffix.
Traces: mpa-session-notfound-errors#task#1.

Cause 2 of 2 — CancelSession takes the same wrong branch. Fixture mpa-session-notfound-errors, step task#2.
AWS: Smithy: CancelSession, http PUT /sessions/{SessionArn}, @idempotent, empty output, same SessionArn @required @httpLabel constraints, and the same error set plus ConflictException/409. The recording shows the same refusal for a session ARN naming no visible session: HTTP 403, Message = "Access Denied.", x-amzn-errortype = "AccessDeniedException:http://internal.amazon.com/coral/com.amazon.awsfluffycoreservice/".
Emulator: HTTP 404, x-amzn-errortype = "ResourceNotFoundException", Message = "The requested resource does not exist."
Why: cancelling a session is a mutation, and a caller that reads 404 concludes the session is already gone and reports success-by-absence, where AWS says the call was refused and the session may well still be pending. Retry/idempotency logic built on @idempotent CancelSession therefore terminates on a refusal it never saw.
Fix: the same component, reached from CancelSession's handler — the shared not-found-vs-denied decision for a session ARN, so both session-scoped operations refuse alike.
Traces: mpa-session-notfound-errors#task#2.

Message requirements: for this error class AWS's Message is the fixed string "Access Denied." with no variable part, and x-amzn-errortype is the fixed string "AccessDeniedException:http://internal.amazon.com/coral/com.amazon.awsfluffycoreservice/" (error shape name + this service's fixed Coral namespace). The session ARN in the recordings is a request value and is not part of the requirement.

Not covered by any open issue: 20261028T192936Z-mpa-getsession-wrong-behavior.md is about a session's ExecutionStatus never reaching EXECUTED on the success path and does not touch the refusal for an unknown session ARN.

## Evidence
Replay of trace set simparity-wire-aws-v1 (11 fixtures, 35 steps, 30 passed, 5 mismatched).

Cause 1 — mpa-session-notfound-errors#task#1, GetSession, expected 403 / actual 404:
  status: expected 403, got 404
  $.Message: expected "Access Denied.", got "The requested resource does not exist."
  $headers.x-amzn-errortype: expected "AccessDeniedException:http://internal.amazon.com/coral/com.amazon.awsfluffycoreservice/", got "ResourceNotFoundException"

Cause 2 — mpa-session-notfound-errors#task#2, CancelSession, expected 403 / actual 404:
  status: expected 403, got 404
  $.Message: expected "Access Denied.", got "The requested resource does not exist."
  $headers.x-amzn-errortype: expected "AccessDeniedException:http://internal.amazon.com/coral/com.amazon.awsfluffycoreservice/", got "ResourceNotFoundException"

Model evidence from services/service-com-amazonaws-mpa-awsfluffycoreservice/smithy-model/com-amazonaws-mpa-awsfluffycoreservice.json: GetSession (GET /sessions/{SessionArn}) and CancelSession (PUT /sessions/{SessionArn}, @idempotent) both model AccessDeniedException @httpError 403 and ResourceNotFoundException @httpError 404; AccessDeniedException$Message is @required String @length(max 1000).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture mpa-session-notfound-errors step task#1 — GET /sessions/{SessionArn} with a session ARN that satisfies SessionArn's pattern but names no session in the store. Expect 403 with x-amzn-errortype "AccessDeniedException:http://internal.amazon.com/coral/com.amazon.awsfluffycoreservice/" and Message "Access Denied."; the emulator answers 404 ResourceNotFoundException.
3. Cause 2: replay fixture mpa-session-notfound-errors step task#2 — PUT /sessions/{SessionArn} for the same unknown session ARN; same expected refusal, same divergence.
4. Both steps must pass before this issue is closed.
