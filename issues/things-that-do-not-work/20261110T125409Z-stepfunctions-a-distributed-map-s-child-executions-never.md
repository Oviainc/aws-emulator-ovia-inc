Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.conc-distributed-map-tolerated-failure-percentage-succeeds-and-names-failed-items
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.conc-distributed-map-tolerated-failure-percentage-succeeds-and-names-failed-items|run#25|AssertionError|assert errors == {"Deliberate.ItemFailure"}, (
Issue-Commit: 463088120b9e448d9d8c5f1556b3fe534259a54a

# stepfunctions: a DISTRIBUTED Map's child executions never observe their siblings' committed progress, so a coordinating child takes its give-up branch and the ResultWriter FAILED records carry the wrong Error

> _Note: this issue was identified against the code base at commit `463088120b9e448d9d8c5f1556b3fe534259a54a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
One net-new root cause behind 1 failing check.

Cause 1 of 1: in a Map state with ProcessorConfig.Mode=DISTRIBUTED, the child executions do not make progress concurrently up to the state's MaxConcurrency against shared service state, so a child whose control flow waits on an effect its siblings commit never observes that effect and takes the give-up branch of its own definition instead of the branch the siblings' progress unlocks. The map run's aggregate numbers are right while the per-item error identity is wrong.

AWS: MaxConcurrency on a Map state bounds how many child executions run at the same time; with MaxConcurrency equal to the item count AWS runs all of them concurrently, and each child is a full execution issuing real service calls, so an `arn:aws:states:::dynamodb:getItem` with `ConsistentRead: true` in one child reads what an `arn:aws:states:::dynamodb:updateItem` with an `ADD` UpdateExpression committed in a sibling child. A `Wait` state with `Seconds: N` delays that child by N seconds. When a child reaches a `Fail` state, the execution's error is the `Error` named on THAT `Fail` state, and that same string is what appears as the `Error` field of the record the ResultWriter writes into the run's FAILED result files and as the failed child execution's own error. Smithy (services/service-com-amazonaws-sfn-awsstepfunctions): DescribeMapRun output carries required `maxConcurrency` (MaxConcurrency, range min 0) and `toleratedFailurePercentage` (range 0..100) alongside required `itemCounts`/`executionCounts`, so MaxConcurrency is a modelled property of the run and not advisory. The ASL members themselves (`MaxConcurrency`, `ToleratedFailurePercentage`, `ItemReader`, `ItemProcessor`, `ResultWriter`, `Wait.Seconds`) are recorded behavior only; the recording shows the FAILED records' `Error` equal to the error of the `Fail` state the barrier releases into.

Emulator: the run does reach SUCCEEDED, the 38 healthy items each write their ledger row exactly once, DescribeMapRun reports itemCounts (total 40, succeeded 38, failed 2) with toleratedFailurePercentage 10, and the ResultWriter FAILED result files name exactly the two planted items — but every FAILED record carries `Error` = `Barrier.NeverReleased`, the error of the definition's give-up branch (`Choice` on `$.tries.n >= 60`, i.e. 60 poll iterations each behind a `Wait` of 2 seconds, so at least ~120 s of polling), instead of `Deliberate.ItemFailure`, the error of the branch taken once the shared counter read by `dynamodb:getItem` with `ConsistentRead: true` reaches 38. The two coordinating children therefore exhausted their whole poll budget without ever observing a single one of the 38 `ADD n :one` increments that their siblings demonstrably committed in the same map run (the same map run reports those 38 children as succeeded, and their per-item rows are all present with hits=1).

Hypothesis (the replay shows the outcome, not which mechanism produced it): the emulator either (a) does not run child executions concurrently up to MaxConcurrency, so no sibling had committed anything by the time a polling child gave up; or (b) does not honour a child `Wait` state's configured `Seconds`, so the 60 iterations are consumed in milliseconds rather than over ~120 s; or (c) runs children against state that is not shared with their siblings, so a strongly-consistent GetItem in one child cannot see an UpdateItem ADD committed in another. All three present identically in this replay.

Why: clients use Distributed Map precisely to fan out and then coordinate the children through shared state — a counter, a lock, a ledger, a barrier. On AWS a child sees its siblings' committed effects and MaxConcurrency is a real concurrency guarantee, so the per-item outcome is decided by the data. Here the data-dependent branch is never taken: the map run's per-item errors, the `Error` field of the ResultWriter FAILED records, the failed child executions' own error, and anything a client routes on them (retry vs. DLQ decisions, alarms keyed on a specific error name, downstream triage of which items to replay) are all wrong. It is silent: the run still reports SUCCEEDED with the correct itemCounts and the correct set of failed item ids, so nothing signals that the failure reason is fabricated by a timeout rather than by the planted condition. Any workflow whose children are not fully independent is unreliable.

Fix: the sfn service crate's Map-state handler and map-run scheduler (start and advance child executions concurrently up to the state's MaxConcurrency rather than draining them in a way that serialises dependent children), the child interpreter's Wait-state timing (honour the configured `Seconds` inside a child execution), and the child task-integration path's state visibility (children must issue their service integration calls against the same shared service state their siblings write, so a ConsistentRead GetItem observes a sibling's committed UpdateItem ADD).

Not covered by what is already filed: the open issue 20261106T214711Z-stepfunctions-4-net-new-causes-resultwriter-refused-as.md lists this check's id in its Issue-Trace-Ids, but its replay key for this check is the `assert exe["status"] == "SUCCEEDED"` failure caused by `ResultWriter` being refused as uninterpretable. That cause no longer reproduces — the run now SUCCEEDS, the manifest is written and readable, and DescribeMapRun agrees — and this failure is a different, deeper one at the later `errors == {"Deliberate.ItemFailure"}` assertion, which that issue does not carry. The resolved issue 20261106T214711Z-s3-stepfunctions-distributed-map-is-unusable-resultwriter-is.md covered the same refusal class under the `s3` heading; this is not a refusal (nothing is refused and no non-2xx is served), so it is a new defect rather than a reopening.

Traces: stepfunctions.conc-distributed-map-tolerated-failure-percentage-succeeds-and-names-failed-items.

## Evidence
Cause 1, failing check stepfunctions.conc-distributed-map-tolerated-failure-percentage-succeeds-and-names-failed-items:

  AssertionError: the FAILED records carry errors {'Barrier.NeverReleased'}, expected the planted Deliberate.ItemFailure

Assertions that PASSED first, which bound the defect: the execution reached `status='SUCCEEDED'`; the per-item ledger rows are exactly the 38 healthy item ids, each with hits=1 (the 2 planted items wrote nothing); `DescribeMapRun.toleratedFailurePercentage == 10`; `DescribeMapRun.itemCounts` == (total 40, succeeded 38, failed 2); and the ResultWriter manifest's FAILED result files name exactly the 2 planted item ids. So the 38 siblings each ran their `dynamodb:updateItem` with `UpdateExpression: "ADD n :one"` against the pre-seeded `<run>#done` row and succeeded.

Expected vs actual on the one diverging value:
  Expected: every record in `manifest["ResultFiles"]["FAILED"]` carries `Error` = `Deliberate.ItemFailure` — the error string named on the `Boom` Fail state, which is reached from the `Gate` Choice on `$.done.n NumericGreaterThanEquals 38`, i.e. once the shared counter has absorbed all 38 sibling increments.
  Actual:   every such record carries `Error` = `Barrier.NeverReleased` — the error string named on the `Stuck` Fail state, reached from the `Gate` Choice on `$.tries.n NumericGreaterThanEquals 60`, the give-up branch after 60 poll iterations each preceded by a `Wait` of `Seconds: 2`.

Fixed text vs per-run values: both `Deliberate.ItemFailure` and `Barrier.NeverReleased` are strings the REQUEST supplies (the `Error` members of the two `Fail` states in the submitted definition), so the requirement is only that the reported error is the one belonging to the Fail state actually reached; neither string is AWS wording. The run tag, bucket name, table name, role name, item ids and manifest keys are per-run values and are not part of the requirement. The requirement being violated is the ordering/visibility one: with MaxConcurrency=40 over 40 items, a child polling the shared counter must observe its 38 siblings' committed increments before a 60-iteration/≥120 s budget elapses.

## Steps to Reproduce
1. Create an S3 bucket and put `items.json` containing 40 objects `{"id": "item-NN", "fail": <bool>}` for NN=00..39, with `fail` true for exactly 2 of them (5% of 40).
2. Create a DynamoDB table (PAY_PER_REQUEST) with a single `pk` (S) hash key, and create an execution role trusted by `states.amazonaws.com` granting states:StartExecution/DescribeExecution/StopExecution, s3:GetObject/PutObject/ListBucket on the bucket and dynamodb:UpdateItem/GetItem on the table.
3. CreateStateMachine (type STANDARD) with one Map state: `MaxConcurrency: 40`, `ToleratedFailurePercentage: 10`, `ItemReader` = `arn:aws:states:::s3:getObject` over `items.json` with `ReaderConfig.InputType=JSON`, `ResultWriter` = `arn:aws:states:::s3:putObject` to a results prefix, and `ItemProcessor` with `ProcessorConfig {Mode: DISTRIBUTED, ExecutionType: STANDARD}` whose states are:
   - `Route` (Choice): `$.fail BooleanEquals true` -> `Poll`, default -> `Record`.
   - `Record` (Task `arn:aws:states:::dynamodb:updateItem`): `ADD hits :one` on key `<run>#item#<id>` -> `Done`.
   - `Done` (Task `arn:aws:states:::dynamodb:updateItem`): `ADD n :one` on key `<run>#done`, End.
   - `Poll` (Task `arn:aws:states:::dynamodb:getItem`, `ConsistentRead: true`) on key `<run>#done`, `ResultSelector {n.$: States.StringToJson($.Item.n.N)}`, `ResultPath $.done` -> `Gate`.
   - `Gate` (Choice): `$.done.n NumericGreaterThanEquals 38` -> `Boom`; `$.tries.n NumericGreaterThanEquals 60` -> `Stuck`; default -> `Nap`.
   - `Nap` (Wait `Seconds: 2`) -> `Tick` (Pass incrementing `$.tries.n`) -> `Poll`.
   - `Boom` (Fail, `Error: Deliberate.ItemFailure`); `Stuck` (Fail, `Error: Barrier.NeverReleased`).
4. PutItem `{pk: "<run>#done", n: 0}` to seed the barrier counter, then StartExecution with input `{"run": "<run>"}` and poll DescribeExecution to terminal.
5. Read `ResultWriterDetails` from the execution output, GetObject the manifest, and GetObject every key under `ResultFiles.FAILED`; collect the `Error` field of each record.
   Expected: every FAILED record's `Error` is `Deliberate.ItemFailure`.
   Observed: every FAILED record's `Error` is `Barrier.NeverReleased`, while the execution is SUCCEEDED, DescribeMapRun reports itemCounts (40, 38, 2), and all 38 healthy per-item rows exist with hits=1.
