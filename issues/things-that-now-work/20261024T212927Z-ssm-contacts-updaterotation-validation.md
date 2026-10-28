Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: aa34a7732176a121f53f6fa0a60cefb8de4c89bc

# UpdateRotation accepts a TimeZoneId that is not an IANA zone

_Filed: 2026-10-24T21:29:27Z_
_Operation: UpdateRotation | Class: validation_

> _Note: this issue was identified against the code base at commit `aa34a7732176a121f53f6fa0a60cefb8de4c89bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateRotation replaces the stored `TimeZoneId` with whatever the request names, checking only the modeled length/pattern at the boundary. The member is documented as IANA-format only, with the explicit note that designators such as "PST" are not supported, so an update can move a live rotation onto a zone the service cannot resolve and GetRotation will report it.

Expected: a pattern-legal but non-IANA TimeZoneId is refused with ValidationException (HTTP 400) and the stored rotation is left exactly as it was. Actual: HTTP 200 and the unusable zone persisted.

Same-class secondary symptom: `Recurrence.NumberOfOnCalls` is not checked against the rotation's team size, so an update can leave a one-contact rotation requiring 30 concurrent on-call members.

## Evidence

Spec: `smithy_query(query="doc", shape="com.amazonaws.ssmcontacts#CreateRotationRequest", member="TimeZoneId")` (the same `TimeZoneId` shape UpdateRotation takes) — IANA format, "Designators for time zones that don't support Daylight Savings Time rules, such as Pacific Standard Time (PST), are not supported."

Handler: `src/ops/update_rotation.rs` lines 188-257 — validates `DayOfWeek` enum values and that the recurrence carries a settings list, resolves `ContactIds`, then applies `rotation.time_zone_id = time_zone_id.clone()` with no zone check.

Tests: `tests/all/rotation_updates.rs` lines 316-342 update to real zones only, and lines 902-911 assert only the modeled pattern/length refusals ("Europe/Berlin!", a 256-character value); nothing asserts a refusal for a pattern-legal non-IANA zone.

## Steps to Reproduce

1. Onboard the account, CreateContact, then CreateRotation with TimeZoneId "UTC".
2. UpdateRotation {RotationId: <rotation arn>, TimeZoneId: "PST", Recurrence: {NumberOfOnCalls: 1, RecurrenceMultiplier: 1, DailySettings: [{HourOfDay: 9, MinuteOfHour: 0}]}}.
3. Observe HTTP 200 instead of a ValidationException; GetRotation now reports TimeZoneId "PST".
