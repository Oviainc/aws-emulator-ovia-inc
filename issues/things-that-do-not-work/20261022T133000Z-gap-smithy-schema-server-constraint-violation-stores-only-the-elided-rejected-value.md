Issue-Severity: MEDIUM
Issue-Gap-Class: framework

# `vendor/smithy-schema-server` stores only the ELIDED rejected value on a constraint `Violation`, so a service cannot word the AWS clause that echoes the value as submitted

_Filed: 2026-10-22T13:30:00Z_
_Operation: vendor/smithy-schema-server (constraint validation and message construction) | Class: framework_

## Description

An AWS constraint-validation message is BUILT FROM THE REQUEST:
`1 validation error detected: Value '<value>' at '<member path>' failed to satisfy
constraint: <requirement>`, with the rejected value reproduced as submitted. The value is
the only place the server reports the offending input back, so SDK test suites,
log-based triage and regression fixtures compare it against what they sent.

`vendor/smithy-schema-server/src/constraints.rs` elides that value *as it builds the
violation*, not as it renders a message:

- `echoed()` (`MAX_ECHOED_VALUE` = 256 bytes) turns a longer value into
  `<prefix>... [<n> bytes]`, text AWS never emits;
- `LengthTrait::check`, `RangeTrait::check`, `PatternTrait::check`, `EnumTrait::check` and
  the `@uniqueItems` check each store `Some(echoed(value).into_owned())` in
  `Violation.value`, whose own doc states it is "ALREADY elided";
- `Violation::message` and `constraints::envelope` then interpolate whatever is stored.

A service has no seam that sees the unelided value: `Service::override_refusal` is handed
the violations alone, never the request they came from, and `constraints::validate`
returns violations that are already elided. So every service that words its own
`ValidationException` from these violations echoes a value the caller never submitted.

## Observable effect

sagemakerjobruntime, `SampleWithResponseStream` with a 300-character
`X-Amzn-SageMaker-Trajectory-Id` (`@length` max 256):

- recorded AWS: `1 validation error detected: Value '<all 300 characters>' at
  'trajectoryId' failed to satisfy constraint: Member must have length less than or equal
  to 256`
- emulator before the workaround: the same sentence with
  `'<first 256 bytes>... [300 bytes]'`.

The same shortening applies to `@pattern`, `@enum` and `@uniqueItems` clauses, and to any
service wording built from `Violation.value` (sagemakerjobruntime words a value that is
not an ARN as `Input arn <value> is malformed`).

## Scope

Every service whose refusal text is built from `constraints::validate`'s violations —
every value longer than 256 bytes on any `@length`/`@pattern`/`@enum`/`@uniqueItems`
member. Two issues already recorded it:
`issues/things-that-now-work/20261022T021402Z-marketplace-discovery-constraint-validation-messages-diverge-from-aws.md`
(cause 3, DECLINED as vendor-blocked) and
`issues/things-that-now-work/20261022T115143Z-sagemakerjobruntime-validationexception-message-truncates-the-rejected-trajectoryid-value-instead.md`
(worked around, see below).

## Fix

Keep the amplification bound, but apply it where the message is RENDERED rather than where
the violation is BUILT. Either:

1. carry both values on `Violation` — the unelided one beside the elided one — so a
   renderer can choose; or
2. store the value unelided and call `echoed()` inside `Violation::message` (and let a
   caller that renders its own clause read the full value), keeping the per-message and
   per-clause budgets in the renderer.

Either shape lets a service echo the value AWS echoes, with the resource bound still
stated in one place. This cannot be done from inside the emulator repo: `vendor/` is
read-only here and the merge gate refuses a change to it.

## Current workaround, and why it should be removed

`services/common/src/validation.rs::with_submitted_values` RE-WALKS the same built input
the constraint walk validated, with a `ShapeSerializer` that mirrors
`ValidatingSerializer`'s private path bookkeeping (`Frame::{List,Map,Struct}`,
`begin_child`/`end_child`/`with_segment`, `write_struct`/`write_list`/`write_map`), to
recover each violation's value from the leaf at its path. It is bounded (16 KiB per
clause, 64 KiB per message, 256 leaves) and fail-safe (a drifted path simply yields no
match, so the framework's own text stands), but it is a second copy of framework-internal
path rules with no compile-time link to the original: the two can diverge silently.
`services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/src/service.rs`
(`refused_by`) is its only caller today. Both the helper and that call should be deleted
once `Violation` exposes the submitted value.

A residual divergence stays even with the workaround: a value above the helper's 16 KiB
per-clause bound still reaches the wire shortened, because an unbounded echo would make
the response size a function of the request.

## Evidence

`vendor/smithy-schema-server/src/constraints.rs` — `echoed()` and `MAX_ECHOED_VALUE`, the
`Violation.value` doc ("ALREADY elided"), and the four `check` methods plus the
`@uniqueItems` arm that call `echoed()` while BUILDING a violation.
`services/service-com-amazonaws-sagemakerjobruntime-agenticrftruntimeservice/tests/all/rejected_value_echo.rs`
— drives the rule over sizes, encodings, bindings and constraint classes, and fails
without the workaround.
