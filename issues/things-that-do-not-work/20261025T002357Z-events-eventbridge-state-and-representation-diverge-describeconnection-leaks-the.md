Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.describe-connection-redacts-secret-auth-material events.create-endpoint-duplicate-name-blocked events.concurrent-update-endpoint-conflict-detected events.endpoint-lifecycle-create-active-delete-404 events.update-endpoint-changes-routing-config events.default-event-bus-listed events.put-permission-same-statement-id-replaces-and-remove-all-clears-policy events.oauth-connection-with-unreachable-endpoint-deauthorizes-and-its-destination-is-inactive events.update-event-bus-partial-update-agrees-across-every-read-surface events.delete-event-bus-removes-resource events.concurrent-update-event-bus-conflict-detected
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events.describe-connection-redacts-secret-auth-material|_assert_no_secrets#14|AssertionError|assert leaked == [], (
Issue-Key: events|CreateEventBus|ResourceAlreadyExistsException
Issue-Key: events.default-event-bus-listed|run#9|AssertionError|assert "CreationTime" not in default_bus, (
Issue-Key: events.put-permission-same-statement-id-replaces-and-remove-all-clears-policy|run#14|AssertionError|assert anyone["Principal"] == "*", f"a wildcard principal is stored as '*', got {anyone['Principal']!r}"
Issue-Key: events.oauth-connection-with-unreachable-endpoint-deauthorizes-and-its-destination-is-inactive|run#5|AssertionError|assert oauth["ConnectionState"] == "AUTHORIZING", (
Issue-Key: events.update-event-bus-partial-update-agrees-across-every-read-surface|run#22|AssertionError|assert missing_level[0] == "ValidationException" and "Level" in missing_level[1], (
Issue-Key: events.delete-event-bus-removes-resource|run#11|AssertionError|assert put_rule_err == expected, (
Issue-Key: events.concurrent-update-event-bus-conflict-detected|run#10|AssertionError|assert conflict_seen, (
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# EventBridge state and representation diverge: DescribeConnection leaks the credential half of AuthParameters, the event-bus namespace is not partitioned by region, and six read/lifecycle surfaces report the wrong value (8 causes)

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeConnection

## Description
Eight independent defects in what EventBridge stores and reports back. Cause 1 is a credential disclosure and sets this issue's severity.

Cause 1 of 8 — DescribeConnection returns the secret half of a connection's AuthParameters.
AWS: Smithy `DescribeConnection` returns AuthParameters as `ConnectionAuthResponseParameters`. Its API-key member is `ConnectionApiKeyAuthResponseParameters`, which has exactly ONE member, `ApiKeyName` — there is no ApiKeyValue in the response shape at all; the same holds for the Basic/OAuth response shapes and for header/query/body parameters flagged secret. The credential lives only in the Secrets Manager secret named by `SecretArn`.
Emulator: DescribeConnection returns the credential value in AuthParameters (the replay found the literal secret `hdrsecret` in the response body alongside SecretArn).
Why: a read-only, widely granted API (events:DescribeConnection) discloses the connection's credential. Any principal allowed to read connection metadata can exfiltrate the secret the SecretArn indirection exists to protect.
Fix: the DescribeConnection serializer, services/service-com-amazonaws-eventbridge-awsevents/src/ops/describe_connection.rs — it must project onto the *Response* auth shapes, not echo the stored request parameters.
Traces: events.describe-connection-redacts-secret-auth-material.

Cause 2 of 8 — the event-bus namespace is not partitioned by region: creating the same bus name in a second region is refused as already existing.
AWS: event buses are regional resources. CreateEventBus(Name=n) in us-west-2 and CreateEventBus(Name=n) in another region both succeed; same-named buses in two regions are the required setup for a Global Endpoint.
Emulator: the second region's CreateEventBus answers ResourceAlreadyExistsException/400 "Event bus <name> already exists." even though the name is unique to the run and was only ever created once per region.
Why: no multi-region EventBridge topology can be built, and the entire Global Endpoint lifecycle (create/describe/update/delete) is unreachable behind this one refusal. Four checks fail at this call before reaching any endpoint assertion.
Fix: key the event-bus store by (region, account, name) in the EventBridge state, and the existence check in .../src/ops/create_event_bus.rs.
Traces: events.create-endpoint-duplicate-name-blocked, events.concurrent-update-endpoint-conflict-detected, events.endpoint-lifecycle-create-active-delete-404, events.update-endpoint-changes-routing-config.

Cause 3 of 8 — the default event bus reports a CreationTime it should not have.
AWS: recorded behavior — the account's default bus was never explicitly created, and ListEventBuses/DescribeEventBus return it with no CreationTime. `EventBus.CreationTime` is an optional member in the Smithy model.
Emulator: returns `{'Name': 'default', 'Arn': '...:event-bus/default', 'CreationTime': <now>, 'LastModifiedTime': <now>}` — a timestamp equal to emulator start, not to any creation.
Why: a client that keys on CreationTime's presence to tell the default bus from a customer bus misclassifies it, and the value it reads is meaningless.
Fix: the default-bus record in EventBridge state and the ListEventBuses/DescribeEventBus serializers.
Traces: events.default-event-bus-listed.

Cause 4 of 8 — PutPermission stores a wildcard principal as an object instead of the string "*".
AWS: `PutPermission.Principal` has pattern `^(\d{12}|\*)$`; with Principal="*" the resulting bus policy statement renders `"Principal": "*"`.
Emulator: the stored policy renders `"Principal": {"AWS": "*"}`.
Why: the policy document a client reads back (DescribeEventBus.Policy) is not the one AWS would produce, so policy diffing, drift detection and CFN reconciliation all report a false change.
Fix: policy-statement construction in .../src/ops/put_permission.rs.
Traces: events.put-permission-same-statement-id-replaces-and-remove-all-clears-policy.

Cause 5 of 8 — a new OAuth connection never enters AUTHORIZING.
AWS: an OAuth-client-credentials connection must fetch a token before it can be used, so it reports ConnectionState AUTHORIZING on creation and settles to AUTHORIZED or DEAUTHORIZED depending on whether the token endpoint answered. `ConnectionState` enum in the Smithy model includes AUTHORIZING and DEAUTHORIZED.
Emulator: the connection reports 'CREATING' and never reaches AUTHORIZING, so an unreachable authorization endpoint never deauthorizes the connection and its API destination is never driven inactive.
Fix: the connection state machine in .../src/ops/create_connection.rs and the connection authorization step.
Traces: events.oauth-connection-with-unreachable-endpoint-deauthorizes-and-its-destination-is-inactive.

Cause 6 of 8 — UpdateEventBus accepts a LogConfig that omits Level.
AWS: recorded behavior — UpdateEventBus with a LogConfig that omits `Level` is refused with a 400 naming the missing field; the Smithy `LogConfig` structure carries IncludeDetail and Level.
Emulator: the call is not refused at all — the check observed `(None, '')`, i.e. no error code and no message.
Fix: LogConfig validation in .../src/ops/update_event_bus.rs.
Traces: events.update-event-bus-partial-update-agrees-across-every-read-surface.

Cause 7 of 8 — PutRule against a deleted bus answers the wrong error class.
AWS: PutRule's modeled client errors are ConcurrentModificationException, InvalidEventPatternException, LimitExceededException, ManagedRuleException and ResourceNotFoundException. A PutRule naming a bus that no longer exists is ResourceNotFoundException/400 "Event bus <name> does not exist."
Emulator: answers ('ValidationException', 'Parameter(s) EventPattern or ScheduleExpression must be specified.') — it validates the body before resolving the bus, and ValidationException is not even in PutRule's modeled error list.
Why: a client cannot tell "the bus is gone" from "my request body is wrong", and retries the wrong remedy.
Fix: order bus resolution before body validation in .../src/ops/put_rule.rs.
Traces: events.delete-event-bus-removes-resource.

Cause 8 of 8 — no ConcurrentModificationException under concurrent writes.
AWS: `ConcurrentModificationException` is a modeled error of UpdateEventBus (and of PutRule/PutTargets/DeleteRule); concurrent mutations of one bus surface it.
Emulator: 6 bursts of 12 concurrent UpdateEventBus calls produced no ConcurrentModificationException at all.
Why: clients that implement optimistic-concurrency retry loops around this code never exercise them, and concurrent writers silently clobber each other.
Fix: per-bus write serialization/version check in .../src/ops/update_event_bus.rs and the EventBridge state layer.
Traces: events.concurrent-update-event-bus-conflict-detected.

## Evidence
1. DescribeConnection: "DescribeConnection(<name>) leaked secret material ['hdrsecret'] — a read-only EventBridge API returned the credential half of the connection's AuthParameters. Response was {\"ConnectionArn\": \"arn:aws:events:us-west-2:...:connection/<name>/<id>\", \"Name\": \"<name>\", \"ConnectionState\": \"AUTHORIZED\", \"AuthorizationType\": \"API_KEY\", \"SecretArn\": \"arn:aws:secretsmanager:us-west-2:...:secret:events/c...\"}". Expected: AuthParameters.ApiKeyAuthParameters with ApiKeyName only (the sole member of ConnectionApiKeyAuthResponseParameters), no credential value anywhere in the body.
2. CreateEventBus in the second region: "botocore.errorfactory.ResourceAlreadyExistsException: An error occurred (ResourceAlreadyExistsException) when calling the CreateEventBus operation: Event bus <unique-name> already exists." — observed on four separate checks, each with a freshly generated unique name. Expected: 200 with EventBusArn carrying the second region.
3. ListEventBuses: "the default bus was never explicitly created and must carry no CreationTime, got {'Name': 'default', 'Arn': 'arn:aws:events:us-west-2:...:event-bus/default', 'CreationTime': datetime(...), 'LastModifiedTime': datetime(...)}".
4. PutPermission(Principal='*'): "a wildcard principal is stored as '*', got {'AWS': '*'}".
5. CreateConnection (OAuth): "an OAuth connection starts by AUTHORIZING (it must fetch a token), got 'CREATING'".
6. UpdateEventBus(LogConfig without Level): "UpdateEventBus with LogConfig missing Level must be refused naming the missing field: got (None, '')" — no refusal at all.
7. PutRule after DeleteEventBus: expected ('ResourceNotFoundException', 'Event bus <bus> does not exist.'), got ('ValidationException', 'Parameter(s) EventPattern or ScheduleExpression must be specified.').
8. UpdateEventBus concurrency: "no ConcurrentModificationException observed across 6 bursts of 12 concurrent UpdateEventBus calls".

## Steps to Reproduce
1. CreateConnection(Name=c, AuthorizationType='API_KEY', AuthParameters={'ApiKeyAuthParameters': {'ApiKeyName': 'x-api-key', 'ApiKeyValue': 'hdrsecret'}, 'InvocationHttpParameters': {'HeaderParameters': [{'Key':'h','Value':'hdrsecret','IsValueSecret':True}]}}); DescribeConnection(Name=c) -> the body contains 'hdrsecret'.
2. CreateEventBus(Name=n) in us-west-2, then CreateEventBus(Name=n) in us-east-1 -> second call 400 ResourceAlreadyExistsException.
3. ListEventBuses(NamePrefix='default') -> the default bus carries CreationTime.
4. PutPermission(EventBusName=b, Action='events:PutEvents', Principal='*', StatementId='sid'); DescribeEventBus(Name=b) -> Policy statement renders Principal as {"AWS":"*"}.
5. CreateConnection with AuthorizationType='OAUTH_CLIENT_CREDENTIALS' -> ConnectionState 'CREATING'.
6. UpdateEventBus(Name=b, LogConfig={'IncludeDetail':'FULL'}) -> succeeds instead of being refused.
7. CreateEventBus(Name=b2); DeleteEventBus(Name=b2); PutRule(Name=r, EventBusName=b2, EventPattern='{"source":["x"]}') -> ValidationException instead of ResourceNotFoundException.
8. Fire 12 concurrent UpdateEventBus(Name=b, Description=<distinct>) calls repeatedly -> never ConcurrentModificationException.
