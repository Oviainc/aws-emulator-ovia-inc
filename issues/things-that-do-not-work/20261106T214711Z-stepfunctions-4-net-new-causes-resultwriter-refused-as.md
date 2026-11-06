Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.conc-distributed-map-200-s3-objects-processed-once-under-concurrency-cap stepfunctions.conc-distributed-map-tolerated-failure-percentage-succeeds-and-names-failed-items stepfunctions.conc-distributed-map-accepts-items-whose-inline-map-history-limit-fails stepfunctions.conc-distributed-map-zero-tolerance-run-fails-and-keeps-completed-items stepfunctions.sfn-result-selector-and-result-path-compose-over-a-real-sdk-task
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.conc-distributed-map-200-s3-objects-processed-once-under-concurrency-cap|run#15|AssertionError|assert exe["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.conc-distributed-map-tolerated-failure-percentage-succeeds-and-names-failed-items|run#10|AssertionError|assert exe["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.conc-distributed-map-accepts-items-whose-inline-map-history-limit-fails|run#8|AssertionError|assert dist["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.conc-distributed-map-zero-tolerance-run-fails-and-keeps-completed-items|_wait_terminal#9|Timeout|time.sleep(interval_s)
Issue-Key: stepfunctions.sfn-result-selector-and-result-path-compose-over-a-real-sdk-task|run#16|AssertionError|assert role_name in output["identity"]["Arn"], (
Issue-Commit: e20658567def5986c911ca4169de4ecb96db84bd

# stepfunctions: 4 net-new causes — ResultWriter refused as uninterpretable, a 1000-state-transition emulator cap fails DISTRIBUTED maps, a zero-tolerance map run never terminates, and an aws-sdk Task runs as the emulator user instead of the execution role

> _Note: this issue was identified against the code base at commit `e20658567def5986c911ca4169de4ecb96db84bd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Four net-new root causes behind 5 failing checks.

Cause 1 of 4: the `ResultWriter` member of a DISTRIBUTED Map state is not interpreted, and the execution is failed instead of the manifest being written.
AWS: a Map state with `ProcessorConfig.Mode=DISTRIBUTED`, an `ItemReader` and a `ResultWriter` runs one child execution per item and writes the run's manifest and per-status result objects under the configured bucket/prefix; the execution ends SUCCEEDED and DescribeMapRun agrees with the manifest (Smithy: DescribeMapRun -> {status: MapRunStatus, itemCounts, executionCounts, maxConcurrency, toleratedFailurePercentage, toleratedFailureCount} in services/service-com-amazonaws-sfn-awsstepfunctions; the ASL members are recorded behavior only). AWS never fails an execution for using a valid ASL member, and `States.Runtime` denotes a runtime fault in the definition.
Emulator: `status='FAILED'`, `error='States.Runtime'`, `cause="state 'Fan' carries the member 'ResultWriter', which this emulator does not interpret, so nothing was run for it"` — emulator-authored fixed text whose only request-derived values are the state name and the member name.
Traces: stepfunctions.conc-distributed-map-200-s3-objects-processed-once-under-concurrency-cap, stepfunctions.conc-distributed-map-tolerated-failure-percentage-succeeds-and-names-failed-items.

Cause 2 of 4: an emulator-internal cap of 1000 state transitions aborts executions AWS runs to completion, and in particular makes a DISTRIBUTED Map fail where its whole purpose is to exceed the inline limits.
AWS: a DISTRIBUTED Map is explicitly the escape from the inline Map's per-execution history limit: each item is processed in its OWN child execution, so the parent's history and transition counts do not grow with the item count. The equivalent INLINE Map over the same 3200 items fails `States.Runtime` with cause `The execution reached the maximum number of history events (25000).` — AWS's fixed text for that quota — while the DISTRIBUTED twin (differing only in `ProcessorConfig.Mode`) SUCCEEDS.
Emulator: the DISTRIBUTED run ends `status='FAILED'`, `error='States.Runtime'`, `cause='the execution made more than the 1000 state transitions this emulator runs, so it was stopped'` — an emulator-specific cap (`1000`) with emulator-specific wording that exists in no AWS quota and is applied to the parent regardless of Map mode.
Traces: stepfunctions.conc-distributed-map-accepts-items-whose-inline-map-history-limit-fails.

Cause 3 of 4: a DISTRIBUTED Map run with `ToleratedFailurePercentage=0` never reaches a terminal state.
AWS: with tolerance 0, the first failing child fails the map and the execution ends FAILED with error `States.ExceedToleratedFailureThreshold`, while the children that already completed keep their side effects (no rollback); the same machine at 10% tolerance SUCCEEDS with the same completed rows.
Emulator: the check is killed at `Check timed out after 60.0s` — neither the refusal nor the tolerant twin ever settles.
Traces: stepfunctions.conc-distributed-map-zero-tolerance-run-fails-and-keeps-completed-items.

Cause 4 of 4: an `arn:aws:states:::aws-sdk:*` Task is not executed under the state machine's execution role — it is executed as the emulator's own principal.
AWS: an AWS SDK service integration is called with credentials derived from the state machine's `roleArn`, so `arn:aws:states:::aws-sdk:sts:getCallerIdentity` returns an `Arn` of the assumed-role session for that role (`arn:aws:sts::<account>:assumed-role/<role-name>/<session>`), and the role's grants are what authorize the call.
Emulator: the task's result carries `identity.Arn = 'arn:aws:iam::000000000000:user/emulator'` — a fixed emulator principal that is neither an assumed-role session nor derived from the state machine's role. Consequently every SDK integration runs with the emulator's privileges rather than the role's.
Traces: stepfunctions.sfn-result-selector-and-result-path-compose-over-a-real-sdk-task.

Why: a client relying on AWS's behavior finds Distributed Map unusable (valid definitions refused with a synthetic `States.Runtime`, no manifest, and the whole point of DISTRIBUTED mode defeated by a parent-level transition cap), finds large runs non-terminal so `ToleratedFailurePercentage` and `States.ExceedToleratedFailureThreshold` are unobservable, and — most seriously — finds that the execution role is not the identity doing the work: every least-privilege assumption about an SDK Task is wrong, denials never occur, and anything keyed on the caller identity (ABAC, resource policies, CloudTrail-style attribution) sees the wrong principal.

Fix: the sfn service crate's Map-state handler and map-run state (interpret ItemReader/ResultWriter and write the manifest; keep per-item work in child executions so the parent's transition/history accounting does not scale with item count; drive map runs to a terminal state and surface `States.ExceedToleratedFailureThreshold` when tolerance is exceeded), the interpreter's execution scheduler (remove the fixed 1000-transition cap and apply AWS's real quotas with AWS's wording — e.g. the 25,000-history-event cause text — only where AWS applies them), and the Task-state integration dispatcher's credential resolution (assume the state machine's roleArn and call the SDK integration as that assumed-role session). The open issue 20261024T043150Z-sfn-step-functions-control-plane-no-iam-passrole-authorization.md covers control-plane PassRole and 20261025T053834Z-sfn-step-functions-distributed-map-creates-no-map-run.md covers an empty ListMapRuns under the `sfn` heading; neither carries these checks' replay keys.

## Evidence
Cause 1: `AssertionError: execution was 'FAILED': 'States.Runtime' "state 'Fan' carries the member 'ResultWriter', which this emulator does not interpret, so nothing was run for it"` (200-object run) and `AssertionError: at ToleratedFailurePercentage=10 with 5% failing the run was 'FAILED': 'States.Runtime' "state 'Fan' carries the member 'ResultWriter', ..."`. Expected SUCCEEDED with the manifest written and DescribeMapRun agreeing.

Cause 2: `AssertionError: the DISTRIBUTED Map over 3200 items was 'FAILED': 'States.Runtime' 'the execution made more than the 1000 state transitions this emulator runs, so it was stopped'`. Expected: the DISTRIBUTED twin SUCCEEDS; only the INLINE twin fails, with AWS's cause `The execution reached the maximum number of history events (25000).` Fixed text vs values: AWS's history-limit cause is fixed text with the quota 25000 in it; the emulator's `1000 state transitions` wording is entirely emulator-invented.

Cause 3: expected FAILED with `error=States.ExceedToleratedFailureThreshold` and the 19 completed items' rows retained, plus a 10%-tolerance twin that SUCCEEDS with the same 19 rows. Actual: `Error: Check timed out after 60.0s`.

Cause 4: `AssertionError: the SDK call was not made under the state machine role 'sim2real-behav-sfnrole-<id>': the identity Arn was 'arn:aws:iam::000000000000:user/emulator'`. Expected an assumed-role session ARN for the state machine's roleArn; the role name is a per-run value, the requirement is that the Arn denotes a session of the execution role, not a fixed emulator user.

## Steps to Reproduce
Cause 1: put 200 objects under a prefix; CreateStateMachine (STANDARD) with a Map state ProcessorConfig.Mode=DISTRIBUTED, MaxConcurrency=5, ItemReader over the prefix and a ResultWriter to a results prefix; StartExecution and poll DescribeExecution. Expected SUCCEEDED; observed FAILED States.Runtime naming ResultWriter. Repeat with 40 items, 2 failing, ToleratedFailurePercentage=10.

Cause 2: build two state machines over 3200 items differing only in ProcessorConfig.Mode (INLINE vs DISTRIBUTED), 3 Pass states per item; StartExecution on both. Expected: INLINE fails with the 25,000-history-event cause, DISTRIBUTED SUCCEEDS; observed DISTRIBUTED FAILED with a 1000-state-transition cause.

Cause 3: run a DISTRIBUTED Map over 20 items with 1 failing and ToleratedFailurePercentage=0, then the same machine at 10%. Expected FAILED States.ExceedToleratedFailureThreshold with the 19 rows kept, and SUCCEEDED for the 10% twin; observed the check never settles within 60 s.

Cause 4: create a role trusted by states.amazonaws.com with sts:GetCallerIdentity; CreateStateMachine with one Task Resource=arn:aws:states:::aws-sdk:sts:getCallerIdentity, ResultSelector {Account.$,Arn.$}, ResultPath $.identity; StartExecution and read the output. Expected identity.Arn to be an assumed-role session of that role; observed arn:aws:iam::000000000000:user/emulator.
