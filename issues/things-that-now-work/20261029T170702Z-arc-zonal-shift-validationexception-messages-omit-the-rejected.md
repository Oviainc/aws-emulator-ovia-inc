Issue-Source: trace-sweep-divergence
Issue-Trace: arc-zonal-shift-list-operations-validation-errors
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: arc-zonal-shift-list-operations-validation-errors arc-zonal-shift-autoshift-observer-notification-toggle arc-zonal-shift-practice-run-configuration-validation-errors
Issue-Shape-Id: com.amazonaws.arczonalshift#PercDataPlane
Issue-Key: arc-zonal-shift|ListZonalShifts|4xx->4xx|$.message
Issue-Key: arc-zonal-shift|UpdateAutoshiftObserverNotificationStatus|4xx->4xx|$.message
Issue-Key: arc-zonal-shift|ListAutoshifts|4xx->4xx|$.message
Issue-Key: arc-zonal-shift|CreatePracticeRunConfiguration|4xx->4xx|$.message
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# arc-zonal-shift ValidationException messages omit the rejected value and use a wrong enum value set

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListZonalShifts

## Description
Four net-new divergences in the arc-zonal-shift input-validation layer. All four return the correct HTTP status (400 ValidationException) but render the message differently from AWS: the quoted rejected value is dropped from every constraint message, and for outcomeAlarms[].type the enum value set the message reports is also wrong. This bundle has 4 causes; all four must be closed.

Cause 1 of 4 - ListZonalShifts / ListAutoshifts maxResults range message (fixture arc-zonal-shift-list-operations-validation-errors, step task#3; same cluster also covers task#5).
AWS: Smithy: ListZonalShifts.maxResults -> MaxResults (integer, @range min 1 max 100), bound @httpQuery("maxResults"); out-of-range input is rejected with ValidationException (httpError 400). Recorded response: 400 with message "1 validation error detected: Value '<supplied value>' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100". Fixed text of the class: "1 validation error detected: Value '", "' at '", "' failed to satisfy constraint: Member must have value less than or equal to "; variable parts: the value taken verbatim from the request query string, the member path, and the @range max from the model.
Emulator: same 400, but the message omits the value and its quotes - "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100".
Why: a client (or an operator reading a log) cannot tell which supplied value was rejected, and any assertion or test that matches AWS's documented "Value '<x>' at '<member>'" message shape fails against the emulator even though the request was rejected for the right reason.
Fix: the arc-zonal-shift request-validation/message-formatting component that builds ValidationException messages (shared across the service's operations) - the value must be interpolated, single-quoted, between "Value " and " at ".

Cause 2 of 4: UpdateAutoshiftObserverNotificationStatus status enum message (fixture arc-zonal-shift-autoshift-observer-notification-toggle, step task#4).
AWS: Smithy: UpdateAutoshiftObserverNotificationStatusRequest.status is @required and targets AutoshiftObserverNotificationStatus (enum values ENABLED, DISABLED); error ValidationException (httpError 400). Recorded response: 400 with message "1 validation error detected: Value '<supplied value>' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [ENABLED, DISABLED]". Fixed text: "1 validation error detected: Value '", "' at '", "' failed to satisfy constraint: Member must satisfy enum value set: [", "]"; variable parts: the rejected value from the request body, the member path, and the enum's value list.
Emulator: same 400 and same enum list, but the message is "1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [ENABLED, DISABLED]" - the rejected value is missing.
Why: same as cause 1 - the error no longer names the offending input, so clients that surface or parse the message lose the only information identifying the bad value.
Fix: the same ValidationException message builder, on the enum-constraint path for this operation's input validation.

Cause 3 of 4: ListAutoshifts status enum message (fixture arc-zonal-shift-list-operations-validation-errors, step task#4).
AWS: Smithy: ListAutoshifts.status -> AutoshiftExecutionStatus (enum values ACTIVE, COMPLETED), bound @httpQuery("status"); error ValidationException (httpError 400). Recorded response: 400 with message "1 validation error detected: Value '<supplied value>' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [ACTIVE, COMPLETED]" - same fixed text as cause 2 with this operation's enum list.
Emulator: 400 with "1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [ACTIVE, COMPLETED]" - rejected value dropped.
Why: same as cause 1.
Fix: the same ValidationException message builder, on the enum-constraint path for query-bound members.

Cause 4 of 4: CreatePracticeRunConfiguration outcomeAlarms[].type - value dropped AND wrong enum value set (fixture arc-zonal-shift-practice-run-configuration-validation-errors, step task#1).
AWS: Smithy: CreatePracticeRunConfiguration.outcomeAlarms -> OutcomeAlarms (list of ControlCondition, @required, length 1..10); ControlCondition.type is @required and targets ControlConditionType; error ValidationException (httpError 400). The public model lists only CLOUDWATCH for ControlConditionType, but the recorded service response reports the set as [CARNAVAL, CLOUDWATCH] - recorded behavior only for that extra value. Recorded response: 400 with message "1 validation error detected: Value '<supplied value>' at 'outcomeAlarms.<1-based index>.member.type' failed to satisfy constraint: Member must satisfy enum value set: [CARNAVAL, CLOUDWATCH]". Fixed text as in cause 2, plus the member path template "outcomeAlarms.<index>.member.type"; variable parts: the rejected value and the list index.
Emulator: 400 with "1 validation error detected: Value at 'outcomeAlarms.1.member.type' failed to satisfy constraint: Member must satisfy enum value set: [CLOUDWATCH]" - the rejected value is dropped (as in the other causes) and the enum value set it advertises is [CLOUDWATCH] instead of the [CARNAVAL, CLOUDWATCH] the service reports.
Why: beyond losing the offending value, the message advertises a narrower set of accepted alarm types than the service does, so a client that reads the accepted values out of the error message is told CARNAVAL is not an accepted type.
Fix: the CreatePracticeRunConfiguration input validation for outcomeAlarms[].type - the enum value list used for the message text must be the service's set ([CARNAVAL, CLOUDWATCH]), independent of the public model's narrower ControlConditionType; plus the shared message builder fix above.

No open issue covers any of these (no existing arc-zonal-shift / zonal issues are filed).

Traces: arc-zonal-shift-list-operations-validation-errors#task#3 (and #task#5), arc-zonal-shift-autoshift-observer-notification-toggle#task#4, arc-zonal-shift-list-operations-validation-errors#task#4, arc-zonal-shift-practice-run-configuration-validation-errors#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200):

1) ListZonalShifts - arc-zonal-shift-list-operations-validation-errors#task#3 (also task#5): status 400 expected, 400 actual. $.message value_mismatch - expected "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100", actual "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100". ('101' is the request's value filling the template, not part of it.)

2) UpdateAutoshiftObserverNotificationStatus - arc-zonal-shift-autoshift-observer-notification-toggle#task#4: 400 expected, 400 actual. $.message value_mismatch - expected "1 validation error detected: Value 'BOGUS' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [ENABLED, DISABLED]", actual "1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [ENABLED, DISABLED]".

3) ListAutoshifts - arc-zonal-shift-list-operations-validation-errors#task#4: 400 expected, 400 actual. $.message value_mismatch - expected "1 validation error detected: Value 'BOGUS' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [ACTIVE, COMPLETED]", actual "1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [ACTIVE, COMPLETED]".

4) CreatePracticeRunConfiguration - arc-zonal-shift-practice-run-configuration-validation-errors#task#1: 400 expected, 400 actual. $.message value_mismatch - expected "1 validation error detected: Value 'BOGUS_TYPE' at 'outcomeAlarms.1.member.type' failed to satisfy constraint: Member must satisfy enum value set: [CARNAVAL, CLOUDWATCH]", actual "1 validation error detected: Value at 'outcomeAlarms.1.member.type' failed to satisfy constraint: Member must satisfy enum value set: [CLOUDWATCH]".

Model evidence (services/service-com-amazonaws-arczonalshift-percdataplane/smithy-model/com-amazonaws-arczonalshift-percdataplane.json): MaxResults integer @range 1..100; AutoshiftObserverNotificationStatus = [ENABLED, DISABLED]; AutoshiftExecutionStatus = [ACTIVE, COMPLETED]; ControlConditionType = [CLOUDWATCH] (service reports [CARNAVAL, CLOUDWATCH]); ValidationException errorKind client, httpError 400, required member message.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
2. Cause 1: GET /zonalshifts?maxResults=101 (ListZonalShifts) - fixture arc-zonal-shift-list-operations-validation-errors step task#3 (and task#5). Compare $.message of the 400 body against the recorded one: the quoted rejected value is absent.
3. Cause 2: PUT /autoshift-observer-notification with {"status":"BOGUS"} (UpdateAutoshiftObserverNotificationStatus) - fixture arc-zonal-shift-autoshift-observer-notification-toggle step task#4.
4. Cause 3: GET /autoshifts?status=BOGUS (ListAutoshifts) - fixture arc-zonal-shift-list-operations-validation-errors step task#4.
5. Cause 4: POST /configuration with outcomeAlarms[0].type set to an unmodeled value (CreatePracticeRunConfiguration) - fixture arc-zonal-shift-practice-run-configuration-validation-errors step task#1; the 400 message both drops the value and reports the enum set as [CLOUDWATCH] instead of [CARNAVAL, CLOUDWATCH].
6. Each step must match the recorded $.message exactly (with the request's own value interpolated) for the cause to be closed.
