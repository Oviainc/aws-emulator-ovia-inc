Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.delete-rule-with-targets-requires-force events.delete-event-bus-blocked-while-a-rule-remains-and-succeeds-once-removed events.delete-default-event-bus-protected events.put-targets-exceeds-max-limit events.put-targets-new-id-on-full-rule-rejected-while-same-id-upsert-accepted events.event-bus-tag-limit-counts-tags-already-on-the-bus-and-overwrites-do-not-count events.list-event-buses-paginates-with-limit-and-next-token events.list-targets-by-rule-paginates-with-limit-and-next-token events.create-connection-missing-auth-parameters-rejected events.put-and-remove-permission-conflicting-parameters-rejected events.put-rule-invalid-pattern-fails events__sqs.events-pattern-language-content-filter-combined-with-a-second-key-rejected events.delete-connection-eventually-not-found
Issue-Key: events.delete-rule-with-targets-requires-force|run#19|AssertionError|assert err == ("ValidationException", "Rule can't be deleted since it has targets."), (
Issue-Key: events.delete-event-bus-blocked-while-a-rule-remains-and-succeeds-once-removed|run#18|AssertionError|assert message == expected_message, (
Issue-Key: events.delete-default-event-bus-protected|run#10|AssertionError|assert message == "Cannot delete event bus default.", (
Issue-Key: events.put-targets-exceeds-max-limit|run#10|AssertionError|assert err.get("Message") == expected_message, (
Issue-Key: events.put-targets-new-id-on-full-rule-rejected-while-same-id-upsert-accepted|run#14|AssertionError|assert message == "The requested resource exceeds the maximum number allowed.", (
Issue-Key: events.event-bus-tag-limit-counts-tags-already-on-the-bus-and-overwrites-do-not-count|run#8|AssertionError|assert (code, msg) == ("ValidationException", _TOO_MANY), (
Issue-Key: events.list-event-buses-paginates-with-limit-and-next-token|run#14|AssertionError|assert (code, message) == ("ValidationException", "Parameter NextToken is not valid."), (
Issue-Key: events.list-targets-by-rule-paginates-with-limit-and-next-token|run#25|AssertionError|assert message == "Parameter NextToken is not valid.", (
Issue-Key: events.create-connection-missing-auth-parameters-rejected|run#10|AssertionError|assert err == expected, f"expected {expected}, got {err}"
Issue-Key: events.put-and-remove-permission-conflicting-parameters-rejected|run#10|AssertionError|assert (code, message) == ("ValidationException", expected), (
Issue-Key: events.put-rule-invalid-pattern-fails|run#10|AssertionError|assert err.get("Message") == expected_message, (
Issue-Key: events__sqs.events-pattern-language-content-filter-combined-with-a-second-key-rejected|run#5|AssertionError|assert put == ("InvalidEventPatternException", message), (
Issue-Key: events.delete-connection-eventually-not-found|run#14|AssertionError|assert final == (
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# EventBridge refusal codes and messages diverge from AWS: PutTargets limit uses the wrong error class, and nine refusal messages are rewritten with an emulator-specific 'Reason:' template (10 causes)

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
The emulator refuses these calls — the status class and (except for cause 4) the error code are right — but it substitutes its own message text for messages whose rule is fixed by AWS: each is a constant string AWS returns for that condition, or a constant plus names taken from the request. For cause 4 the error CLASS is wrong too. Ten causes, one per refusal rule.

Cause 1 of 10 — DeleteRule with attached targets. AWS: ValidationException/400 with the constant message "Rule can't be deleted since it has targets." (the message names no rule and no count). Emulator: ValidationException with "Rule <name> cannot be deleted since it still has 1 target(s). Remove them with RemoveTargets first." Fix: the refusal in services/service-com-amazonaws-eventbridge-awsevents/src/ops/delete_rule.rs. Traces: events.delete-rule-with-targets-requires-force.

Cause 2 of 10 — DeleteEventBus while a rule remains. AWS: the constant message "EventBus can't be deleted since it has rules." Emulator: "Event bus <name> cannot be deleted since it still has 1 rule(s). Delete them with DeleteRule first." Fix: .../src/ops/delete_event_bus.rs. Traces: events.delete-event-bus-blocked-while-a-rule-remains-and-succeeds-once-removed.

Cause 3 of 10 — DeleteEventBus on the default bus. AWS: its own specific default-bus refusal message. Emulator: "Event bus default is the account's default event bus and cannot be deleted." Fix: delete_event_bus.rs. Traces: events.delete-default-event-bus-protected.

Cause 4 of 10 — PutTargets past the per-rule target limit: wrong error class AND wrong message. AWS: the per-rule limit refusal is the limit-exceeded class carrying the constant message "The requested resource exceeds the maximum number allowed." `LimitExceededException` is a modeled PutTargets error in the Smithy model. Emulator: ValidationException with "Rule <name> cannot hold more than 5 targets." Why: a client retrying on limit errors cannot classify this one, because the code it keys on never appears. Fix: .../src/ops/put_targets.rs. Traces: events.put-targets-exceeds-max-limit, events.put-targets-new-id-on-full-rule-rejected-while-same-id-upsert-accepted.

Cause 5 of 10 — TagResource past the 50-tag limit. AWS: the limit refusal carries AWS's constant too-many-tags message. Emulator: ValidationException with "Parameter Tags is not valid. Reason: a resource can carry at most 50 tags, and the request would leave it carrying 51." Fix: .../src/ops/tag_resource.rs. Traces: events.event-bus-tag-limit-counts-tags-already-on-the-bus-and-overwrites-do-not-count.

Cause 6 of 10 — a NextToken the service never issued. AWS: ValidationException/400 with exactly "Parameter NextToken is not valid." Emulator appends an extra clause: "Parameter NextToken is not valid. Reason: it is not a token this service issued." Fix: the shared pagination-token decoder used by list_event_buses.rs and list_targets_by_rule.rs. Traces: events.list-event-buses-paginates-with-limit-and-next-token, events.list-targets-by-rule-paginates-with-limit-and-next-token.

Cause 7 of 10 — CreateConnection with AuthParameters that name no authorization type. AWS: ValidationException/400 with "Failed to create the connection(s). Parameter AuthParameters is not valid. Reason: The value for AuthParameters must include exactly one authorization type." Emulator: "Parameter AuthParameters is not valid. Reason: ApiKeyAuthParameters is required for AuthorizationType API_KEY." — it drops AWS's "Failed to create the connection(s)." lead-in and states a different rule. Fix: .../src/ops/create_connection.rs. Traces: events.create-connection-missing-auth-parameters-rejected.

Cause 8 of 10 — PutPermission with Policy combined with StatementId/Action/Principal. AWS: ValidationException/400 with "Action, Principal, StatementId and Condition could not be combined with Policy parameter." Emulator: "Parameter Policy is not valid. Reason: Policy describes the whole permission policy, so it cannot be combined with StatementId, Action or Principal." Fix: .../src/ops/put_permission.rs. Traces: events.put-and-remove-permission-conflicting-parameters-rejected.

Cause 9 of 10 — event-pattern rejection messages are not the service's pattern-compiler diagnostics. AWS: InvalidEventPatternException/400 whose message is the compiler's own text, including the source excerpt and position, e.g. "Event pattern is not valid. Reason: Unexpected character ('n' (code 110)): was expecting double-quote to start field name\n at [Source: (String)\"{not valid json\"; line: 1, column: 2]" and "Event pattern is not valid. Reason: Only one key allowed in match expression\n at [Source: (String)\"<pattern>\"; line: 1, column: 135]". Emulator: its own shorter text with no source excerpt or position — "Event pattern is not valid. Reason: Event pattern is not valid JSON: key must be a string at line 1 column 2" and "Event pattern is not valid. Reason: detail.k admits literal values and single-operator objects only". Fix: the pattern validator's diagnostic rendering, shared by put_rule.rs and the pattern compiler. Traces: events.put-rule-invalid-pattern-fails, events__sqs.events-pattern-language-content-filter-combined-with-a-second-key-rejected.

Cause 10 of 10 — connection not-found messages omit the per-operation lead-in. AWS: ResourceNotFoundException/400 with "Failed to describe the connection(s). Connection '<name>' does not exist." from DescribeConnection and "Failed to update the connection(s). Connection '<name>' does not exist." from UpdateConnection — the lead-in names the operation that failed. Emulator: "Connection '<name>' does not exist." Fix: the not-found refusal shared by .../src/ops/describe_connection.rs and update_connection.rs. Traces: events.delete-connection-eventually-not-found.

Why (all causes): these constants are what clients, SDK wrappers and CFN providers string-match on; and for cause 4 the retry classification keys on the error code itself, which the emulator never emits.

## Evidence
Replayed (expected -> actual):
1. DeleteRule with a target: ('ValidationException', "Rule can't be deleted since it has targets.") -> ('ValidationException', 'Rule <rule> cannot be deleted since it still has 1 target(s). Remove them with RemoveTargets first.')
2. DeleteEventBus with a rule: "EventBus can't be deleted since it has rules." -> 'Event bus <bus> cannot be deleted since it still has 1 rule(s). Delete them with DeleteRule first.'
3. DeleteEventBus(default): AWS's default-bus message -> "Event bus default is the account's default event bus and cannot be deleted."
4. PutTargets 6th target: 'The requested resource exceeds the maximum number allowed.' (limit-exceeded class) -> ('ValidationException', 'Rule <rule> cannot hold more than 5 targets.')
5. TagResource 11 tags onto a bus with 40: AWS's too-many-tags message -> ('ValidationException', 'Parameter Tags is not valid. Reason: a resource can carry at most 50 tags, and the request would leave it carrying 51.')
6. ListEventBuses / ListTargetsByRule with NextToken='sim2real-not-a-token': 'Parameter NextToken is not valid.' -> 'Parameter NextToken is not valid. Reason: it is not a token this service issued.'
7. CreateConnection without a usable AuthParameters: 'Failed to create the connection(s). Parameter AuthParameters is not valid. Reason: The value for AuthParameters must include exactly one authorization type.' -> 'Parameter AuthParameters is not valid. Reason: ApiKeyAuthParameters is required for AuthorizationType API_KEY.'
8. PutPermission(Policy=..., StatementId=...): 'Action, Principal, StatementId and Condition could not be combined with Policy parameter.' -> 'Parameter Policy is not valid. Reason: Policy describes the whole permission policy, so it cannot be combined with StatementId, Action or Principal.'
9. PutRule(EventPattern='{not valid json'): "Event pattern is not valid. Reason: Unexpected character ('n' (code 110)): was expecting double-quote to start field name\n at [Source: (String)\"{not valid json\"; line: 1, column: 2]" -> 'Event pattern is not valid. Reason: Event pattern is not valid JSON: key must be a string at line 1 column 2'. And for `{"detail":{"k":[{"prefix":"a","suffix":"z"}]}}`: 'Event pattern is not valid. Reason: Only one key allowed in match expression\n at [Source: (String)\"...\"; line: 1, column: 135]' -> ('InvalidEventPatternException', 'Event pattern is not valid. Reason: detail.k admits literal values and single-operator objects only')
10. DescribeConnection after DeleteConnection: ('ResourceNotFoundException', "Failed to describe the connection(s). Connection '<name>' does not exist.") -> ('ResourceNotFoundException', "Connection '<name>' does not exist.")

## Steps to Reproduce
1. PutRule(Name=r, EventBusName=b, EventPattern='{"source":["x"]}'); PutTargets(Rule=r, Targets=[1 target]); DeleteRule(Name=r, EventBusName=b) -> compare message.
2. With that rule still present, DeleteEventBus(Name=b) -> compare message.
3. DeleteEventBus(Name='default') -> compare message.
4. PutTargets five distinct Ids on one rule, then a sixth -> compare error code and message.
5. CreateEventBus with 40 tags, then TagResource with 11 more -> compare message.
6. ListEventBuses(NextToken='not-a-token') and ListTargetsByRule(Rule=r, NextToken='not-a-token') -> compare message.
7. CreateConnection(AuthorizationType='API_KEY', AuthParameters={}) -> compare message.
8. PutPermission(EventBusName=b, Policy='{...}', StatementId='sid', Action='events:PutEvents', Principal='*') -> compare message.
9. PutRule(EventPattern='{not valid json') and PutRule(EventPattern='{"detail":{"k":[{"prefix":"a","suffix":"z"}]}}') -> compare messages.
10. CreateConnection then DeleteConnection then DescribeConnection/UpdateConnection on that name -> compare messages.
