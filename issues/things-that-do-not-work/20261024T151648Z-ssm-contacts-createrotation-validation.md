Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d0704f2fe7a69fbeda61cd50377e188330130aec

# CreateRotation accepts non-PERSONAL contacts in ContactIds and an arbitrary TimeZoneId

_Filed: 2026-10-24T15:16:48Z_
_Operation: CreateRotation | Class: validation_

> _Note: this issue was identified against the code base at commit `d0704f2fe7a69fbeda61cd50377e188330130aec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two spec-required input checks are missing.

1. `ContactIds`: the model's own documentation for this member says "The Amazon Resource Names (ARNs) of the contacts to add to the rotation. Note: Only the `PERSONAL` contact type is supported. The contact types `ESCALATION` and `ONCALL_SCHEDULE` are not supported for this operation." Expected: a `ContactIds` entry that resolves to a contact whose `Type` is `ESCALATION` or `ONCALL_SCHEDULE` is refused with `ValidationException` (HTTP 400). Actual: the handler only checks that each ARN resolves to *some* stored contact, so a rotation can be created whose team is an escalation plan or another on-call schedule, and `GetRotation`/`ListRotations` then report it as a valid rotation team.

2. `TimeZoneId`: documented as "The time zone to base the rotation's activity on in Internet Assigned Numbers Authority (IANA) format. For example: \"America/Los_Angeles\", \"UTC\", or \"Asia/Seoul\" ... Designators for time zones that don't support Daylight Savings Time rules, such as Pacific Standard Time (PST), are not supported." Expected: a value that is not an IANA zone name (e.g. "PST", "Not/AZone") is refused with `ValidationException`. Actual: the handler stores whatever satisfies the modelled pattern `^[:a-zA-Z0-9_\-\s\.\\/]*$` and `GetRotation` echoes it back, so the rotation's shift computation would be based on a zone that does not exist.

## Evidence

Spec: `smithy_query(query="doc", shape="com.amazonaws.ssmcontacts#CreateRotationRequest", member="ContactIds")` returns the explicit note that only `PERSONAL` is supported for this operation; the same query for `TimeZoneId` returns the IANA-format requirement and the explicit exclusion of non-DST designators such as PST. `CreateRotation` declares `ValidationException` (HTTP 400). Handler: `services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/create_rotation.rs` lines ~243-257 check only `state.contacts.contains_key(contact_id)` (never `contact.contact_type`), and `input.time_zone_id` is copied straight into `RotationRecord` at line ~302 with no zone check; the only validations performed are the `DayOfWeek` enum values and the presence of one of daily/weekly/monthly settings. Tests: `tests/all/rotation_reads.rs` and `tests/all/data_plane_precondition.rs` only use valid IANA zones ("UTC", "America/Los_Angeles", "Asia/Seoul", "Etc/UTC") and personal contacts, so no test asserts the permissive behaviour.

## Steps to Reproduce

1. Onboard the account, then CreateContact with Alias="escalation-tier-1", Type="ESCALATION", Plan={"Stages":[{"DurationInMinutes":5,"Targets":[]}]}.
2. CreateRotation with Name="weeknights", ContactIds=[<the ESCALATION contact ARN>], TimeZoneId="UTC", Recurrence={"NumberOfOnCalls":1,"RecurrenceMultiplier":1,"DailySettings":[{"HourOfDay":9,"MinuteOfHour":0}]}.
3. Expected: ValidationException (400) — only PERSONAL contacts may be rotation members. Actual: HTTP 200 with a RotationArn, and GetRotation reports the escalation plan as a rotation member.
4. CreateRotation again with a PERSONAL contact but TimeZoneId="PST" (or "Not/AZone").
5. Expected: ValidationException (400) for a non-IANA time zone. Actual: HTTP 200, and GetRotation/ListRotations echo "PST" back as the rotation's time zone.
