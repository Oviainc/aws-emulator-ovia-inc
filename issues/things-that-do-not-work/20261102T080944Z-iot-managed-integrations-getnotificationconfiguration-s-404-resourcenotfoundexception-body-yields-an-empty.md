Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot-managed-integrations.notification-configuration-lifecycle
Issue-Shape-Id: com.amazonaws.iotmanagedintegrations#IotManagedIntegrations
Issue-Key: iot-managed-integrations.notification-configuration-lifecycle|run#7|AssertionError|assert not_created_code_msg is None, not_created_code_msg
Issue-Commit: 120b9066736a9aae86613aec18d737c516cfde8b

# GetNotificationConfiguration's 404 ResourceNotFoundException body yields an empty-string Message where AWS yields no message at all

> _Note: this issue was identified against the code base at commit `120b9066736a9aae86613aec18d737c516cfde8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetNotificationConfiguration

## Description
One net-new root cause in the iot-managed-integrations emulator, surfaced by the behavioral suite. This bundle has 1 cause; it must be fixed before the issue is closed.

Smithy model used: services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/smithy-model/com-amazonaws-iotmanagedintegrations-iotmanagedintegrations.json

--------------------------------------------------------------------
Cause 1 of 1: the ResourceNotFoundException a GetNotificationConfiguration read answers for an event type that holds no configuration makes a client read an EMPTY-STRING message where AWS makes it read NO message.

AWS: Smithy: GetNotificationConfiguration, GET /notification-configurations/{EventType}, success code 200; input EventType is an @required @httpLabel enum; modelled errors include ResourceNotFoundException (errorKind client, httpError 404). On ResourceNotFoundException every member is OPTIONAL — Message (ErrorMessage), ResourceId, ResourceType (smithy_query query="shape" shape="ResourceNotFoundException"), so the model states nothing about which of them a given surface sends: recorded behavior only. The recording for this surface is that the refusal is HTTP 404 with error code ResourceNotFoundException and NO readable message — a botocore client sees response["Error"]["Code"] == "ResourceNotFoundException" and response["Error"].get("Message") is None. The status and the error code are the fixed part of this error class; there is no fixed message text and no variable part, and the ABSENCE of a readable message is itself the requirement. Hypothesis (neither the model nor the recording states the mechanism): botocore's rest-json error parser defaults Error.Message to the empty string when the body carries no message member, and only reports None when the body carries the member with a JSON null, so AWS's recorded body must carry the message member explicitly as null (`{"message": null}`) rather than omit it.

Emulator: status and error code are correct (HTTP 404, ResourceNotFoundException), but the body makes the client read Message as the EMPTY STRING "" instead of None. The handler builds the refusal with crate::state::bare_resource_not_found(), i.e. the shape with message/resource_id/resource_type all None, and the service's rest-json error serializer then omits the member entirely (or writes it as ""), which is the one body shape botocore renders as "" rather than None. Note this is NOT the already-fixed defect 20261029T171839Z-iot-managed-integrations-resourcenotfoundexception-bodies-carry-resourceid-resourcetype-and-message-that.md (an invented sentence plus ResourceId/ResourceType): no invented text and no extra members are present any more. The surviving divergence is only in how an ABSENT message is put on the wire, so a bare struct with every member None does not reproduce AWS here. The sibling read GetDestination (src/ops/get_destination.rs:170-177) builds its 404 through the same helper, so the same wire shape is produced there.

Why: a client that branches on whether the refusal carries a message — `if err.response["Error"].get("Message") is None:` for the no-detail path, or `if msg:` to decide whether to surface text — takes the opposite branch against the emulator, because "" is a present-but-empty value where AWS gives none at all. Error-handling code and conformance assertions keyed on the absence of Message pass against AWS and fail against the emulator even though the status and code match.

Fix: the service's rest-json error-body serialization for ResourceNotFoundException on the notification-configuration read path — the construction at src/ops/get_notification_configuration.rs:174-183 together with crate::state::bare_resource_not_found() (src/state.rs, the `bare_resource_not_found` helper) and the serializer that renders its None members. The message member must reach the client as a JSON null rather than as an omitted key or an empty string, so that a client observes no message. Apply the same rendering to every surface that answers this bare 404 (the destination reads/mutations use the same helper), not just this one operation.

Traces: iot-managed-integrations.notification-configuration-lifecycle (the check's GetNotificationConfiguration probe immediately after the rejected CreateNotificationConfiguration, before any configuration exists).

## Evidence
Suite: simparity-behavioral-aws-v1, 11/21 passed; iot-managed-integrations 11/21 (52%), 10 failed. Of those failures, four are marked ALREADY FILED (blocked by unimplemented StartAccountAssociationRefresh, DeleteCloudConnector, ListEventLogConfigurations, CreateProvisioningProfile) and four are marked OWNED by 20261029T193935Z-iot-managed-integrations-createaccountassociation-skips-connector-destination-resolution.md; all eight are excluded. iot-managed-integrations.notification-configuration-lifecycle is the only surviving failure and is NOT owned: that issue's recorded key for this check is `run#4|AssertionError|assert missing_code_msg == "Destination doesn't exist"`, an assertion that now PASSES, and the check has advanced to a new failure point.

[mismatch] iot-managed-integrations.notification-configuration-lifecycle (check file line 59):
  File "check_iot_managed_integrations_notification_configuration_lifecycle.py", line 59, in run
    assert not_created_code_msg is None, not_created_code_msg
  AssertionError

What the check did before that line and what passed:
  - CreateNotificationConfiguration(EventType='DEVICE_STATE', DestinationName='<a name never created>') -> 400 ValidationException with Message "Destination doesn't exist": code AND message both matched (lines 55-56 passed).
  - GetNotificationConfiguration(EventType='DEVICE_STATE') with nothing created -> the error Code matched: `assert not_created_code == "ResourceNotFoundException"` passed (line 58).
  - Line 59 then failed on the Message: expected `None` (no message readable by the client), actual a value whose str() is EMPTY — the AssertionError carries the offending value as its only argument and the reported detail is bare `AssertionError` with nothing after it, i.e. the emulator's Error.Message is the empty string "".

So: expected HTTP 404 / Code 'ResourceNotFoundException' / Message absent (client reads None); actual HTTP 404 / Code 'ResourceNotFoundException' / Message '' (client reads an empty string). Status and code match; only the presence of the Message member diverges.

Code under test: services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/src/ops/get_notification_configuration.rs:174-183 raises ResourceNotFoundException via crate::state::bare_resource_not_found(); services/.../src/state.rs defines that helper as the shape with message/resource_id/resource_type all None (around the `bare_resource_not_found` definition, documented as "the status and the wire error code alone, and an otherwise empty body"). services/.../src/ops/get_destination.rs:170-177 uses the same helper, and the destination check asserts the same `Message is None` contract.

## Steps to Reproduce
1. Boot the emulator and point a botocore client at the iot-managed-integrations endpoint (the check pins the client to a supported region, e.g. eu-west-1).
2. Make sure no notification configuration exists for EventType 'DEVICE_STATE' (fresh state, or DeleteNotificationConfiguration first).
3. Call GetNotificationConfiguration(EventType='DEVICE_STATE') — i.e. GET /notification-configurations/DEVICE_STATE.
4. Catch the ClientError and inspect it: e.response['Error']['Code'] is 'ResourceNotFoundException' (correct) but e.response['Error'].get('Message') is '' where AWS makes it None. Equivalently, inspect the raw 404 body: it must put the message member on the wire as JSON null so a client reads no message, instead of omitting the key / writing an empty string.
5. Full check: replay iot-managed-integrations.notification-configuration-lifecycle; line 59 (`assert not_created_code_msg is None`) must pass, as must the identical assertion on the post-delete read and the post-delete UpdateNotificationConfiguration later in the same check.
