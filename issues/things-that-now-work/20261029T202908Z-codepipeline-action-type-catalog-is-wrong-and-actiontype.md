Issue-Source: trace-sweep-divergence
Issue-Trace: codepipeline-catalog-list-action-types
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codepipeline-catalog-list-action-types codepipeline-custom-action-job-worker-api codepipeline-custom-action-type-lifecycle
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline|ListActionTypes|2xx->2xx|$.actionTypes[*]
Issue-Key: codepipeline|CreateCustomActionType|2xx->2xx|$.actionType.actionConfigurationProperties
Issue-Key: codepipeline|CreateCustomActionType|2xx->2xx|$.actionType.id.__type
Issue-Key: codepipeline|CreateCustomActionType|2xx->2xx|$.actionType.settings
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# CodePipeline action-type catalog is wrong and ActionType responses omit modeled members

> _Note: this issue was identified against the code base at commit `6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListActionTypes

## Description
ListActionTypes serves a built-in catalog that does not match AWS's, and every ActionType the service returns is serialized without its modeled settings/actionConfigurationProperties members.

Cause 1 of 4 — the AWS-owned catalog is wrong in content, ordering and per-entry detail. AWS: ListActionTypes with actionOwnerFilter=AWS returns 200 with actionTypes (Smithy: ListActionTypesOutput.actionTypes -> ActionTypeList, required) listing the AWS-provided action types, each as an ActionType carrying id (with the recorded `__type`:'ActionTypeId'), inputArtifactDetails/outputArtifactDetails with the provider's real min/max counts, settings (e.g. entityUrlTemplate) and actionConfigurationProperties (name/key/required/secret/queryable/description per property) — 28 entries in the recording, beginning with Source/AWS/S3. Emulator: 200 but a different, shorter list that starts with Approval/AWS/Manual and whose entries carry only id and zeroed artifact-detail counts, with no settings and no actionConfigurationProperties. Why: a console or client that discovers available providers and their required configuration properties from this catalog cannot build a valid action declaration for any AWS provider. Fix: the built-in action-type catalog seed plus the ActionType serializer in the codepipeline service state. Traces: codepipeline-catalog-list-action-types#task#1.

Cause 2 of 4 — the ThirdParty-owned catalog is likewise wrong. AWS: actionOwnerFilter=ThirdParty returns the third-party types (5 entries in the recording, first Test/ThirdParty/GhostInspector) with their settings (entityUrlTemplate, executionUrlTemplate, thirdPartyConfigurationUrl) and actionConfigurationProperties, including secret:true for token properties. Emulator: a different list starting with Source/ThirdParty/GitHub and no settings/actionConfigurationProperties on any entry. Fix: same catalog seed, ThirdParty partition. Traces: codepipeline-catalog-list-action-types#task#2.

Cause 3 of 4 — CreateCustomActionType's echoed actionType omits modeled members. AWS: 200 with actionType.id carrying `__type`:'ActionTypeId' (recorded behavior for this JSON 1.1 service), actionConfigurationProperties serialized as [] and settings serialized as {} when the request declared neither (Smithy: ActionType.actionConfigurationProperties -> ActionConfigurationPropertyList length 0..10; ActionType.settings -> ActionTypeSettings). Emulator: all three absent. Why: a worker that reads actionConfigurationProperties to learn which job-data keys to expect gets null instead of an empty list. Fix: the ActionType serializer on the CreateCustomActionType response path. Traces: codepipeline-custom-action-job-worker-api#task#1 (2 affected steps, incl. codepipeline-custom-action-type-lifecycle#task#1).

Cause 4 of 4 — the same omissions when the custom type is read back through ListActionTypes. AWS: the custom entry carries actionConfigurationProperties [], id.__type 'ActionTypeId' and settings {} alongside its artifact details. Emulator: only id and the artifact details. Fix: same serializer, on the ListActionTypes path. Traces: codepipeline-custom-action-type-lifecycle#task#2.

## Evidence
codepipeline-catalog-list-action-types#task#1 (ListActionTypes owner=AWS, 200->200): '$.actionTypes[0]..[27]' all diverge; expected actionTypes[0] = Source/AWS/S3 with actionConfigurationProperties S3Bucket/S3ObjectKey/PollForSourceChanges/AllowOverrideForS3ObjectKey, outputArtifactDetails min=1 max=1 and settings.entityUrlTemplate, got Approval/AWS/Manual with all artifact counts 0 and no settings/actionConfigurationProperties.
codepipeline-catalog-list-action-types#task#2 (ListActionTypes owner=ThirdParty, 200->200): '$.actionTypes[0]..[4]' diverge; expected Test/ThirdParty/GhostInspector with ClientId/ClientToken(secret)/TestId/ExtraParams and three settings templates, got Source/ThirdParty/GitHub with no settings/properties.
codepipeline-custom-action-job-worker-api#task#1 (CreateCustomActionType, 200->200): '$.actionType.actionConfigurationProperties: missing (expected [], got None)', '$.actionType.id.__type: missing (expected ActionTypeId)', '$.actionType.settings: missing (expected {}, got None)'.
codepipeline-custom-action-type-lifecycle#task#2 (ListActionTypes, 200->200): '$.actionTypes[0]: missing (expected the custom Build type with actionConfigurationProperties [], id.__type ActionTypeId and settings {})'.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus.
2. ListActionTypes with actionOwnerFilter=AWS and then ThirdParty -> compare the entries, their order, their artifact-detail counts and their settings/actionConfigurationProperties against AWS's catalog.
3. CreateCustomActionType with no settings and no configurationProperties -> the echoed actionType must carry actionConfigurationProperties [], settings {} and id.__type.
4. ListActionTypes with actionOwnerFilter=Custom -> the custom entry must carry the same members.
5. Re-replay codepipeline-catalog-list-action-types#task#1/#task#2, codepipeline-custom-action-job-worker-api#task#1 and codepipeline-custom-action-type-lifecycle#task#2.
