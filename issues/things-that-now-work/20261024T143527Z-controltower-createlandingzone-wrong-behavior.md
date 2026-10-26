Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# Mint Control Tower resource ids in AWS's 16-character uppercase alphanumeric form, not as 36-character UUIDs

_Filed: 2026-10-24T14:35:27Z_
_Operation: CreateLandingZone | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every ARN this service mints uses a 36-character lowercase-hex UUID (the same generator as `operationIdentifier`) as the resource segment, so `CreateLandingZone` answers with e.g. `arn:aws:controltower:us-west-2:123456789012:landingzone/4f9a1c27-9e62-4f3f-bd4a-1f2a3b4c5d6e`.

Expected (real AWS): Control Tower resource ids are 16-character uppercase alphanumeric strings — the documented landing zone ARN form is `arn:aws:controltower:us-west-2:123456789012:landingzone/4B3E6KDSOOU1BV4V`, and the same house style applies to the `enabledcontrol/` and `enabledbaseline/` resources (the model's own `BaselineArn` pattern spells it out as `[A-Z0-9]{16}`). A client that round-trips, parses, or pattern-matches a Control Tower resource id sees a 16-character uppercase token, never a hyphenated UUID.

Actual (emulator): `new_landing_zone_id`, `new_enabled_control_id` and `new_enabled_baseline_id` all alias `new_operation_identifier()`, so the landing zone ARN, the enabled-control ARN and the enabled-baseline ARN all end in a UUID that is indistinguishable from an operation identifier. The ids satisfy the generic `Arn` pattern, so nothing refuses them, but they are not values AWS would ever produce — and an operation identifier and a resource id become confusable in a response body.

## Evidence

Smithy model: `BaselineArn` is `^arn:[a-z-]+:controltower:[a-z0-9-]*:[0-9]{0,12}:baseline/[A-Z0-9]{16}$` — the one Control Tower resource ARN the model constrains precisely, establishing the service's resource-id form (16 chars of [A-Z0-9]); `OperationIdentifier` is separately constrained as a 36-character lowercase-hex UUID, so the model treats the two as different kinds of token. AWS's own documented landing zone ARN example (`...:landingzone/4B3E6KDSOOU1BV4V`) uses the 16-character uppercase form.

Handler/state: src/ops/create_landing_zone.rs lines 242-243 — `crate::state::landing_zone_arn(&region, &crate::state::new_landing_zone_id())`; src/state.rs lines 1556-1573 — `pub fn new_landing_zone_id() -> String { new_operation_identifier() }` with the comment "The same 36-character lowercase-hex UUID form as an OperationIdentifier"; the same aliasing at src/state.rs lines 1436-1442 (`new_enabled_baseline_id`) and 1575-1581 (`new_enabled_control_id`), used by EnableBaseline/EnableControl. The emulator's own `baseline_arn` (src/state.rs line 1537) does use catalog ids of the correct 16-char uppercase shape, so the divergence is confined to the minted per-account resources.

Tests: tests/all/landing_zone_lifecycle.rs `every_minted_identifier_is_well_formed_and_unique` (around line 1066) asserts minted ids satisfy `is_operation_identifier`, i.e. it pins the UUID form rather than the AWS form; no test asserts the AWS resource-id shape. A sibling ARN-shape issue was accepted and fixed for GetBaseline ("GetBaseline returns a baseline ARN with an account-id segment"), so this class of fidelity gap is in scope; no issue covers the minted landing zone / enabled control / enabled baseline ids.

## Steps to Reproduce

1. POST /create-landingzone with {"version":"3.3","manifest":{...}} — the returned `arn` ends in `landingzone/<36-char hyphenated UUID>`; AWS returns `landingzone/<16 uppercase alphanumerics>`.
2. POST /list-landingzones — the listed ARN carries the same UUID resource segment.
3. POST /enable-control (or /enable-baseline) with a valid control/baseline and target — the returned `arn` ends in `enabledcontrol/<UUID>` / `enabledbaseline/<UUID>`, while the `operationIdentifier` returned alongside it has the identical form, so the two tokens are indistinguishable.
4. Compare with POST /list-baselines, whose catalog ARNs correctly end in 16 uppercase alphanumerics (e.g. baseline/17BSJV3IGJ2QSGA2).
