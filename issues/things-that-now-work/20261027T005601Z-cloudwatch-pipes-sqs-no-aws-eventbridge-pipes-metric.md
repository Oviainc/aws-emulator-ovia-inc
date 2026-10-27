Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__pipes__sqs.target-stage-failed-publishes-for-the-failing-pipe-and-zero-for-its-twin
Issue-Key: cloudwatch__pipes__sqs.target-stage-failed-publishes-for-the-failing-pipe-and-zero-for-its-twin|_poll_until#22|Timeout|time.sleep(interval_s)
Issue-Commit: 64e3d31da0f219acb3f0523749fc49dddb7bfb57

# cloudwatch__pipes__sqs: no AWS/EventBridge/Pipes metric is ever published — neither a failing pipe's TargetStageFailed nor a healthy pipe's explicit zero (1 cause)

> _Note: this issue was identified against the code base at commit `64e3d31da0f219acb3f0523749fc49dddb7bfb57`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribePipe

## Description
One net-new root cause, grounded in 1 failing behavioral check that the replay report marks NEITHER "ALREADY FILED" nor "OWNED by".

Cause 1 of 1: the EventBridge Pipes runtime publishes no CloudWatch metrics at all, so the distinction between "the pipe ran and its target refused" and "the pipe never ran" cannot be observed.
Failing check id: cloudwatch__pipes__sqs.target-stage-failed-publishes-for-the-failing-pipe-and-zero-for-its-twin

AWS: recorded behavior only — this check is an unblessed draft, so nothing here is a replayed oracle and the claims below are the AWS DOCUMENTATION the check cites (EventBridge Pipes monitoring, eb-pipes-monitoring.html), labelled as such. The documented contract is: a pipe publishes to the CloudWatch namespace AWS/EventBridge/Pipes, dimensioned on PipeName, "every minute"; the counters EventCount, ExecutionFailed, TargetStageFailed and their siblings carry Unit "None" (NOT "Count"), while the Duration family carries "Milliseconds"; each counter is documented "This value will be 0 if no executions failed", i.e. a quiet series EXISTS and reads 0 rather than being absent; and delivery is explicitly best-effort ("might not be delivered at all"), which is why the failing pipe is held only to a relative property (Sum >= 1 and strictly greater than its twin's) and never to a pinned count. Hypothesis (the check's own H5, unverified against a recording): a pipe whose execution role can read its source queue but lacks sqs:SendMessage on its TARGET queue fails at the TARGET stage, so TargetStageFailed moves rather than only ExecutionFailed. The Smithy model says nothing about any of this — metric emission is a runtime side effect with no modeled member — so the model cannot be cited here; the two pipes themselves are ordinary CreatePipe/DescribePipe resources.

Emulator: the check does not complete — 'Check timed out after 60.0s'. Its steps are, in order: create four SQS queues and two IAM roles, CreatePipe twice, wait for DescribePipe CurrentState == 'RUNNING' (budget 300 s, interval 10 s), drain the healthy pipe's target queue for its delivered message (budget 300 s), then poll GetMetricStatistics over namespace AWS/EventBridge/Pipes with Dimensions=[{PipeName}] for the failing pipe's TargetStageFailed Sum >= 1 (budget 900 s, interval 20 s), then read ListMetrics for the dimension set, then assert the healthy twin's same series exists and reads 0 and HOLDS at 0. None of those waits can be satisfied: the emulator's Pipes runtime does not run pipes, so neither the delivery nor any AWS/EventBridge/Pipes datapoint ever appears, and the first unbounded poll consumes the whole budget. Because the poll aborts the check, every downstream claim — the Unit being "None" rather than "Count", the ListMetrics dimension-set catalogue, and the healthy twin's explicit zero — is left ungraded.

Why: AWS/EventBridge/Pipes is the only signal that distinguishes a pipe that executed and was refused at its target from a pipe that never executed; both look identical at the target queue. A client or operator wiring an alarm on TargetStageFailed, or a dashboard on EventCount, gets permanent silence (and, if a later implementation materializes only non-empty series, a quiet pipe's series would be missing where AWS documents an explicit 0 — which is itself a divergence to avoid when fixing this).

Fix: the Pipes runtime in the pipes service crate plus its CloudWatch metric emission — a RUNNING pipe must poll its source, attempt its target under its execution role, and publish the AWS/EventBridge/Pipes counters per minute dimensioned on PipeName with Unit 'None', including a 0-valued series for a pipe whose executions did not fail. Open issue 20261023T235727Z-pipes-eventbridge-pipes-has-no-runtime-a-running-pipe.md (service token 'pipes') describes the runtime half — a RUNNING pipe never polling its source or delivering to its target — and is the likely upstream cause here, but it does NOT carry this check id and says nothing about metric emission; this issue registers the CloudWatch-observability half and this replay so the metric claims are re-graded when the runtime lands. Open issues 20261025T025049Z-pipes-data-plane-a-fifo-sourced-running-pipe.md and 20261025T132558Z-pipes-updatepipe-rolearn-switch-is-never-exercised-a.md likewise cover delivery, not metrics.

Traces: cloudwatch__pipes__sqs.target-stage-failed-publishes-for-the-failing-pipe-and-zero-for-its-twin.

## Evidence
Replayed behavioral suite simparity-behavioral-ga-prod-v1 against the emulator built from this checkout: 166/412 passed (40.3%); service cloudwatch__pipes__sqs 0/1 (0%), 1 failed. The check is [mismatch] in the failing list and is marked neither "ALREADY FILED" nor "OWNED by".

cloudwatch__pipes__sqs.target-stage-failed-publishes-for-the-failing-pipe-and-zero-for-its-twin — verdict 'mismatch'.
  expected: both pipes reach DescribePipe CurrentState 'RUNNING'; the healthy pipe's message arrives at its target queue; CloudWatch GetMetricStatistics over Namespace 'AWS/EventBridge/Pipes', Dimensions [{Name:'PipeName', Value:<failing pipe>}], MetricName 'TargetStageFailed', Period 60, Statistics [Sum, SampleCount, Maximum] returns Sum >= 1 at Unit 'None'; ListMetrics names that dimension set; the healthy twin's TargetStageFailed series EXISTS and reads 0 and stays 0 for the hold.
  actual:   Check timed out after 60.0s — no datapoint, no dimension set, and (per the already-open pipes runtime issue) no delivery either. The failing poll is the first unbounded one in the check; its declared budgets are _PIPE_READY_BUDGET_S=300, _DELIVERY_BUDGET_S=300, _METRIC_BUDGET_S=900, _ABSENCE_HOLD_S=10.
  Model: the cloudwatch Smithy model has no member describing metric emission; the namespace/dimension/unit contract comes from AWS's Pipes monitoring documentation, as the check records. The check header declares itself a DRAFT whose numbers are hypotheses, so the per-metric values above are documentation-derived, not recorded.

## Steps to Reproduce
Boot the emulator from this checkout, then:
1. sqs:CreateQueue four times: <bad-source>, <bad-target>, <ok-source>, <ok-target>.
2. iam:CreateRole twice for pipes.amazonaws.com: role A with sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on <bad-source> and NO sqs:SendMessage on <bad-target>; role B with the same source permissions on <ok-source> PLUS sqs:SendMessage on <ok-target>.
3. pipes:CreatePipe <failing> (Source=<bad-source> ARN, Target=<bad-target> ARN, RoleArn=role A) and pipes:CreatePipe <healthy> (Source=<ok-source>, Target=<ok-target>, RoleArn=role B).
4. Poll pipes:DescribePipe for CurrentState == 'RUNNING' on both.
5. sqs:SendMessage into <bad-source> and into <ok-source>.
6. sqs:ReceiveMessage on <ok-target> -> expect the healthy pipe's message to arrive.
7. cloudwatch:GetMetricStatistics Namespace='AWS/EventBridge/Pipes', MetricName='TargetStageFailed', Dimensions=[{Name:'PipeName', Value:<failing>}], Period=60, Statistics=['Sum','SampleCount','Maximum'] -> expect a datapoint with Sum >= 1 and Unit 'None'.
8. cloudwatch:ListMetrics Namespace='AWS/EventBridge/Pipes' -> expect the PipeName dimension set for <failing>.
9. The same GetMetricStatistics for <healthy> -> expect the series to EXIST and read 0, and to stay 0 across a short hold.
   OBSERVED: steps 7-9 never produce anything (and step 6 does not deliver), so the check expires at the harness's 60 s cap.

Re-replay cloudwatch__pipes__sqs.target-stage-failed-publishes-for-the-failing-pipe-and-zero-for-its-twin to confirm the fix.
