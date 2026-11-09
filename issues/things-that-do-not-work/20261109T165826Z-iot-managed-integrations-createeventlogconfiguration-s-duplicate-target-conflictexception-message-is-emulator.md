Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot-managed-integrations.event-log-configuration-lifecycle-and-duplicate-conflict
Issue-Shape-Id: com.amazonaws.iotmanagedintegrations#IotManagedIntegrations
Issue-Key: iot-managed-integrations.event-log-configuration-lifecycle-and-duplicate-conflict|run#14|AssertionError|assert dup_code_msg == 'Log configuration already exists.', dup_code_msg
Issue-Commit: c33038d6637516191c9271b81207503148b3a194

# CreateEventLogConfiguration's duplicate-target ConflictException message is emulator-authored and leaks the existing configuration id

> _Note: this issue was identified against the code base at commit `c33038d6637516191c9271b81207503148b3a194`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventLogConfiguration

## Description
One net-new root cause in the iot-managed-integrations emulator, surfaced by the behavioral suite. This bundle has 1 cause; it must be fixed before the issue is closed.

Smithy model used: services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/smithy-model/com-amazonaws-iotmanagedintegrations-iotmanagedintegrations.json

--------------------------------------------------------------------
Cause 1 of 1: a second CreateEventLogConfiguration for a log target that already has a configuration is correctly refused with ConflictException, but the body carries a message AWS never words, and it interpolates the existing configuration's identifier.

AWS: Smithy: CreateEventLogConfiguration, POST /event-log-configurations, success code 201; input ResourceType @required (pattern ^[*]$|^(managed-thing|credential-locker|provisioning-profile|ota-task|account-association)$), ResourceId optional (@length 1..200, pattern ^[a-zA-Z0-9+*]*$), EventLogLevel @required (enum LogLevel: DEBUG|ERROR|INFO|WARN); modelled errors include ConflictException (httpError 409), whose only member is Message (ErrorMessage), documented "There is a conflict with the request." The model states no wording for the duplicate-target rule, so: recorded behavior only — when the account already holds an event log configuration for the requested ResourceType/ResourceId target, AWS answers HTTP 409 ConflictException with Message 'Log configuration already exists.' That whole sentence is FIXED text for this error class: it interpolates NOTHING — no ResourceType, no ResourceId, no existing configuration id, no requested log level. The surrounding recorded behavior the check also asserts and that the emulator satisfies: the duplicate adds no second record (ListEventLogConfigurations still shows exactly the one id for ResourceType '*').

Emulator: correct status and error code (409 ConflictException), but Message is the emulator's own sentence "an event log configuration already exists for this resource type and resource, with identifier '<existing configuration id>'" — a different sentence that additionally embeds the id of the pre-existing configuration. ('<existing configuration id>' is the variable the emulator wrongly inserts; the concrete value seen in one run is not part of any requirement.)

Why: an error message is part of the error contract. A client that matches, logs or displays AWS's wording for this refusal diverges — an exact-text comparison against AWS fails, and a caller that keys remediation off the message sees a sentence (and a resource identifier) AWS never emits, so code written against AWS's bare 'Log configuration already exists.' cannot recognize the emulator's conflict.

Fix: CreateEventLogConfiguration's duplicate-target error construction — the ConflictException built in the handler's event_log_configuration_for_target branch (services/service-com-amazonaws-iotmanagedintegrations-iotmanagedintegrations/src/ops/create_event_log_configuration.rs, the branch around lines 222-239, which formats the message with the existing record's id). Emit AWS's fixed text for this refusal and interpolate nothing. Note the same check asserts further post-delete behavior that this run never reached and which is therefore NOT claimed here: GetEventLogConfiguration after delete answering ResourceNotFoundException with Message "Event log configuration doesn't exist", UpdateEventLogConfiguration on the dead id answering ResourceNotFoundException with NO Message member, and a restore create minting a fresh id.

Related but NOT covering this: open issue 20261029T193935Z-iot-managed-integrations-createaccountassociation-skips-connector-destination-resolution.md bundles five other refusal-body divergences of this service; none of its causes is CreateEventLogConfiguration's conflict message, and its replay keys do not include this check.

Traces: iot-managed-integrations.event-log-configuration-lifecycle-and-duplicate-conflict (see trace_ids).

## Evidence
Suite: simparity-behavioral-aws-v1, 13/22 passed (59.1%); iot-managed-integrations 13/22 (59%), 9 failed. Eight of those failures were marked ALREADY FILED (blocked by unimplemented StartAccountAssociationRefresh, ListOtaTasks, CreateProvisioningProfile) or OWNED by an open issue and are excluded. The one below is the surviving failure.

Cause 1 — [mismatch] iot-managed-integrations.event-log-configuration-lifecycle-and-duplicate-conflict (check line 37):
  assert dup_code_msg == 'Log configuration already exists.', dup_code_msg
  AssertionError: an event log configuration already exists for this resource type and resource, with identifier '<existing configuration id>'
  Expected: HTTP 409, Code 'ConflictException', Message 'Log configuration already exists.'
  Actual:   HTTP 409, Code 'ConflictException' (matched), Message "an event log configuration already exists for this resource type and resource, with identifier '<existing configuration id>'".
  Fixed text in AWS's message: the entire sentence 'Log configuration already exists.'; variable parts: none. The emulator's extra variable part is the pre-existing configuration's id.
  Positive controls in the same check PASSED before the failing assertion: CreateEventLogConfiguration(ResourceType='*', EventLogLevel='INFO') returned an Id, GetEventLogConfiguration round-tripped ResourceType '*'/EventLogLevel 'INFO', UpdateEventLogConfiguration to DEBUG was visible on the next Get, and the duplicate create did create no second record — so the divergence is isolated to the ConflictException message text.
  Emulator source confirming the authored text: src/ops/create_event_log_configuration.rs formats ConflictException { message: Some(format!("an event log configuration already exists for this resource type and resource, with identifier '{existing_id}'")) }.

## Steps to Reproduce
Boot the emulator and replay the behavioral check in trace_ids (the check pins the client to a supported region, eu-west-1).

1. POST /event-log-configurations with ResourceType='*' and EventLogLevel='INFO'; keep the returned Id.
2. POST /event-log-configurations again with the SAME target (ResourceType='*', no ResourceId) and any level, e.g. EventLogLevel='WARN', and no ClientToken.
3. The call is refused HTTP 409 ConflictException. Compare the body's Message with AWS's 'Log configuration already exists.' — the emulator instead returns "an event log configuration already exists for this resource type and resource, with identifier '<existing configuration id>'".
4. Confirm the rest still holds (it does today): ListEventLogConfigurations across all pages shows exactly one configuration with ResourceType '*', the one created in step 1.
