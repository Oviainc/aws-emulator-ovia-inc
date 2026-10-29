Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.api-destination-4xx-dead-letters-without-retry-while-5xx-is-retried-until-its-retry-attempts-are-used-up
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events.api-destination-4xx-dead-letters-without-retry-while-5xx-is-retried-until-its-retry-attempts-are-used-up|run#21|TimeoutError|raise TimeoutError("an event dead-lettered by both targets: the run budget ended first; not graded")
Issue-Commit: f60279ee1b26e62c04ba7bfaecc753f1127e63e1

# EventBridge api-destination targets never deliver, so the 4xx-vs-5xx retry classification and the dead-letter attributes are never produced and the check exhausts its whole run budget ungraded

> _Note: this issue was identified against the code base at commit `f60279ee1b26e62c04ba7bfaecc753f1127e63e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Cause 1 of 1: an EventBridge rule target whose Arn is an api-destination ARN never issues an HTTP request, so neither the non-retryable 4xx dead-letter nor the retried-then-exhausted 5xx dead-letter is ever produced; the check burns its entire delivery window (360 s, capped by the run budget) and ends with the harness's own ungraded TimeoutError. Failing check id: events.api-destination-4xx-dead-letters-without-retry-while-5xx-is-retried-until-its-retry-attempts-are-used-up.

AWS: Smithy models the target-side contract that this behavior hangs off — PutTargets.Target carries RetryPolicy{MaximumRetryAttempts @range(min 0, max 185), MaximumEventAgeInSeconds @range(min 60, max 86400)} and DeadLetterConfig{Arn -> ResourceArn @length(1..1600), "The ARN of the SQS queue specified as the target for the dead-letter queue"}. The delivery mechanics themselves are recorded behavior only (the model has no shape for them): for one event matched by a rule with two api-destination targets that differ only in the status their endpoint returns, EventBridge invokes the destination's InvocationEndpoint with the destination's HttpMethod; a 4xx response is NOT retried and the event is dead-lettered immediately with SQS message attributes ERROR_CODE = "INVALID_PARAMETER", ERROR_MESSAGE = "ApiDestination returned HTTP status <status> with payload: <payload>", RULE_ARN = the rule's ARN and TARGET_ARN = the api-destination ARN; a 5xx response IS retried until the target's MaximumRetryAttempts is used up and its dead letter additionally carries ERROR_CODE = "UNKNOWN", EXHAUSTED_RETRY_CONDITION = "MaximumRetryAttempts" and RETRY_ATTEMPTS = the configured MaximumRetryAttempts rendered as a decimal string. In those messages the fixed text is "ApiDestination returned HTTP status " / " with payload: ", the attribute names, the literals INVALID_PARAMETER / UNKNOWN / MaximumRetryAttempts; the variable parts are the status code the endpoint returned, the response payload, the rule ARN, the api-destination ARN and the configured retry count taken from the request. Endpoint host/port, rule and destination names and ARNs are per-environment and are not part of the requirement.

Emulator: PutTargets accepts both api-destination targets with FailedEntryCount 0, then the receiving endpoint records no request at all and the dead-letter queue stays empty for the whole window. No event is ever dead-lettered by either target, so the check cannot grade and raises TimeoutError: "an event dead-lettered by both targets: the run budget ended first; not graded" (check_events_api_destination_4xx_dead_letters_without_retry_while_5xx_is_retried_until_its_retry_attempts_are_used_up.py line 77). The emulator caused this: the publishing of the event succeeds (PutEvents FailedEntryCount 0) and it is the emulator's target dispatcher that never invokes the destination and never writes a dead letter.

Why: a client that relies on EventBridge's api-destination failure semantics gets silence instead. A permanent 4xx misconfiguration is never surfaced on the DLQ, a transient 5xx is never retried, and nothing distinguishes "delivered" from "dropped" — error-handling, alerting and retry budgeting built on ERROR_CODE / EXHAUSTED_RETRY_CONDITION / RETRY_ATTEMPTS never fire.

Fix: the api-destination branch of the eventbridge rule-target dispatcher (the HTTP invocation of the destination's InvocationEndpoint), plus its retry classification (4xx non-retryable, 5xx retried to MaximumRetryAttempts) and the dead-letter writer that stamps the ERROR_CODE / ERROR_MESSAGE / RULE_ARN / TARGET_ARN / EXHAUSTED_RETRY_CONDITION / RETRY_ATTEMPTS message attributes onto the DeadLetterConfig queue.

Related already-open issue (does not carry this run's replay key, so this failure is not covered by it): 20261025T084539Z-events-eventbridge-api-destinations-and-connections-diverge-no-http.md names the same check in its trace ids under its Cause 1, but its recorded Issue-Key for this check is `_receive#18|Timeout` whereas this run fails earlier, at run#77 with the harness's ungraded TimeoutError, so the replay report lists this check as neither OWNED nor ALREADY FILED. The fixed issues 20261025T002357Z-events-eventbridge-rule-state-connection-archive-api-destination-and.md, 20261025T084539Z-events-sqs-archive-replay-is-unreachable-startreplay-is.md and 20261027T194338Z-events-eventbridge-connections-createconnection-s-non-https-authorizationendpoint-refusal.md do not restore this delivery path — this failure is a timeout, not a refusal of an operation they claimed to fix.

Traces: events.api-destination-4xx-dead-letters-without-retry-while-5xx-is-retried-until-its-retry-attempts-are-used-up

## Evidence
Failing check events.api-destination-4xx-dead-letters-without-retry-while-5xx-is-retried-until-its-retry-attempts-are-used-up — verdict [error]:

  File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
  File "check_events_api_destination_4xx_dead_letters_without_retry_while_5xx_is_retried_until_its_retry_attempts_are_used_up.py", line 77, in run
    raise TimeoutError("an event dead-lettered by both targets: the run budget ended first; not graded")
  TimeoutError: an event dead-lettered by both targets: the run budget ended first; not graded

Setup that the emulator accepted before going silent (from the check source): CreateConnection(AuthorizationType='API_KEY', ApiKeyAuthParameters={ApiKeyName:'x-sim2real-key', ...}); two api destinations on that connection pointing at <base>/s400 and <base>/s500; a custom bus and a rule matching {"source": [<unique source>]}; PutTargets with both destination ARNs, a RoleArn allowing events:InvokeApiDestination, DeadLetterConfig={Arn: <dlq arn>} and RetryPolicy={MaximumRetryAttempts: 1, MaximumEventAgeInSeconds: 900} — PutTargets answered FailedEntryCount 0. PutEvents answered FailedEntryCount 0 on every republish (the check republishes every 10 s for the whole window).

Expected (AWS), asserted by the check once a dead letter exists:
  400 target's dead-letter attributes == {"ERROR_CODE": "INVALID_PARAMETER", "ERROR_MESSAGE": "ApiDestination returned HTTP status 400 with payload: ", "RULE_ARN": <rule arn>, "TARGET_ARN": <400 destination arn>} and the /s400 endpoint received the event at least once (no retry).
  500 target's dead-letter attributes == {"ERROR_CODE": "UNKNOWN", "ERROR_MESSAGE": "ApiDestination returned HTTP status 500 with payload: ", "EXHAUSTED_RETRY_CONDITION": "MaximumRetryAttempts", "RETRY_ATTEMPTS": "1", "RULE_ARN": <rule arn>, "TARGET_ARN": <500 destination arn>} and the /s500 endpoint received the event at least twice (one retry).

Actual (emulator): 0 requests recorded at either endpoint and 0 messages on the dead-letter queue for the entire _DELIVERY_TIMEOUT_S = 360 s window, so neither assertion was ever reachable.

Smithy citation: PutTargets.Target.RetryPolicy.MaximumRetryAttempts @range(min 0, max 185); PutTargets.Target.RetryPolicy.MaximumEventAgeInSeconds @range(min 60, max 86400); PutTargets.Target.DeadLetterConfig.Arn -> ResourceArn @length(min 1, max 1600).

## Steps to Reproduce
1. Boot the emulator and create events, sqs and iam clients.
2. Stand up an HTTPS endpoint that records the requests it receives and answers 400 on path /s400 and 500 on path /s500.
3. Create an SQS queue to serve as the dead-letter queue, with a policy allowing events.amazonaws.com to sqs:SendMessage.
4. CreateConnection(AuthorizationType='API_KEY', AuthParameters={'ApiKeyAuthParameters': {'ApiKeyName': 'x-sim2real-key', 'ApiKeyValue': <any>}}).
5. CreateApiDestination twice on that connection, with InvocationEndpoint <base>/s400 and <base>/s500.
6. Create a custom event bus and a rule with EventPattern {"source": [<unique source>]}; PutTargets both destination ARNs with RoleArn (events:InvokeApiDestination on both destinations), DeadLetterConfig={'Arn': <dlq arn>} and RetryPolicy={'MaximumRetryAttempts': 1, 'MaximumEventAgeInSeconds': 900}. Observe FailedEntryCount 0.
7. PutEvents one entry on that bus with that Source and DetailType 'status'. Poll the recording endpoint and the DLQ.
8. Expect: one request at /s400 and the event dead-lettered immediately with ERROR_CODE INVALID_PARAMETER; at least two requests at /s500 and a dead letter carrying ERROR_CODE UNKNOWN, EXHAUSTED_RETRY_CONDITION MaximumRetryAttempts and RETRY_ATTEMPTS 1, both with RULE_ARN and TARGET_ARN. Observed: the endpoint records nothing and the DLQ stays empty until the run budget ends.
9. Re-replay events.api-destination-4xx-dead-letters-without-retry-while-5xx-is-retried-until-its-retry-attempts-are-used-up to confirm the fix.
