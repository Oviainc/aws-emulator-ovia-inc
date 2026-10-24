Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: aeaddd52642475574d853bf8e089296dd7f2f86c

# GetWorkflow always returns WorkflowDefinition and ScheduleConfiguration as null

_Filed: 2026-10-24T00:24:00Z_
_Operation: GetWorkflow | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `aeaddd52642475574d853bf8e089296dd7f2f86c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: GetWorkflow reports the definition that was snapshotted for the version being read — `WorkflowDefinition` is documented as 'The workflow definition content', and `ScheduleConfiguration` describes the schedule the definition declares. Because the service snapshots the definition file at creation time, a GetWorkflow after the S3 object is overwritten still reports the original content.

Actual: the handler hardcodes both members to None (`schedule_configuration: None`, `workflow_definition: None`) with the comment 'the definition body itself is never fetched from S3'. CreateWorkflow stores only the S3 pointer (bucket/key/versionId) in `WorkflowSnapshot`, never the object body, so no code path can ever populate them. A client reading back a workflow sees DefinitionS3Location but no definition content and no schedule, for every workflow and every version.

## Evidence

Smithy model: GetWorkflowResponse.WorkflowDefinition doc 'The workflow definition content'; GetWorkflowResponse.ScheduleConfiguration is a modeled output member of the same read. Handler: src/ops/get_workflow.rs lines ~274-280 (`schedule_configuration: None`, `workflow_definition: None`). State: src/state.rs `WorkflowSnapshot` holds `definition_s3_location` only — no definition body field. Behavioral trace already recorded for this service: `mwaa-serverless.workflow-definition-snapshotted-at-creation-unaffected-by-s3-overwrite`. Test coverage: no test in tests/all mentions WorkflowDefinition or ScheduleConfiguration (grep returns no matches).

## Steps to Reproduce

1. PutObject a DAG YAML to an S3 bucket in the emulator.
2. CreateWorkflow pointing DefinitionS3Location at that object.
3. Call GetWorkflow with the returned WorkflowArn.
4. Observe WorkflowDefinition and ScheduleConfiguration absent/null in the response, where the real service returns the snapshotted definition content (and the schedule it declares) even after the S3 object is overwritten.
