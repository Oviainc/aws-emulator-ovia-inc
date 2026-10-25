Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# Validate EnableControl's controlIdentifier, targetIdentifier and parameters instead of enabling anything ARN-shaped

_Filed: 2026-10-24T07:41:10Z_
_Operation: EnableControl | Class: validation_

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

EnableControl persists whatever `controlIdentifier`, `targetIdentifier` and `parameters` the request carries, with no semantic check at all, so requests real AWS refuses with a 4xx answer 200 and mint an `EnabledControl` resource (plus its ARN, target regions and tags) that could not exist in AWS.

Expected (real AWS): `controlIdentifier` must name a control from Control Tower's fixed control catalog (ARNs of the form `arn:aws:controltower:<region>::control/<CONTROL-NAME>` or the newer `arn:<partition>:controlcatalog:::control/<id>`); an ARN of another service (e.g. `arn:aws:s3:::my-bucket`) or a controltower-shaped ARN naming no catalogued control is refused with ValidationException (400) / ResourceNotFoundException (404), both of which this operation models. `targetIdentifier` must name an organizational unit that is registered with (governed by) the Control Tower landing zone — an ARN that is not an Organizations OU, or an OU/account the organization does not contain, is refused; enabling a control also requires a landing zone to exist in the account at all. `parameters` keys must be parameters the named control declares (e.g. `AllowedRegions` for the region-deny control); an unknown key, or any parameter at all for a control that declares none, is refused with ValidationException.

Actual (emulator): the handler's only input checks are generic ones — a cap on the parameter count, a cap on the parameter payload weight, and a duplicate-key refusal — plus the store's enabled-control quota and the already-enabled-pair rule. The control identifier, the target identifier and the parameter KEYS are never checked against anything: they are written verbatim into `state.enabled_controls` and reported back by `GetEnabledControl`/`ListEnabledControls`. An enablement of a non-existent control on a non-existent target therefore becomes persisted state, consumes the account's enablement quota, and (with no landing zone deployed) is reported as deployed into the request's signing region.

## Evidence

Smithy model: EnableControl (POST /enable-control) declares both ValidationException (400) and ResourceNotFoundException (404) among its errors, and its `controlIdentifier`/`targetIdentifier` are typed as the generic `ControlIdentifier`/`TargetIdentifier` (len 20..2048, pattern ^arn:aws[0-9a-zA-Z_\-:\/]+$) — the wire type cannot express "is a catalogued control" or "is a governed OU", so the semantic check belongs to the handler. The member docs say `controlIdentifier` is found via the service's control overview page and `targetIdentifier` is "the ARN of the organizational unit", i.e. both are service-owned identifiers rather than caller-chosen strings. Control Tower requires a landing zone and a registered OU before a control can be enabled — well-established service behavior.

Handler: services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/enable_control.rs lines 183-342 — parameter count/weight/duplicate-key checks, then the already-enabled and quota checks, then `store.enabled_controls.insert(arn, EnabledControl { control_identifier: input.control_identifier, target_identifier: input.target_identifier, parameters, .. })` with no catalog, organization or parameter-name validation. `crate::state::governed_regions()` even falls back to the signing region when NO landing zone is deployed (src/state.rs:714-734), i.e. the handler is designed to succeed with no landing zone.

Tests: tests/all/enabled_control_lifecycle.rs uses realistic-looking control ARNs but asserts only front-door enum refusals, the duplicate-parameter-key refusal, the parameter count/weight caps and the already-enabled rule (`a_request_the_state_cannot_satisfy_is_refused_with_its_modeled_error`, `the_parameter_list_is_bounded_in_count_and_in_weight`, `a_control_is_enabled_at_most_once_per_target`); `target_regions_are_the_governed_regions_of_the_deployed_landing_zone` exercises the no-landing-zone path. No test asserts that an uncatalogued control, a non-Organizations target or an undeclared parameter key is refused. The backlog holds the analogous EnableBaseline validation issue but none for EnableControl.

## Steps to Reproduce

1. POST /enable-control with {"controlIdentifier":"arn:aws:s3:::my-example-bucket-name","targetIdentifier":"arn:aws:organizations::123456789012:ou/o-exampleorg/ou-aa11-1a"} on a fresh emulator (no landing zone created) — the emulator answers 200 with an `arn` and an `operationIdentifier`; AWS refuses the request because that ARN names no control (and because the account has no landing zone).
2. POST /get-enabled-control with the returned arn — the emulator reports the bogus controlIdentifier/targetIdentifier back and, after the settle span, status SUCCEEDED with drift IN_SYNC, i.e. the enablement is now persisted state.
3. POST /enable-control with a catalog-shaped control ARN (e.g. arn:aws:controltower:us-west-2::control/AWS-GR_ENCRYPTED_VOLUMES) but a targetIdentifier that is not an Organizations target, e.g. "arn:aws:iam::123456789012:role/not-a-target" — emulator answers 200 and stores the enablement; AWS refuses with a validation/not-found failure.
4. POST /enable-control with {"controlIdentifier":"arn:aws:controltower:us-west-2::control/AWS-GR_ENCRYPTED_VOLUMES","targetIdentifier":"arn:aws:organizations::123456789012:ou/o-exampleorg/ou-bb22-2b","parameters":[{"key":"NoSuchParameter","value":true}]} — emulator answers 200 and stores the undeclared parameter, which GetEnabledControl then reports; AWS refuses a parameter the control does not declare.
