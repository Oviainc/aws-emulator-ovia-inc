Issue-Source: trace-sweep-divergence
Issue-Trace: connectparticipant-participant-session-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: connectparticipant-participant-session-lifecycle
Issue-Shape-Id: com.amazonaws.connectparticipant#AmazonConnectParticipantServiceLambda
Issue-Key: connectparticipant|CreateParticipantConnection|4xx->2xx|$.message
Issue-Key: connectparticipant|CreateParticipantConnection|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: connectparticipant|CreateParticipantConnection|4xx->2xx|status
Issue-Key: connectparticipant|CreateParticipantConnection|4xx->4xx|$.message
Issue-Key: connectparticipant|SendMessage|4xx->4xx|$.message
Issue-Commit: b00bffca1001eed9eb3256fe81ec389ea90933f7

# ConnectParticipant error paths miss AccessDenied and AWS message text

> _Note: this issue was identified against the code base at commit `b00bffca1001eed9eb3256fe81ec389ea90933f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateParticipantConnection

## Description
Three distinct root causes on connectparticipant's error paths, all found by replaying the wire corpus (trace set simparity-wire-aws-v1) against the emulator: one refusal AWS makes that the emulator does not make at all, and two whose body does not carry AWS's error message. Smithy model consulted: services/service-com-amazonaws-connectparticipant-amazonconnectparticipantservicelambda/smithy-model/com-amazonaws-connectparticipant-amazonconnectparticipantservicelambda.json (sdkId ConnectParticipant, restJson1; common errors AccessDeniedException/InternalServerException/ThrottlingException/ValidationException apply to every operation).

Cause 1 of 3: CreateParticipantConnection answers 200 where AWS refuses with 403 AccessDeniedException - it fails open on a ParticipantToken the service never issued. AWS: Smithy CreateParticipantConnection declares AccessDeniedException (errorKind client, httpError 403, member Message required, doc "You do not have sufficient access to perform this action"); a restJson1 error also carries the x-amzn-errortype header naming the shape. The recording for this denial class is HTTP 403 with body member message = 'User is not authorized to access this resource with an explicit deny in an identity-based policy' - that text is entirely FIXED for this class, with no parts taken from the request or from state. Emulator: HTTP 200 with a live connection; no message member and no x-amzn-errortype header are emitted at all. Mechanism: services/.../src/ops/create_participant_connection.rs, the None arm of the state.participant_tokens lookup, mints a fresh contact id and ChatSession for ANY previously unseen ParticipantToken and registers the token, so the only 403 path left is a token that IS known and whose session is no longer connected. Hypothesis: the fixture step presents a participant token this service never issued/was denied. Why: relative to AWS this is an authentication bypass - an arbitrary or explicitly denied participant token is handed a working chat session, so a client's (and a test's) handling of the denial is never exercised and negative-path coverage of the service's only credential is unreachable. Fix: CreateParticipantConnection's handler/authorization must refuse an unissued participant token with the modeled AccessDeniedException instead of creating a session for it. Trace: connectparticipant-participant-session-lifecycle#task#6.

Cause 2 of 3: the empty-Type refusal carries the framework's generic constraint text instead of AWS's message. AWS: Smithy CreateParticipantConnectionRequest$Type targets ConnectionTypeList with @length min 1 and the operation declares ValidationException (httpError 400, "The input fails to satisfy the constraints specified by Amazon Connect", member Message required, len 1..2000). The recorded 400 body's message is the fixed text 'Failed to create participant connection due to missing connection type' - no values from the request appear in it. Emulator: same 400 status, but the body's message is the boundary validator's generic rendering "1 validation error detected: Value '0 items' at 'type' failed to satisfy constraint: Member must have length greater than or equal to 1" - wording AWS never emits, which additionally leaks the emulator's internal lowercased member name 'type' and the raw length bound. Why: a client that branches on, logs or surfaces the refusal text for a missing connection type never matches AWS's message, and the emulator's text exposes internals instead of the service's documented reason. Fix: CreateParticipantConnection's own input validation, ahead of (or overriding) the generic length check in the request walk. Trace: connectparticipant-participant-session-lifecycle#task#5.

Cause 3 of 3: a 403 refusal is served with NO message member in its body. AWS: AccessDeniedException models Message as REQUIRED (httpError 403), and the recorded refusal of SendMessage on a connection token that is no longer valid is HTTP 403 with body member message = 'Access denied' - again entirely fixed text with no request- or state-derived parts. Emulator: the status 403 is right, but the replayed body carries no message member at all ($.message missing). The handler does construct the modeled error with its own wording - services/.../src/ops/send_message.rs returns AccessDeniedException { message: "the connection token does not name a connected chat participant" } - so the text is ALSO wrong wherever it does surface. Hypothesis for the member going missing on the wire: the restJson1 error serializer writes the modeled member name 'Message' while AWS wires 'message' for this service's exceptions - note that Cause 2's 400, whose body is produced by the framework's own validation path, DID surface a lowercase $.message. Why: every SDK client reads the reason out of that member, so a denial arrives with an empty message - callers cannot log, classify or display why the call was refused - and when the member is restored the text must be AWS's 'Access denied', not the emulator's own sentence. Fix: the restJson1 error serializer for modeled exceptions (member/wire casing) plus the refusal message text in the operation handlers that build AccessDeniedException. Trace: connectparticipant-participant-session-lifecycle#task#15.

Related open issue: 20261105T033734Z-connectparticipant-7-net-new-causes-connection-token-refusal.md (behavioral) describes the same fail-open as its Cause 7, the empty-Type wording as its Cause 6 and the 'Access denied' wording as its Cause 1, but it carries NO wire replay key for any of the fixture steps above, so these wire divergences are filed here with their own keys.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200): 26 steps, 18 passed, 8 mismatched, score 69.2%.

Cause 1 - fixture connectparticipant-participant-session-lifecycle step task#6, operation CreateParticipantConnection, verdict mismatch, expected status 403 / actual status 200:
  "status: expected 403, got 200"
  "$.message: missing (expected 'User is not authorized to access this resource with an explicit deny in an identity-based policy', got None)"
  "$headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)"
  Expected: HTTP 403, AccessDeniedException (x-amzn-errortype present), message exactly 'User is not authorized to access this resource with an explicit deny in an identity-based policy' (all fixed text). Actual: a successful 200 connection response.
  Source: services/.../src/ops/create_participant_connection.rs - the None arm of the participant_tokens lookup inserts a new session and token binding, then returns ConnectionCredentials.

Cause 2 - fixture connectparticipant-participant-session-lifecycle step task#5, operation CreateParticipantConnection, expected status 400 / actual status 400:
  "$.message: value_mismatch (expected 'Failed to create participant connection due to missing connection type', got \"1 validation error detected: Value '0 items' at 'type' failed to satisfy constraint: Member must have length greater than or equal to 1\")"
  Status and error class match; only the message text diverges. The expected text is wholly fixed; the actual text's "'0 items'", "'type'" and bound are framework-derived.

Cause 3 - fixture connectparticipant-participant-session-lifecycle step task#15, operation SendMessage, expected status 403 / actual status 403:
  "$.message: missing (expected 'Access denied', got None)"
  Status matches; the body has no message member. Expected message is the fixed text 'Access denied'.
  Source: services/.../src/ops/send_message.rs (AccessDeniedException built with the message "the connection token does not name a connected chat participant").

## Steps to Reproduce
1. Boot the emulator from this checkout and point a ConnectParticipant client at its data listener.
2. Cause 1: CreateParticipantConnection(ParticipantToken="bogus-participant-token-<uuid>", Type=["CONNECTION_CREDENTIALS"]) with a token this emulator never issued - the call SUCCEEDS with HTTP 200 and live ConnectionCredentials, where AWS answers HTTP 403 AccessDeniedException with message 'User is not authorized to access this resource with an explicit deny in an identity-based policy' and an x-amzn-errortype header.
3. Cause 2: with client-side parameter validation disabled, CreateParticipantConnection(ParticipantToken=<valid token>, Type=[]) - the 400 body's message is "1 validation error detected: Value '0 items' at 'type' failed to satisfy constraint: Member must have length greater than or equal to 1" instead of 'Failed to create participant connection due to missing connection type'.
4. Cause 3: obtain a ConnectionToken via CreateParticipantConnection, invalidate it (DisconnectParticipant, or use a token this service did not mint), then SendMessage with it and inspect the RAW 403 body - it carries no lowercase message member (AWS returns {"message": "Access denied"}).
5. Re-replay fixture steps connectparticipant-participant-session-lifecycle#task#6, #task#5 and #task#15 to confirm a fix; all three must pass.
