Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.update-map-run-validation-exception stepfunctions.map-run-tolerated-failure-enforced
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.update-map-run-validation-exception|_error#6|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: stepfunctions.map-run-tolerated-failure-enforced|run#16|AssertionError|assert after_first["status"] == "RUNNING", (
Issue-Commit: 5dcd1bd7969c2f584c7eb42c4a26285019dddcd0

# Step Functions UpdateMapRun: a no-field update is accepted with 200 instead of ValidationException, and toleratedFailureCount is never enforced server-side (2 causes)

> _Note: this issue was identified against the code base at commit `5dcd1bd7969c2f584c7eb42c4a26285019dddcd0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateMapRun

## Description
Two net-new root causes in the Step Functions Map-Run surface, both grounded in failing behavioral checks the replay report left UNMARKED (no ALREADY-FILED and no OWNED marker). Both are in service `sfn`, which get_bootstrap_status reports as bootstrapped (service-com-amazonaws-sfn-awsstepfunctions).

Cause 1 of 2 — UpdateMapRun with ONLY mapRunArn is accepted and returns HTTP 200 with an empty body, instead of being refused with ValidationException.
Failing check id: sfn.update-map-run-validation-exception.
AWS: Smithy (services/service-com-amazonaws-sfn-awsstepfunctions/smithy-model/com-amazonaws-sfn-awsstepfunctions.json) models UpdateMapRun input as {mapRunArn (required, LongArn, @length 1..2000), maxConcurrency (default null, @range min 0), toleratedFailurePercentage (default null, @range 0..100), toleratedFailureCount (default null, @range min 0)}, output UpdateMapRunOutput with no members, and modeled errors InvalidArn (400), ResourceNotFound (404) and ValidationException (400). All three threshold members are optional with a null default, so the "at least one of" rule is the operation's own rule: when NONE of maxConcurrency / toleratedFailureCount / toleratedFailurePercentage is supplied the request carries no update at all and AWS refuses it with HTTP 400, error code ValidationException. Recorded message (the requirement is the rule; this wording is its fixed text, and it has no variable parts — no name, arn, limit or value is interpolated): "Either the max concurrency, the tolerated failure count, or the tolerated failure percentage must be specified".
Emulator: the call SUCCEEDS. The replay shows HTTPStatusCode 200, content-type application/x-amz-json-1.0, content-length 2 (i.e. the empty `{}` UpdateMapRunOutput), and the check's `_error()` helper raises `AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}`. No error code and no 4xx is produced at all.
Why: a client that mis-builds an UpdateMapRun request (for example by passing a threshold value that its own config left unset/None, which the SDK then omits from the wire body) gets a silent success and believes the running Map Run's concurrency/tolerance was retuned, when nothing changed. There is no way to distinguish "update applied" from "update was empty", and `except sfn.exceptions.ValidationException` handlers never fire.
Fix: UpdateMapRun's request validation in its handler (services/service-com-amazonaws-sfn-awsstepfunctions/src/ops/update_map_run.rs) — refuse with ValidationException (HTTP 400) before touching map-run state when none of the three threshold members is present.
Related but NOT covering this: open issue 20261025T075213Z-sfn-step-functions-activity-tasks-are-never-delivered-to.md recorded this check when the same call was refused with an unmodeled `NotImplemented` code; the behavior has since changed to a silent 200, the replay key for this check no longer matches that issue, and the report did not mark the check as owned by it. The fixed issue 20261025T075213Z-sfn-updatemaprun-stub.md implemented the operation; this is not a refusal, so it is reported as a net-new validation gap rather than a reopen.
Traces: sfn.update-map-run-validation-exception.

Cause 2 of 2 — toleratedFailureCount is accepted and echoed by DescribeMapRun but is not enforced by the Map Run runtime: the Map Run flips to FAILED on the FIRST failed item even though the tolerance permits it.
Failing check id: sfn.map-run-tolerated-failure-enforced.
AWS: Smithy models DescribeMapRun output with required status (MapRunStatus), maxConcurrency, toleratedFailurePercentage, toleratedFailureCount and itemCounts (MapRunItemCounts), and UpdateMapRun as the operation that retunes them on a RUNNING Map Run. The threshold is a server-side failure budget: while the number of failed child items is within toleratedFailureCount (and within toleratedFailurePercentage), the Map Run stays RUNNING and keeps dispatching remaining items; the Map Run only transitions to FAILED once a failure BREACHES the budget, and that failure then propagates to the parent execution (status FAILED). Recorded behavior for this scenario: with toleratedFailureCount=1 and MaxConcurrency=1 over two items that both end in a Fail state, DescribeMapRun reports status RUNNING with itemCounts.failed == 1 after the first item fails, then status FAILED with itemCounts.failed >= 2 after the second, and the parent execution ends FAILED.
Emulator: UpdateMapRun(mapRunArn, toleratedFailureCount=1, maxConcurrency=1) is accepted and the following DescribeMapRun does report toleratedFailureCount == 1 (that assertion passes), but as soon as the first child item fails the Map Run is already terminal: the check polls DescribeMapRun until itemCounts.failed >= 1 and gets status FAILED, raising `AssertionError: map run should tolerate 1 failure and stay RUNNING, got FAILED`. The stored tolerance is therefore cosmetic — the runtime treats any failed item as fatal to the Map Run.
Why: a client using a Distributed Map with a failure budget (the standard pattern for bulk item processing: tolerate N or X% bad records, process the rest, surface the failed items at the end) loses every item after the first failure. The run aborts early, the remaining items are never dispatched, the parent execution fails when AWS would have succeeded or failed later, and raising tolerance via UpdateMapRun has no observable effect on the outcome.
Fix: the Map Run state/runtime that evaluates child-execution outcomes — the failure-budget evaluation must compare itemCounts.failed against the Map Run's current toleratedFailureCount / toleratedFailurePercentage (as mutated by UpdateMapRun) and keep status RUNNING while the count is within budget, transitioning to FAILED only on the breaching failure and only then failing the parent execution.
Related but NOT covering this: open issues 20261025T053834Z-sfn-step-functions-distributed-map-creates-no-map-run.md and 20261025T161834Z-sfn-step-functions-distributed-map-spawns-no-child-executions.md describe Map Runs/children not being created at all; in this check a Map Run IS listed, reaches RUNNING and accumulates a failed item count, so the divergence here is the tolerance evaluation, and the report did not mark this check as owned by either issue.
Traces: sfn.map-run-tolerated-failure-enforced.

## Evidence
Cause 1 — sfn.update-map-run-validation-exception, verdict mismatch.
Expected (AWS): UpdateMapRun(mapRunArn=<RUNNING map run>) with no maxConcurrency / toleratedFailureCount / toleratedFailurePercentage -> HTTP 400, Error.Code "ValidationException", Error.Message "Either the max concurrency, the tolerated failure count, or the tolerated failure percentage must be specified" (fixed text, no interpolated values).
Actual (emulator, from the replay traceback):
  File "check_sfn_update_map_run_validation_exception.py", line 227, in run
    code, message = _error(lambda: sfn.update_map_run(mapRunArn=map_run_arn))
  File "check_sfn_update_map_run_validation_exception.py", line 86, in _error
    raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
  AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.0', 'content-length': '2', ...}, 'RetryAttempts': 0}}
So: expected 400/ValidationException, got 200 with an empty body and no error code.

Cause 2 — sfn.map-run-tolerated-failure-enforced, verdict mismatch.
Setup: STANDARD state machine with a single Map state, MaxConcurrency 1, ItemProcessor ProcessorConfig {Mode: DISTRIBUTED, ExecutionType: STANDARD} whose per-item flow is Wait(SecondsPath $.delaySeconds) -> Choice on $.outcome -> Fail(Error "ProbeError"); input items [{outcome: fail, delaySeconds: 20}, {outcome: fail, delaySeconds: 9}]. After the Map Run is observed RUNNING, UpdateMapRun(mapRunArn, toleratedFailureCount=1, maxConcurrency=1) is called and DescribeMapRun confirms toleratedFailureCount == 1.
Expected (AWS): DescribeMapRun after the first item fails -> status RUNNING, itemCounts.failed == 1; after the second -> status FAILED, itemCounts.failed >= 2; DescribeExecution(parent) -> FAILED.
Actual (emulator, from the replay traceback):
  File "check_sfn_map_run_tolerated_failure_enforced.py", line 165, in run
    assert after_first["status"] == "RUNNING", (
  AssertionError: map run should tolerate 1 failure and stay RUNNING, got FAILED
So the Map Run was already FAILED at the first poll where itemCounts.failed >= 1, i.e. the accepted toleratedFailureCount=1 budget was not applied.

## Steps to Reproduce
Cause 1 (sfn.update-map-run-validation-exception):
1. iam:CreateRole with a trust policy for states.amazonaws.com and an inline policy allowing states:StartExecution, states:DescribeExecution, states:StopExecution on "*".
2. sfn:CreateStateMachine (type STANDARD) with a single Map state, MaxConcurrency 1, ItemProcessor ProcessorConfig {"Mode":"DISTRIBUTED","ExecutionType":"STANDARD"} whose only state is {"Type":"Wait","Seconds":10,"End":true}, ItemsPath "$.items".
3. sfn:StartExecution with input {"items":[1,2]}; poll sfn:ListMapRuns(executionArn) and sfn:DescribeMapRun until status == "RUNNING" to get <mapRunArn>.
4. sfn:UpdateMapRun(mapRunArn=<mapRunArn>) and pass NO maxConcurrency, toleratedFailureCount or toleratedFailurePercentage.
5. Expected HTTP 400 with Error.Code "ValidationException" and the message above; the emulator answers HTTP 200 with an empty body.

Cause 2 (sfn.map-run-tolerated-failure-enforced):
1. Same role as above (states:StartExecution on arn:aws:states:<region>:<account>:stateMachine:*, states:DescribeExecution/StopExecution on arn:aws:states:<region>:<account>:execution:*:*).
2. sfn:CreateStateMachine (STANDARD) with Map state, ItemsPath "$.items", MaxConcurrency 1, ItemProcessor {Mode DISTRIBUTED, ExecutionType STANDARD}: WaitItem {Type Wait, SecondsPath "$.delaySeconds"} -> CheckItem {Type Choice, $.outcome StringEquals "fail" -> FailItem} -> FailItem {Type Fail, Error "ProbeError"}; Default SucceedItem.
3. sfn:StartExecution with input {"items":[{"outcome":"fail","delaySeconds":20},{"outcome":"fail","delaySeconds":9}]}.
4. Poll sfn:ListMapRuns / sfn:DescribeMapRun until the Map Run is RUNNING, then sfn:UpdateMapRun(mapRunArn, toleratedFailureCount=1, maxConcurrency=1) and confirm DescribeMapRun reports toleratedFailureCount == 1.
5. Poll sfn:DescribeMapRun until itemCounts.failed >= 1. Expected status "RUNNING" (first failure is within the budget); the emulator reports "FAILED".
6. (unreachable today) Poll until itemCounts.failed >= 2: expect status FAILED and the parent sfn:DescribeExecution to report FAILED only at that point.
