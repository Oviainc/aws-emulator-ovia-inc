Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda.recursive-invocations-dropped-is-emitted-only-when-the-guardrail-terminates
Issue-Key: cloudwatch__lambda.recursive-invocations-dropped-is-emitted-only-when-the-guardrail-terminates|_collect_depths#9|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: adb8f986b66b684ed38690a8bd355d4f46d0ebf5

# Lambda's recursion guardrail is inert: no AWS/Lambda RecursiveInvocationsDropped datapoint and no self-invoking chain progress, so the cloudwatch__lambda check never reaches a verdict

> _Note: this issue was identified against the code base at commit `adb8f986b66b684ed38690a8bd355d4f46d0ebf5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutFunctionRecursionConfig

## Description
One net-new root cause, grounded in the single failing cloudwatch__lambda check that the replay report marks NEITHER "ALREADY FILED" nor "OWNED by".

Cause 1 of 1: a function configured with RecursiveLoop=Terminate never drives, breaks or meters a self-invoking chain, so AWS/Lambda RecursiveInvocationsDropped is never published and the check cannot evaluate either arm of its matrix.
Failing check id: cloudwatch__lambda.recursive-invocations-dropped-is-emitted-only-when-the-guardrail-terminates.

AWS: recorded behavior only for the cross-service effect — the check is explicitly an unblessed draft with no .gt.jsonl oracle, so what it encodes is the documented contract plus the behavior its replay must confirm. The contract: when a function's recursion configuration is RecursiveLoop=Terminate (lambda:PutFunctionRecursionConfig) and Lambda detects that the same request has looped back into the same function, Lambda STOPS the chain and publishes the metric RecursiveInvocationsDropped into the CloudWatch namespace AWS/Lambda with the FunctionName dimension and Unit 'Count'. The discriminating half is the twin: an otherwise identical function with RecursiveLoop=Allow, driven through the SAME self-invoking chain to the same bounded depth, publishes NO RecursiveInvocationsDropped datapoint at all — not a 0.0 series, no series. The magnitude is asserted only as Sum >= 1 (how many invocations Lambda drops once it breaks a loop is not documented, so an exact count would be invented); the depth at which the guardrail trips is likewise NOT asserted — the chain is driven well past any plausible threshold (_MAX_DEPTH = 24) and the assertion is on the metric, not on the depth. Publication latency budget _METRIC_BUDGET_S = 600.0; chain budget _CHAIN_BUDGET_S = 300.0; the absence hold for the Allow arm is at least _ABSENCE_HOLD_S = 10.0 and is stretched to however long the positive metric took to land.

Emulator: the check reaches no assertion at all. It is reported [mismatch] with the detail 'Check timed out after 60.0s' — the harness kills it while it is still blocked, with every one of its own budgets (600.0s metric, 300.0s chain, 180.0s _ROLE_PROPAGATION_BUDGET_S) far from exhausted. The conditions it blocks on never become true on this emulator: the python3.12 inline-zip handler reports each hop of the chain to a side-channel SQS queue, and the check waits on that queue for the chain's own depth markers and terminal marker before it ever queries CloudWatch; then it waits for an AWS/Lambda RecursiveInvocationsDropped datapoint under the FunctionName dimension. Neither the side-channel messages nor the metric ever arrive, so the self-invoking chain never advances past its first hop and the recursion guardrail never fires or meters anything. Hypothesis (neither the model nor the recording states which wait the run is sitting in when the harness kills it): the stall is in the side-channel SQS drain because no Lambda invocation ever executes the handler, which is consistent with the already-open 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md (that issue owns many sibling cloudwatch__lambda* checks but the report did NOT mark this one owned by it, and it says nothing about RecursiveLoop configuration or the RecursiveInvocationsDropped metric, so it does not cover this cause); an alternative is the 180s IAM role-propagation wait that precedes function creation.

Why: a client that relies on Lambda's recursive-loop guardrail gets no protection and no signal on this emulator. The guardrail is the mechanism that stops a runaway self-invoking chain from becoming an unbounded bill, and RecursiveInvocationsDropped is the only way a client learns it fired — an alarm or dashboard over that metric stays permanently in INSUFFICIENT_DATA, which is indistinguishable from "no loop ever happened". Because the Terminate arm never publishes, the Allow arm's asserted ABSENCE is also unverifiable: an emulator that published the metric for both arms, or for neither, would look identical to a client, so the RecursiveLoop setting has no observable meaning at all.

Fix: the Lambda recursion-detection path and its metric emission — the function recursion configuration state (lambda:PutFunctionRecursionConfig / GetFunctionRecursionConfig) and the invoke path in the lambda crate (services/service-com-amazonaws-lambda-awsgirapiservice), which must detect a request looping back into the same function, stop the chain when RecursiveLoop=Terminate, and publish a RecursiveInvocationsDropped datapoint (Unit 'Count', dimension FunctionName) into the cloudwatch crate's AWS/Lambda metric store — and publish nothing when RecursiveLoop=Allow. It also requires the asynchronous (InvocationType='Event') self-invocation path to actually execute the handler, so the chain can progress at all.

Traces: cloudwatch__lambda.recursive-invocations-dropped-is-emitted-only-when-the-guardrail-terminates.

## Evidence
Replayed behavioral suite simparity-behavioral-ga-prod-v1 against the built emulator: 125/412 passed (30.3%). Per-service line: `cloudwatch__lambda 0/5 (0%) 5 failed`; four of the five are marked "ALREADY FILED" or "OWNED by" and this one is not.

Failing check: [mismatch] cloudwatch__lambda.recursive-invocations-dropped-is-emitted-only-when-the-guardrail-terminates.
Failing detail: `Error: Check timed out after 60.0s` — the harness killed the check before any assertion ran, so the emulator never satisfied the condition being polled.

Expected (the check's two arms):
  Terminate arm: create a python3.12 inline-zip function whose handler sends {depth, fn} to a side-channel SQS queue and then self-invokes with lambda:Invoke InvocationType='Event', Payload={"depth": depth+1}, stopping itself at MAX_DEPTH=24; set its recursion configuration to RecursiveLoop='Terminate'; drive the chain from depth 0. Lambda breaks the loop, and cloudwatch reports AWS/Lambda / RecursiveInvocationsDropped, dimension FunctionName=<that function>, Unit 'Count', with Sum >= 1 within _METRIC_BUDGET_S=600.0.
  Allow arm (load-bearing twin): an identical function with RecursiveLoop='Allow', driven through the same chain to the same bounded depth, has NO RecursiveInvocationsDropped series at all — the absence is held for at least _ABSENCE_HOLD_S=10.0 and for as long as the positive metric took to appear.
Actual: neither arm is evaluated. The chain produces no side-channel SQS messages (so the handler never ran and no hop past the first is observable) and no AWS/Lambda RecursiveInvocationsDropped datapoint appears under any FunctionName dimension. The check consumes its entire wall-clock allowance polling and is terminated at 60.0s.

Status/code/message: no error payload divergence is involved — the setup calls (role creation, function creation, recursion configuration, the first Invoke) are not reported as refusals in this check's detail. The divergence is a missing EFFECT: zero RecursiveInvocationsDropped datapoints and zero chain progress where AWS stops the loop and meters it.

Related open issue that does NOT cover this cause: 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md owns several sibling cloudwatch__lambda / cloudwatch__lambda__logs checks, but the report did not mark this check owned by it and that issue describes neither the RecursiveLoop configuration nor the RecursiveInvocationsDropped metric.

## Steps to Reproduce
1. Create an IAM role the emulator's Lambda will accept, with permission to invoke Lambda and send to SQS; create an SQS queue to use as the side channel.
2. Create a python3.12 inline-zip function whose handler does: read depth from the event, sqs:SendMessage {"depth": depth, "fn": context.function_name} to the side-channel queue, and if depth < 24 call lambda:Invoke FunctionName=context.function_name, InvocationType='Event', Payload={"depth": depth+1}; otherwise return {"stopped_at": depth}. Set the side-channel queue URL in the function's environment.
3. Set the function's recursion configuration to RecursiveLoop='Terminate' (lambda:PutFunctionRecursionConfig), and verify it reads back.
4. Invoke the function once with Payload={"depth": 0}.
5. Drain the side-channel queue.
   Expected: a sequence of hop messages showing the chain advancing past depth 0 until Lambda breaks it.
   Observed: no messages arrive; the chain never advances.
6. Poll cloudwatch:GetMetricStatistics (or GetMetricData) Namespace='AWS/Lambda', MetricName='RecursiveInvocationsDropped', Dimensions=[{"Name": "FunctionName", "Value": <function>}], Period=60, Statistics=['Sum'], Unit='Count' over a window covering the chain.
   Expected: Datapoints non-empty with Sum >= 1.
   Observed: Datapoints stays empty for the whole budget; the check is killed at 60.0s.
7. Twin (negative control): repeat steps 2-6 with a second function configured RecursiveLoop='Allow'.
   Expected: the chain runs to its self-imposed depth ceiling of 24 and NO RecursiveInvocationsDropped series exists for that FunctionName — an absence, not a 0.0 series — held for at least as long as the Terminate arm's metric took to appear.
   Observed: unverifiable, because the Terminate arm never publishes anything to compare against.

Re-replay the check id in trace_ids to confirm the fix.
