Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events.delete-rule-removes-it events.enable-nonexistent-rule-fails events.enable-disable-rule-roundtrip events.resource-tag-condition-scopes-disable-rule-to-rules-carrying-the-tag events.custom-bus-rule-calls-without-event-bus-name-not-found events__sqs.enable-rule-direct-call-resumes-delivery events__sqs.put-rule-update-omitting-state-re-enables-a-disabled-rule events__lambda__sqs.eventbridge-rule-disabled-state-blocks-delivery events.archive-managed-rule-refuses-customer-writes-and-survives-them events.cancel-replay-in-flight-reaches-cancelled-and-stays-terminal events.create-archive-and-describe events.create-archive-duplicate-fails events.create-archive-nonexistent-bus-fails events.delete-archive-not-blocked-by-in-flight-replay-and-start-replay-on-it-refused events.delete-archive-removes-it events.delete-event-bus-blocked-while-an-archive-targets-it events.describe-replay-nonexistent-not-found events.e2e-bus-rule-archive-replay-redelivers-event events.events-archive-replay-filter-arn-naming-a-rule-on-another-bus-rejected events.events-archive-replay-to-a-bus-other-than-the-archive-source-rejected events.events-archive-stores-only-the-events-matching-its-event-pattern events.events-create-archive-with-an-invalid-event-pattern-rejected events.list-archives-filters-by-prefix-and-source events.list-replays-combined-filters-rejected events.replay-completion-populates-timestamps-and-list-state-filter-discriminates events.replay-reaches-completed events.start-replay-duplicate-name-rejected events.start-replay-end-time-before-start-rejected events.start-replay-invalid-source-rejected events.update-archive-event-pattern-governs-what-is-archived-afterwards events.update-archive-pattern-is-mirrored-into-its-managed-rule-and-delete-removes-it events__sqs.events-archive-replay-does-not-redeliver-to-a-rule-outside-filter-arns events__sqs.events-archive-replay-reaches-a-rule-created-after-the-events-were-archived events__sqs.events-archive-replay-rule-matching-replay-name-exists-false-is-not-redelivered events__sqs.events-replay-redelivers-only-the-archived-events-inside-the-window events.api-destination-4xx-dead-letters-without-retry-while-5xx-is-retried-until-its-retry-attempts-are-used-up events.api-destination-api-key-connection-adds-its-key-and-parameters-and-its-values-win-over-the-targets events.api-destination-basic-connection-sends-the-basic-authorization-header-with-the-destinations-method-and-the-bare-envelope events.api-destination-invocation-rate-limit-maximum-is-enforced events.api-destination-lifecycle events.api-destination-oauth-connection-fetches-a-client-credentials-token-and-sends-it-as-the-bearer-header events.api-destination-rate-limit-defers-a-burst-it-cannot-absorb-and-still-delivers-every-event events.api-destination-state-follows-connection-deauthorize-and-reauthorize events.create-api-destination-accepts-non-url-endpoint events.create-api-destination-requires-existing-connection events.delete-connection-in-use-by-api-destination-allowed events__sqs.api-destination-timeout-retries-are-bounded-by-maximum-retry-attempts events__lambda__sqs.events-delivery-and-failure-event-age-limit-dead-letters-without-further-attempts events.test-event-pattern-malformed-operator-rejection events.nested-or-inside-detail-agrees-between-test-event-pattern-and-real-delivery events.test-event-pattern-anything-but-exclusion-forms events.test-event-pattern-anything-but-nested-suffix-equals-ignore-case-and-wildcard-forms events.test-event-pattern-anything-but-over-array-valued-field-admits-any-escaping-element events.test-event-pattern-cidr-matcher-boundaries events.test-event-pattern-envelope-required-fields-and-parseable-time events.test-event-pattern-exact-value-case-and-type-sensitivity events.test-event-pattern-exists-and-null-vs-empty-string events.test-event-pattern-exists-false-matches-when-an-intermediate-parent-is-missing events.test-event-pattern-exists-treats-every-object-leaf-and-the-empty-array-as-absent events.test-event-pattern-match events.test-event-pattern-match-and-mismatch events.test-event-pattern-multiple-filters-in-one-list-are-or-joined events.test-event-pattern-numeric-range-boundaries events.test-event-pattern-numeric-rejects-non-numeric-json-values-and-degenerate-ranges events.test-event-pattern-or-and-semantics-across-keys-and-arrays events.test-event-pattern-string-content-filters events__sqs.events-pattern-language-malformed-pattern-update-rejected-and-live-rule-keeps-matching events.list-connections-filters-are-mutually-exclusive events.update-connection-authorization-type events.list-rule-names-by-target events.list-rule-names-by-target-paginates-with-limit-and-next-token
Issue-Key: events.delete-rule-removes-it|run#13|AssertionError|assert mutate_err == expected, (
Issue-Key: events|EnableRule|NotImplemented
Issue-Key: events|DisableRule|NotImplemented
Issue-Key: events.custom-bus-rule-calls-without-event-bus-name-not-found|run#12|AssertionError|assert code == "ResourceNotFoundException", f"{op} on a custom-bus rule without EventBusName gave {code!r}"
Issue-Key: events__sqs|DisableRule|NotImplemented
Issue-Key: events__lambda__sqs|DisableRule|NotImplemented
Issue-Key: events|CreateArchive|NotImplemented
Issue-Key: events.events-create-archive-with-an-invalid-event-pattern-rejected|run#13|AssertionError|assert err == expected, (
Issue-Key: events__sqs|CreateArchive|NotImplemented
Issue-Key: events|CreateApiDestination|NotImplemented
Issue-Key: events__sqs|CreateApiDestination|NotImplemented
Issue-Key: events__lambda__sqs|CreateApiDestination|NotImplemented
Issue-Key: events.test-event-pattern-malformed-operator-rejection|_expect_pattern_invalid#2|AssertionError|assert code == "InvalidEventPatternException", (
Issue-Key: events|TestEventPattern|NotImplemented
Issue-Key: events__sqs|TestEventPattern|NotImplemented
Issue-Key: events|ListConnections|NotImplemented
Issue-Key: events|UpdateConnection|NotImplemented
Issue-Key: events|ListRuleNamesByTarget|NotImplemented
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# EventBridge rule-state, connection, archive, API-destination and TestEventPattern operations are unimplemented scaffold stubs (8 causes)

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
EnableRule

## Description
Eight EventBridge operations answer every request with error code `NotImplemented` and the message "operation <Op> is scaffolded but not implemented in this emulator". Each is a separate operation handler and therefore a separate cause; together they make rule enable/disable, connections, archives/replays, API destinations and pattern testing entirely unusable. None of these is covered by an existing open issue (the only EventBridge stub issue filed for this suite names `DescribeEndpoint`).

Cause 1 of 8 — EnableRule.
AWS: Smithy `EnableRule` takes Name (required) + EventBusName and returns Unit; its modeled errors are ConcurrentModificationException, InternalException, ManagedRuleException, ResourceNotFoundException. Enabling a rule that does not exist is ResourceNotFoundException/400; enabling an existing rule is a 200 with an empty body.
Emulator: `NotImplemented` for every call.
Why: a client can never re-enable a rule, and cannot distinguish a deleted rule from a live one via EnableRule.
Fix: the EnableRule handler, services/service-com-amazonaws-eventbridge-awsevents/src/ops/enable_rule.rs.
Traces: events.delete-rule-removes-it.

Cause 2 of 8 — DisableRule.
AWS: Smithy `DisableRule` mirrors EnableRule (Name required, EventBusName optional, Unit output, ResourceNotFoundException for an unknown rule). After it returns, the rule's State is DISABLED and it stops matching, which is the only supported way to pause delivery without deleting targets.
Emulator: `NotImplemented` for every call, so a rule can never be disabled and every "disabled rule delivers nothing" behaviour is unreachable.
Why: the documented pause/resume control plane is missing; clients that disable a rule before a maintenance window instead keep delivering.
Fix: the DisableRule handler, .../src/ops/disable_rule.rs, plus the rule State the matcher consults.
Traces: events.enable-disable-rule-roundtrip, events.custom-bus-rule-calls-without-event-bus-name-not-found, events__sqs.enable-rule-direct-call-resumes-delivery, events__sqs.put-rule-update-omitting-state-re-enables-a-disabled-rule, events__lambda__sqs.eventbridge-rule-disabled-state-blocks-delivery.

Cause 3 of 8 — CreateArchive (and the archive/replay surface behind it).
AWS: Smithy `CreateArchive` takes ArchiveName + EventSourceArn (an event-bus ARN), optional EventPattern/RetentionDays/KmsKeyIdentifier, and returns ArchiveArn, State, StateReason, CreationTime; modeled errors include InvalidEventPatternException, ResourceAlreadyExistsException, ResourceNotFoundException, LimitExceededException. An archive then captures matching events for replay.
Emulator: `NotImplemented`, so no archive exists and no replay can ever be started.
Why: archive/replay is the documented recovery path after a target outage; it is completely unavailable, and the managed-rule protections that surround an archive cannot be observed either.
Fix: the CreateArchive handler, .../src/ops/create_archive.rs, and the archive/replay state it must populate.
Traces: events.archive-managed-rule-refuses-customer-writes-and-survives-them, events__sqs.events-archive-replay-does-not-redeliver-to-a-rule-outside-filter-arns, events__sqs.events-archive-replay-reaches-a-rule-created-after-the-events-were-archived, events__sqs.events-archive-replay-rule-matching-replay-name-exists-false-is-not-redelivered, events__sqs.events-replay-redelivers-only-the-archived-events-inside-the-window.

Cause 4 of 8 — CreateApiDestination.
AWS: Smithy `CreateApiDestination` takes Name, ConnectionArn, InvocationEndpoint (HttpsEndpoint), HttpMethod (all required) and optional InvocationRateLimitPerSecond; it returns ApiDestinationArn, ApiDestinationState, CreationTime, LastModifiedTime, and refuses with ResourceNotFoundException when the ConnectionArn is unknown.
Emulator: `NotImplemented`, so no API destination can be created and the HTTP-target retry/dead-letter semantics behind it are unreachable.
Why: EventBridge's only built-in way to call an arbitrary HTTPS endpoint is unavailable; 4xx-vs-5xx retry behaviour and MaximumRetryAttempts cannot be exercised at all.
Fix: the CreateApiDestination handler, .../src/ops/create_api_destination.rs, and the api-destination delivery path.
Traces: events.api-destination-4xx-dead-letters-without-retry-while-5xx-is-retried-until-its-retry-attempts-are-used-up, events__sqs.api-destination-timeout-retries-are-bounded-by-maximum-retry-attempts, events__lambda__sqs.events-delivery-and-failure-event-age-limit-dead-letters-without-further-attempts.

Cause 5 of 8 — TestEventPattern.
AWS: Smithy `TestEventPattern` takes EventPattern + Event (both required) and returns `Result` (boolean); its only client error is InvalidEventPatternException/400, which is what a malformed pattern (for example a leaf value not wrapped in an array) must produce.
Emulator: `NotImplemented` for every call, so a malformed pattern is answered NotImplemented instead of InvalidEventPatternException, and no pattern can be validated offline.
Why: clients validate patterns with TestEventPattern before PutRule; they get a non-modeled error code and cannot tell a bad pattern from a service fault.
Fix: the TestEventPattern handler, .../src/ops/test_event_pattern.rs, sharing the pattern matcher/validator used by rule matching.
Traces: events.test-event-pattern-malformed-operator-rejection, events.nested-or-inside-detail-agrees-between-test-event-pattern-and-real-delivery, events__sqs.events-pattern-language-malformed-pattern-update-rejected-and-live-rule-keeps-matching.

Cause 6 of 8 — ListConnections.
AWS: Smithy `ListConnections` takes NamePrefix, ConnectionState, NextToken, Limit (range 1-100) and returns Connections + NextToken; mutually exclusive filter combinations are refused as a validation error.
Emulator: `NotImplemented`.
Why: connections cannot be enumerated, so no client can discover or reconcile them.
Fix: the ListConnections handler, .../src/ops/list_connections.rs.
Traces: events.list-connections-filters-are-mutually-exclusive.

Cause 7 of 8 — UpdateConnection.
AWS: Smithy `UpdateConnection` takes Name (required) plus Description/AuthorizationType/AuthParameters/KmsKeyIdentifier and returns ConnectionArn, ConnectionState, CreationTime, LastModifiedTime, LastAuthorizedTime; ResourceNotFoundException/400 for an unknown name.
Emulator: `NotImplemented`, so a connection's authorization type can never be changed and a mutation against a deleted connection cannot be refused as not-found.
Fix: the UpdateConnection handler, .../src/ops/update_connection.rs.
Traces: events.update-connection-authorization-type.

Cause 8 of 8 — ListRuleNamesByTarget.
AWS: Smithy `ListRuleNamesByTarget` takes TargetArn (required), EventBusName, NextToken, Limit and returns RuleNames + NextToken, with ResourceNotFoundException/400 for an unknown bus.
Emulator: `NotImplemented`.
Why: the reverse index from a target ARN to the rules pointing at it is unavailable, so a client cannot find what still references a resource before deleting it.
Fix: the ListRuleNamesByTarget handler, .../src/ops/list_rule_names_by_target.rs.
Traces: events.list-rule-names-by-target.

## Evidence
Replayed responses, all HTTP 400 with error code `NotImplemented`:
- EnableRule: "An error occurred (NotImplemented) when calling the EnableRule operation: operation EnableRule is scaffolded but not implemented in this emulator" — raised where the check expected ('ResourceNotFoundException', 'Rule <name> does not exist on EventBus <bus>.').
- DisableRule: same NotImplemented text; also surfaces as "DisableRule on a custom-bus rule without EventBusName gave 'NotImplemented'".
- CreateArchive: "operation CreateArchive is scaffolded but not implemented in this emulator".
- CreateApiDestination: "operation CreateApiDestination is scaffolded but not implemented in this emulator".
- TestEventPattern: "TestEventPattern must reject a leaf value not wrapped in an array: expected InvalidEventPatternException, got NotImplemented".
- ListConnections / UpdateConnection / ListRuleNamesByTarget: "operation <Op> is scaffolded but not implemented in this emulator".

Expected per the Smithy model: 200 (Unit for EnableRule/DisableRule), the modeled output shapes for CreateArchive/CreateApiDestination/ListConnections/UpdateConnection/ListRuleNamesByTarget/TestEventPattern, and 400 with the operation's own modeled error code (ResourceNotFoundException, InvalidEventPatternException, ...) when the request is bad. `NotImplemented` is not a modeled error for any of these operations.

## Steps to Reproduce
1. Create a custom event bus and a rule on it: CreateEventBus(Name=b), PutRule(Name=r, EventBusName=b, EventPattern='{"source":["x"]}').
2. Call DisableRule(Name=r, EventBusName=b) and EnableRule(Name=r, EventBusName=b) -> both answer 400 NotImplemented instead of 200.
3. Call TestEventPattern(EventPattern='{"source":"x"}', Event='{}') -> 400 NotImplemented instead of 400 InvalidEventPatternException.
4. Call CreateArchive(ArchiveName=a, EventSourceArn=<bus arn>) -> 400 NotImplemented.
5. Call CreateConnection then CreateApiDestination / UpdateConnection / ListConnections -> 400 NotImplemented.
6. Call ListRuleNamesByTarget(TargetArn=<any sqs arn>) -> 400 NotImplemented.
