Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.e2e-interactive-session-statement-execution-flow glue.stop-session-drives-a-ready-session-to-stopped-and-refuses-new-statements glue.delete-session-while-statement-running-accepted glue.list-statements-ordering-by-creation glue.list-statements-returns-submission-order glue.run-statement-on-busy-session-rejected glue.get-statement-nonexistent-rejected glue.get-statement-nonexistent-not-found
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.e2e-interactive-session-statement-execution-flow|run#14|AssertionError|assert statement["State"] == "AVAILABLE", f"statement did not complete: {statement}"
Issue-Key: glue.stop-session-drives-a-ready-session-to-stopped-and-refuses-new-statements|run#10|AssertionError|assert statement["State"] == "AVAILABLE", (
Issue-Key: glue.delete-session-while-statement-running-accepted|_wait_statement_state#8|Timeout|time.sleep(interval_s)
Issue-Key: glue|RunStatement|InvalidInputException
Issue-Key: glue.run-statement-on-busy-session-rejected|run#8|AssertionError|assert code == "InvalidInputException", f"expected InvalidInputException, got {code!r}: {message!r}"
Issue-Key: glue.get-statement-nonexistent-rejected|run#12|AssertionError|assert code == "IllegalSessionStateException", code
Issue-Key: glue.get-statement-nonexistent-not-found|run#15|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 42112e68a35449969ef0708992459a3b7df02e34

# Glue interactive sessions: 5 net-new root causes — statements never execute (ERROR/RuntimeUnavailable), a second RunStatement is refused, and three RunStatement/GetStatement refusal classes and messages are emulator-authored

> _Note: this issue was identified against the code base at commit `42112e68a35449969ef0708992459a3b7df02e34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunStatement

## Description
Five (5) net-new root causes in the Glue interactive-session statement surface (RunStatement / GetStatement / ListStatements). All five are independent and each must be closed before this issue is submitted. None is covered by the open Glue session issues: 20261029T103029Z-glue-2-net-new-root-causes-getdashboardurl-s.md covers GetDashboardUrl's message and Session.IdleTimeout, 20261029T201859Z-glue-job-job-run-session-and-connection-reads.md covers dropped modeled members on session reads, and 20261029T185330Z-glue-deletesession-stub.md covers the DeleteSession scaffold stub — none of them covers statement execution or the RunStatement/GetStatement refusal classes below.

Model consulted via smithy_query on services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json.

=== Cause 1 of 5: a statement submitted to a READY session is recorded but never executed ===
AWS: Smithy: GetStatement -> Statement{Id, Code, State: StatementState (WAITING|RUNNING|AVAILABLE|CANCELLING|CANCELLED|ERROR), Output: StatementOutput{Data: StatementOutputData{TextPlain = "The code execution output in text format"}, ExecutionCount, Status, ErrorName, ErrorValue, Traceback}, Progress, StartedOn, CompletedOn}. Recorded behavior: a statement run on a READY session executes on the session's Spark REPL — it is observable in RUNNING, then settles State=AVAILABLE with Output.Status=AVAILABLE, a non-zero ExecutionCount and Output.Data.TextPlain carrying the value the submitted code computed (the checks submit code whose answer cannot be echoed from the request, e.g. print(sum(range(1,101))) -> output text contains "5050").
Emulator: every statement settles State='ERROR' with Output={'ExecutionCount': 0, 'Status': 'ERROR', 'ErrorName': 'RuntimeUnavailable', 'ErrorValue': 'This emulator records statements but runs no Spark runtime, so the statement ended without running.'}, Progress 0.0, and no Output.Data at all. RUNNING is never observed either: a statement submitted as 'import time; time.sleep(45)' is never seen in RUNNING, so a check that waits for an in-flight statement hangs until the harness's deadline.
Why: a client that uses an interactive session for its purpose — run code, read the result — gets no result and an error state for every cell, and cannot distinguish a session that is broken from code that genuinely failed; any flow keyed to RUNNING (cancel/delete mid-statement) can never be exercised.
Fix: the statement execution/settling in this service's session state (the statement record's terminal state and Output are synthesized there, not in the handler) and the read that rebuilds Statement for GetStatement.
Traces: glue.e2e-interactive-session-statement-execution-flow (AssertionError: statement did not complete: State 'ERROR', ErrorName 'RuntimeUnavailable'), glue.stop-session-drives-a-ready-session-to-stopped-and-refuses-new-statements (positive-control statement settled 'ERROR', expected AVAILABLE with TextPlain containing 5050), glue.delete-session-while-statement-running-accepted (waits for State=RUNNING, never produced -> check timed out after 60.0s).

=== Cause 2 of 5: RunStatement refuses a second statement on a READY session instead of queueing it ===
AWS: recorded behavior: two back-to-back RunStatement calls on the SAME READY session both succeed and return distinct ids; the session runs them one at a time and ListStatements then returns both in submission order. Smithy: RunStatement does model a dedicated SessionBusyException ("The session is currently busy processing another request and cannot accept new operations"), but the recording shows no refusal at all for a second submission on a READY session.
Emulator: the second RunStatement is refused with HTTP 400 InvalidInputException and the message "Session <sessionId> is still running statement <id>, so it cannot take another statement yet." — fixed text "Session " / " is still running statement " / ", so it cannot take another statement yet.", with <sessionId> taken from the request and <id> the in-flight statement id from emulator state. It is not even the modeled busy error. See services/service-com-amazonaws-glue-awsglue/src/ops/run_statement.rs lines 227-247 (the in-flight gate) and the state::messages::session_busy_with_statement helper it calls.
Why: every notebook-style client submits more than one statement per session; with this gate a session can hold at most one statement ever, so ListStatements can never return a second statement, submission ordering is unobservable, and clients see a validation error for a perfectly valid call.
Fix: the in-flight gate in the RunStatement handler (and the queueing/one-at-a-time execution it should hand to the session state instead of refusing).
Traces: glue.list-statements-ordering-by-creation, glue.list-statements-returns-submission-order (both raise InvalidInputException on the second RunStatement).

=== Cause 3 of 5: RunStatement on a PROVISIONING session answers the wrong error class and message ===
AWS: recorded behavior: RunStatement against a session that is still PROVISIONING is refused with InvalidInputException and the message "Session is not ready, session_id=<sessionId>, status=PROVISIONING" — fixed text "Session is not ready, session_id=" and ", status=", with <sessionId> from the request and the status token from the session's current state. Smithy: RunStatement declares both InvalidInputException and IllegalSessionStateException, so the model alone does not pick one; the recording does.
Emulator: IllegalSessionStateException with the message "Session <sessionId> is in PROVISIONING status, and a statement can only be run on a session in READY status." — a different error code and emulator-authored prose. See ops/run_statement.rs lines 216-226 and state::messages::session_not_ready.
Why: clients branch on the error code (retry-until-ready loops key on InvalidInputException here); an IllegalSessionStateException is treated as a terminal session-state fault, so the client aborts instead of retrying.
Fix: the session-status gate in the RunStatement handler and the message helper it uses.
Traces: glue.run-statement-on-busy-session-rejected (expected InvalidInputException, got 'IllegalSessionStateException').

=== Cause 4 of 5: GetStatement for an id the session never issued answers EntityNotFoundException where AWS answers IllegalSessionStateException ===
AWS: recorded behavior only (the model lists both errors for GetStatement and documents neither case): on a READY session that has issued statement N, GetStatement(SessionId=<session>, Id=N+1) is refused with IllegalSessionStateException whose message is exactly "Illegal Session Status" — fixed text with no variable part. (Hypothesis: AWS distinguishes an id inside/adjacent to the session's live id space, which it reports as a session-state problem, from an id far outside it, which it reports as not-found — see Cause 5, where Id=N+987654 on the same kind of session is refused EntityNotFoundException.)
Emulator: answers EntityNotFoundException for every unknown statement id, with its own message (see Cause 5), so this class of refusal can never be produced. See services/service-com-amazonaws-glue-awsglue/src/ops/get_statement.rs lines 191-203.
Why: a client that keys on IllegalSessionStateException to decide the session (not the id) is the problem never sees it, and gets a not-found it will treat as a permanently absent statement.
Fix: GetStatement's lookup-failure classification in its handler.
Traces: glue.get-statement-nonexistent-rejected (assert code == "IllegalSessionStateException", got 'EntityNotFoundException').

=== Cause 5 of 5: GetStatement's EntityNotFoundException message is emulator-authored ===
AWS: recorded behavior: GetStatement for a statement id far outside the session's issued id space is refused with EntityNotFoundException whose message is "Statement <id> not found." — fixed text "Statement " and " not found.", with <id> the statement id taken from the request and nothing else. Smithy: EntityNotFoundException.Message is "A message describing the problem" (MessageString), so the text is the requirement, not the shape.
Emulator: "Statement <id> not found in session <sessionId>." — the same leading text with an extra " in session <sessionId>" clause appended from the request's SessionId. Produced by state::messages::statement_not_found, called from ops/get_statement.rs lines 191-203.
Why: clients and test suites that match on Glue's refusal text (and anything that logs/diffs it) see a message real Glue never emits.
Fix: the statement-not-found message helper in this service's state messages module, used by the GetStatement handler.
Traces: glue.get-statement-nonexistent-not-found (expected message 'Statement 987655 not found.', got 'Statement 987655 not found in session sim2real-glue-sess-16529840e21d.').

## Evidence
Cause 1 — glue.e2e-interactive-session-statement-execution-flow:
  expected: Statement.State == 'AVAILABLE' with Output.Data.TextPlain carrying the computed value
  actual:   AssertionError: statement did not complete: {'Id': 1, 'Code': 'print(5997 * 3241)', 'State': 'ERROR', 'Output': {'ExecutionCount': 0, 'Status': 'ERROR', 'ErrorName': 'RuntimeUnavailable', 'ErrorValue': 'This emulator records statements but runs no Spark runtime, so the statement ended without running.'}, 'Progress': 0.0, ...}
  glue.stop-session-...-refuses-new-statements: 'the positive-control statement settled at ERROR, expected AVAILABLE. Output: {... ErrorName: RuntimeUnavailable ...}'
  glue.delete-session-while-statement-running-accepted: polls for State=RUNNING after submitting 'time.sleep(45)' -> 'Check timed out after 60.0s'.

Cause 2 — glue.list-statements-ordering-by-creation and glue.list-statements-returns-submission-order:
  expected: run_statement(Code='1 + 1') then run_statement(Code='2 + 2') on the same READY session both return ids; ListStatements returns them in submission order
  actual:   botocore.errorfactory.InvalidInputException: An error occurred (InvalidInputException) when calling the RunStatement operation: Session <sessionId> is still running statement 1, so it cannot take another statement yet.

Cause 3 — glue.run-statement-on-busy-session-rejected:
  expected: InvalidInputException, Message == 'Session is not ready, session_id=<sessionId>, status=PROVISIONING'
  actual:   IllegalSessionStateException, Message == 'Session <sessionId> is in PROVISIONING status, and a statement can only be run on a session in READY status.'

Cause 4 — glue.get-statement-nonexistent-rejected:
  expected: GetStatement(Id = issued_id + 1) -> IllegalSessionStateException, Message == 'Illegal Session Status'
  actual:   EntityNotFoundException

Cause 5 — glue.get-statement-nonexistent-not-found:
  expected: GetStatement(Id = issued_id + 987654) -> EntityNotFoundException, Message == 'Statement 987655 not found.'
  actual:   EntityNotFoundException, Message == 'Statement 987655 not found in session sim2real-glue-sess-16529840e21d.'

Handler evidence (checkout): services/service-com-amazonaws-glue-awsglue/src/ops/run_statement.rs lines 216-247 implement both the PROVISIONING refusal (IllegalSessionStateException + session_not_ready) and the in-flight refusal (InvalidInputException + session_busy_with_statement); services/service-com-amazonaws-glue-awsglue/src/ops/get_statement.rs lines 191-203 implement the single EntityNotFoundException + statement_not_found path.

## Steps to Reproduce
Common setup: create an IAM role trusted by glue.amazonaws.com, then glue.create_session(Id=<id>, Role=<role>, Command={'Name':'glueetl','PythonVersion':'3'}, GlueVersion='4.0'/'5.0') and poll glue.get_session until Status == 'READY'.

Cause 1: stmt = glue.run_statement(SessionId=<id>, Code='print(sum(range(1,101)))')['Id']; poll glue.get_statement(SessionId=<id>, Id=stmt)['Statement'] -> emulator settles State='ERROR' with Output.ErrorName='RuntimeUnavailable' and no Output.Data; AWS settles 'AVAILABLE' with Output.Data.TextPlain containing '5050'. Variant: submit Code='import time\ntime.sleep(45)' and poll for State=='RUNNING' — never observed.

Cause 2: on the same READY session call glue.run_statement(SessionId=<id>, Code='1 + 1') and immediately glue.run_statement(SessionId=<id>, Code='2 + 2'); the second call raises InvalidInputException 'Session <id> is still running statement 1, so it cannot take another statement yet.' instead of returning a second id. Then glue.list_statements(SessionId=<id>) can never hold both ids in submission order.

Cause 3: call glue.run_statement(SessionId=<id>, Code='1 + 1') immediately after create_session, while GetSession still reports PROVISIONING; inspect the error code and message.

Cause 4: on a READY session, n = glue.run_statement(SessionId=<id>, Code='1 + 1')['Id']; then glue.get_statement(SessionId=<id>, Id=n + 1) — expect IllegalSessionStateException 'Illegal Session Status', observe EntityNotFoundException.

Cause 5: on the same session, glue.get_statement(SessionId=<id>, Id=n + 987654) — expect EntityNotFoundException 'Statement <n+987654> not found.', observe the same code with ' in session <sessionId>' appended.
