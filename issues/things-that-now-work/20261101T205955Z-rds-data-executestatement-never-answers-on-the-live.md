Issue-Source: trace-sweep-divergence
Issue-Trace: rdsdata-execute-statement-live-cluster-round-trip
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rdsdata-execute-statement-live-cluster-round-trip
Issue-Shape-Id: com.amazonaws.rdsdata#RdsDataService
Issue-Key: rds-data|ExecuteStatement|0->None|$
Issue-Key: rds-data|ExecuteStatement|0->None|status
Issue-Commit: 92d1533bc984843777edac81f6082914baeb07b3

# rds-data ExecuteStatement never answers on the live-cluster round trip: the call hangs until the fixture budget is spent instead of returning a bounded result or StatementTimeoutException

> _Note: this issue was identified against the code base at commit `92d1533bc984843777edac81f6082914baeb07b3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
Cause 1 of 1: an ExecuteStatement request on the live-cluster round-trip flow never completes — the emulator produces no HTTP response at all for the step, and the fixture's entire 600s budget is consumed on it, which strands every later step of that fixture.

AWS: Smithy (services/service-com-amazonaws-rdsdata-rdsdataservice/smithy-model/com-amazonaws-rdsdata-rdsdataservice.json): ExecuteStatement is POST /Execute with http code 200, and every outcome it can have is a bounded HTTP response. The RDS Data API call is explicitly time-bounded: ExecuteStatementRequest.continueAfterTimeout is documented as "A value that indicates whether to continue running the statement after the call times out. By default, the statement stops running when the call times out", and the operation models StatementTimeoutException (errorKind client, httpError 400, doc "The execution of the SQL statement timed out.") for exactly that case, plus DatabaseResumingException (client, 400), DatabaseUnavailableException (server, 504) and ServiceUnavailableError (server, 503) for a cluster that is not yet able to serve. So AWS always answers this operation within its call timeout: either a 200 result envelope, or one of those modeled statuses. It never leaves the request open indefinitely. The fixture recorded no response assertion for this step (recorded behavior only: the replay reports expected status as empty), so the requirement being violated is the bounded-response contract itself rather than a particular body.

Emulator: the replay of rdsdata-execute-statement-live-cluster-round-trip#task#8 reports actual_status null with detail "status: expected , got None" and "$: fixture budget 600s spent" — no status line, no headers, no body ever arrive for the request, and the 600-second per-fixture budget is exhausted while waiting on this one call. Steps task#1..task#7 of the same fixture were answered normally, so the front door is up and reachable (the replay confirms /__admin/health answers 200 and the data listener accepts); it is this ExecuteStatement that does not return. The resulting stall blocked the remainder of the fixture — the run reports 21 blocked steps, which includes the later steps of this fixture (for example task#11, task#15 and task#22, which earlier runs did reach and which the open issue 20261023T045849Z-rds-data-skips-resource-secret-and-transaction-preconditions cites).

Hypothesis: the handler waits on something that is never satisfied for this flow — a lock held across statement execution, or a wait for a cluster/DB-instance readiness condition the emulator's state never reaches — rather than enforcing a call timeout. The replay output does not state the mechanism, only that no response is produced.

Why: a client calling ExecuteStatement against this emulator can hang forever on a request AWS would have terminated at its call timeout with 400 StatementTimeoutException (or refused with 400 DatabaseResumingException / 503 / 504). SDK retry and circuit-breaker logic is driven by those responses, so against the emulator the call simply never returns: CI jobs and readiness loops wedge until an external timeout kills them, and no amount of correct client handling recovers. It is strictly worse than a wrong answer — the operation is unusable and it takes every subsequent call in the same flow with it, so all downstream fidelity of this fixture is unverifiable.

Note on existing issues: no open rds-data issue covers this step. 20261023T045849Z-rds-data-skips-resource-secret-and-transaction-preconditions covers the precondition/error-classification gaps of this service (and names task#22, task#11 and task#15 of this same fixture), 20261023T045849Z-rds-data-result-fidelity-postgresql-statements-rejected-column covers result/column-metadata fidelity, and 20261029T191739Z-rds-data-committransaction-skips-the-http-endpoint-enabled covers CommitTransaction's endpoint gate. None of them concerns a request that produces no response at all; the replay reports this cluster as unowned.

Fix: the ExecuteStatement handler of the rds-data service — statement execution must be bounded by a call timeout that produces a modeled response (StatementTimeoutException at 400, honouring continueAfterTimeout's default of stopping the statement), and any readiness/lock wait in that path must fail fast with the modeled DatabaseResumingException / DatabaseUnavailableException / ServiceUnavailableError instead of blocking the request.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator booted from this checkout (emulator_up true, /__admin/health 200, data listener accepting).

Divergence cluster service=rds-data|ExecuteStatement|status=0->None|keys=$,status, verdict "mismatch", stub_path empty, owned_by empty:
  fixture_id: rdsdata-execute-statement-live-cluster-round-trip
  step_key:   task#8
  expected_status: (none recorded)   actual_status: null
  detail: "status: expected , got None" / "$: fixture budget 600s spent (expected None, got None)"
  affected: [rdsdata-execute-statement-live-cluster-round-trip#task#8] (affected_count 1)

Expected (AWS): one bounded HTTP response for POST /Execute — 200 with an ExecuteStatementResponse envelope, or a modeled refusal: 400 StatementTimeoutException ("The execution of the SQL statement timed out."), 400 DatabaseResumingException, 504 DatabaseUnavailableException, 503 ServiceUnavailableError. Fixed text of the timeout class is AWS's own sentence; it carries no request- or state-derived values.
Actual (emulator): no status, no x-amzn-errortype header and no body are ever emitted; the request remains outstanding until the fixture's 600s budget is exhausted.

Blast radius in the same run: steps_blocked 21 — the later steps of this fixture never ran because the budget was spent here.

## Steps to Reproduce
1. Clone this checkout and boot the emulator from it.
2. Replay wire trace set simparity-wire-aws-v1.
3. Run fixture rdsdata-execute-statement-live-cluster-round-trip. Steps up to task#7 are answered normally.
4. At step task#8, POST /Execute (ExecuteStatement) for the live-cluster round trip: observe that no HTTP response is returned at all — the replay records actual status None and reports "fixture budget 600s spent" on that step, and the remaining steps of the fixture are blocked.
5. Compare with AWS, which answers the same call within its call timeout: either 200 with an ExecuteStatementResponse, or a modeled refusal such as 400 StatementTimeoutException ("The execution of the SQL statement timed out.").
