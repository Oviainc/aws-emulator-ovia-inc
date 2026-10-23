Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot-managed-integrations.device-discovery-lifecycle-and-discovered-devices iot-managed-integrations.notification-configuration-lifecycle iot-managed-integrations.runtime-log-configuration-put-get-reset-cycle
Issue-Shape-Id: com.amazonaws.iotmanagedintegrations#IotManagedIntegrations
Issue-Key: iot-managed-integrations.device-discovery-lifecycle-and-discovered-devices|run#3|AssertionError|assert missing_code == "ResourceNotFoundException", missing_code
Issue-Key: iot-managed-integrations.notification-configuration-lifecycle|run#3|AssertionError|assert missing_code == "ValidationException", missing_code
Issue-Key: iot-managed-integrations.runtime-log-configuration-put-get-reset-cycle|run#3|AssertionError|assert missing_code == "ResourceNotFoundException", (
Issue-Commit: 001920e7906d433f041aca694d32f13b44f642fa

# Three iot-managed-integrations operations answer 501 NotImplemented instead of their modeled responses (ListDiscoveredDevices, CreateNotificationConfiguration, PutRuntimeLogConfiguration)

> _Note: this issue was identified against the code base at commit `001920e7906d433f041aca694d32f13b44f642fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDiscoveredDevices

## Description
Three distinct iot-managed-integrations operations are still scaffold stubs that answer HTTP 501 with error code `NotImplemented`. They are NOT covered by the auto-filed stub issues this suite reported (those name CreateCloudConnector, CreateConnectorDestination, ListCredentialLockers, GetDefaultEncryptionConfiguration, CreateEventLogConfiguration, GetHubConfiguration, ListAccountAssociations, GetManagedThingCapabilities, TagResource, CreateOtaTaskConfiguration, RegisterCustomEndpoint), and the two open iot-managed-integrations issues (`UpdateManagedThing 204 responses omit the Content-Length: 0 header`, `ValidationException enum value-set order reversed from AWS in validation messages`) are about unrelated response details and do not cover them either. There are 3 causes in this issue; all 3 must be closed.

Cause 1 of 3: ListDiscoveredDevices never answers, so no not-found classification exists.
AWS: Smithy (services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/smithy-model/com-amazonaws-iotmanagedintegrations-iotmanagedintegrations.json) models `ListDiscoveredDevices` as GET /device-discoveries/{Identifier}/devices, success 200, with modeled errors ResourceNotFoundException (HTTP 404), ValidationException (400), AccessDeniedException (403), UnauthorizedException (401), ThrottlingException (429), InternalServerException (500), ServiceUnavailableException (503). `Identifier` targets DeviceDiscoveryId (len 1..200, pattern ^[A-Za-z0-9]+$). Recorded behavior: an Identifier that satisfies that pattern but was never allocated is rejected with ResourceNotFoundException (example message `Discovery resource not found.`); a real discovery job returns 200 with Items.
Emulator: returns HTTP 501, error code `NotImplemented`, message `operation ListDiscoveredDevices is scaffolded but not implemented in this emulator`, for every request including the well-formed not-found case.
Why: a client cannot distinguish "no such discovery job" from "job exists with no devices", and botocore raises an unmodeled error the SDK's retry/exception mapping does not classify, so discovery polling loops break. The whole device-discovery surface (StartDeviceDiscovery -> ListDeviceDiscoveries -> ListDiscoveredDevices) is unusable.
Fix: the operation's handler — replace the `Scaffolded::not_implemented` return in `ListDiscoveredDevices::handle` with real behavior, resolving Identifier against discovery-job state and returning ResourceNotFoundException when it does not resolve (services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/src/ops/list_discovered_devices.rs, line 172).
Traces: iot-managed-integrations.device-discovery-lifecycle-and-discovered-devices (assert missing_code == "ResourceNotFoundException", got 'NotImplemented').

Cause 2 of 3: CreateNotificationConfiguration never answers, so the dangling-destination rejection is missing.
AWS: Smithy models `CreateNotificationConfiguration` as POST /notification-configurations with success code 201, required members EventType (enum) and DestinationName (len 1..128, pattern ^[\p{L}\p{N} ._-]+$), output EventType, and modeled errors ValidationException (HTTP 400), ConflictException (409), AccessDeniedException (403), ThrottlingException (429), InternalServerException (500). Note the model declares NO ResourceNotFoundException for this operation. Recorded behavior: a DestinationName that does not exist is rejected with ValidationException (example message `Destination doesn't exist`) and nothing is created — a subsequent GetNotificationConfiguration for that EventType reports ResourceNotFoundException; against a real destination the create round-trips and UpdateNotificationConfiguration re-points it.
Emulator: returns HTTP 501, error code `NotImplemented`, instead of the modeled 400 ValidationException (and instead of the 201 success on the happy path).
Why: clients get an unmodeled 501 where AWS gives a 400 client error, so input-validation handling never fires and the notification-configuration lifecycle cannot be driven at all.
Fix: the operation's handler and its validation — resolve DestinationName against destination state and return the modeled ValidationException when absent, otherwise create and return 201 (services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/src/ops/create_notification_configuration.rs).
Traces: iot-managed-integrations.notification-configuration-lifecycle (assert missing_code == "ValidationException", got 'NotImplemented').

Cause 3 of 3: the runtime-log-configuration surface never answers, so neither of its two rejection classes nor its persistence exists.
AWS: Smithy models `PutRuntimeLogConfiguration` as PUT /runtime-log-configurations/{ManagedThingId} (idempotent, success 200, output Unit) with modeled errors ResourceNotFoundException (HTTP 404), ValidationException (400), AccessDeniedException (403), ThrottlingException (429), InternalServerException (500); `GetRuntimeLogConfiguration` as GET on the same URI (200, output ManagedThingId + RuntimeLogConfigurations) and `ResetRuntimeLogConfiguration` as DELETE (200), both with the same error set. Recorded behavior: a ManagedThingId that does not exist is rejected with ResourceNotFoundException and installs nothing; a ManagedThingId that DOES exist but is not a 1P device/hub is rejected with ValidationException (HTTP 400, example message `Only 1P device or hub is supported for Runtime logging`) and YET the configuration is persisted — GetRuntimeLogConfiguration, which reported ResourceNotFoundException a moment earlier, then returns exactly the submitted LogLevel and UploadLog, and ResetRuntimeLogConfiguration removes it again (Get back to ResourceNotFoundException).
Emulator: PutRuntimeLogConfiguration returns HTTP 501, error code `NotImplemented`, for the not-found case, so neither rejection class is produced and no state is written for Get/Reset to observe.
Why: two distinct causes collapse into one unmodeled 501, so a client cannot tell a missing managed thing from an unsupported device class, and the reversible persist-on-rejection behavior the put/get/reset cycle depends on is absent.
Fix: the operation's handler, its validation and its state — resolve ManagedThingId first (404 when absent), then apply the device-class check (400) while still writing the configuration into state so Get/Reset observe it (services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/src/ops/put_runtime_log_configuration.rs, plus get_runtime_log_configuration.rs and reset_runtime_log_configuration.rs).
Traces: iot-managed-integrations.runtime-log-configuration-put-get-reset-cycle (assert missing_code == "ResourceNotFoundException" ... got 'NotImplemented').

## Evidence
Suite report (simparity-behavioral-aws-v1), 1/21 passed, iot-managed-integrations 1/21 (5%).

[mismatch] iot-managed-integrations.device-discovery-lifecycle-and-discovered-devices
  check_iot_managed_integrations_device_discovery_lifecycle_and_discovered_devices.py line 29:
  assert missing_code == "ResourceNotFoundException", missing_code
  AssertionError: NotImplemented
  Expected: ResourceNotFoundException (HTTP 404), example message 'Discovery resource not found.'
  Actual:   NotImplemented (HTTP 501)

[mismatch] iot-managed-integrations.notification-configuration-lifecycle
  check_iot_managed_integrations_notification_configuration_lifecycle.py line 55:
  assert missing_code == "ValidationException", missing_code
  AssertionError: NotImplemented
  Expected: ValidationException (HTTP 400), example message 'Destination doesn't exist'
  Actual:   NotImplemented (HTTP 501)

[mismatch] iot-managed-integrations.runtime-log-configuration-put-get-reset-cycle
  check_iot_managed_integrations_runtime_log_configuration_put_get_reset_cycle.py line 43:
  AssertionError: expected ResourceNotFoundException for a managed thing id that does not exist, got 'NotImplemented'
  Expected: ResourceNotFoundException (HTTP 404)
  Actual:   NotImplemented (HTTP 501)

Source confirmation in the checkout: services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/src/ops/list_discovered_devices.rs carries the '//! SCAFFOLDED, NOT IMPLEMENTED' marker (line 4) and returns Err(crate::scaffolded::Scaffolded::not_implemented("ListDiscoveredDevices")) at line 172; put_runtime_log_configuration.rs carries the same marker at line 4.

## Steps to Reproduce
1. Boot the emulator and point a botocore iot-managed-integrations client at it.
2. ListDiscoveredDevices: call list_discovered_devices(Identifier="sim2realbogusdiscoveryid0000") — an Identifier matching ^[A-Za-z0-9]+$ that was never allocated. Expect ClientError with code ResourceNotFoundException (HTTP 404); observe NotImplemented (HTTP 501).
3. CreateNotificationConfiguration: call create_notification_configuration(EventType="DEVICE_STATE", DestinationName="sim2real-bogus-destination-name"). Expect ValidationException (HTTP 400); observe NotImplemented (HTTP 501).
4. PutRuntimeLogConfiguration: call put_runtime_log_configuration(ManagedThingId="sim2real-bogus-managed-thing-id", RuntimeLogConfigurations={"LogLevel": "INFO"}). Expect ResourceNotFoundException (HTTP 404); observe NotImplemented (HTTP 501). Then create a managed thing, re-put against its id, expect ValidationException (HTTP 400) and verify GetRuntimeLogConfiguration nonetheless returns the submitted LogLevel/UploadLog, and that ResetRuntimeLogConfiguration restores the ResourceNotFoundException.
5. Replay the checks named in trace_ids to confirm.
