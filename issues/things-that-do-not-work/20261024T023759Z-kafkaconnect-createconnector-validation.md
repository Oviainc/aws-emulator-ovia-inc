Issue-Severity: HIGH
Issue-Gap-Class: validation
Issue-Commit: 047af81d647cd24bcec0ef3f34fd68e55311879a

# Enforce CreateConnector capacity constraints and validate the referenced workerConfiguration

_Filed: 2026-10-24T02:37:59Z_
_Operation: CreateConnector | Class: validation_

> _Note: this issue was identified against the code base at commit `047af81d647cd24bcec0ef3f34fd68e55311879a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateConnector accepts capacity values and worker-configuration references that the real service refuses with BadRequestException / NotFoundException.

Expected (real service):
1. `capacity.autoScaling.mcuCount` / `capacity.provisionedCapacity.mcuCount` — "The valid values are 1,2,4,8" (member documentation). A value of 3, 5, 6 or 7 is inside the modelled range 1..8 but is NOT a valid MCU count and is rejected with HTTP 400 BadRequestException.
2. `capacity.autoScaling` — `minWorkerCount` must not exceed `maxWorkerCount`, and `maxAutoscalingTaskCount` "Must be at least equal to maxWorkerCount" (member documentation). Violations are HTTP 400 BadRequestException.
3. `workerConfiguration.workerConfigurationArn` / `workerConfiguration.revision` must name a worker configuration that exists in this account (and the revision it carries). A connector cannot be created against a worker configuration ARN the service never minted; the real service refuses it (404 NotFoundException / 400 BadRequestException) rather than storing the dangling reference.

Actual (emulator): the handler validates only `plugins.len() == 1`, the exactly-one-of capacity form, `serviceExecutionRoleArn` ARN syntax, duplicate connector name, the quota, and the custom-plugin reference. No MCU-count set check, no autoScaling worker-count relation check, and no lookup of `input.worker_configuration` against `state.worker_configurations` — the member is stored verbatim and echoed back by DescribeConnector / ListConnectors (`worker_configuration_description()`), so the emulator answers 200 and persists a connector that references a nonexistent worker configuration.

## Evidence

Spec: Smithy model `AutoScaling$mcuCount` / `ProvisionedCapacity$mcuCount` doc "The number of microcontroller units (MCUs) allocated to each connector worker. The valid values are 1,2,4,8." (range 1..8 only, so the model cannot express the set); `AutoScaling$maxAutoscalingTaskCount` doc "Must be at least equal to maxWorkerCount."; `WorkerConfiguration$workerConfigurationArn` (required) "The Amazon Resource Name (ARN) of the worker configuration."; CreateConnector declares BadRequestException (400) and NotFoundException (404).
Handler: services/service-com-amazonaws-kafkaconnect-kafkaconnect/src/ops/create_connector.rs lines 206-316 — the complete validation set; `input.worker_configuration` is only moved into `ConnectorRecord` (line 339) and never checked against `state.worker_configurations`.
Tests: no test exercises an mcuCount outside {1,2,4,8}, the worker-count relations, or a connector referencing a worker configuration — `grep workerConfigurationArn tests/` shows no connector request carrying the member, and tests/all/validation_messages.rs only pins the existing refusals.

## Steps to Reproduce

1. Create a custom plugin and wait for it to become ACTIVE.
2. POST /v1/connectors with `capacity: {"provisionedCapacity": {"mcuCount": 3, "workerCount": 2}}` — the emulator answers 200 CREATING; the real service answers 400 BadRequestException naming mcuCount.
3. POST /v1/connectors with `capacity: {"autoScaling": {"maxWorkerCount": 2, "mcuCount": 1, "minWorkerCount": 8}}` — the emulator answers 200; the real service refuses minWorkerCount > maxWorkerCount with 400.
4. POST /v1/connectors with `workerConfiguration: {"workerConfigurationArn": "arn:aws:kafkaconnect:us-east-1:000000000000:worker-configuration/never-created/00000000-0000-4000-8000-000000000000", "revision": 1}` — the emulator answers 200 and a later GET /v1/connectors/{arn} reports the dangling reference; the real service refuses the create.
