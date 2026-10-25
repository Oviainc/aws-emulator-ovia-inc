Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 30fa86cd244b801d1c6064a0a321e2bf32f37967

# Validate that StartSuiteRun's primaryDevice names a thing or certificate ARN

_Filed: 2026-10-24T13:49:49Z_
_Operation: StartSuiteRun | Class: validation_

> _Note: this issue was identified against the code base at commit `30fa86cd244b801d1c6064a0a321e2bf32f37967`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StartSuiteRun accepts a `suiteRunConfiguration.primaryDevice` that names no device at all (e.g. `{"primaryDevice": {}}`) and starts a real, persisted suite run (HTTP 200, PENDING run stored, run id/ARN/endpoint minted). The Smithy model states the requirement in prose on `SuiteRunConfiguration$primaryDevice` — "Sets the primary device for the test suite run. This requires a thing ARN or a certificate ARN" — and on the `DeviceUnderTest` shape itself — "Information of a test device. A thing ARN, certificate ARN or device role ARN is required". Real Device Advisor cannot schedule a run against no device, so it refuses the request with the modelled `ValidationException` (HTTP 400) instead of creating a run.

Expected: StartSuiteRun with a `primaryDevice` carrying neither `thingArn` nor `certificateArn` is refused 400 `ValidationException` and nothing is stored (no new run visible to GetSuiteRun/ListSuiteRuns, and no in-progress run blocking the next start with ConflictException).
Actual: the handler never inspects `primaryDevice`; it stores the configuration verbatim and answers 200.

Secondarily, the values are not checked to BE ARNs of the expected services: `primaryDevice.thingArn` / `certificateArn` accept any 20..2048-character string (e.g. `"my-favourite-thing-name12345"`). The same crate already applies exactly this rule for the equivalent members of GetEndpoint (`crate::state::is_arn_of_service`, `src/ops/get_endpoint.rs:191-204`), so the rule exists in the crate but is not applied to the run configuration.

## Evidence

Spec: `smithy_query(query="shape", shape="SuiteRunConfiguration")` — `primaryDevice` is @required with doc "This requires a thing ARN or a certificate ARN"; `DeviceUnderTest` doc "A thing ARN, certificate ARN or device role ARN is required" (all three of its members are optional, so the framework's @required/length validator cannot catch an empty object).
Handler: `services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/src/ops/start_suite_run.rs:158-258` — it validates the definition id, the version and the one-run-at-a-time rule, then builds `crate::state::SuiteRun { configuration: input.suite_run_configuration, .. }` with no inspection of `primaryDevice`.
Tests: no coverage. `grep -n 'primaryDevice' tests/all` shows every case passing `{"primaryDevice": {"thingArn": <arn>}}`; `tests/all/suite_lifecycle.rs:1132-1199` covers only unknown definition id / unknown version refusals. Contrast `tests/all/endpoint.rs:500-560`, which asserts 400 ValidationException for non-ARN `thingArn`/`certificateArn` on GetEndpoint — the same values are accepted here.

## Steps to Reproduce

1. CreateSuiteDefinition with a valid configuration (suiteDefinitionName, rootGroup "g", devicePermissionRoleArn <role ARN>); keep suiteDefinitionId.
2. POST /suiteDefinitions/{suiteDefinitionId}/suiteRuns with body {"suiteRunConfiguration": {"primaryDevice": {}}}.
3. AWS: HTTP 400 ValidationException (primaryDevice requires a thing ARN or a certificate ARN). Emulator: HTTP 200 with a minted suiteRunId/suiteRunArn.
4. GET /suiteRuns — the emulator lists the bogus run; a following StartSuiteRun on the same definition is now refused with ConflictException because the phantom run is in progress.
5. Variant: POST the same path with {"suiteRunConfiguration": {"primaryDevice": {"thingArn": "not-an-arn-but-long-enough"}}} — AWS refuses with ValidationException, the emulator accepts it (while GetEndpoint refuses the same value).
