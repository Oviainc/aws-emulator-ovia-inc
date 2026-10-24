Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: aeaddd52642475574d853bf8e089296dd7f2f86c

# CreateWorkflow accepts an unreachable S3 definition location and a non-assumable execution role

_Filed: 2026-10-24T00:24:00Z_
_Operation: CreateWorkflow | Class: validation_

> _Note: this issue was identified against the code base at commit `aeaddd52642475574d853bf8e089296dd7f2f86c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: CreateWorkflow validates the workflow's dependencies before minting it. The modeled ValidationException is documented for exactly this: 'This can occur when workflow definitions contain unsupported operators, when required IAM permissions are missing, when S3 locations are inaccessible...'. A DefinitionS3Location pointing at a bucket/object that does not exist is refused with ValidationException, and an execution RoleArn that does not exist or whose trust policy does not let airflow-serverless assume it is refused with the modeled AccessDeniedException ('You do not have sufficient permission to perform this action').

Actual: the handler checks only the declared @length/@pattern constraints of the members (and DefinitionS3Location's Bucket/ObjectKey declare no constraints at all), then persists the workflow and answers 200. Nothing in the crate ever consults the emulated S3 service or the role: `grep` over src/ shows DefinitionS3Location is only cloned into `WorkflowSnapshot`, never dereferenced, and RoleArn is only pattern-checked. So `CreateWorkflow` succeeds with Bucket='does-not-exist', ObjectKey='nope.yaml', or with a syntactically valid role ARN for a role that was never created, and a client gets a READY workflow that could never run on AWS.

## Evidence

Smithy model: CreateWorkflow declares ValidationException (400) with the documented causes above and AccessDeniedException (403); DefinitionS3Location.Bucket/ObjectKey target plain String with no length/pattern, so the boundary cannot reject anything. Handler: src/ops/create_workflow.rs `validate_input` (checks Name, RoleArn, ClientToken, Description, TriggerMode, Tags, Code, EncryptionConfiguration, NetworkConfiguration constraints only) and the snapshot at lines ~333-343 which merely stores the S3 pointer. Behavioral traces already recorded for this service name both rejections: `mwaa-serverless.create-workflow-untrusted-execution-role-access-denied-rejected` and `mwaa-serverless.create-workflow-get-workflow-round-trip` / `workflow-definition-snapshotted-at-creation-unaffected-by-s3-overwrite` (which require the definition object to actually exist). Test coverage: tests/all/validation_errors.rs only covers constraint violations; no test asserts an S3-inaccessible or untrusted-role refusal.

## Steps to Reproduce

1. Boot the emulator with no S3 bucket created.
2. Call CreateWorkflow with Name='w1', RoleArn='arn:aws:iam::123456789012:role/never-created', DefinitionS3Location={Bucket:'no-such-bucket', ObjectKey:'dags/none.yaml'}.
3. Observe HTTP 200 with a WorkflowArn and WorkflowStatus=READY; real AWS answers 400 ValidationException (S3 location inaccessible) or 403 AccessDeniedException (role not assumable by airflow-serverless).
