Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a

# Timestamps are serialized as epoch-second numbers instead of the modeled date-time strings

_Filed: 2026-10-24T15:59:09Z_
_Operation: GetConfigurationManager | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every timestamp this service puts on the wire carries `@timestampFormat("date-time")` in the model, so restJson1 must render it as an RFC3339 string (e.g. `"CreatedAt":"2026-10-24T15:16:32.123Z"`). The emulator's member schemas are macro-derived and carry NO timestamp format, so the restJson1 codec falls back to the protocol default and writes a fractional epoch NUMBER (e.g. `"CreatedAt":1761318992.123`).

Affected members, all of them `@timestampFormat("date-time")` in the model:

* `GetConfigurationManagerOutput$CreatedAt` / `$LastModifiedAt`
* `GetConfigurationOutput$CreatedAt` / `$LastModifiedAt`
* `ConfigurationSummary$CreatedAt` (in `ListConfigurations`)
* `StatusSummary$LastUpdatedAt` (in `GetConfigurationManager`, `GetConfiguration`, `ListConfigurationManagers` and `ListConfigurations` — it is `@required`, so it appears in every status summary of every one of those responses)

Expected: a JSON string in RFC3339 date-time form for each of those members. Actual: a JSON number of seconds since the epoch, so a client (or recording comparison) that reads the documented wire form sees the wrong JSON type for every timestamp this service reports.

Other service crates in this repo work around the same vendored-codegen limitation by hand-writing the affected member schemas with `.with_timestamp_format(::aws_smithy_schema::traits::TimestampFormat::DateTime)` (e.g. `service-com-amazonaws-artifact-artifact/src/ops/types/shared_types.rs`, `service-com-amazonaws-detective-amazondetective/src/ops/types/shared_types.rs`); this crate does not, so it is inconsistent with them as well as with the model.

## Evidence

Spec grounding: `smithy_query(query="operation", operation="GetConfiguration")` reports `CreatedAt`/`LastModifiedAt` as `{"target": "Timestamp", "timestampFormat": "date-time"}`; `smithy_query(query="shape", shape="StatusSummary")` reports `LastUpdatedAt` as `{"target": "Timestamp", "timestampFormat": "date-time", "required": true}`; `ConfigurationSummary$CreatedAt` likewise. Protocol rule: restJson1 serializes a timestamp as epoch-seconds ONLY when no `@timestampFormat` is modeled.

Handler/type evidence: `src/ops/types/shared_types.rs:50-58` (`StatusSummary` built by `#[smithy_macros::smithy(shape = ...)]`), `src/ops/types/get_configuration_manager_types.rs:20-31`, `src/ops/types/get_configuration_types.rs:30-31` and `src/ops/types/list_configurations_types.rs:41` all rely on macro-derived member schemas; `grep -r with_timestamp_format services/service-com-amazonaws-ssmquicksetup-quicksetup/` returns nothing, so no member of this crate declares the date-time format the model states.

Test coverage: none asserts the wire form — on the contrary `tests/all/configuration_managers.rs:299-322` (`epoch_seconds`) deliberately accepts EITHER a number or a date-time string, and its doc comment records the current behavior: "the framework's restJson1 serializer writes the JSON protocol's default epoch-seconds number". `tests/all/configuration_definitions.rs:283-302` has the same helper.

## Steps to Reproduce

1. POST /configurationManager with {"Name":"stamps","ConfigurationDefinitions":[{"Type":"AWSQuickSetupType-SSMHostMgmt","Parameters":{"TargetOrganizationalUnits":"ou-1","TargetRegions":"us-west-2"}}]} and keep the returned ManagerArn.
2. GET /configurationManager/{ManagerArn} and inspect the raw JSON body.
3. Observe `CreatedAt`, `LastModifiedAt` and `StatusSummaries[0].LastUpdatedAt` are JSON numbers (fractional epoch seconds). Expected: RFC3339 date-time STRINGS, because those members are modeled `@timestampFormat("date-time")`.
4. POST /listConfigurations with {"ManagerArn":"<ManagerArn>"} and GET /getConfiguration/{Id} for the listed id: `ConfigurationsList[].CreatedAt`, `CreatedAt`, `LastModifiedAt` and every `StatusSummaries[].LastUpdatedAt` are numbers there too.
