Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs__stepfunctions.eventbridge-to-sqs-delivers-envelope events__lambda__sqs__stepfunctions.eventbridge-to-lambda-invokes-with-resource-policy events__lambda__sqs__stepfunctions.eventbridge-to-lambda-blocked-without-resource-policy events__lambda__sqs__stepfunctions.eventbridge-fanout-lambda-sqs-sfn events__lambda__sqs__stepfunctions.sqs-to-lambda-esm-preserves-attributes events__lambda__sqs__stepfunctions.sqs-to-lambda-esm-handler-error-returns-message events__lambda__sqs__stepfunctions.sqs-to-lambda-esm-rejects-role-without-receive events__lambda__sqs__stepfunctions.sfn-task-lambda-fails-without-invoke-permission events__lambda__sqs__stepfunctions.sfn-task-sqs-denied-without-permission
Issue-Key: events__lambda__sqs__stepfunctions.eventbridge-to-sqs-delivers-envelope|run#22|ValueError|event_time = datetime.datetime.strptime(
Issue-Key: events__lambda__sqs__stepfunctions.eventbridge-to-lambda-invokes-with-resource-policy|_receive_raw#8|Timeout|resp = ctx.client("sqs").receive_message(
Issue-Key: events__lambda__sqs__stepfunctions.eventbridge-to-lambda-blocked-without-resource-policy|_receive_and_drain#13|Timeout|resp = sqs.receive_message(
Issue-Key: events__lambda__sqs__stepfunctions.eventbridge-fanout-lambda-sqs-sfn|_receive_raw#22|Timeout|resp = ctx.client("sqs").receive_message(
Issue-Key: events__lambda__sqs__stepfunctions.sqs-to-lambda-esm-preserves-attributes|_wait_for_messages#8|Timeout|resp = sqs.receive_message(
Issue-Key: events__lambda__sqs__stepfunctions.sqs-to-lambda-esm-handler-error-returns-message|_wait_for_messages#9|AssertionError|raise AssertionError(
Issue-Key: events__lambda__sqs__stepfunctions.sqs-to-lambda-esm-rejects-role-without-receive|_error#4|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: events__lambda__sqs__stepfunctions.sfn-task-lambda-fails-without-invoke-permission|run#11|AssertionError|assert "AccessDenied" in blob, f"expected an access-denied cause, got: {blob[:600]}"
Issue-Key: events__lambda__sqs__stepfunctions.sfn-task-sqs-denied-without-permission|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Commit: b1cd28f1ca277d6a032df605f83af5017e934043

# The EventBridge envelope `time` field carries sub-second precision AWS never emits, and the EventBridge→Lambda, SQS ESM and SFN lambda-integration paths are inert (5 causes)

> _Note: this issue was identified against the code base at commit `b1cd28f1ca277d6a032df605f83af5017e934043`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Resolution

Causes 1, 4 and 5 are FIXED, and their four replay keys pass: the EventBridge event envelope's `time` is
written as an ISO-8601 instant at whole-second precision closed by `Z` in both events crates' envelope
serializers; Lambda words the refusal of an event source mapping whose function execution role cannot
receive from the source queue as the service words that error class; and a `Task` state's integration call is
authorized against the state machine's execution role over the integration's own published IAM action and the
ARN of the resource its request addresses, with the optimized `lambda:invoke` integration now a call this
emulator makes through Lambda's own REST binding — so a denied call fails the task with the invoked service's
access-denied refusal instead of `States.Runtime`.

Causes 2 and 3 are NOT fixed and are NOT closed: both need the invoked function's deployment package to RUN,
and this emulator has no in-process runtime for one — `Invoke` answers 501 for a `RequestResponse` invocation
(`services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs`), the recorded functions of these
checks are `python3.12` packages, and the platform provides no Python execution engine. Everything around that
gap is already in place and is not the defect: EventBridge invokes a Lambda target per matching rule inside
its fan-out loop (`put_events.rs`, `TargetKind::Function`), and the event source mapping poller already polls
its queue, filters, invokes and deletes on success only (`src/state/sqs_poller.rs`) — each meets that same 501.
Their five replay keys
(`eventbridge-to-lambda-invokes-with-resource-policy`, `eventbridge-to-lambda-blocked-without-resource-policy`,
`eventbridge-fanout-lambda-sqs-sfn`, `sqs-to-lambda-esm-preserves-attributes`,
`sqs-to-lambda-esm-handler-error-returns-message`) therefore still fail, and the defect behind them is the one
described by the open report that owns it,
`issues/things-that-do-not-work/20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md`.
No refusal was re-worded to make either cause look served.

## Operation
PutEvents

## Description
Five root causes. 20261023T114649Z-sfn-step-functions-never-executes-task-states-every-execution.md and 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md own other checks in this service; the checks below were not bound to them.

Cause 1 of 5: the envelope's `time` field is serialised with fractional seconds.
AWS: the EventBridge event envelope's `time` member is an ISO-8601 instant with whole-second precision and a trailing Z — the format %Y-%m-%dT%H:%M:%SZ, e.g. 2026-10-29T17:38:22Z. Consumers parse it with exactly that format.
Emulator: the delivered envelope carries microseconds: `"time":"2026-10-29T17:38:22.626812Z"`. The fixed part of the requirement is the format (whole seconds, trailing Z); the instant itself is a per-run value and not part of it.
Why: any consumer that parses `time` with the documented format raises instead of processing the event — here the check dies with "ValueError: time data '2026-10-29T17:38:22.626812Z' does not match format '%Y-%m-%dT%H:%M:%SZ'". This breaks every downstream that strptimes the envelope, and it breaks it on the first event rather than intermittently.
Fix: the EventBridge envelope serializer's timestamp formatting (truncate to whole seconds before formatting).
Traces: events__lambda__sfn__sqs.eventbridge-to-sqs-delivers-envelope

Cause 2 of 5: an EventBridge rule never invokes its Lambda target, with or without the resource-policy grant.
AWS: with a lambda:InvokeFunction statement for events.amazonaws.com the function is invoked once per matching event; without it the invocation is refused and the function does not run.
Emulator: the function is never invoked in either state; the checks end "Check timed out after 60.0s", and the fan-out variant delivers to none of its Lambda, SQS and Step Functions targets.
Why: the pattern is unusable and the authorization difference is unobservable.
Fix: the EventBridge Lambda-target invoker and the per-target fan-out loop.
Traces: events__lambda__sfn__sqs.eventbridge-to-lambda-invokes-with-resource-policy, events__lambda__sfn__sqs.eventbridge-to-lambda-blocked-without-resource-policy, events__lambda__sfn__sqs.eventbridge-fanout-lambda-sqs-sfn

Cause 3 of 5: an SQS event source mapping never invokes the function and never returns the message.
AWS: an enabled SQS ESM receives messages and invokes the function with Records[] carrying body, messageId, receiptHandle, attributes and messageAttributes; if the handler raises, the batch is not deleted and the messages become visible again for redelivery.
Emulator: the function is never invoked, so no message is returned to the queue and the attributes cannot be checked. "AssertionError: Expected 1 messages, got 0 within 45s" and "Error: Check timed out after 60.0s".
Why: SQS-driven Lambda processing does nothing, and the at-least-once redelivery guarantee on handler error cannot be relied on.
Fix: the Lambda event-source-mapping poller for SQS (receive, batch assembly with message attributes, and delete-on-success only).
Traces: events__lambda__sfn__sqs.sqs-to-lambda-esm-preserves-attributes, events__lambda__sfn__sqs.sqs-to-lambda-esm-handler-error-returns-message

Cause 4 of 5: CreateEventSourceMapping accepts an execution role with no sqs:ReceiveMessage.
AWS: CreateEventSourceMapping is refused with HTTP 400 InvalidParameterValueException when the function's execution role cannot receive from the source queue, with a message of the fixed form "The provided execution role does not have permissions to call ReceiveMessage on SQS".
Emulator: the call succeeds and returns a UUID. "AssertionError: expected the call to be rejected, but it SUCCEEDED and returned '<uuid>'" — the UUID is a per-run value; the requirement is the 400 and the message template.
Why: the single validation AWS performs at ESM-create time is skipped, so a mapping is created that can never poll, and the caller gets no error until nothing happens.
Fix: execution-role permission validation in the Lambda CreateEventSourceMapping handler.
Traces: events__lambda__sfn__sqs.sqs-to-lambda-esm-rejects-role-without-receive

Cause 5 of 5: Step Functions does not implement the lambda and sqs Task integrations, and misreports the failure as States.Runtime.
AWS: a Task state with Resource "arn:aws:states:::lambda:invoke" invokes the function; when the execution role lacks lambda:InvokeFunction the task fails with error "States.TaskFailed" (or Lambda.AccessDeniedException) and a cause carrying the AccessDeniedException from Lambda. The same holds for arn:aws:states:::sqs:sendMessage.
Emulator: the execution fails with error "States.Runtime" and cause "state 'Invoke': 'arn:aws:states:::lambda:invoke' names an integration with 'lambda', which is not one of the services this emulator makes a 'Task' state's integration call to, so nothing was run for it"; the SQS variant times out.
Why: an authorization failure and an unsupported integration are reported identically, so a client testing its Catch/Retry on Lambda.AccessDeniedException never matches; and no Lambda- or SQS-calling state machine can run.
Fix: the Step Functions interpreter's Task-state integration dispatch for lambda and sqs, and the error classification for a denied integration call (States.TaskFailed, not States.Runtime).
Traces: events__lambda__sfn__sqs.sfn-task-lambda-fails-without-invoke-permission, events__lambda__sfn__sqs.sfn-task-sqs-denied-without-permission

## Evidence
Cause 1: "ValueError: time data '2026-10-29T17:38:22.626812Z' does not match format '%Y-%m-%dT%H:%M:%SZ'" while parsing the delivered envelope's time field.
Cause 2 and 3: "Error: Check timed out after 60.0s" / "AssertionError: Expected 1 messages, got 0 within 45s".
Cause 4: "AssertionError: expected the call to be rejected, but it SUCCEEDED and returned '3900a5bd-03a9-417f-9fea-3bae83402719'" — expected HTTP 400 InvalidParameterValueException naming the missing ReceiveMessage permission.
Cause 5: execution FAILED with error 'States.Runtime' and cause "state 'Invoke': 'arn:aws:states:::lambda:invoke' names an integration with 'lambda', which is not one of the services this emulator makes a 'Task' state's integration call to, so nothing was run for it"; expected States.TaskFailed with an access-denied cause.

## Steps to Reproduce
(1) PutRule + PutTargets with an SQS target and a permissive queue policy, PutEvents, read the message body and parse `time` with %Y-%m-%dT%H:%M:%SZ — it fails. (2) PutRule with a Lambda target (grant and no grant) and PutEvents — never invoked. (3) CreateEventSourceMapping on an SQS queue, SendMessage — the function never runs; make the handler raise — the message never reappears. (4) CreateEventSourceMapping with an execution role lacking sqs:ReceiveMessage — accepted. (5) StartExecution on a state machine whose Task Resource is arn:aws:states:::lambda:invoke — FAILED/States.Runtime.
