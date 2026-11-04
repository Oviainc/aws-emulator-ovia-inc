Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.async-returns-202 lambda.invoke-async-fire-and-forget-managed lambda.invoke-async-invalid-qualifier-rejected lambda.esm-drains-every-message-and-the-handler-logs-each-body
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.async-returns-202|run#15|AssertionError|assert found, (
Issue-Key: lambda.invoke-async-fire-and-forget-managed|_poll_until#17|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s (last={result!r})")
Issue-Key: lambda.invoke-async-invalid-qualifier-rejected|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: lambda.esm-drains-every-message-and-the-handler-logs-each-body|_await_lines#12|Timeout|time.sleep(5)
Issue-Commit: b4ead8169e48e6047949699a772a41fd75a6bc4d

# Asynchronous Invoke is accepted with 202 but the function never runs

> _Note: this issue was identified against the code base at commit `b4ead8169e48e6047949699a772a41fd75a6bc4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Two net-new root causes. (Open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md covers the synchronous Invoke refusal; the asynchronous path behaves differently — it accepts the request and drops it — and that issue does not carry these replay keys.)

Cause 1 of 2: Invoke with InvocationType=Event answers 202 and then never executes the function.
AWS: recorded behavior only — the recording invokes with InvocationType=Event, expects HTTP 202 with an empty payload, and then finds the handler's per-run completion marker in /aws/lambda/<function-name>, proving the invocation really ran in the background.
Emulator: the 202 is returned, but the marker never appears: "AssertionError: completion marker '<marker>' never appeared in /aws/lambda/<function-name> - the async invocation was accepted but the function never actually ran", and the fire-and-forget twin reports "completion marker <marker> appearing in /aws/lambda/<function-name> after background execution not met within 40.0s". The status (202) is right; the behavior behind it is not.
Why: this is worse than a refusal — a client is told the work was queued, gets no error anywhere, and the work never happens. Every event-driven, fire-and-forget design silently loses invocations, and there is no log, no FunctionError and no DLQ record to find.
Fix: the Lambda Invoke handler's InvocationType=Event path — it must enqueue and actually execute the invocation (and surface its failures through the async failure plumbing) rather than acknowledging and discarding it.
Traces: lambda.async-returns-202, lambda.invoke-async-fire-and-forget-managed.

Cause 2 of 2: two further invoke/mapping checks never complete.
AWS: recorded behavior only — invoke-async-invalid-qualifier-rejected expects an asynchronous invoke naming a non-existent qualifier to be refused promptly (AWS refuses the qualifier before accepting the invocation), and esm-drains-every-message-and-the-handler-logs-each-body expects an event source mapping to drain the queue with the handler logging each body; both settle inside the suite's 60s budget on AWS.
Emulator: both checks are cut off with "Error: Check timed out after 60.0s" — no refusal is returned for the bad qualifier and no message is ever drained, so each poll runs out the budget.
Why: a client gets neither the validation error that would name the bad qualifier nor any progress from the mapping; both calls hang instead of answering.
Fix: the Invoke handler's qualifier resolution on the asynchronous path, and the event-source-mapping poller that drains the queue into the handler.
Traces: lambda.invoke-async-invalid-qualifier-rejected, lambda.esm-drains-every-message-and-the-handler-logs-each-body.

## Evidence
Expected (AWS): Invoke(InvocationType='Event') -> 202, and the handler's completion marker appears in /aws/lambda/<name>; an invalid qualifier on an async invoke is refused; the event source mapping drains every message.
Actual (emulator): "AssertionError: completion marker 'marker-...' never appeared in /aws/lambda/sim2real-behav-fn-... - the async invocation was accepted but the function never actually ran"; 'AssertionError: completion marker ... appearing in /aws/lambda/... after background execution not met within 40.0s (last=False)'; 'Error: Check timed out after 60.0s' for the other two.

## Steps to Reproduce
1. CreateFunction with a handler that writes a unique marker to stdout.
2. Invoke it with InvocationType='Event'; observe HTTP 202.
3. Poll FilterLogEvents on /aws/lambda/<function-name> for the marker. Observe it never appears.
4. Invoke asynchronously with a Qualifier that does not exist, and separately create an event source mapping on a queue with messages. Observe neither call ever settles.
