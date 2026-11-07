Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs.events-delivery-and-failure-event-age-limit-dead-letters-without-further-attempts
Issue-Key: events__lambda__sqs.events-delivery-and-failure-event-age-limit-dead-letters-without-further-attempts|run#38|AssertionError|assert "HTTP status 500" in (t_attrs.get("ERROR_MESSAGE") or ""), f"twin DLQ attrs {t_attrs!r}"
Issue-Commit: 69fbb491696def14b5ee7dd45913958907d046a9

# events__lambda__sqs: an api-destination dead letter carries an invented ERROR_CODE and an emulator-authored ERROR_MESSAGE instead of AWS's status-bearing attributes (1 cause)

> _Note: this issue was identified against the code base at commit `69fbb491696def14b5ee7dd45913958907d046a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Cause 1 of 1: the dead-letter message attributes EventBridge writes for an api-destination target whose endpoint answers a retryable 5xx do not use AWS's error code or message template. Failing check: events__lambda__sqs.events-delivery-and-failure-event-age-limit-dead-letters-without-further-attempts.

AWS: Smithy (eventbridge, PutTargets): Target.RetryPolicy.MaximumRetryAttempts has range 0..185 and MaximumEventAgeInSeconds range 60..86400, and retries "continue until either the maximum number of attempts is made or until the duration of the MaximumEventAgeInSeconds is met"; Target.DeadLetterConfig.Arn is the SQS queue the undeliverable event is written to. When an api destination's endpoint answers HTTP 5xx and the event is finally dead-lettered, the DLQ message attributes are ERROR_CODE=`UNKNOWN`, ERROR_MESSAGE=`ApiDestination returned HTTP status 500 with payload: ` (fixed text `ApiDestination returned HTTP status ` + ` with payload: `, with the status the endpoint answered and the response payload as the variable parts), EXHAUSTED_RETRY_CONDITION naming which bound was hit (`MaximumRetryAttempts` or `MaximumEventAgeInSeconds`), RETRY_ATTEMPTS = the attempts actually made, RULE_ARN and TARGET_ARN. The whole point of the check is that the HTTP status is recoverable from ERROR_MESSAGE and that EXHAUSTED_RETRY_CONDITION/RETRY_ATTEMPTS reflect WHICH bound ended the delivery.

Emulator: the letter carries ERROR_CODE `ApiDestinationInvocationFailed` — a code AWS does not use — and an emulator-authored ERROR_MESSAGE of the form `Api destination <destination name> did not accept the event after <n> attempt(s) at <endpoint url>: the endpoint answered HTTP <status>`, with RETRY_ATTEMPTS `0` even though the target was configured with MaximumRetryAttempts=185 and an age bound, and EXHAUSTED_RETRY_CONDITION `MaximumRetryAttempts` where the age bound is the one that should have ended it. AWS's `HTTP status <code>` phrasing is absent, so a consumer matching on it finds nothing.

Why: a DLQ consumer reads ERROR_CODE to decide whether to retry the event itself, and reads ERROR_MESSAGE/EXHAUSTED_RETRY_CONDITION/RETRY_ATTEMPTS to tell an endpoint outage that exhausted attempts from one that exhausted the event's age. With a single invented code, a rewritten message and RETRY_ATTEMPTS pinned to 0, none of those decisions can be made, and an event-age limit is indistinguishable from an attempt limit.

Fix: the api-destination dead-letter message-attribute construction and the retry-bound bookkeeping (which bound was exhausted, how many attempts were made) in the events target dispatcher. This is the same defect surface as the api-destination cause filed in this sweep under the `events` service token; it is filed separately here because an issue carries one service token.

Traces: events__lambda__sqs.events-delivery-and-failure-event-age-limit-dead-letters-without-further-attempts.

## Evidence
Expected (AWS): the DLQ letter for the api-destination twin carries `ERROR_CODE: "UNKNOWN"` and an `ERROR_MESSAGE` containing `HTTP status 500` in AWS's form `ApiDestination returned HTTP status 500 with payload: `, with `EXHAUSTED_RETRY_CONDITION` naming the bound actually hit and `RETRY_ATTEMPTS` the attempts actually made.
Actual (emulator): `AssertionError: twin DLQ attrs {'TARGET_ARN': 'arn:aws:events:<region>:<account>:api-destination/<name>/<id>', 'ERROR_CODE': 'ApiDestinationInvocationFailed', 'ERROR_MESSAGE': 'Api destination <name> did not accept the event after 1 attempt(s) at <endpoint url>: the endpoint answered HTTP 500', 'RETRY_ATTEMPTS': '0', 'RULE_ARN': 'arn:aws:events:<region>:<account>:rule/<rule>', 'EXHAUSTED_RETRY_CONDITION': 'MaximumRetryAttempts'}`.
Fixed text that diverges: `Api destination ` / ` did not accept the event after ` / ` attempt(s) at ` / `: the endpoint answered HTTP ` (AWS: `ApiDestination returned HTTP status ` + ` with payload: `). Values taken from request/state in the template: the destination name, the endpoint URL, the attempt count and the HTTP status; the recorded ARNs, name and id are per-run values, not part of the requirement.
Diverging code: `ApiDestinationInvocationFailed` (AWS: `UNKNOWN` for a 5xx). Diverging values: RETRY_ATTEMPTS `0` against a target configured with MaximumRetryAttempts=185.

## Steps to Reproduce
1. CreateConnection with an API-key auth parameter and CreateApiDestination at an endpoint that records every request and answers HTTP 500.
2. PutRule, then PutTargets with that destination plus DeadLetterConfig and RetryPolicy{MaximumRetryAttempts: 185, MaximumEventAgeInSeconds: 60}.
3. PutEvents a matching entry and wait for the event's age bound to be reached.
4. ReceiveMessage on the DLQ with MessageAttributeNames=['All'] and compare ERROR_CODE, ERROR_MESSAGE, EXHAUSTED_RETRY_CONDITION and RETRY_ATTEMPTS against AWS's.
5. Re-replay the check in trace_ids to confirm.
