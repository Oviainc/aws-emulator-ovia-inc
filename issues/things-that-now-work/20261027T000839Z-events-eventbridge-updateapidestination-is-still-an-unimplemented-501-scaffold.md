Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.api-destination-lifecycle
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events|UpdateApiDestination|NotImplemented
Issue-Commit: da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a

# EventBridge UpdateApiDestination is still an unimplemented 501 scaffold stub, so an API destination can never be mutated in place (1 cause)

> _Note: this issue was identified against the code base at commit `da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateApiDestination

## Description
REOPENS 20261025T002357Z-events-eventbridge-rule-state-connection-archive-api-destination-and.md: its fix left the call refused.

That issue is marked fixed, names events.api-destination-lifecycle in its Issue-Trace-Ids, and claimed to stand up the API-destination surface (it implemented CreateApiDestination, ListConnections, UpdateConnection, CreateArchive, TestEventPattern, EnableRule/DisableRule, ListRuleNamesByTarget). The fix left UpdateApiDestination refused: the handler services/service-com-amazonaws-eventbridge-awsevents/src/ops/update_api_destination.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED` marker (line 4) and still answers HTTP 501 `NotImplemented`. This is one cause. Cause count: 1.

The check is marked neither OWNED nor ALREADY FILED. The open issue 20261025T084539Z-events-eventbridge-api-destinations-and-connections-diverge-no-http.md covers six OTHER api-destination/connection causes (no HTTP request is issued, the rate-limit maximum is 1, DeleteConnection revokes destinations, UpdateConnection accepts AuthParameters with no AuthorizationType, and two refusal messages) and nowhere says UpdateApiDestination is refused — its Cause 2 even assumes UpdateApiDestination answers, by measuring the rate-limit bound on 'one of the two paths'.

=== Cause 1 of 1: UpdateApiDestination answers HTTP 501 `NotImplemented`, so a destination cannot be mutated and a mutation against a deleted destination cannot be refused not-found ===

AWS: Smithy (com.amazonaws.eventbridge#UpdateApiDestination) takes Name (ApiDestinationName, @required, @length 1..64, @pattern ^[\.\-_A-Za-z0-9]+$) plus optional Description, ConnectionArn, InvocationEndpoint (HttpsEndpoint), HttpMethod (ApiDestinationHttpMethod enum) and InvocationRateLimitPerSecond (@range min 1), and returns HTTP 200 with {ApiDestinationArn, ApiDestinationState, CreationTime, LastModifiedTime}; the submitted members are applied in place, the ApiDestinationArn is STABLE across the update, and the omitted members keep their prior values. Its modeled error set is ConcurrentModificationException, LimitExceededException and ResourceNotFoundException (all client, HTTP 400) plus InternalException (server, 500) — `NotImplemented` is not a member. For a name that does not exist, the refusal is ResourceNotFoundException whose message is built from the request: the fixed text `Failed to update the api-destination(s). An api-destination '` + the submitted Name + `' does not exist.` — the prefix and the trailing clause are the fixed parts, the destination name is the request-derived part (DescribeApiDestination uses the same template with `Failed to describe the api-destination(s).`).
Emulator: `botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UpdateApiDestination operation: operation UpdateApiDestination is scaffolded but not implemented in this emulator` — HTTP 501, error code `NotImplemented`, and a message whose only variable part is the operation name; it echoes nothing from the request or from state, where every legitimate refusal AWS has here carries the submitted Name. The check reaches it after CreateConnection, the AUTHORIZED poll, CreateApiDestination (which returns a state in {ACTIVE, INACTIVE}), the ACTIVE poll, DescribeApiDestination (HttpMethod POST, InvocationRateLimitPerSecond 10, ConnectionArn echoed) and ListApiDestinations(NamePrefix) all succeeded, so everything before the update is already working.
Why: a client relying on AWS's behavior cannot change a destination's endpoint, HTTP method, rate limit or connection at all — the only alternative is delete-and-recreate, which mints a new ApiDestinationArn and breaks every rule target that names the old one. And because the error code is outside the SDK's modeled set for the operation, no `except ResourceNotFoundException` / `except ConcurrentModificationException` clause matches it and generic retry logic retries a permanent failure. The deleted-destination contract (a mutation on a deleted destination must answer ResourceNotFoundException with the `Failed to update the api-destination(s).` message) is also unreachable.
Fix: implement the operation in its own handler, services/service-com-amazonaws-eventbridge-awsevents/src/ops/update_api_destination.rs, over the crate's api-destination state (resolve Name, refuse an unknown name with the modeled 400 ResourceNotFoundException and AWS's message template, validate a submitted ConnectionArn against the connection store, apply only the submitted members, leave ApiDestinationArn and CreationTime unchanged, move LastModifiedTime, return 200 with the modeled output), and delete the SCAFFOLDED marker line.
Traces: events.api-destination-lifecycle.

## Evidence
events.api-destination-lifecycle [error]:

  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UpdateApiDestination operation: operation UpdateApiDestination is scaffolded but not implemented in this emulator

Raised at the check's `ev.update_api_destination(Name=<dest>, HttpMethod='PUT', InvocationRateLimitPerSecond=5)` — i.e. AFTER create_connection(AuthorizationType='API_KEY', ApiKeyAuthParameters={...}), the ConnectionState poll, create_api_destination(ConnectionArn=..., InvocationEndpoint='https://example.com/webhook', HttpMethod='POST', InvocationRateLimitPerSecond=10), the ApiDestinationState=='ACTIVE' poll, describe_api_destination (HttpMethod 'POST', InvocationRateLimitPerSecond 10, ConnectionArn equal to the created connection's) and list_api_destinations(NamePrefix=<dest>) had all succeeded.

Expected (AWS): HTTP 200 with {ApiDestinationArn (byte-identical to the ARN CreateApiDestination returned), ApiDestinationState, CreationTime, LastModifiedTime}; a following describe_api_destination then reports HttpMethod 'PUT' and InvocationRateLimitPerSecond 5. Then, after delete_api_destination, describe_api_destination answers ('ResourceNotFoundException', "Failed to describe the api-destination(s). An api-destination '<Name>' does not exist.") and update_api_destination answers ('ResourceNotFoundException', "Failed to update the api-destination(s). An api-destination '<Name>' does not exist.") — the quoted prefixes and trailing clause are the fixed text, `<Name>` is taken from the request.
Actual (emulator): HTTP 501, error code `NotImplemented`, message `operation UpdateApiDestination is scaffolded but not implemented in this emulator`. The fixed part of that message is "operation <Op> is scaffolded but not implemented in this emulator"; its only variable part is the operation name. None of the in-place mutation, the ARN stability or the two not-found refusals is graded.

Source evidence: services/service-com-amazonaws-eventbridge-awsevents/src/ops/update_api_destination.rs line 4 — `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 \`NotImplemented\` and asserts NOTHING about \`UpdateApiDestination\`'s real behaviour.` The modeled error set for UpdateApiDestination (ConcurrentModificationException / LimitExceededException / ResourceNotFoundException at 400, InternalException at 500) contains no `NotImplemented`. Per-run values in the trace (the generated connection/destination names and the account id) are not part of the requirement.

## Steps to Reproduce
1. Boot the emulator; create an events client.
2. ev.create_connection(Name=C, AuthorizationType='API_KEY', AuthParameters={'ApiKeyAuthParameters': {'ApiKeyName': 'x-api-key', 'ApiKeyValue': 'secret-value'}}) -> note ConnectionArn; poll describe_connection until ConnectionState is AUTHORIZED or CREATE_FAILED.
3. ev.create_api_destination(Name=D, ConnectionArn=<C's arn>, InvocationEndpoint='https://example.com/webhook', HttpMethod='POST', InvocationRateLimitPerSecond=10) -> note ApiDestinationArn; poll describe_api_destination until ApiDestinationState == 'ACTIVE'.
4. ev.update_api_destination(Name=D, HttpMethod='PUT', InvocationRateLimitPerSecond=5) -> the emulator answers HTTP 501 `NotImplemented` ("operation UpdateApiDestination is scaffolded but not implemented in this emulator"); AWS answers 200 with {ApiDestinationArn, ApiDestinationState, CreationTime, LastModifiedTime}.
5. (Blocked by step 4, and what must pass after the fix) ev.describe_api_destination(Name=D) -> HttpMethod 'PUT', InvocationRateLimitPerSecond 5, ApiDestinationArn unchanged from step 3.
6. ev.delete_api_destination(Name=D); then ev.update_api_destination(Name=D, HttpMethod='GET') -> expect 400 ResourceNotFoundException with message "Failed to update the api-destination(s). An api-destination '<D>' does not exist."
Replay events.api-destination-lifecycle to confirm a fix.
