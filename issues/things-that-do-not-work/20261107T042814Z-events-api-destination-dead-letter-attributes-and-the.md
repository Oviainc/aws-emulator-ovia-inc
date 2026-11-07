Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.api-destination-4xx-dead-letters-without-retry-while-5xx-is-retried-until-its-retry-attempts-are-used-up events.api-destination-rate-limit-defers-a-burst-it-cannot-absorb-and-still-delivers-every-event events.express-and-standard-state-machine-targets-start-with-the-envelope-as-input-named-after-the-event-id events.lambda-target-is-invoked-asynchronously-and-its-on-success-destination-records-the-envelope events.lambda-target-function-error-is-retried-by-lambda-and-never-dead-lettered-by-the-rule
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events.api-destination-4xx-dead-letters-without-retry-while-5xx-is-retried-until-its-retry-attempts-are-used-up|run#28|AssertionError|assert four == {"ERROR_CODE": "INVALID_PARAMETER",
Issue-Key: events.api-destination-rate-limit-defers-a-burst-it-cannot-absorb-and-still-delivers-every-event|run#17|Timeout|resp = events.put_events(Entries=[
Issue-Key: events.express-and-standard-state-machine-targets-start-with-the-envelope-as-input-named-after-the-event-id|run#24|AssertionError|assert re.fullmatch(re.escape(event_id) + "_" + _UUID, rep["name"]), (
Issue-Key: events.lambda-target-is-invoked-asynchronously-and-its-on-success-destination-records-the-envelope|_receive_for#20|Timeout|for msg in sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: events.lambda-target-function-error-is-retried-by-lambda-and-never-dead-lettered-by-the-rule|_receive_for#21|Timeout|for msg in sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Commit: 69fbb491696def14b5ee7dd45913958907d046a9

# events: api-destination dead-letter attributes and the 4xx/5xx retry classification diverge, a rate-limited burst never completes, the Step Functions target execution name drops its UUID suffix, and Lambda-target async destinations never record (4 causes)

> _Note: this issue was identified against the code base at commit `69fbb491696def14b5ee7dd45913958907d046a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Four net-new root causes in the EventBridge target-delivery surface. None of the five checks below was reported as owned by an open issue.

=== Cause 1 of 4: the api-destination dead-letter message attributes are invented and a 4xx is classified as retry exhaustion. Failing check: events.api-destination-4xx-dead-letters-without-retry-while-5xx-is-retried-until-its-retry-attempts-are-used-up ===

AWS: Smithy (eventbridge, PutTargets): Target.DeadLetterConfig.Arn is the SQS DLQ EventBridge writes an undeliverable event to; Target.RetryPolicy.MaximumRetryAttempts has range 0..185 and MaximumEventAgeInSeconds 60..86400, and retries "continue until either the maximum number of attempts is made or until the duration of the MaximumEventAgeInSeconds is met". An api-destination endpoint answering a 4xx is NOT retryable: the event is dead-lettered immediately, with exactly the message attributes ERROR_CODE=`INVALID_PARAMETER`, ERROR_MESSAGE=`ApiDestination returned HTTP status 400 with payload: `, RULE_ARN and TARGET_ARN — and no retry attributes at all. A 5xx IS retryable: the endpoint is called 1+MaximumRetryAttempts times and the event is then dead-lettered with ERROR_CODE=`UNKNOWN`, ERROR_MESSAGE=`ApiDestination returned HTTP status 500 with payload: `, EXHAUSTED_RETRY_CONDITION=`MaximumRetryAttempts`, RETRY_ATTEMPTS=`<MaximumRetryAttempts>`, RULE_ARN and TARGET_ARN. Fixed text of the message: `ApiDestination returned HTTP status ` + ` with payload: `; variable parts: the HTTP status the endpoint answered and the response payload. RETRY_ATTEMPTS comes from the target's RetryPolicy, RULE_ARN/TARGET_ARN from the rule and target. recorded behavior only for the attribute set itself.

Emulator: both the 400 and the 500 dead letters carry the SAME invented ERROR_CODE `ApiDestinationInvocationFailed` and an emulator-authored ERROR_MESSAGE of the form `Api destination <destination name> did not accept the event after <n> attempt(s) at <endpoint url>: the endpoint answered HTTP <status>`, plus RETRY_ATTEMPTS=`0` and EXHAUSTED_RETRY_CONDITION=`MaximumRetryAttempts` — the retry-exhaustion attributes are attached even to the NON-retryable 4xx. So the retryability classification is lost (4xx and 5xx are indistinguishable), the modelled ERROR_CODE vocabulary (INVALID_PARAMETER / UNKNOWN) is not used, the AWS message template is replaced, and RETRY_ATTEMPTS does not reflect the target's RetryPolicy.

Why: a DLQ consumer classifies by ERROR_CODE — INVALID_PARAMETER means "do not retry, fix the request" while UNKNOWN with EXHAUSTED_RETRY_CONDITION means "the endpoint faulted". With one invented code for both, a client cannot tell a client-side mistake from an endpoint outage, cannot recover the HTTP status from the message in AWS's form, and sees retry exhaustion reported for a delivery that was never retried.

Fix: the api-destination target delivery path's retryable-status classification and dead-letter message-attribute construction in the events target dispatcher. The same wrong attribute set shows up under the events__lambda__sqs grouping and is filed separately there (one issue per service token).

Traces: events.api-destination-4xx-dead-letters-without-retry-while-5xx-is-retried-until-its-retry-attempts-are-used-up.

=== Cause 2 of 4: a burst larger than the api destination's invocation rate limit never finishes delivering. Failing check: events.api-destination-rate-limit-defers-a-burst-it-cannot-absorb-and-still-delivers-every-event ===

AWS: Smithy (eventbridge, CreateApiDestination) models InvocationRateLimitPerSecond — the destination throttles invocations to that rate. EventBridge DEFERS the excess rather than dropping it and, within the target's MaximumEventAgeInSeconds, still delivers every matched event exactly once; a 60-event burst at a 1 invocation/second destination arrives complete and spread over at least ~60 seconds.

Emulator: the endpoint never receives the whole burst and the check exhausts the harness budget: `Check timed out after 60.0s`. Hypothesis: the timeout line carries no further diagnosis; an absence of this shape means the deferred events are either dropped at the rate limiter or never resumed after the first second's quota is spent.

Why: a client that sizes a destination's rate limit below its event rate loses events silently instead of having them deferred, which is the whole point of the rate limit.

Fix: the api-destination invocation rate limiter and its deferred-delivery queue in the events target dispatcher.

Traces: events.api-destination-rate-limit-defers-a-burst-it-cannot-absorb-and-still-delivers-every-event.

=== Cause 3 of 4: the Step Functions target's StartExecution name is the bare event id, with no `_<uuid>` suffix. Failing check: events.express-and-standard-state-machine-targets-start-with-the-envelope-as-input-named-after-the-event-id ===

AWS: when an EventBridge rule target is a state machine, EventBridge calls StartExecution with a Name of the form `<event id>_<uuid>` — fixed separator `_`, the delivered envelope's event id before it and a lowercase hyphenated v4 UUID (8-4-4-4-12 hex) after it. This holds for both EXPRESS and STANDARD targets. recorded behavior only for the exact generator.

Emulator: the EXPRESS target's execution name is 36 characters long and is exactly the event id — the envelope id alone, with no `_` separator and no appended UUID. It does not match `^<event id>_[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`.

Why: the UUID suffix is what makes the name unique per delivery. Without it, a redelivery or a second target firing on the same envelope collides on the execution name — on a STANDARD machine that surfaces as ExecutionAlreadyExists and the execution is simply lost — and a client that parses the name to recover the event id plus a per-attempt discriminator cannot.

Fix: the StartExecution name generator in the events Step Functions target dispatcher.

Traces: events.express-and-standard-state-machine-targets-start-with-the-envelope-as-input-named-after-the-event-id.

=== Cause 4 of 4: a Lambda target's asynchronous-invoke destinations never record, so neither the success envelope nor the Lambda-side retry/RetriesExhausted record is observable. Failing checks: events.lambda-target-is-invoked-asynchronously-and-its-on-success-destination-records-the-envelope, events.lambda-target-function-error-is-retried-by-lambda-and-never-dead-lettered-by-the-rule ===

AWS: an EventBridge Lambda target is invoked ASYNCHRONOUSLY (the Lambda-side equivalent of InvocationType=Event), which is why async-only features apply to it. The function's OnSuccess destination therefore records an `Lambda Function Invocation Result - Success` envelope whose detail.requestPayload is the delivered EventBridge envelope and whose detail.responsePayload is the handler's return value. When the handler raises, LAMBDA retries it (1+MaximumRetryAttempts invokes) and writes an OnFailure record with requestContext.condition `RetriesExhausted` carrying the envelope and the error — and the RULE's DeadLetterConfig receives NOTHING, because from EventBridge's point of view the invoke succeeded. (https://docs.aws.amazon.com/lambda/latest/dg/invocation-async.html) recorded behavior only for the exact detail body.

Emulator: neither destination record ever appears for an EventBridge-delivered Lambda target, and both checks exhaust the harness budget: `Check timed out after 60.0s`. Hypothesis: this shares the async-destination dispatch gap recorded for the dynamodb__events__lambda grouping in the companion issue filed in this sweep; it may additionally be that the events Lambda target invokes the function synchronously, in which case no async destination is reachable at all. The timeout lines carry no further diagnosis.

Why: a client wiring a rule to a Lambda target loses the success/failure record it relies on for auditing and for error routing, and cannot distinguish "Lambda retried and gave up" from "the rule dead-lettered" — the two have opposite remediations.

Fix: the invocation type the events Lambda-target dispatcher uses, plus the Lambda asynchronous invoke path's destination dispatch.

Traces: events.lambda-target-is-invoked-asynchronously-and-its-on-success-destination-records-the-envelope, events.lambda-target-function-error-is-retried-by-lambda-and-never-dead-lettered-by-the-rule.

## Evidence
Cause 1 — events.api-destination-4xx-dead-letters-without-retry-while-5xx-is-retried-until-its-retry-attempts-are-used-up
Expected (AWS), 400 target: `{"ERROR_CODE": "INVALID_PARAMETER", "ERROR_MESSAGE": "ApiDestination returned HTTP status 400 with payload: ", "RULE_ARN": <rule arn>, "TARGET_ARN": <target arn>}` and no retry attributes.
Expected (AWS), 500 target: `{"ERROR_CODE": "UNKNOWN", "ERROR_MESSAGE": "ApiDestination returned HTTP status 500 with payload: ", "EXHAUSTED_RETRY_CONDITION": "MaximumRetryAttempts", "RETRY_ATTEMPTS": "<MaximumRetryAttempts>", "RULE_ARN": ..., "TARGET_ARN": ...}` with the endpoint receiving the event 1+MaximumRetryAttempts times.
Actual (emulator), 400 target: `{'ERROR_MESSAGE': 'Api destination <destination name> did not accept the event after 1 attempt(s) at <endpoint url>: the endpoint answered HTTP 400', 'TARGET_ARN': <target arn>, 'ERROR_CODE': 'ApiDestinationInvocationFailed', 'RULE_ARN': <rule arn>, 'RETRY_ATTEMPTS': '0'}`.
Diverging fixed text: `Api destination ` ... ` did not accept the event after ` ... ` attempt(s) at ` ... `: the endpoint answered HTTP ` (AWS: `ApiDestination returned HTTP status ` + ` with payload: `). Diverging code: `ApiDestinationInvocationFailed` (AWS: `INVALID_PARAMETER` for a 4xx, `UNKNOWN` for a 5xx).

Cause 2 — `Error: Check timed out after 60.0s`; of 60 events burst at a destination with InvocationRateLimitPerSecond=1, the endpoint never receives the full set.

Cause 3 — `AssertionError: the EXPRESS execution name must be '<event id>_<uuid>'; got one of length 36 starting with the event id: True` — i.e. the name is the 36-character event id itself, with the `_` separator and the 36-character UUID both absent.

Cause 4 — `Error: Check timed out after 60.0s` on both checks: the OnSuccess destination record for an EventBridge-delivered invoke never appears, and the raising handler produces neither the 1+MaximumRetryAttempts invokes nor the OnFailure `RetriesExhausted` record (while the rule DLQ correctly stays empty, that cannot be graded without the positive side).

## Steps to Reproduce
Cause 1: CreateConnection + CreateApiDestination for two endpoints that answer HTTP 400 and HTTP 500; PutRule and PutTargets with both destinations, each with DeadLetterConfig and RetryPolicy{MaximumRetryAttempts:1, MaximumEventAgeInSeconds:900}; PutEvents a matching entry; read the DLQ message attributes of both letters and the endpoints' hit counts.

Cause 2: CreateApiDestination with InvocationRateLimitPerSecond=1; PutRule/PutTargets at it; PutEvents 60 matching entries in a burst; count distinct deliveries at the endpoint over ~10 minutes.

Cause 3: CreateStateMachine (type EXPRESS) and another (STANDARD); PutRule and PutTargets with both as targets of one rule; PutEvents one matching entry; ListExecutions and compare each execution's name against `<event id>_<uuid>`.

Cause 4: CreateFunction (one handler returning its input, one raising); PutFunctionEventInvokeConfig with an OnSuccess/OnFailure destination; wire the function as a rule target with the rule's own DeadLetterConfig; PutEvents a matching entry; poll the destination and the rule DLQ.

Re-replay the five checks in trace_ids to confirm.
