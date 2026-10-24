Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail.lookup-events-cloudtrail-event-is-json-string cloudtrail.lookup-events-invalid-event-category-fails cloudtrail.lookup-events-invalid-next-token-fails cloudtrail.lookup-events-invalid-time-range-fails cloudtrail.lookup-events-max-results-bound-and-token-stability cloudtrail.lookup-events-ordering-and-shape cloudtrail.lookup-events-single-lookup-attribute-enforced cloudtrail.lookup-events-window-boundary-is-exact-and-future-window-is-empty cloudtrail.op-contract-lookup-events cloudtrail__cognito-identity.obs-cognito-identity-1-describe-missing-pool-is-rejected-and-audited-with-its-error cloudtrail__lambda.obs-lambda-3-duplicate-permission-rejected-and-audited cloudtrail__lambda.obs-lambda-3-management-events cloudtrail__sts.source-identity-propagates-from-assume-role-into-the-sessions-own-audit-records
Issue-Key: cloudtrail|LookupEvents|NotImplemented
Issue-Commit: c50178ef0adb9f41de3fed65acf95f496e080c81

# Implement cloudtrail LookupEvents: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c50178ef0adb9f41de3fed65acf95f496e080c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
LookupEvents

## Description
`LookupEvents` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/lookup_events.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

13 recorded unit(s) fail on their `LookupEvents` call, so each one unblocks only when `LookupEvents` works: `cloudtrail.lookup-events-cloudtrail-event-is-json-string`, `cloudtrail.lookup-events-invalid-event-category-fails`, `cloudtrail.lookup-events-invalid-next-token-fails`, `cloudtrail.lookup-events-invalid-time-range-fails`, `cloudtrail.lookup-events-max-results-bound-and-token-stability`, `cloudtrail.lookup-events-ordering-and-shape`, `cloudtrail.lookup-events-single-lookup-attribute-enforced`, `cloudtrail.lookup-events-window-boundary-is-exact-and-future-window-is-empty`, `cloudtrail.op-contract-lookup-events`, `cloudtrail__cognito-identity.obs-cognito-identity-1-describe-missing-pool-is-rejected-and-audited-with-its-error`, `cloudtrail__lambda.obs-lambda-3-duplicate-permission-rejected-and-audited`, `cloudtrail__lambda.obs-lambda-3-management-events`, `cloudtrail__sts.source-identity-propagates-from-assume-role-into-the-sessions-own-audit-records`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/lookup_events.rs`. Replaying the corpus, the emulator refused the `LookupEvents` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the LookupEvents operation: operation amazonaws.cloudtrail.v20131101.CloudTrail_20131101.LookupEvents is not implemented in this emulator

## Steps to Reproduce
Boot the cloudtrail emulator
Call LookupEvents and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

## Resolution

`LookupEvents` is implemented in
`services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/src/ops/lookup_events.rs`
(the `SCAFFOLDED, NOT IMPLEMENTED` header line is gone) and serves the account's
management-event log — the shared store `services/common/src/management_events.rs`, held
once in the engine, written by the service that SERVED a call. CloudTrail's four
trail-mutating handlers (`CreateTrail`, `DeleteTrail`, `StartLogging`, `StopLogging`)
record their calls there, successes and refusals alike. End-to-end coverage lives in
`services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/tests/all/management_event_lookup.rs`.

A boundary refusal of `LookupEvents`'s `EventCategory` or of anything under its
`LookupAttributes` is answered in the error the model declares for exactly that case
(`InvalidEventCategoryException` / `InvalidLookupAttributesException`, `src/service.rs`'s
`override_refusal`) rather than in the service-wide `ValidationException` envelope — the
same rule the event- and Insights-selector refusals already follow. The two rows that
asserted the generic envelope for those members in
`tests/all/enum_refusal_wording.rs` therefore moved into that suite's new
`a_lookup_events_enum_violation_keeps_its_modelled_error`, which spans several
out-of-set values and both the top-level and the list-nested attribute path.

Three things this change deliberately does NOT do, recorded here so a later issue can
pick them up rather than rediscover them (plus the recorders it does not wire up, item 4):

1. **`MaxResults` outside `1..50` answers `ValidationException`, not the modelled
   `InvalidMaxResultsException`.** The modelled `@range` is refused at the framework
   boundary before any handler runs, and the only seam that can reword a boundary refusal
   (`Service::override_refusal`) is not told which operation was called — while a dozen
   operations of this model carry a `MaxResults` member and model DIFFERENT errors for a
   bad one (`InvalidMaxResultsException`, `InvalidParameterException`, or none at all), so
   a service-wide rewording would answer the wrong code for most of them. The structural
   fix is to pass the resolved operation to `override_refusal` in
   `vendor/smithy-server-runtime`, which is outside an emulator fixer's reach (the
   `vendor/` tree is read-only). Until then the unit
   `cloudtrail.lookup-events-max-results-bound-and-token-stability` may still diverge on
   that error code alone; the page bound and the token stability it also checks are
   implemented and tested.
2. **Read-only management events are not recorded.** An audit record is state, and in this
   emulator a reporting call must not change what a later reader sees, so `LookupEvents`,
   `GetTrail`, `GetTrailStatus`, `DescribeTrails` and `ListTrails` record nothing and every
   recorded event carries `readOnly: false`.
3. **Cross-service recording is not wired up.** The units
   `cloudtrail__lambda.*`, `cloudtrail__sts.*` and `cloudtrail__cognito-identity.*` expect
   those services' calls to be audited; that needs each of those services' handlers to
   append to the shared log (which is why the store lives in `services/common`), and their
   handlers belong to their own issues.
4. **Only this service's four trail-mutating handlers record.** `CreateTrail`,
   `DeleteTrail`, `StartLogging` and `StopLogging` call `record_management_event`; the
   other state-changing operations this crate implements (the dashboard, event data store,
   import, Lake query and tagging handlers) still serve their calls without an audit
   record, so a `LookupEvents` caller sees an audit log without them. Each needs its own
   `requestParameters` projection of the nested modelled shapes it takes, which belongs
   with that handler; the helper they call is already in place and nothing about the log is
   per-operation.
