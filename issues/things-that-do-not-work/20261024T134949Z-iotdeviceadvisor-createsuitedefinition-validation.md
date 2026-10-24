Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 30fa86cd244b801d1c6064a0a321e2bf32f37967

# Validate the device entries and devicePermissionRoleArn of a suite definition configuration

_Filed: 2026-10-24T13:49:49Z_
_Operation: CreateSuiteDefinition | Class: validation_

> _Note: this issue was identified against the code base at commit `30fa86cd244b801d1c6064a0a321e2bf32f37967`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateSuiteDefinition (and UpdateSuiteDefinition, which shares the same configuration-validation path) accepts a `suiteDefinitionConfiguration` whose `devices` list contains an entry naming no device — `"devices": [{}]` — and stores it, so GetSuiteDefinition/ListSuiteDefinitions later report a device with no identity. The model documents the requirement in prose on the `DeviceUnderTest` shape: "Information of a test device. A thing ARN, certificate ARN or device role ARN is required." All three members of `DeviceUnderTest` are optional in the model, so the framework's @required/@length validator cannot catch it and the handler has to; the handler already does exactly this for the other prose-only rule of the same shape (`rootGroup` vs `intendedForQualification`, via `crate::state::root_group_violation`), but not for devices.

In addition, `devicePermissionRoleArn` (@required, `AmazonResourceName` len 20..2048, documented as "Gets the device permission ARN") is accepted as any 20+ character string — e.g. "not-an-arn-at-all-xxxx" — and likewise the device entries' `thingArn`/`certificateArn`/`deviceRoleArn` are not checked to be ARNs of the service that member names. The crate already owns that check (`crate::state::is_arn_of_service`, used by `src/ops/get_endpoint.rs:191-204` to refuse exactly these malformed values with ValidationException), so the same input is refused on GetEndpoint and accepted here.

Expected: a configuration carrying a device entry with none of the three ARNs, or a `devicePermissionRoleArn` that is not an ARN, is refused with the modelled `ValidationException` (HTTP 400) and nothing is stored.
Actual: HTTP 200, the definition is created and the invalid configuration reads back verbatim.

## Evidence

Spec: `smithy_query(query="shape", shape="SuiteDefinitionConfiguration,DeviceUnderTest")` — `DeviceUnderTest` doc "A thing ARN, certificate ARN or device role ARN is required" with all members optional; `devicePermissionRoleArn` is @required with length 20..2048 and documented as an ARN.
Handler: `services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/src/ops/create_suite_definition.rs:156-241` validates only `root_group_violation(&config)` and then stores the configuration as given; `src/ops/update_suite_definition.rs:168-215` does the same for the update path.
Tests: `tests/all/suite_lifecycle.rs:690-747` covers the rootGroup prose rule only; the configuration tables (`definition_rows`, `tests/all/suite_lifecycle.rs:200-288`) always supply `devices: [{"thingArn": <arn>}]` and a valid ROLE_ARN. No test asserts a refusal for an empty device entry or a non-ARN devicePermissionRoleArn, so this is net-new.

## Steps to Reproduce

1. POST /suiteDefinitions with body {"suiteDefinitionConfiguration": {"suiteDefinitionName": "devices-check", "rootGroup": "g", "devicePermissionRoleArn": "arn:aws:iam::000000000000:role/DeviceAdvisorRole", "devices": [{}]}}.
2. AWS: HTTP 400 ValidationException (a device requires a thing ARN, certificate ARN or device role ARN). Emulator: HTTP 200 with a minted suiteDefinitionId.
3. GET /suiteDefinitions/{suiteDefinitionId} — the emulator reads back `devices: [{}]`, and GET /suiteDefinitions reports it in `defaultDevices`.
4. Variant: POST /suiteDefinitions with devicePermissionRoleArn "not-an-arn-at-all-xxxx" (>= 20 chars) — AWS refuses with ValidationException, the emulator creates the definition; the same string in GetEndpoint's deviceRoleArn query is refused 400 by this emulator, showing the inconsistency.
5. Variant: PATCH /suiteDefinitions/{suiteDefinitionId} with the same invalid configuration — the update path accepts it too.
