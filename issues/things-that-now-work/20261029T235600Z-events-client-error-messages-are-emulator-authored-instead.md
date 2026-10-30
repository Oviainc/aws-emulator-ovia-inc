Issue-Source: trace-sweep-divergence
Issue-Trace: events-custom-event-bus-rule-target-lifecycle
Issue-Step: task#14
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: events-custom-event-bus-rule-target-lifecycle events-put-rule-validation-errors events-archive-create-configure-verify-delete events-put-events-and-pattern-matching events-test-event-pattern-error-responses events-connection-and-api-destination-lifecycle events-disable-rule-absent-rule-and-absent-bus-not-found events-endpoint-create-validation-errors events-global-endpoint-lifecycle events-multi-target-rule-management-and-quota-errors events-put-events-per-entry-failure-envelope
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events|PutRule|4xx->4xx|$.message
Issue-Key: events|UpdateArchive|4xx->4xx|$.message
Issue-Key: events|TestEventPattern|4xx->4xx|$.message
Issue-Key: events|CreateConnection|4xx->4xx|$.message
Issue-Key: events|CreateApiDestination|4xx->4xx|$.message
Issue-Key: events|DisableRule|4xx->4xx|$.message
Issue-Key: events|CreateEndpoint|4xx->4xx|$.message
Issue-Key: events|PutTargets|4xx->4xx|$.message
Issue-Key: events|PutEvents|4xx->4xx|$.message
Issue-Commit: 10534c2dd099d7fcc20a17236779012152da4f21

# events client-error messages are emulator-authored instead of AWS wording across nine refusals, and DisableRule reports the wrong missing resource

> _Note: this issue was identified against the code base at commit `10534c2dd099d7fcc20a17236779012152da4f21`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRule

## Description
Nine EventBridge refusals carry the right HTTP status and error type but a message the emulator wrote itself rather than the message AWS returns for that error class. Clients that branch on message text (and every human reading a CLI failure) get wording that does not match AWS; in one case the message names the wrong missing resource, which is a validation-order defect, not just wording. Nine causes.

Cause 1 of 9 — PutRule invalid-EventPattern message uses the JSON parser's own wording.
AWS: Smithy: PutRule binds InvalidEventPatternException (client error, HTTP 400, 'The event pattern is not valid.'). Recorded response: 400 with message 'Event pattern is not valid. Reason: Unexpected character (\'<c>\' (code <n>)): was expecting double-quote to start field name\n at [Source: (String)"<pattern>"; line: <l>, column: <col>]'. Fixed text: 'Event pattern is not valid. Reason: ' then the Jackson parse diagnostic 'Unexpected character (...): was expecting double-quote to start field name' and the ' at [Source: (String)"..."; line: L, column: C]' tail. Variable parts: the offending character and its code, the submitted pattern echoed verbatim, and the line/column — all taken from the request.
Emulator: 400 with 'Event pattern is not valid. Reason: Event pattern is not valid JSON: key must be a string at line 1 column 3\n at [Source: (String)"<pattern>"; line: 1, column: 3]' — the Rust JSON library's diagnostic substituted into the AWS envelope.
Why: a client matching the documented reason text (or a test asserting it) sees an unrecognised reason; the character/code detail AWS reports is lost.
Fix: the event-pattern validator in the events crate must render the Jackson-shaped diagnostic for a malformed-JSON pattern, in one place shared by all pattern-accepting operations (see causes 2 and 3 — same root).
Traces: events-custom-event-bus-rule-target-lifecycle#task#14 (cluster also covers events-put-rule-validation-errors#task#3).

Cause 2 of 9 — UpdateArchive invalid-EventPattern message, same substitution.
AWS: Smithy: UpdateArchive binds InvalidEventPatternException (400). Recorded response: the same 'Event pattern is not valid. Reason: Unexpected character (...): was expecting double-quote to start field name\n at [Source: (String)"<pattern>"; ...]' template.
Emulator: the serde-style 'Event pattern is not valid JSON: key must be a string at line 1 column 3' reason.
Why: same as cause 1, on the archive path.
Fix: the shared event-pattern validator, called by the UpdateArchive handler.
Traces: events-archive-create-configure-verify-delete#task#9.

Cause 3 of 9 — TestEventPattern invalid-EventPattern message, same substitution.
AWS: Smithy: TestEventPattern binds InvalidEventPatternException (400). Recorded response: the same Jackson-shaped reason template.
Emulator: the serde-style reason.
Why: TestEventPattern exists to be called for exactly this diagnostic, so the wrong reason text is the whole payload of the operation's failure mode.
Fix: the shared event-pattern validator, called by the TestEventPattern handler.
Traces: events-put-events-and-pattern-matching#task#5 (cluster also covers events-test-event-pattern-error-responses#task#3).

Cause 4 of 9 — CreateConnection duplicate-name message is reworded.
AWS: Smithy: CreateConnection binds ResourceAlreadyExistsException (client, 400). Recorded response: 400 with message "Failed to create the connection(s). A connection with name '<name>' already exists." — fixed text 'Failed to create the connection(s). A connection with name ' + the requested Name in single quotes + ' already exists.'
Emulator: 'Failed to create the connection(s). Connection <name> already exists.' — different second sentence and the name is unquoted.
Why: the message is the only distinguishing payload of this error class; a client that parses it (or an operator diffing against AWS) sees text AWS never emits.
Fix: the CreateConnection handler's already-exists refusal message.
Traces: events-connection-and-api-destination-lifecycle#task#10.

Cause 5 of 9 — CreateApiDestination missing-connection message names the connection by short name instead of the ARN from the request.
AWS: Smithy: CreateApiDestination binds ResourceNotFoundException (client, 400) and its ConnectionArn input member is an ARN (pattern ^arn:...:connection/<name>/<id>$). Recorded response: 400 with "Failed to create the api-destination(s). Connection '<connectionArn>' does not exist." — fixed text with the ConnectionArn echoed verbatim from the request in single quotes.
Emulator: "Failed to create the api-destination(s). Connection '<name>' does not exist." — it substitutes the name parsed out of the ARN, dropping the value the caller actually sent.
Why: the caller cannot correlate the refusal with the ARN it supplied, which matters when the same connection name exists in another account/region.
Fix: the CreateApiDestination handler's not-found refusal must interpolate the request's ConnectionArn, not a derived name.
Traces: events-connection-and-api-destination-lifecycle#task#11.

Cause 6 of 9 — DisableRule on a nonexistent event bus blames the rule instead of the bus (validation order).
AWS: Smithy: DisableRule binds ResourceNotFoundException (client, 400). Recorded response: 400 with message 'Event bus <busName> does not exist.' — the bus is resolved FIRST, so when EventBusName names no bus the refusal is about the bus; fixed text 'Event bus ' + the EventBusName from the request + ' does not exist.'
Emulator: 400 with 'Rule <ruleName> does not exist on EventBus <busName>.' — it skips the bus-existence check and reports the rule lookup failure.
Why: the caller is told to create a rule when the actual fault is a missing bus; automation that branches on which resource is missing takes the wrong remediation. The same precedence governs the sibling case in this fixture where the bus does exist and only the rule is missing, so the two cases are indistinguishable today only by the wrong message.
Fix: DisableRule's handler/validation must resolve EventBusName before looking the rule up, and emit the bus-level message when the bus is absent.
Traces: events-disable-rule-absent-rule-and-absent-bus-not-found#task#3 (cluster also covers task#7).

Cause 7 of 9 — CreateEndpoint replication-without-role message is reworded as a parameter diagnostic.
AWS: Recorded behavior only (this refusal is not bound in the model; CreateEndpoint's modelled errors are InternalException, LimitExceededException, ResourceAlreadyExistsException). Recorded response: 400 with message 'When replication is enabled, role cannot be empty.' — entirely fixed text, no variable parts.
Emulator: 400 with 'Parameter RoleArn is not valid. Reason: RoleArn is required when event replication is ENABLED.' — an emulator-authored parameter-validation template.
Why: the fixed sentence is the whole contract of this refusal; clients and docs-driven tests matching it fail against the emulator.
Fix: the CreateEndpoint validation that checks ReplicationConfig.State against RoleArn must emit AWS's fixed sentence.
Traces: events-endpoint-create-validation-errors#task#1 (cluster also covers events-global-endpoint-lifecycle#task#2).

Cause 8 of 9 — PutTargets over-quota message leaks an internal limit sentence.
AWS: Smithy: PutTargets binds LimitExceededException (client, 400, 'The request failed because it attempted to create resource beyond the allowed service quota.'). Recorded response: 400 with message 'The requested resource exceeds the maximum number allowed.' — fixed text, no rule name and no numeric limit.
Emulator: 400 with 'Rule <ruleName> cannot hold more than 5 targets.' — it names the rule and the per-rule target cap AWS does not disclose in this message.
Why: the message AWS returns is fixed, so any consumer keyed to it misses; conversely the emulator's wording invites clients to parse a limit that is not part of the contract.
Fix: the PutTargets handler's limit-exceeded refusal message.
Traces: events-multi-target-rule-management-and-quota-errors#task#6.

Cause 9 of 9 — PutEvents over-length Entries list message does not use AWS's constraint-violation template.
AWS: Smithy: PutEvents input Entries is a list with @length min 1, max 10. Recorded response: 400 whose message is the AWS SDK constraint-violation template, beginning "1 validation error detected: Value '[...]'" and rendering the rejected list as the service's own per-entry representation (each entry printed as PutEventsRequestEntry(time=..., source=..., resources=..., detailType=..., eventGroupId=..., detail=..., eventBusName=..., traceHeader=..., kmsKeyIdentifier=..., internalMetadata=...), with unset members shown as null). Fixed parts: the '<N> validation error detected: Value ' prefix, the member rendering with all members present, and the constraint clause; variable parts: the count and the entries echoed from the request.
Emulator: answers 400 but with a different $.message (the replay reports a value_mismatch on $.message at this step; the expected/actual pair is longer than the replay detail's print budget, so re-replay this step to see both in full).
Why: the envelope is right but the message a caller surfaces for 'too many entries' is not AWS's, so a client cannot tell which constraint was violated from the text.
Fix: the PutEvents input validation / error serializer must render the constraint-violation template over the rejected Entries list.
Traces: events-put-events-per-entry-failure-envelope#task#3.

Related (does not cover these): open issue 'events: 4 net-new behavioral root causes — api-destination rate-limit and TestEventPattern refusal messages are rewritten...' concerns behavioral checks, not these nine wire steps; none of these clusters was reported as owned.

## Evidence
Replay of trace set simparity-wire-aws-v1 — all nine clusters have the expected status (400->400); only $.message diverges:
- PutRule / UpdateArchive / TestEventPattern: expected 'Event pattern is not valid. Reason: Unexpected character (\'t\' (code 116)): was expecting double-quote to start field name\n at [Source: (String)"{ this is not valid json"; line: 1, column: 3]' (and the 'b'/code 98 and 'n'/code 110 variants for the other two patterns), got 'Event pattern is not valid. Reason: Event pattern is not valid JSON: key must be a string at line 1 column 3\n at [Source: ...]'.
- CreateConnection: expected "Failed to create the connection(s). A connection with name '<name>' already exists.", got 'Failed to create the connection(s). Connection <name> already exists.'
- CreateApiDestination: expected "Failed to create the api-destination(s). Connection '<connectionArn>' does not exist.", got "...Connection '<name>' does not exist."
- DisableRule: expected 'Event bus <bus> does not exist.', got 'Rule <rule> does not exist on EventBus <bus>.'
- CreateEndpoint: expected 'When replication is enabled, role cannot be empty.', got 'Parameter RoleArn is not valid. Reason: RoleArn is required when event replication is ENABLED.'
- PutTargets: expected 'The requested resource exceeds the maximum number allowed.', got 'Rule <rule> cannot hold more than 5 targets.'
- PutEvents: expected the "1 validation error detected: Value '[PutEventsRequestEntry(time=null, source=a, ...)...]'" constraint template (truncated in the replay detail), got a different message.

## Steps to Reproduce
1. Replay the corpus (replay_wire_corpus).
2. Malformed pattern: events-custom-event-bus-rule-target-lifecycle#task#14 (PutRule), events-archive-create-configure-verify-delete#task#9 (UpdateArchive), events-put-events-and-pattern-matching#task#5 (TestEventPattern) — submit EventPattern '{ this is not valid json' etc. and compare $.message.
3. Duplicate connection: events-connection-and-api-destination-lifecycle#task#10 (CreateConnection with an existing Name).
4. Missing connection: same fixture task#11 (CreateApiDestination with a ConnectionArn for a nonexistent connection) — the message must echo the ARN sent.
5. Missing bus: events-disable-rule-absent-rule-and-absent-bus-not-found#task#3 (DisableRule with EventBusName naming no bus) — the message must be about the bus.
6. Replication without role: events-endpoint-create-validation-errors#task#1 (CreateEndpoint, ReplicationConfig enabled, no RoleArn).
7. Target quota: events-multi-target-rule-management-and-quota-errors#task#6 (PutTargets past the per-rule cap).
8. Entries quota: events-put-events-per-entry-failure-envelope#task#3 (PutEvents with more than 10 entries).
