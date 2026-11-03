Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.run-statement-on-non-ready-session-rejected glue.session-lifecycle-create-run-get-delete-404
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.run-statement-on-non-ready-session-rejected|run#17|AssertionError|assert message == "Entity not found", message
Issue-Key: glue.session-lifecycle-create-run-get-delete-404|run#15|AssertionError|assert run_message == "Entity not found", run_message
Issue-Commit: a32a3e20a1235e995b8b82a77105d44e9a1e2c6f

# Glue RunStatement against a fully removed session answers EntityNotFoundException with an emulator-authored message ("Session <id> not found.") where AWS answers "Entity not found"

> _Note: this issue was identified against the code base at commit `a32a3e20a1235e995b8b82a77105d44e9a1e2c6f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunStatement

## Description
One (1) net-new root cause in the Glue interactive-session statement surface. It is NOT covered by the open issue 20261029T230701Z-glue-interactive-sessions-5-net-new-root-causes.md: that issue's RunStatement causes are the PROVISIONING refusal class/message (IllegalSessionStateException vs InvalidInputException) and the in-flight/busy gate, plus GetStatement's not-found classification and message. Neither it nor any other open Glue session issue covers RunStatement's refusal text for a session that no longer exists at all.

Cause 1 of 1: RunStatement on a session id that resolves to nothing is refused with the right error code but the wrong message.

AWS: Smithy (services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json): RunStatement input {SessionId (required, NameString, @length 1-255), Code (required)}; modeled errors include EntityNotFoundException (client, HTTP 400) and IllegalSessionStateException. EntityNotFoundException.Message is only "A message describing the problem" (MessageString), so the model fixes the code and status but not the text — the recording fixes the text. Recorded behavior: after DeleteSession has driven a session through STOPPING/STOPPED and the session row is fully removed (GetSession on it is refused EntityNotFoundException), a subsequent RunStatement(SessionId=<the removed id>, Code=...) is refused with HTTP 400, error code EntityNotFoundException and the message exactly "Entity not found" — wholly fixed text, with NO variable part: the session id from the request does not appear in it. (Contrast Glue's other not-found texts, e.g. ListStatements on the same removed session, which AWS words "Session ID <sessionId> not found" with <sessionId> taken from the request — so this is a per-operation message, not one shared helper.)

Emulator: the same HTTP 400 and the same code EntityNotFoundException, but the message is "Session <sessionId> not found." — emulator-authored text (fixed parts "Session " and " not found.") with <sessionId> interpolated from the request. Observed as "Session sim2real-glue-sess-48118218bfc0 not found." and "Session sim2real-session-c20846963b13 not found." in the two replayed checks; the ids are per-run values that fill the template and are not part of the requirement. Source in the checkout: services/service-com-amazonaws-glue-awsglue/src/ops/run_statement.rs lines 192-204 return EntityNotFoundException with crate::state::messages::session_not_found(&input.session_id).

Why: a client (and any test suite, log differ, or support tooling) that matches on Glue's refusal text for a vanished session never sees the string real Glue emits, and instead sees one that leaks the session id into a message AWS keeps constant. Code-only clients are unaffected, but anything asserting or parsing the message diverges, and the shared session_not_found helper makes the same wrong text appear on every operation that reuses it, hiding the fact that AWS words this per operation.

Fix: the not-found refusal path of the RunStatement handler and the message helper it calls (services/service-com-amazonaws-glue-awsglue/src/ops/run_statement.rs, crate::state::messages::session_not_found) — RunStatement must answer the fixed text "Entity not found" for an unresolvable SessionId, without changing the error code or the 400 status, and without changing the differently-worded texts other session operations require.

Traces: glue.run-statement-on-non-ready-session-rejected, glue.session-lifecycle-create-run-get-delete-404.

## Evidence
Check glue.session-lifecycle-create-run-get-delete-404 (mismatch):
  expected: run_statement against the fully removed session -> code EntityNotFoundException, Message == 'Entity not found'
  actual:   code EntityNotFoundException, Message == 'Session sim2real-session-c20846963b13 not found.'
  assertion: check_glue_session_lifecycle_create_run_get_delete_404.py line 73: assert run_message == "Entity not found", run_message -> AssertionError: Session sim2real-session-c20846963b13 not found.

Check glue.run-statement-on-non-ready-session-rejected (mismatch):
  the session settled to GONE (GetSession refused EntityNotFoundException) after DeleteSession, so the check grades the GONE branch
  expected: run_statement -> code EntityNotFoundException, Message == 'Entity not found'
  actual:   code EntityNotFoundException, Message == 'Session sim2real-glue-sess-48118218bfc0 not found.'
  assertion: check_glue_run_statement_on_non_ready_session_rejected.py line 86: assert message == "Entity not found", message -> AssertionError: Session sim2real-glue-sess-48118218bfc0 not found.

Message anatomy: AWS text is entirely fixed ("Entity not found"); the emulator's text is fixed parts "Session " + " not found." with the request's SessionId interpolated. The ids above are per-run values, not requirements.

Handler evidence (checkout): services/service-com-amazonaws-glue-awsglue/src/ops/run_statement.rs lines 192-204 (store.session_mut(...) miss -> EntityNotFoundException { message: Some(crate::state::messages::session_not_found(&input.session_id)) }).

## Steps to Reproduce
1. Create an IAM role trusted by glue.amazonaws.com (attach arn:aws:iam::aws:policy/service-role/AWSGlueServiceRole).
2. glue.create_session(Id=<session-id>, Role=<role-arn>, Command={'Name':'glueetl','PythonVersion':'3'}, GlueVersion='4.0'); poll glue.get_session(Id=<session-id>) until Session.Status == 'READY'.
3. glue.delete_session(Id=<session-id>); poll glue.get_session(Id=<session-id>) until it is refused with EntityNotFoundException (the session row is fully removed).
4. Call glue.run_statement(SessionId=<session-id>, Code='1').
5. Inspect the refusal: HTTP 400 / EntityNotFoundException in both AWS and the emulator, but AWS's Error.Message is 'Entity not found' while the emulator returns 'Session <session-id> not found.'.
