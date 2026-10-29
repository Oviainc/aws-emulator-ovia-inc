Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail.lookup-events-invalid-event-category-fails cloudtrail.lookup-events-invalid-next-token-fails cloudtrail.lookup-events-invalid-time-range-fails cloudtrail.lookup-events-single-lookup-attribute-enforced
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail.lookup-events-invalid-event-category-fails|run#5|AssertionError|assert message == "Invalid event category specified.", message
Issue-Key: cloudtrail.lookup-events-invalid-next-token-fails|run#7|AssertionError|assert message == "Can't decrypt marker", message
Issue-Key: cloudtrail.lookup-events-invalid-time-range-fails|run#7|AssertionError|assert message == "The start time precedes the end time.", message
Issue-Key: cloudtrail.lookup-events-single-lookup-attribute-enforced|run#13|AssertionError|assert unknown["Error"].get("Message") == "Lookup attribute key cannot be null or empty.", unknown["Error"]
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# CloudTrail LookupEvents rejection messages use a generic constraint-violation template instead of AWS's fixed per-error text (4 causes)

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
LookupEvents

## Description
LookupEvents refuses the right inputs with the right error codes and HTTP 400, but every rejection message is generated from a generic validation template rather than the fixed text AWS returns for that error class. The error code is only half of an AWS error; clients, SDK retry/branching helpers and incident runbooks match on the message, and operator-facing tooling prints it verbatim. 4 distinct net-new causes, one per modelled error:

Cause 1 of 4: InvalidEventCategoryException message.
AWS: Smithy (com-amazonaws-cloudtrail-cloudtrail-20131101) models LookupEvents.EventCategory as enum EventCategory and binds error InvalidEventCategoryException (errorKind client, httpError 400, awsQueryError code InvalidEventCategory). Recorded behavior: an EventCategory outside the enum is refused 400 / InvalidEventCategoryException with the message "Invalid event category specified." — entirely FIXED text, with no variable part: it does not echo the rejected value and does not enumerate the supported values.
Emulator: returns code InvalidEventCategoryException (correct) with the message "Value 'sim2real-behav-bogus-category' at 'eventCategory' is not valid: the supported values are [insight]" — a generic enum-constraint template that interpolates the rejected value and the enum member list.
Why: a client asserting on, logging or branching on AWS's "Invalid event category specified." sees an unrecognized message; anything parsing the AWS shape (a message with no interpolated value) mis-parses.
Fix: the LookupEvents handler's EventCategory validation in the cloudtrail service crate must raise InvalidEventCategoryException with CloudTrail's own fixed message instead of falling through to the shared enum-constraint message builder.
Traces: cloudtrail.lookup-events-invalid-event-category-fails

Cause 2 of 4: InvalidNextTokenException message.
AWS: Smithy binds InvalidNextTokenException (client, 400, awsQueryError InvalidNextToken) to LookupEvents, whose NextToken is an opaque encrypted marker (operation is paginated: inputToken/outputToken NextToken). Recorded behavior: a NextToken the service never minted is refused 400 / InvalidNextTokenException with the message "Can't decrypt marker" — all FIXED text, no variable part, and in particular it never echoes the supplied token.
Emulator: returns code InvalidNextTokenException (correct) with the message "NextToken is not valid: pass the token exactly as the previous LookupEvents call returned it, with the same lookup parameters" — prose the service does not produce.
Why: pagination wrappers that distinguish "token expired/rotated" from other validation failures by matching AWS's marker-decrypt message never match, and a client that surfaces the message to an operator shows text no AWS runbook mentions.
Fix: NextToken decode/validation in the LookupEvents handler must emit CloudTrail's fixed marker message.
Traces: cloudtrail.lookup-events-invalid-next-token-fails

Cause 3 of 4: InvalidTimeRangeException message.
AWS: Smithy binds InvalidTimeRangeException (client, 400, awsQueryError InvalidTimeRange) to LookupEvents, with StartTime/EndTime targeting timestamp shape Date. Recorded behavior: StartTime after EndTime is refused 400 / InvalidTimeRangeException with the message "The start time precedes the end time." — all FIXED text; it contains NO request-derived values (no timestamps, no member names).
Emulator: returns code InvalidTimeRangeException (correct) with the message "StartTime (<start> ms since the epoch) is after EndTime (<end> ms since the epoch): the start of a lookup window must not be after its end", interpolating the two request epoch-millisecond values. The epoch numbers are per-run values filling that template — the defect is the template itself, not the numbers.
Why: the emulator's message is both differently worded and differently SHAPED (variable where AWS is fixed), so a client or test asserting AWS's sentence fails, and log-based alerting keyed on it never fires.
Fix: the StartTime/EndTime ordering check in the LookupEvents handler must raise CloudTrail's fixed message rather than a formatted comparison message.
Traces: cloudtrail.lookup-events-invalid-time-range-fails

Cause 4 of 4: InvalidLookupAttributesException message for an unmodelled AttributeKey.
AWS: Smithy binds InvalidLookupAttributesException (client, 400, awsQueryError InvalidLookupAttributes) to LookupEvents; LookupAttributes is a list of LookupAttribute{AttributeKey (enum), AttributeValue}. Recorded behavior: an AttributeKey outside the enum is refused 400 / InvalidLookupAttributesException with the FIXED message "Lookup attribute key cannot be null or empty." (and a modelled key given an out-of-domain value — e.g. ReadOnly=maybe — is refused with the FIXED message "Lookup attribute key is invalid"). Neither message echoes the rejected value, the list index or the member path.
Emulator: returns code InvalidLookupAttributesException (correct) with the message "Value 'NotARealAttributeKey' at 'lookupAttributes.1.member.attributeKey' is not valid: the supported values are [EventId, EventName, ReadOnly, Username, ResourceType, ResourceName, EventSource, AccessKeyId]" — the same generic enum-constraint template as Cause 1, here additionally leaking a Smithy member path ('lookupAttributes.1.member.attributeKey') that AWS never puts in this error.
Why: callers that classify lookup-attribute refusals by AWS's two fixed sentences cannot tell an unknown KEY from an out-of-domain VALUE, and the leaked member path is an emulator-internal detail no AWS consumer should see.
Fix: LookupAttributes validation in the LookupEvents handler must raise the two fixed CloudTrail messages for the key-unknown and value-out-of-domain cases instead of the shared enum/member-path constraint builder.
Traces: cloudtrail.lookup-events-single-lookup-attribute-enforced

Note: this is NOT covered by the open control-plane issue 20261024T114720Z-cloudtrail-control-plane-fidelity-gettrailstatus-omits-its-deprecated.md (that one is about GetTrailStatus members, ListTags limits and Lake queries); these four checks are not owned by any open issue in the replay report.

## Evidence
Replay report (suite simparity-behavioral-aws-v1), four [mismatch] verdicts, each an assertion on the message while the code matched:

1) cloudtrail.lookup-events-invalid-event-category-fails
   assert message == "Invalid event category specified."
   AssertionError: Value 'sim2real-behav-bogus-category' at 'eventCategory' is not valid: the supported values are [insight]
   (code InvalidEventCategoryException asserted first and PASSED)

2) cloudtrail.lookup-events-invalid-next-token-fails
   assert message == "Can't decrypt marker"
   AssertionError: NextToken is not valid: pass the token exactly as the previous LookupEvents call returned it, with the same lookup parameters
   (code InvalidNextTokenException asserted first and PASSED)

3) cloudtrail.lookup-events-invalid-time-range-fails
   assert message == "The start time precedes the end time."
   AssertionError: StartTime (1793294032335 ms since the epoch) is after EndTime (1793207632335 ms since the epoch): the start of a lookup window must not be after its end
   (code InvalidTimeRangeException asserted first and PASSED; the two epoch values are this run's clock and are NOT part of the requirement)

4) cloudtrail.lookup-events-single-lookup-attribute-enforced
   assert unknown["Error"].get("Message") == "Lookup attribute key cannot be null or empty."
   AssertionError: {'Message': "Value 'NotARealAttributeKey' at 'lookupAttributes.1.member.attributeKey' is not valid: the supported values are [EventId, EventName, ReadOnly, Username, ResourceType, ResourceName, EventSource, AccessKeyId]", 'Code': 'InvalidLookupAttributesException'}

Expected vs actual, per cause: HTTP status 400 and the error Code match AWS in all four; only Error.Message diverges. Expected messages are fixed text with no request-derived parts; actual messages are generated templates that interpolate the rejected value, the enum member list, the request timestamps and a Smithy member path.

## Steps to Reproduce
Against the booted emulator, with a cloudtrail client:
1. ct.lookup_events(MaxResults=1) — positive control, must succeed (it does).
2. ct.lookup_events(EventCategory="not-a-category") -> expect 400 InvalidEventCategoryException / "Invalid event category specified."; observe the generic enum-constraint message.
3. ct.lookup_events(NextToken="not-a-real-token") -> expect 400 InvalidNextTokenException / "Can't decrypt marker"; observe the pass-the-token-exactly prose.
4. ct.lookup_events(StartTime=now, EndTime=now-1day) -> expect 400 InvalidTimeRangeException / "The start time precedes the end time."; observe the epoch-millisecond comparison message.
5. ct.lookup_events(LookupAttributes=[{"AttributeKey":"EventName","AttributeValue":"x"},{"AttributeKey":"NotARealAttributeKey","AttributeValue":"y"}]) -> expect 400 InvalidLookupAttributesException / "Lookup attribute key cannot be null or empty."; observe the member-path enum message.
6. ct.lookup_events(LookupAttributes=[{"AttributeKey":"ReadOnly","AttributeValue":"maybe"}]) -> expect 400 InvalidLookupAttributesException / "Lookup attribute key is invalid".
