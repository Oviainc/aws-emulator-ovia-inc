Issue-Source: trace-sweep-divergence
Issue-Trace: shield-protection-group-error-paths
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: shield-protection-group-error-paths
Issue-Shape-Id: com.amazonaws.shield#AWSShield_20160616
Issue-Key: shield|CreateProtectionGroup|4xx->4xx|$.fields
Issue-Key: shield|CreateProtectionGroup|4xx->4xx|$.reason
Issue-Commit: 14fcb532a6243119e1963a7bcc0017c1001c0e15

# shield CreateProtectionGroup InvalidParameterException omits reason and fields

> _Note: this issue was identified against the code base at commit `14fcb532a6243119e1963a7bcc0017c1001c0e15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProtectionGroup

## Description
1 cause.

Cause 1 of 1: CreateProtectionGroup's Pattern/ResourceType/Members field-combination rejection carries only a message; the structured detail members AWS populates on InvalidParameterException are absent. (fixture shield-protection-group-error-paths, step task#2)
AWS: Smithy models InvalidParameterException (HTTP 400) with members message, reason (ValidationExceptionReason enum, values FIELD_VALIDATION_FAILED | OTHER) and fields (ValidationExceptionFieldList of ValidationExceptionField {name, message}, both required), documented as "Exception that indicates that the parameters passed to the API are invalid. If available, this exception includes details in additional properties." The ResourceType member is documented "You must set this when you set Pattern to BY_RESOURCE_TYPE and you must not set it for any other Pattern setting" (and Members likewise for ARBITRARY). Recorded response for a request that sets ResourceType with a Pattern that forbids it: HTTP 400, code InvalidParameterException, reason = "FIELD_VALIDATION_FAILED", fields = one entry whose message is the fixed text "This field should not be defined for this pattern" and whose name is the offending input member in its modeled spelling (here ResourceType, taken from the request). So the per-field diagnosis travels in reason/fields, not only in the message.
Emulator: the same HTTP 400, the same error code and a matching top-level message, but reason and fields are omitted from the body entirely (serialized as absent), so the response carries no indication of which member was at fault or why.
Why: a client that reads the exception's additional properties — `err.response['Error']`/modelled `fields` and `reason` in the SDKs, or a UI that highlights the offending input — sees nothing to act on and cannot localize the bad member; code switching on reason == FIELD_VALIDATION_FAILED to distinguish a field-combination refusal from any other InvalidParameterException never matches.
Fix: the CreateProtectionGroup handler's InvalidParameterException construction — the field-combination validator must report which member it rejected and populate reason = FIELD_VALIDATION_FAILED and a fields entry {name: <offending member>, message: "This field should not be defined for this pattern"} instead of passing reason: None / fields: None.
Traces: shield-protection-group-error-paths#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, bootstrap_signal=false), service shield:

shield-protection-group-error-paths#task#2 CreateProtectionGroup — expected 400 / actual 400, error code matched; $.reason: missing (expected 'FIELD_VALIDATION_FAILED', got None); $.fields: missing (expected [{'name': 'ResourceType', 'message': 'This field should not be defined for this pattern'}], got None).

Source in the checkout: services/service-com-amazonaws-shield-awsshield-20160616/src/ops/create_protection_group.rs lines 197-210 — the handler calls guard.protection_group_fields_accepted(pattern, resource_type, members), and on rejection builds InvalidParameterException { message: Some(crate::state::PROTECTION_GROUP_FIELDS_MESSAGE), reason: None, fields: None }, i.e. the validator returns only a boolean and the detail members are hard-coded to None.

Smithy (services/service-com-amazonaws-shield-awsshield-20160616/smithy-model/com-amazonaws-shield-awsshield-20160616.json): com.amazonaws.shield#InvalidParameterException members = message, reason -> ValidationExceptionReason (FIELD_VALIDATION_FAILED | OTHER, "Additional information about the exception"), fields -> ValidationExceptionFieldList ("Fields that caused the exception"); ValidationExceptionField = { name (required, "The name of the parameter that failed validation"), message (required, "The message describing why the parameter failed validation") }. CreateProtectionGroup declares InvalidParameterException at HTTP 400.

## Steps to Reproduce
Boot the emulator and point a shield client at it with an active Shield Advanced subscription in state, then submit a protection group whose Pattern forbids ResourceType:

1. create_protection_group(ProtectionGroupId='<id>', Aggregation='SUM', Pattern='ALL', ResourceType='ELASTIC_IP_ALLOCATION') -> emulator: 400 InvalidParameterException with only a message; AWS: 400 InvalidParameterException with reason='FIELD_VALIDATION_FAILED' and fields=[{'name': 'ResourceType', 'message': 'This field should not be defined for this pattern'}].
2. Inspect the raw error body (or the SDK exception's reason/fields) — both members are absent in the emulator's response.

Re-replay shield-protection-group-error-paths#task#2 to confirm the fix.
