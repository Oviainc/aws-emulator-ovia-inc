Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.alarm-mute-rule-create-get-list-delete-lifecycle cloudwatch.alarm-mute-rule-lifecycle-crud cloudwatch.list-alarm-mute-rules-reflects-put-immediately-with-metadata cloudwatch.put-alarm-mute-rule-limit-exceeded-unreachable cloudwatch.alarm-mute-rule-timestamp-microsecond-precision-preserved cloudwatch.put-alarm-mute-rule-timestamp-precision-round-trip cloudwatch.alarm-mute-rule-timestamp-round-trip-preserved cloudwatch.put-alarm-mute-rule-duration-exceeds-max-rejected cloudwatch.put-alarm-mute-rule-end-before-start-rejected cloudwatch.put-alarm-mute-rule-invalid-schedule-expression-rejected cloudwatch.put-alarm-mute-rule-target-limit-exceeded-rejected cloudwatch.rpcv2-cbor-insight-rule-mute-rule-and-error-maps cloudwatch.put-alarm-mute-rule-generates-alarm-history-entry
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.alarm-mute-rule-create-get-list-delete-lifecycle|run#13|AssertionError|assert got["MuteType"] == "one-time", f"expected one-time, got {got['MuteType']}"
Issue-Key: cloudwatch.alarm-mute-rule-lifecycle-crud|run#17|AssertionError|assert got["MuteType"] == "one-time", f"expected MuteType one-time, got {got['MuteType']!r}"
Issue-Key: cloudwatch.list-alarm-mute-rules-reflects-put-immediately-with-metadata|run#16|AssertionError|assert summary["MuteType"] == "one-time", summary["MuteType"]
Issue-Key: cloudwatch.put-alarm-mute-rule-limit-exceeded-unreachable|run#10|AssertionError|assert got["MuteType"] == "one-time", f"expected one-time, got {got['MuteType']!r}"
Issue-Key: cloudwatch.alarm-mute-rule-timestamp-microsecond-precision-preserved|run#10|AssertionError|raise AssertionError("sub-minute-precision StartDate was accepted instead of rejected")
Issue-Key: cloudwatch.put-alarm-mute-rule-timestamp-precision-round-trip|run#14|AssertionError|raise AssertionError("expected sub-minute-precision StartDate to be rejected")
Issue-Key: cloudwatch.alarm-mute-rule-timestamp-round-trip-preserved|_client_error#11|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: cloudwatch.put-alarm-mute-rule-duration-exceeds-max-rejected|run#9|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: cloudwatch.put-alarm-mute-rule-end-before-start-rejected|_client_error#7|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cloudwatch.put-alarm-mute-rule-invalid-schedule-expression-rejected|run#7|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: cloudwatch.put-alarm-mute-rule-target-limit-exceeded-rejected|run#18|AssertionError|assert err["Message"] == expected_message, (
Issue-Key: cloudwatch.rpcv2-cbor-insight-rule-mute-rule-and-error-maps|_assert_creation_id#25|AssertionError|assert isinstance(value, str) and _CREATION_ID_RE.match(value), (
Issue-Key: cloudwatch.put-alarm-mute-rule-generates-alarm-history-entry|run#30|AssertionError|assert matched, f"expected an alarm-history entry referencing mute rule {mute_rule_name!r}, got {new_items!r}"
Issue-Commit: b668b970d0d9c7dee5ca992802cd99dd932aec82

# cloudwatch: alarm mute rule writes accept four requests AWS refuses, three refusal messages are emulator-authored, and MuteType/CreationId/mute-rule alarm history diverge (9 causes)

> _Note: this issue was identified against the code base at commit `b668b970d0d9c7dee5ca992802cd99dd932aec82`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAlarmMuteRule

## Description
NINE net-new behavioral root causes on the CloudWatch alarm-mute-rule surface (PutAlarmMuteRule validation, the GetAlarmMuteRule/ListAlarmMuteRules projection, and the alarm-history write a mute rule produces). The replay report marks none of the 13 grounding checks ALREADY FILED or OWNED. Nearest open issues, named for the fixer but covering none of these check keys: 20261108T134822Z-monitoring-alarm-mute-rule-reads-diverge-mutetype-wire-casing.md (wire corpus; its causes 1-4 are the same MuteType casing surface as cause 1 here and its cause 1 the same missing CreationId as cause 8, but it is keyed on wire fixture steps, not on these behavioral checks), 20261025T020208Z-cloudwatch-putalarmmuterule-stub.md, 20261029T182452Z-cloudwatch-listalarmmuterules-stub.md and 20261029T210614Z-cloudwatch-4-net-new-root-causes-putalarmmuterule-describealarmcontributors.md (those three describe the operations answering NotImplemented; here they answer, and answer wrongly).

Cause 1 of 9: GetAlarmMuteRule and ListAlarmMuteRules serialize MuteType in the wrong case.
AWS: Smithy: GetAlarmMuteRuleOutput.MuteType and AlarmMuteRuleSummary.MuteType both target com.amazonaws.cloudwatch#MuteType, a plain string with NO enum constraint (the member doc prose says "ONE_TIME or RECURRING", so the wire value is whatever the service emits, not a modeled constraint). Recorded behavior: a one-time rule reads back MuteType "one-time" (and a recurring rule "recurring") from both the single read and the listing summary.
Emulator: returns the upper-case underscored constant — "ONE_TIME" — from GetAlarmMuteRule and from every AlarmMuteRuleSummaries[].MuteType.
Why: a client that switches on the token AWS gave it (or that filters/groups a listing by mute type) sees an unknown mute type, so a one-time rule is treated as unclassified and mute-type filtering silently matches nothing.
Fix: the GetAlarmMuteRule and ListAlarmMuteRules output serializers / the shared mute-rule state projection — emit the service's lower-case hyphenated token.
Traces: cloudwatch.alarm-mute-rule-create-get-list-delete-lifecycle, cloudwatch.alarm-mute-rule-lifecycle-crud, cloudwatch.list-alarm-mute-rules-reflects-put-immediately-with-metadata, cloudwatch.put-alarm-mute-rule-limit-exceeded-unreachable.

Cause 2 of 9: PutAlarmMuteRule accepts a StartDate carrying seconds instead of refusing it.
AWS: Smithy: PutAlarmMuteRuleInput.StartDate/ExpireDate are plain Timestamp members with no modeled precision constraint, so recorded behavior only: AWS enforces minute-level precision and refuses a StartDate with a non-zero seconds component with HTTP 400, error code ValidationError and the message "The Alarm Mute Rule StartDate only supports minute-level precision. Please provide the time without seconds." — that message is entirely fixed text, with no value taken from the request. A minute-precision StartDate/ExpireDate is accepted and read back exactly.
Emulator: accepts the sub-minute StartDate with HTTP 200 and creates the rule (no refusal at all).
Why: the client's timestamp is silently truncated or stored at a precision AWS would never have accepted, so a mute window computed to the second activates at a different instant than the caller asked for, and the error branch that tells a caller to drop the seconds never fires.
Fix: PutAlarmMuteRule's input validation (the StartDate/ExpireDate precision guard) and its ValidationError serializer.
Traces: cloudwatch.alarm-mute-rule-timestamp-microsecond-precision-preserved, cloudwatch.put-alarm-mute-rule-timestamp-precision-round-trip.

Cause 3 of 9: PutAlarmMuteRule accepts an at(...) schedule combined with StartDate/ExpireDate.
AWS: Smithy: Schedule.Expression is a required 1-256 char string documented as either cron(Minutes Hours Day-of-month Month Day-of-week) for recurring windows or at(yyyy-MM-ddThh:mm) for one-time windows; the interaction with StartDate/ExpireDate is not modeled, so recorded behavior only: AWS refuses an at(...) rule that also carries StartDate/ExpireDate with HTTP 400 ValidationError and the fixed message "Can not set start or expire dates for At expressions", and creates nothing (the follow-up GetAlarmMuteRule answers 404 ResourceNotFoundException with an empty message).
Emulator: answers HTTP 200 with an empty body and creates the rule.
Why: a one-time mute window given a redundant/contradictory validity window is stored instead of rejected, so the rule the caller gets is not the rule AWS would have created and the client never learns its request was invalid.
Fix: PutAlarmMuteRule's input validation — reject StartDate/ExpireDate when the schedule expression is an at(...) expression.
Traces: cloudwatch.alarm-mute-rule-timestamp-round-trip-preserved.

Cause 4 of 9: the over-maximum Schedule.Duration refusal message is emulator-authored.
AWS: Smithy: Schedule.Duration is a required 1-50 char string documented as "ISO 8601 duration format with a minimum of 1 minute (PT1M) and maximum of 15 days (P15D)". Recorded behavior: a Duration above that maximum is refused HTTP 400 with code ValidationError and the message "The Alarm Mute Rule Duration cannot exceed 15 days." — all fixed text; the 15-day bound is the API's own constant, not a per-request value.
Emulator: refuses with the right code but a generic invalid-parameter message that echoes the request value into the emulator's own template: "The value <Duration> for parameter Rule.Schedule.Duration is invalid." (recorded instance: value P16D).
Why: a client (or a human reading a log) is told a parameter is "invalid" with no statement of the limit it violated, and any handling keyed on AWS's message text does not match.
Fix: PutAlarmMuteRule's Duration validation message in the operation's validation layer.
Traces: cloudwatch.put-alarm-mute-rule-duration-exceeds-max-rejected.

Cause 5 of 9: PutAlarmMuteRule accepts an ExpireDate earlier than its StartDate.
AWS: not modeled (both are plain Timestamps), so recorded behavior only: AWS refuses the inverted window with HTTP 400 ValidationError and the fixed message "The Alarm Mute Rule is expired.", and creates no rule (GetAlarmMuteRule then answers 404 ResourceNotFoundException with an empty message). The same call with StartDate < ExpireDate succeeds and reads back Status SCHEDULED.
Emulator: answers HTTP 200 for the inverted window and creates the rule.
Why: an already-expired mute rule is accepted into the account, so an operator believes a mute window exists that can never activate, and the validation branch that would have caught the swapped arguments never runs.
Fix: PutAlarmMuteRule's input validation (StartDate/ExpireDate ordering guard).
Traces: cloudwatch.put-alarm-mute-rule-end-before-start-rejected.

Cause 6 of 9: the malformed-schedule refusal message is emulator-authored.
AWS: Schedule.Expression must be a cron(...) or at(...) expression (model doc, cause 3). Recorded behavior: an expression that is neither is refused HTTP 400 with code ValidationError and the message "Schedule is malformed." — all fixed text; AWS does NOT echo the offending expression.
Emulator: refuses with ValidationError but the message "The value <Expression> for parameter Rule.Schedule.Expression is invalid." (recorded instance echoed the whole bad expression).
Why: the message differs from AWS's wording for this error class, so message-keyed client handling and log scraping do not match, and the emulator additionally leaks the raw request value into a message AWS keeps fixed.
Fix: PutAlarmMuteRule's schedule-expression validation message.
Traces: cloudwatch.put-alarm-mute-rule-invalid-schedule-expression-rejected.

Cause 7 of 9: the over-100 MuteTargets.AlarmNames constraint message replaces the offending value with an item count.
AWS: Smithy: MuteTargets.AlarmNames is a required list with @length max 100 (the PutAlarmMuteRule reference: "You can specify up to 100 alarm names"). Recorded behavior: 101 names is refused HTTP 400 with code ValidationError and the Coral constraint template "1 validation error detected: Value '[<the member's values, comma-separated, exactly as sent>]' at 'muteTargets.alarmNames' failed to satisfy constraint: Member must have length less than or equal to 100". Fixed text: "1 validation error detected: Value '", "' at 'muteTargets.alarmNames' failed to satisfy constraint: Member must have length less than or equal to "; variable parts: the bracketed list of values from the request and the limit 100 from the model. Exactly 100 names is accepted, and a refused create/update stores nothing and leaves a stored rule's targets and Duration untouched.
Emulator: uses the right template and limit but renders the value as a count — "Value '101 items' at 'muteTargets.alarmNames' ..." — instead of the bracketed member values.
Why: the whole point of this template is that a client or user can read the offending value back out of the message; a count cannot be mapped to the member values, so tooling that parses the Coral template to point at the bad input gets nothing usable.
Fix: the service's Coral constraint-message renderer for list members (the value rendering), reached from PutAlarmMuteRule's MuteTargets validation.
Traces: cloudwatch.put-alarm-mute-rule-target-limit-exceeded-rejected.

Cause 8 of 9: GetAlarmMuteRule omits CreationId (observed on the RPC v2 CBOR protocol path).
AWS: Smithy: GetAlarmMuteRuleOutput does not declare a CreationId member, so recorded behavior only: AWS returns a generated CreationId on a mute-rule read, shaped <uuid>/<epoch-ms>.
Emulator: the CBOR GetAlarmMuteRule response carries no CreationId at all (decoded value None).
Why: a client that threads CreationId for idempotency or to correlate the rule it just created with later reads gets a null and cannot correlate; the omission is protocol-independent (it is the projection, not the CBOR serializer).
Fix: the mute-rule state projection / GetAlarmMuteRule output serializer — populate a generated CreationId.
Traces: cloudwatch.rpcv2-cbor-insight-rule-mute-rule-and-error-maps.

Cause 9 of 9: a state transition of an alarm targeted by an ACTIVE mute rule writes no alarm-history entry referencing the mute rule.
AWS: not modeled (DescribeAlarmHistory items are free-form HistorySummary/HistoryData), so recorded behavior only: when an alarm targeted by an ACTIVE mute rule transitions state, AWS writes a new alarm-history entry that names the mute rule, so the history records that the transition was muted.
Emulator: DescribeAlarmHistory after the transition contains only the ordinary StateUpdate entry and an Action entry — no entry mentions the mute rule (the recorded run shows the alarm's action being executed and failing, i.e. the mute rule left no trace on the history at all).
Why: the alarm history is the audit trail an operator uses to explain why a notification did not arrive; with no mute-rule entry, a muted transition is indistinguishable from a broken action, and mute-rule effectiveness cannot be verified after the fact.
Fix: the alarm state-transition path's history writer — emit a mute-rule history entry when an ACTIVE mute rule targets the transitioning alarm.
Traces: cloudwatch.put-alarm-mute-rule-generates-alarm-history-entry.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout. Every line below is the failing assertion the suite reported; none of the 13 checks is marked ALREADY FILED or OWNED.

Cause 1 — cloudwatch.alarm-mute-rule-create-get-list-delete-lifecycle: AssertionError: expected one-time, got ONE_TIME; cloudwatch.alarm-mute-rule-lifecycle-crud: expected MuteType one-time, got 'ONE_TIME'; cloudwatch.list-alarm-mute-rules-reflects-put-immediately-with-metadata: assert summary["MuteType"] == "one-time" -> AssertionError: ONE_TIME; cloudwatch.put-alarm-mute-rule-limit-exceeded-unreachable: expected one-time, got 'ONE_TIME'.

Cause 2 — cloudwatch.alarm-mute-rule-timestamp-microsecond-precision-preserved: AssertionError: sub-minute-precision StartDate was accepted instead of rejected (expected ValidationError "The Alarm Mute Rule StartDate only supports minute-level precision. Please provide the time without seconds."); cloudwatch.put-alarm-mute-rule-timestamp-precision-round-trip: AssertionError: expected sub-minute-precision StartDate to be rejected (same expected code and message).

Cause 3 — cloudwatch.alarm-mute-rule-timestamp-round-trip-preserved: AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ... 'content-length': '2' ...}} for an at(...) rule carrying StartDate/ExpireDate; expected 400 ValidationError "Can not set start or expire dates for At expressions" and no rule created.

Cause 4 — cloudwatch.put-alarm-mute-rule-duration-exceeds-max-rejected: AssertionError: expected message 'The Alarm Mute Rule Duration cannot exceed 15 days.', got 'The value P16D for parameter Rule.Schedule.Duration is invalid.' (P16D is the request's Duration, not part of AWS's template).

Cause 5 — cloudwatch.put-alarm-mute-rule-end-before-start-rejected: AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}; expected 400 ValidationError 'The Alarm Mute Rule is expired.' with nothing stored.

Cause 6 — cloudwatch.put-alarm-mute-rule-invalid-schedule-expression-rejected: AssertionError: expected message 'Schedule is malformed.', got 'The value not a cron or at expression!! for parameter Rule.Schedule.Expression is invalid.'

Cause 7 — cloudwatch.put-alarm-mute-rule-target-limit-exceeded-rejected: AssertionError: expected the 101-target constraint message, got "1 validation error detected: Value '101 items' at 'muteTargets.alarmNames' failed to satisfy constraint: Member must have length less than or equal to 100"; AWS renders that same template with Value '[<the 101 alarm names as sent>]'.

Cause 8 — cloudwatch.rpcv2-cbor-insight-rule-mute-rule-and-error-maps: AssertionError: CBOR GetAlarmMuteRule CreationId is None, not a <uuid>/<epoch-ms> generated id.

Cause 9 — cloudwatch.put-alarm-mute-rule-generates-alarm-history-entry: AssertionError: expected an alarm-history entry referencing mute rule <rule name>, got only [{'HistoryItemType': 'Action', 'HistorySummary': 'Failed to execute action <topic arn>' ...}, {'HistoryItemType': 'StateUpdate', 'HistorySummary': 'Alarm updated from OK to ALARM' ...}] (generated names, arns and timestamps in that payload are per-run values and are not part of the requirement).

Model citations: smithy_query operation=PutAlarmMuteRule,GetAlarmMuteRule,ListAlarmMuteRules and shape=Rule,Schedule,MuteTargets,MuteType,AlarmMuteRuleSummary on services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/smithy-model/com-amazonaws-cloudwatch-graniteserviceversion20100801.json (MuteType is an unconstrained string; MuteTargets.AlarmNames @length max 100; Schedule.Duration doc gives PT1M..P15D; GetAlarmMuteRuleOutput declares no CreationId).

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the simparity-behavioral-aws-v1 behavioral suite (run_behavioral_suite).
3. Cause 1: replay cloudwatch.alarm-mute-rule-create-get-list-delete-lifecycle, cloudwatch.alarm-mute-rule-lifecycle-crud, cloudwatch.list-alarm-mute-rules-reflects-put-immediately-with-metadata and cloudwatch.put-alarm-mute-rule-limit-exceeded-unreachable — PutAlarmMuteRule a one-time (at(...)) rule, then GetAlarmMuteRule and ListAlarmMuteRules: MuteType must be "one-time" on both paths (and "recurring" for a cron(...) rule).
4. Cause 2: replay cloudwatch.alarm-mute-rule-timestamp-microsecond-precision-preserved and cloudwatch.put-alarm-mute-rule-timestamp-precision-round-trip — PutAlarmMuteRule with a StartDate whose seconds are non-zero must answer 400 ValidationError "The Alarm Mute Rule StartDate only supports minute-level precision. Please provide the time without seconds."; a minute-precision StartDate/ExpireDate must round-trip exactly.
5. Cause 3: replay cloudwatch.alarm-mute-rule-timestamp-round-trip-preserved — PutAlarmMuteRule with Rule.Schedule.Expression at(...) plus StartDate/ExpireDate must answer 400 ValidationError "Can not set start or expire dates for At expressions" and create nothing.
6. Cause 4: replay cloudwatch.put-alarm-mute-rule-duration-exceeds-max-rejected — Duration P16D must answer ValidationError "The Alarm Mute Rule Duration cannot exceed 15 days."; PT30M must be accepted.
7. Cause 5: replay cloudwatch.put-alarm-mute-rule-end-before-start-rejected — ExpireDate < StartDate must answer ValidationError 'The Alarm Mute Rule is expired.' and store nothing; the same call with StartDate < ExpireDate must succeed as SCHEDULED.
8. Cause 6: replay cloudwatch.put-alarm-mute-rule-invalid-schedule-expression-rejected — a non-cron/non-at expression must answer ValidationError 'Schedule is malformed.'
9. Cause 7: replay cloudwatch.put-alarm-mute-rule-target-limit-exceeded-rejected — 101 MuteTargets.AlarmNames must answer the Coral constraint message with the bracketed list of the names sent (100 names must be accepted, and the refused create/update must leave state untouched).
10. Cause 8: replay cloudwatch.rpcv2-cbor-insight-rule-mute-rule-and-error-maps — the RPC v2 CBOR GetAlarmMuteRule response must carry a CreationId shaped <uuid>/<epoch-ms>.
11. Cause 9: replay cloudwatch.put-alarm-mute-rule-generates-alarm-history-entry — after an alarm targeted by an ACTIVE mute rule transitions state, DescribeAlarmHistory must contain a new entry referencing the mute rule.
12. Re-replay all 13 check ids listed in trace_ids after the fix; every one must pass.
