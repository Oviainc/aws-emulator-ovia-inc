Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2a22026c0f737b6187455fb636429eae77d015a7

# PutBandwidthRateLimitSchedule accepts semantically invalid schedules (overlapping intervals, end before start)

_Filed: 2026-10-24T16:45:21Z_
_Operation: PutBandwidthRateLimitSchedule | Class: validation_

> _Note: this issue was identified against the code base at commit `2a22026c0f737b6187455fb636429eae77d015a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: a bandwidth rate limit schedule is a set of non-overlapping intervals the gateway appliance can actually apply, so the service refuses a schedule whose intervals cannot hold together, with the modeled ValidationException (HTTP 400) and nothing persisted:
* two intervals that share at least one day of the week and whose [StartHourOfDay:StartMinuteOfHour, EndHourOfDay:EndMinuteOfHour] windows overlap (an appliance cannot apply two different upload rate limits at the same instant);
* an interval whose end is earlier than its start on the same day (an interval does not wrap midnight);
* (same family) a DaysOfWeek list that repeats a day.

Actual: the handler performs no semantic check at all. It validates nothing beyond the per-member constraints the boundary validator already enforced (ranges on the hour/minute members, 51200.. on AverageUploadRateLimitInBitsPerSec, 0..20 intervals, 1..7 DaysOfWeek) and stores the list verbatim, answering HTTP 200. GetBandwidthRateLimitSchedule then reports a contradictory schedule — e.g. Monday 09:00-17:00 at 51200 bits/s together with Monday 10:00-12:00 at 8000000 bits/s, or an interval 22:00-06:00 — that AWS would never have accepted. A client that relies on the service to reject a mis-built schedule (the common case when a UI or IaC tool computes intervals) gets a success and a schedule the real gateway refuses.

## Evidence

Spec/well-established semantics: smithy_query(shape="BandwidthRateLimitInterval") — "A bandwidth rate limit schedule consists of one or more bandwidth rate limit intervals. A bandwidth rate limit interval defines a period of time on one or more days of the week, during which bandwidth rate limits are specified"; StartHourOfDay/StartMinuteOfHour/EndHourOfDay/EndMinuteOfHour and DaysOfWeek are all @required, and the interval is an intra-day window (the EndMinuteOfHour doc: "The bandwidth rate limit interval ends at the end of the minute"). The non-overlap rule is established behavior of the gateway bandwidth-rate-limit-schedule API family (the same appliance stack as Storage Gateway, whose bandwidth rate limit intervals are documented as not being allowed to overlap and whose API rejects an overlapping or end-before-start interval); PutBandwidthRateLimitSchedule declares ValidationException among its errors (smithy_query(operation="PutBandwidthRateLimitSchedule")), which is the vehicle for the refusal.
Handler: src/ops/put_bandwidth_rate_limit_schedule.rs lines ~174-200 — the only checks are the store lookup for the GatewayArn; the comment states every modeled constraint was refused at the boundary "so reaching here means the input satisfies all of them" and the list is moved straight into state. src/state.rs `set_bandwidth_rate_limit_schedule` (lines ~1007-1014) assigns the intervals verbatim with no cross-interval validation.
Tests: tests/all/put_bandwidth_rate_limit_schedule.rs and tests/all/bandwidth_rate_limit_schedule.rs span interval counts, ordering and each published member bound only (module docs enumerate the cases); a repo-wide grep for "overlap" in this crate's tests returns nothing, so no test asserts either acceptance or refusal of an overlapping / inverted interval.

## Steps to Reproduce

1. Boot the emulator, make a gateway exist and note its GatewayArn.
2. Call PutBandwidthRateLimitSchedule with two intervals that overlap on one day: {StartHourOfDay:9, StartMinuteOfHour:0, EndHourOfDay:17, EndMinuteOfHour:59, DaysOfWeek:[1], AverageUploadRateLimitInBitsPerSec:51200} and {StartHourOfDay:10, StartMinuteOfHour:0, EndHourOfDay:12, EndMinuteOfHour:0, DaysOfWeek:[1], AverageUploadRateLimitInBitsPerSec:8000000}.
3. Observe HTTP 200 and the GatewayArn, where a ValidationException (400) is expected and nothing should be stored.
4. Call GetBandwidthRateLimitSchedule for that gateway and observe both conflicting intervals reported.
5. Call PutBandwidthRateLimitSchedule with a single interval whose end precedes its start on the same day: {StartHourOfDay:22, StartMinuteOfHour:0, EndHourOfDay:6, EndMinuteOfHour:0, DaysOfWeek:[3]} -> observe HTTP 200 instead of a ValidationException.
6. Call PutBandwidthRateLimitSchedule with DaysOfWeek:[1,1] -> observe HTTP 200 instead of a ValidationException.
