Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.api-destination-4xx-dead-letters-without-retry-while-5xx-is-retried-until-its-retry-attempts-are-used-up events.api-destination-api-key-connection-adds-its-key-and-parameters-and-its-values-win-over-the-targets events.api-destination-basic-connection-sends-the-basic-authorization-header-with-the-destinations-method-and-the-bare-envelope events.api-destination-oauth-connection-fetches-a-client-credentials-token-and-sends-it-as-the-bearer-header events.api-destination-rate-limit-defers-a-burst-it-cannot-absorb-and-still-delivers-every-event events.api-destination-invocation-rate-limit-maximum-is-enforced events.delete-connection-in-use-by-api-destination-allowed events.api-destination-state-follows-connection-deauthorize-and-reauthorize events.create-api-destination-requires-existing-connection events.list-connections-filters-are-mutually-exclusive
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events.api-destination-4xx-dead-letters-without-retry-while-5xx-is-retried-until-its-retry-attempts-are-used-up|_receive#18|Timeout|for msg in sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=5,
Issue-Key: events.api-destination-api-key-connection-adds-its-key-and-parameters-and-its-values-win-over-the-targets|_receive#12|Timeout|for msg in sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=5,
Issue-Key: events.api-destination-basic-connection-sends-the-basic-authorization-header-with-the-destinations-method-and-the-bare-envelope|_receive#13|Timeout|for msg in sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=5,
Issue-Key: events.api-destination-oauth-connection-fetches-a-client-credentials-token-and-sends-it-as-the-bearer-header|_receive#17|Timeout|for msg in sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=5,
Issue-Key: events.api-destination-rate-limit-defers-a-burst-it-cannot-absorb-and-still-delivers-every-event|_receive#14|Timeout|for msg in sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=5,
Issue-Key: events.api-destination-invocation-rate-limit-maximum-is-enforced|run#15|AssertionError|assert _FLOOR <= update_max < _CEILING, (
Issue-Key: events.delete-connection-in-use-by-api-destination-allowed|run#13|AssertionError|assert still["ApiDestinationState"] == "ACTIVE", (
Issue-Key: events.api-destination-state-follows-connection-deauthorize-and-reauthorize|_error#13|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: events.create-api-destination-requires-existing-connection|run#14|AssertionError|assert err == expected, f"expected {expected}, got {err}"
Issue-Key: events.list-connections-filters-are-mutually-exclusive|run#11|AssertionError|assert err == ("ValidationException", _COMBINED_FILTER_REFUSAL), (
Issue-Commit: 8b92040b06acdac381093d8b397331a442bd94ad

# EventBridge API destinations and connections diverge: no HTTP request is ever issued to a destination, the rate-limit maximum is 1, DeleteConnection revokes its destinations, an incomplete UpdateConnection is accepted, and two refusal messages are rewritten (6 causes)

> _Note: this issue was identified against the code base at commit `8b92040b06acdac381093d8b397331a442bd94ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApiDestination

## Description
The API-destination/connection surface answers calls but diverges from AWS in six independent ways. The open issue 20261025T002357Z-events-eventbridge-target-delivery-never-completes-for-sns-step.md covers SNS/Step Functions/cross-region/input-transformer/trace-header/fan-out targets, NOT api-destination (HTTP) targets, and 20261025T002357Z-events-eventbridge-refusal-codes-and-messages-diverge-from-aws.md and ...-state-and-representation-diverge-... do not name any of the operations below; the replay report marks none of these checks OWNED or ALREADY FILED.

Cause 1 of 6: an EventBridge api-destination target never issues an HTTP request, so no api-destination delivery, retry classification, parameter merge or rate limiting is ever observable.
AWS: recorded behavior only for the delivery mechanics (the Smithy model has no shape for them). AWS's documented and audited behavior: a matched event on a rule whose target Arn is an api-destination ARN makes EventBridge POST (or use the destination's HttpMethod) to the destination's InvocationEndpoint, with the connection's auth material and InvocationHttpParameters merged with the target's HttpParameters (the connection's value winning on a collision), content-type `application/json; charset=utf-8` and user agent `Amazon/EventBridge/ApiDestinations`; a 4xx response is non-retryable and dead-letters immediately while a 5xx is retried up to the target's MaximumRetryAttempts, with the DLQ message carrying ERROR_CODE / EXHAUSTED_RETRY_CONDITION / RETRY_ATTEMPTS attributes.
Emulator: all five delivery checks hang until the 60 s harness cap with the receiving endpoint never recording a request and the DLQ never receiving a message — PutTargets accepts the api-destination target (FailedEntryCount 0) and nothing is ever invoked. Hypothesis: the api-destination target type has no delivery path at all in the emulator's target dispatcher; the shared receiving endpoint in these checks is a Lambda function URL, so a Lambda-side defect (see the open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md) could also account for the silence — either way the emulator never completes the delivery.
Why: every HTTP/webhook integration built on API destinations silently does nothing, and the retry/dead-letter semantics a client depends on to detect failure never fire.
Fix: the api-destination branch of the eventbridge rule-target dispatcher, its connection-auth/parameter merge, and its retry/dead-letter classification.
Traces: events.api-destination-4xx-dead-letters-without-retry-while-5xx-is-retried-until-its-retry-attempts-are-used-up, events.api-destination-api-key-connection-adds-its-key-and-parameters-and-its-values-win-over-the-targets, events.api-destination-basic-connection-sends-the-basic-authorization-header-with-the-destinations-method-and-the-bare-envelope, events.api-destination-oauth-connection-fetches-a-client-credentials-token-and-sends-it-as-the-bearer-header, events.api-destination-rate-limit-defers-a-burst-it-cannot-absorb-and-still-delivers-every-event.

Cause 2 of 6: the accepted maximum for InvocationRateLimitPerSecond is 1, so ordinary rate limits are refused.
AWS: Smithy models ApiDestinationInvocationRateLimitPerSecond with @range(min: 1) and no maximum, so the upper bound is a service-side rule; AWS documents the usable range as 1..300 events per second (https://aws.amazon.com/blogs/compute/using-api-destinations-with-amazon-eventbridge/). The requirement asserted is structural: the enforced maximum must be finite and well above 1, and Create and Update must enforce the SAME maximum.
Emulator: the measured maximum accepted InvocationRateLimitPerSecond is 1 — every larger value is refused on one of the two paths.
Why: a client setting any realistic rate limit (10, 300) is refused a request AWS accepts, so the destination cannot be created/updated at all.
Fix: InvocationRateLimitPerSecond validation in the CreateApiDestination and UpdateApiDestination handlers (both must share one bound).
Traces: events.api-destination-invocation-rate-limit-maximum-is-enforced.

Cause 3 of 6: DeleteConnection revokes the API destinations that reference it.
AWS: recorded behavior only. Real EventBridge permits deleting a connection that an API destination still references and does NOT revoke the destination: DescribeApiDestination still answers ApiDestinationState `ACTIVE` with the now-dangling ConnectionArn.
Emulator: after DeleteConnection, DescribeApiDestination answers HTTP 200 with ApiDestinationState `INACTIVE`.
Why: a client that deletes and recreates a connection sees its destinations go INACTIVE and stop delivering, a state transition AWS never performs.
Fix: the DeleteConnection handler must not cascade into api-destination state; api-destination state derivation in the eventbridge state layer.
Traces: events.delete-connection-in-use-by-api-destination-allowed.

Cause 4 of 6: UpdateConnection naming AuthParameters without an AuthorizationType is accepted.
AWS: recorded behavior only. AWS refuses an UpdateConnection that supplies AuthParameters (e.g. BasicAuthParameters) without the AuthorizationType that selects them, and the connection is left in its prior (DEAUTHORIZED) state.
Emulator: HTTP 200 with ConnectionState `UPDATING` and a refreshed LastModifiedTime — the call succeeds and the connection moves state.
Why: a malformed update is silently applied, so a client cannot distinguish a rejected credential rotation from a successful one and the connection's authorization state is wrong.
Fix: UpdateConnection input validation in the eventbridge crate (AuthParameters must be validated against AuthorizationType) and the connection state machine it drives.
Traces: events.api-destination-state-follows-connection-deauthorize-and-reauthorize.

Cause 5 of 6: CreateApiDestination's ResourceNotFoundException message does not name the connection ARN.
AWS: Smithy models ResourceNotFoundException (400) on CreateApiDestination. The message is built from the request: AWS answers `Failed to create the api-destination(s). Connection '<the ConnectionArn as submitted>' does not exist.` The rule — the `Failed to create the api-destination(s).` prefix and the submitted connection ARN echoed back — is the requirement; the recorded string is an example.
Emulator: the code is right (ResourceNotFoundException) but the message is `Connection '<bare connection name>' does not exist.` — the prefix is dropped and the submitted ARN is replaced by a name the request did not contain.
Why: a client that logs or parses the refusal cannot tell which ARN it submitted was rejected, and the ARN-vs-name substitution shows the handler is resolving the connection by name rather than by the submitted ARN.
Fix: the CreateApiDestination handler's connection-lookup error construction in the eventbridge crate.
Traces: events.create-api-destination-requires-existing-connection.

Cause 6 of 6: ListConnections' combined-filter refusal message is generic.
AWS: Smithy models ListConnections with {NamePrefix, ConnectionState, NextToken, Limit}; AWS refuses a request naming more than one filter with ValidationException and a message that NAMES the two mutually exclusive members: `Failed to list the connection(s). Request failed because only one filter is supported per request, either ConnectionState or NamePrefix.` Because the message names the members of this operation's filter set, that rule is the requirement; the recorded string is the example.
Emulator: code ValidationException is right, message is the operation-agnostic `At most one filter is allowed for this request. Please use only one filter.` — it names neither member.
Why: the refusal does not tell the caller which parameters conflicted, so a generic handler cannot be driven off it.
Fix: the ListConnections handler's filter-exclusivity validation message in the eventbridge crate.
Traces: events.list-connections-filters-are-mutually-exclusive.

## Evidence
Cause 1 (5 checks, all [mismatch] / `Check timed out after 60.0s`): PutTargets accepts the api-destination target with FailedEntryCount 0, then the receiving endpoint records no request and the DLQ stays empty for the whole window, so the checks never reach their assertions. Expected: a POST/HTTP request at the destination's InvocationEndpoint carrying the connection's auth header, the merged connection+target header/query/body parameters, content-type `application/json; charset=utf-8`, user agent `Amazon/EventBridge/ApiDestinations`; a 400 response dead-letters with no retry, a 500 response is retried up to MaximumRetryAttempts and its DLQ message carries EXHAUSTED_RETRY_CONDITION=MaximumRetryAttempts and RETRY_ATTEMPTS=1. Actual: no request, no dead letter.

Cause 2 — events.api-destination-invocation-rate-limit-maximum-is-enforced [mismatch]:
AssertionError: the measured maximum InvocationRateLimitPerSecond must be finite and no smaller than 10 (the documented range is 1..300); measured 1

Cause 3 — events.delete-connection-in-use-by-api-destination-allowed [mismatch]:
AssertionError: API destination should stay ACTIVE even though its connection was just deleted from under it (real EventBridge does not revoke on dangling ref): {... 'ApiDestinationState': 'INACTIVE', 'ConnectionArn': 'arn:aws:events:...:connection/...', 'HTTPStatusCode': 200 ...}
Expected ApiDestinationState 'ACTIVE'; actual 'INACTIVE'.

Cause 4 — events.api-destination-state-follows-connection-deauthorize-and-reauthorize [mismatch]:
AssertionError: expected the call to be refused, but it succeeded: {'ConnectionArn': 'arn:aws:events:...:connection/...', 'ConnectionState': 'UPDATING', 'CreationTime': ..., 'LastModifiedTime': ..., 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
Call: update_connection(Name=<conn>, AuthParameters={'BasicAuthParameters': {...}}) with no AuthorizationType. Expected: a 400 client refusal. Actual: HTTP 200, ConnectionState UPDATING.

Cause 5 — events.create-api-destination-requires-existing-connection [mismatch]:
AssertionError: expected ('ResourceNotFoundException', "Failed to create the api-destination(s). Connection 'arn:aws:events:us-west-2:...:connection/sim2real-behav-no-conn-.../00000000-0000-0000-0000-000000000000' does not exist."), got ('ResourceNotFoundException', "Connection 'sim2real-behav-no-conn-...' does not exist.")

Cause 6 — events.list-connections-filters-are-mutually-exclusive [mismatch]:
AssertionError: expected ValidationException when combining ListConnections filters, got ('ValidationException', 'At most one filter is allowed for this request. Please use only one filter.')
Expected message rule: `Failed to list the connection(s). Request failed because only one filter is supported per request, either ConnectionState or NamePrefix.`

## Steps to Reproduce
1. Boot the emulator; create events/sqs/iam/lambda clients.
2. Cause 1: create an HTTPS endpoint that records requests; create_connection(AuthorizationType='API_KEY', ...); create_api_destination(ConnectionArn=..., InvocationEndpoint=<endpoint>/orders/*, HttpMethod='POST'); create a custom bus + rule; put_targets with the api-destination ARN, a RoleArn allowing events:InvokeApiDestination, DeadLetterConfig and RetryPolicy; put_events a matching event. Observe that the endpoint records nothing and the DLQ stays empty.
3. Cause 2: create_api_destination(..., InvocationRateLimitPerSecond=N) and update_api_destination(..., InvocationRateLimitPerSecond=N) for increasing N; observe that N=1 is the largest accepted value on one of the two paths.
4. Cause 3: create a connection and an api destination on it; delete_connection(Name=<conn>); describe_api_destination(Name=<dest>) -> ApiDestinationState is INACTIVE (AWS: ACTIVE).
5. Cause 4: create a BASIC connection, deauthorize_connection, then update_connection(Name=<conn>, AuthParameters={'BasicAuthParameters': {...}}) with no AuthorizationType -> HTTP 200 (AWS refuses).
6. Cause 5: create_api_destination(ConnectionArn=<ARN of a connection that does not exist>, ...) -> inspect Error.Message.
7. Cause 6: list_connections(NamePrefix='x', ConnectionState='AUTHORIZED') -> inspect Error.Message.
Replay the ten check ids in trace_ids to confirm.

## Resolution
Causes 2, 3, 5 and 6 are fixed and their checks pass: the
InvocationRateLimitPerSecond maximum is the documented 300 on both the create and
the update path, deleting a connection no longer revokes the api destinations
that name it (their ApiDestinationState is taken from the connection's
authorization when the destination is written or repointed and carried forward
when that connection's own transition settles), CreateApiDestination's
ResourceNotFoundException echoes the submitted ConnectionArn after the
`Failed to create the api-destination(s).` clause, and ListConnections' combined
filter refusal names ConnectionState and NamePrefix.

Cause 4 is refused as AWS refuses it — HTTP 400 ValidationException, nothing
written, the connection left in the state it had — with the sentence
`Failed to update the connection(s). Parameter AuthParameters is not valid.
Reason: AuthorizationType must be provided when AuthParameters is provided.` No
recorded sentence for this refusal is quoted above, so if the recording words it
differently that wording is the only part of this cause still outstanding.

Cause 1 is served in part: an api-destination target IS invoked — the
destination's HttpMethod request to its InvocationEndpoint, through the
in-process emulator transport, carrying the connection's authorization (an API
key under the header the connection names, or the RFC 7617
`Authorization: Basic` header), the connection's InvocationHttpParameters merged
with the target's HttpParameters (the connection's value winning on a collision,
path wildcards filled from PathParameterValues), the content type and user agent
AWS sends, a 4xx taken as non-retryable and a 5xx retried within the target's
MaximumRetryAttempts before the event is dead-lettered. Its five delivery checks
still cannot pass here, and what each of them waits for is tracked where it
belongs rather than in this record:

* the receiving endpoint of all five is a Lambda function URL whose function has
  to execute to record the request and answer 400/500 —
  `20261103T165236Z-lambda-invoke-does-not-execute-the-deployment-package.md`;
* the OAuth client-credentials token exchange and the `Bearer` header —
  `20261027T200000Z-events-eventbridge-an-oauth-client-credentials-connection-never-fetches-a-token-so-its-api-destination-never-delivers.md`;
* the dead-letter ERROR_CODE/ERROR_MESSAGE wording and the 4xx-vs-5xx retry
  accounting AWS stamps on the DLQ message —
  `20261029T092208Z-events-eventbridge-api-destination-targets-never-deliver-so-the.md`;
* the per-destination InvocationRateLimitPerSecond deferral of a burst, which no
  other open record names yet.
