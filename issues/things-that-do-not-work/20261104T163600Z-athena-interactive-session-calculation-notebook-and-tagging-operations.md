Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena.get-session-accessors-nonexistent-resource-not-found athena.get-session-endpoint-malformed-id-rejected athena.get-session-endpoint-unknown-session-not-found athena.session-endpoint-unknown-session-not-found athena.get-session-endpoint-status-nonexistent-resource-not-found athena.list-executors-nonexistent-session-not-found athena.get-calculation-execution-nonexistent-resource-not-found athena.calculation-ops-reject-unknown-calculation athena.create-presigned-notebook-url-nonexistent-notebook-not-found athena.athena-tag-resource-nonexistent-arn-not-found athena.op-contract-update-workgroup-nonexistent-rejected athena.data-catalog-update-type-immutable-rejected athena.delete-workgroup-with-active-queries-rejected athena.list-prepared-statements-unknown-workgroup-rejected athena.create-notebook-on-sql-workgroup-rejected athena.list-notebook-sessions-nonexistent-notebook-not-found athena.list-notebook-sessions-unknown-notebook-not-found athena.notebook-mutations-throttle-too-many-requests athena.concurrent-prepared-statement-create-race athena.get-resource-dashboard-nonexistent-resource-not-found athena.get-resource-dashboard-unknown-resource-not-found athena.calculation-execution-lifecycle-start-get-status-code-stop athena.session-endpoint-unavailable-before-session-ready athena.spark-calculation-returns-computed-stdout-result athena.terminate-session-already-terminated-fails athena.terminate-session-idempotent-after-terminated athena.terminate-session-idempotent-during-creating athena.terminate-session-never-existed-resource-not-found athena.telemetry-resource-dashboard-emitted-for-workgroup athena.e2e-query-lifecycle-dashboard-and-workgroup-teardown
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena.get-session-accessors-nonexistent-resource-not-found|run#6|AssertionError|assert not deviations, (
Issue-Key: athena.get-session-endpoint-malformed-id-rejected|run#4|AssertionError|assert malformed_code == "InvalidRequestException", (
Issue-Key: athena.get-session-endpoint-unknown-session-not-found|run#4|AssertionError|assert start_code == "InvalidRequestException", f"GOT={start_code!r}"
Issue-Key: athena.session-endpoint-unknown-session-not-found|run#4|AssertionError|assert code == "InvalidRequestException", (
Issue-Key: athena.get-session-endpoint-status-nonexistent-resource-not-found|run#4|AssertionError|assert code == "InvalidRequestException", f"GetSession: expected InvalidRequestException, got {code}"
Issue-Key: athena.list-executors-nonexistent-session-not-found|run#4|AssertionError|assert code == "InvalidRequestException", f"ListExecutors: expected InvalidRequestException, got {code}"
Issue-Key: athena.get-calculation-execution-nonexistent-resource-not-found|run#4|AssertionError|assert code == "InvalidRequestException", (
Issue-Key: athena.calculation-ops-reject-unknown-calculation|_start_spark_session#4|TimeoutError|raise TimeoutError("StartSession succeeding past role propagation: the run budget ended "
Issue-Key: athena.create-presigned-notebook-url-nonexistent-notebook-not-found|run#4|AssertionError|assert code == "InvalidRequestException", (
Issue-Key: athena.athena-tag-resource-nonexistent-arn-not-found|run#10|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Key: athena.op-contract-update-workgroup-nonexistent-rejected|run#4|AssertionError|assert code == "InvalidRequestException", f"expected InvalidRequestException, got {code}"
Issue-Key: athena.data-catalog-update-type-immutable-rejected|run#8|AssertionError|assert code == "InvalidRequestException", code
Issue-Key: athena.delete-workgroup-with-active-queries-rejected|run#14|AssertionError|assert code == "InvalidRequestException", f"expected delete of busy workgroup to be rejected, got {code}"
Issue-Key: athena.list-prepared-statements-unknown-workgroup-rejected|run#4|AssertionError|assert code == "InvalidRequestException", (
Issue-Key: athena.create-notebook-on-sql-workgroup-rejected|run#7|AssertionError|assert code == "InvalidRequestException", code
Issue-Key: athena.list-notebook-sessions-nonexistent-notebook-not-found|run#4|AssertionError|assert code == "InvalidRequestException", (
Issue-Key: athena.list-notebook-sessions-unknown-notebook-not-found|run#4|AssertionError|assert code == "InvalidRequestException", (
Issue-Key: athena.notebook-mutations-throttle-too-many-requests|run#27|AssertionError|assert not unexpected, f"unexpected error codes among concurrent CreateNotebook calls: {unexpected}"
Issue-Key: athena.concurrent-prepared-statement-create-race|run#12|AssertionError|assert successes >= 1, f"expected at least one CreatePreparedStatement to succeed, got {successes}"
Issue-Key: athena.get-resource-dashboard-nonexistent-resource-not-found|run#6|AssertionError|assert code == "InvalidRequestException", (
Issue-Key: athena.get-resource-dashboard-unknown-resource-not-found|run#6|AssertionError|assert code == "InvalidRequestException", f"GOT={code!r}"
Issue-Key: athena.calculation-execution-lifecycle-start-get-status-code-stop|_start_spark_session#7|Timeout|time.sleep(5)
Issue-Key: athena.session-endpoint-unavailable-before-session-ready|_start_session_with_retry#5|Timeout|time.sleep(5)
Issue-Key: athena.spark-calculation-returns-computed-stdout-result|_start_session_when_role_is_usable#10|Timeout|time.sleep(5)
Issue-Key: athena.terminate-session-already-terminated-fails|_start_session_with_retry#9|Timeout|time.sleep(interval_s)
Issue-Key: athena.terminate-session-idempotent-after-terminated|_start_session_with_retry#5|Timeout|time.sleep(interval_s)
Issue-Key: athena.terminate-session-idempotent-during-creating|_start_session_with_retry#9|Timeout|time.sleep(interval_s)
Issue-Key: athena.terminate-session-never-existed-resource-not-found|_start_session_with_retry#9|Timeout|time.sleep(interval_s)
Issue-Key: athena.telemetry-resource-dashboard-emitted-for-workgroup|_start_session_with_retry#19|Timeout|time.sleep(2)
Issue-Key: athena.e2e-query-lifecycle-dashboard-and-workgroup-teardown|_start_session_with_retry#7|Timeout|time.sleep(5)
Issue-Commit: 1b83187822ca52f98e00143d7b0b9c4d0812f404

# Athena interactive-session, calculation, notebook and tagging operations answer NotImplemented, so their modeled ResourceNotFound/InvalidRequest contracts never surface and session-lifecycle checks exhaust their budget (7 causes)

> _Note: this issue was identified against the code base at commit `1b83187822ca52f98e00143d7b0b9c4d0812f404`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSessionStatus

## Description
The Athena interactive-sessions / Spark-calculation / notebook surface and the resource-tagging surface answer every request with error code 'NotImplemented' and the body 'operation <Op> is scaffolded but not implemented in this emulator'. NotImplemented is not an error shape bound to any Athena operation in the service's Smithy model, so no SDK client can classify it; AWS answers each of these with a modeled ResourceNotFoundException or InvalidRequestException. 7 causes.

Separate scaffold-stub issues are already filed for StartSession, GetSession, CreateNotebook, ListNotebookSessions, GetResourceDashboard, CreatePreparedStatement, ListPreparedStatements, UpdateWorkGroup, UpdateDataCatalog, DeleteWorkGroup, TagResource and ListTagsForResource. The operations in causes 1-4 have no filed stub issue, and causes 5-7 add the error contracts those stubs must satisfy, which the stub issues do not state.

Cause 1 of 7: the session accessors GetSessionStatus, GetSessionEndpoint, TerminateSession and ListSessions are unimplemented.
AWS: Smithy: each of GetSessionStatus, GetSessionEndpoint and TerminateSession takes SessionId (required, @length 1..256) and binds InternalServerException, InvalidRequestException (httpError 400) and ResourceNotFoundException (httpError 400). For a SessionId that was never issued — or a malformed one — AWS refuses with HTTP 400 InvalidRequestException; for TerminateSession on a never-existed id the Message is the wholly fixed 'Not authorized to make this request.' On a real, freshly issued session TerminateSession returns State TERMINATING or TERMINATED (Smithy: TerminateSessionResponse.State, SessionState enum).
Emulator: Code 'NotImplemented', Message 'operation <Op> is scaffolded but not implemented in this emulator'.
Why: a client cannot distinguish 'no such session' from any other failure, and no session can be inspected or torn down, so session-based workloads leak resources.
Fix: the athena crate's session handlers — services/service-com-amazonaws-athena-amazonathena/src/ops/get_session_status.rs, get_session_endpoint.rs, terminate_session.rs, list_sessions.rs — plus the session state store they read.
Traces: athena.get-session-accessors-nonexistent-resource-not-found, athena.get-session-endpoint-malformed-id-rejected, athena.get-session-endpoint-unknown-session-not-found, athena.session-endpoint-unknown-session-not-found, athena.get-session-endpoint-status-nonexistent-resource-not-found.

Cause 2 of 7: ListExecutors is unimplemented.
AWS: Smithy: ListExecutors takes SessionId (required) and binds InvalidRequestException and ResourceNotFoundException (both httpError 400); output SessionId (required) + ExecutorsSummary + NextToken. For a SessionId that does not exist AWS refuses with HTTP 400 InvalidRequestException.
Emulator: Code 'NotImplemented'.
Why: a client cannot observe or size the executors backing a session, and cannot classify the absence of a session.
Fix: services/service-com-amazonaws-athena-amazonathena/src/ops/list_executors.rs and the session/executor state store.
Traces: athena.list-executors-nonexistent-session-not-found.

Cause 3 of 7: the calculation operations StartCalculationExecution, GetCalculationExecution and ListCalculationExecutions are unimplemented.
AWS: Smithy: GetCalculationExecution takes CalculationExecutionId (required, @length 1..36) and binds InvalidRequestException and ResourceNotFoundException (both httpError 400); its output carries Status, Statistics and Result. For a CalculationExecutionId that does not exist AWS refuses with HTTP 400 InvalidRequestException.
Emulator: Code 'NotImplemented' for all three.
Why: the Spark calculation data plane does not exist — no calculation can be started, polled, listed or diagnosed, and 'unknown calculation' is unclassifiable.
Fix: services/service-com-amazonaws-athena-amazonathena/src/ops/start_calculation_execution.rs, get_calculation_execution.rs, list_calculation_executions.rs and the calculation state store.
Traces: athena.get-calculation-execution-nonexistent-resource-not-found, athena.calculation-ops-reject-unknown-calculation.

Cause 4 of 7: CreatePresignedNotebookUrl is unimplemented.
AWS: Smithy: CreatePresignedNotebookUrl takes SessionId (required) and binds InvalidRequestException and ResourceNotFoundException (both httpError 400); output NotebookUrl, AuthToken and AuthTokenExpirationTime, all required. For a SessionId/notebook that does not exist AWS refuses with HTTP 400 InvalidRequestException.
Emulator: Code 'NotImplemented'.
Why: no notebook can be opened, and the not-found refusal a caller branches on never appears.
Fix: services/service-com-amazonaws-athena-amazonathena/src/ops/create_presigned_notebook_url.rs.
Traces: athena.create-presigned-notebook-url-nonexistent-notebook-not-found.

Cause 5 of 7: TagResource and ListTagsForResource must refuse an ARN of a missing resource with ResourceNotFoundException and AWS's message.
AWS: Smithy: TagResource takes ResourceARN (required, @length 1..1011) and Tags (required), and binds ResourceNotFoundException (httpError 400) alongside InvalidRequestException. For a well-formed ARN naming a workgroup that does not exist, AWS refuses with HTTP 400 ResourceNotFoundException and a Message of the fixed form 'Could not find valid resource from ARN: <ResourceARN>' — fixed text 'Could not find valid resource from ARN: ' plus the ResourceARN from the request. The same holds for ListTagsForResource; once a workgroup exists at that ARN both calls succeed.
Emulator: Code 'NotImplemented' — the modeled ResourceNotFoundException is never reachable.
Why: a tag-reconciliation tool cannot tell 'resource gone' (skip) from a service fault (retry), and ResourceNotFoundException is the only signal AWS gives it.
Fix: the TagResource / ListTagsForResource handlers (the already-filed stub issues name those files); this cause states the refusal contract they must implement.
Traces: athena.athena-tag-resource-nonexistent-arn-not-found.

Cause 6 of 7: the workgroup / data-catalog / prepared-statement mutation operations must refuse an absent target with InvalidRequestException, not NotImplemented.
AWS: UpdateWorkGroup on a workgroup that does not exist, UpdateDataCatalog that attempts to change an immutable Type, DeleteWorkGroup on a workgroup with active queries (without RecursiveDeleteOption), ListPreparedStatements for an unknown WorkGroup, CreateNotebook on a SQL-engine workgroup, and ListNotebookSessions for an unknown NotebookId are each refused with HTTP 400 InvalidRequestException. For ListNotebookSessions AWS's Message has the fixed form 'Notebook <NotebookId> was not found' — fixed text 'Notebook ' + the NotebookId from the request + ' was not found'. Concurrent CreateNotebook bursts are throttled with TooManyRequestsException, never with an unmodeled code.
Emulator: every one of these answers Code 'NotImplemented'.
Why: the whole control-plane mutation surface is unusable and its refusal classes are indistinguishable, so no client can implement create-or-update, safe-delete or throttle-backoff logic.
Fix: the respective op handlers (already filed as scaffold stubs); this cause states the refusal class and message each must produce.
Traces: athena.op-contract-update-workgroup-nonexistent-rejected, athena.data-catalog-update-type-immutable-rejected, athena.delete-workgroup-with-active-queries-rejected, athena.list-prepared-statements-unknown-workgroup-rejected, athena.create-notebook-on-sql-workgroup-rejected, athena.list-notebook-sessions-nonexistent-notebook-not-found, athena.list-notebook-sessions-unknown-notebook-not-found, athena.notebook-mutations-throttle-too-many-requests, athena.concurrent-prepared-statement-create-race, athena.get-resource-dashboard-nonexistent-resource-not-found, athena.get-resource-dashboard-unknown-resource-not-found.

Cause 7 of 7: because StartSession refuses, every session-lifecycle and Spark-calculation check spins on its retry loop until the harness budget ends.
AWS: StartSession on a PySpark workgroup with an ExecutionRole returns a SessionId and the session progresses to IDLE within the retry budget, after which GetSessionStatus/GetSessionEndpoint/TerminateSession/StartCalculationExecution all answer.
Emulator: StartSession answers Code 'NotImplemented', so the clients' role-propagation retry loops never obtain a SessionId and the checks run out their 60 s budget without ever reaching an assertion.
Why: no Spark/session workload can start at all; the failure presents as a hang rather than a classifiable refusal, which is the worst outcome for a caller with a timeout.
Fix: the StartSession handler (already filed as a scaffold stub) together with the session state machine the accessors in causes 1-3 read; these checks are the acceptance evidence that the whole session lifecycle works end to end.
Traces: athena.calculation-execution-lifecycle-start-get-status-code-stop, athena.session-endpoint-unavailable-before-session-ready, athena.spark-calculation-returns-computed-stdout-result, athena.terminate-session-already-terminated-fails, athena.terminate-session-idempotent-after-terminated, athena.terminate-session-idempotent-during-creating, athena.terminate-session-never-existed-resource-not-found, athena.telemetry-resource-dashboard-emitted-for-workgroup, athena.e2e-query-lifecycle-dashboard-and-workgroup-teardown.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout.

Cause 1 — athena.get-session-accessors-nonexistent-resource-not-found: 'expected InvalidRequestException from every accessor on a never-issued SessionId; deviations: {GetSession: NotImplemented, GetSessionStatus: NotImplemented, GetSessionEndpoint: NotImplemented, TerminateSession: NotImplemented}'. athena.get-session-endpoint-malformed-id-rejected: 'expected a malformed SessionId to raise InvalidRequestException, got NotImplemented'. athena.get-session-endpoint-unknown-session-not-found and athena.session-endpoint-unknown-session-not-found: same, 'got NotImplemented'. A companion check also records ListSessions: ('NotImplemented', 'operation ListSessions is scaffolded but not implemented in this emulator').
Cause 2 — athena.list-executors-nonexistent-session-not-found: 'ListExecutors: expected InvalidRequestException, got NotImplemented'.
Cause 3 — athena.get-calculation-execution-nonexistent-resource-not-found: 'GetCalculationExecution: expected InvalidRequestException, got NotImplemented'; StartCalculationExecution and ListCalculationExecutions likewise record ('NotImplemented', 'operation ... is scaffolded but not implemented in this emulator').
Cause 4 — athena.create-presigned-notebook-url-nonexistent-notebook-not-found: 'CreatePresignedNotebookUrl: expected InvalidRequestException, got NotImplemented'.
Cause 5 — athena.athena-tag-resource-nonexistent-arn-not-found, TagResource(ResourceARN='arn:aws:athena:<region>:<account>:workgroup/<absent name>', Tags=[{k,v}]): expected Code='ResourceNotFoundException' and Message='Could not find valid resource from ARN: <that ARN>'; actual Code='NotImplemented'. ListTagsForResource on the same ARN: identical.
Cause 6 — athena.op-contract-update-workgroup-nonexistent-rejected: 'expected InvalidRequestException, got NotImplemented'. athena.delete-workgroup-with-active-queries-rejected: 'expected delete of busy workgroup to be rejected, got NotImplemented'. athena.list-prepared-statements-unknown-workgroup-rejected: 'expected InvalidRequestException for unknown workgroup <name>, got NotImplemented'. athena.list-notebook-sessions-unknown-notebook-not-found: 'expected ListNotebookSessions on an unknown NotebookId to raise InvalidRequestException, got NotImplemented' (a companion check records the expected pair ('InvalidRequestException', 'Notebook <id> was not found')). athena.notebook-mutations-throttle-too-many-requests: all 90+ concurrent CreateNotebook calls returned 'NotImplemented'. athena.concurrent-prepared-statement-create-race: 'expected at least one CreatePreparedStatement to succeed, got 0'. athena.get-resource-dashboard-{nonexistent,unknown}-resource-not-found: 'expected InvalidRequestException, got NotImplemented'.
Cause 7 — nine checks end with 'Error: Check timed out after 60.0s'; athena.calculation-ops-reject-unknown-calculation reports 'TimeoutError: StartSession succeeding past role propagation: the run budget ended first'. The retry loops they hang in call StartSession, which answers 'NotImplemented'.

Model check (services/service-com-amazonaws-athena-amazonathena/smithy-model): GetSessionStatus, GetSessionEndpoint, TerminateSession, ListExecutors, GetCalculationExecution and CreatePresignedNotebookUrl each bind exactly InternalServerException, InvalidRequestException and ResourceNotFoundException; TagResource binds InternalServerException, InvalidRequestException and ResourceNotFoundException. None binds a 'NotImplemented' shape.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay suite simparity-behavioral-aws-v1 and inspect the athena checks listed in trace_ids.
3. Minimal manual repro (every call below currently answers Code='NotImplemented'):
   a. GetSessionStatus / GetSessionEndpoint / TerminateSession / ListExecutors with SessionId=str(uuid4()) — expect 400 InvalidRequestException (TerminateSession Message 'Not authorized to make this request.').
   b. GetCalculationExecution(CalculationExecutionId=<random>) — expect 400 InvalidRequestException.
   c. CreatePresignedNotebookUrl(SessionId=<random>) — expect 400 InvalidRequestException.
   d. TagResource(ResourceARN='arn:aws:athena:<region>:<account>:workgroup/<absent>', Tags=[{'Key':'k','Value':'v'}]) and ListTagsForResource on the same ARN — expect 400 ResourceNotFoundException with Message 'Could not find valid resource from ARN: <that ARN>'; then CreateWorkGroup at that name and expect both to succeed.
   e. UpdateWorkGroup on an absent workgroup; DeleteWorkGroup on a workgroup with a running query and no RecursiveDeleteOption; ListPreparedStatements for an absent WorkGroup; ListNotebookSessions(NotebookId=<random>) — each expect 400 InvalidRequestException (ListNotebookSessions Message 'Notebook <id> was not found').
   f. CreateWorkGroup with Configuration.EngineVersion.SelectedEngineVersion='PySpark engine version 3' and an ExecutionRole, then StartSession on it — expect a SessionId and progression to IDLE; observe NotImplemented, which is what makes the nine 60 s timeouts.
