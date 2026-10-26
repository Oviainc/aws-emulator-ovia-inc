Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: cf1145e7beae3fcf58bc3c47bccc54d7fcf374b0

# The workflow-configuration digest is implemented twice (create_workflow::request_fingerprint and update_workflow::request_fingerprint)

_Filed: 2026-10-26T07:29:50Z_
_Operation: UpdateWorkflow | Class: maintainability_

> _Note: this issue was identified against the code base at commit `cf1145e7beae3fcf58bc3c47bccc54d7fcf374b0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The same computation — a length-delimited digest over a workflow configuration (RoleArn, Description, TriggerMode, EngineVersion, the three DefinitionS3Location fields, the Code union's S3 triple or its placeholder, the LoggingConfiguration log group, and the NetworkConfiguration id lists) — is written out field-by-field in two handlers of this crate. The copies already differ (one leads with `Name`, the other with `WorkflowArn`; only one covers `EncryptionConfiguration`), which is exactly the drift a single derivation would prevent: a field added to the configuration in one copy silently leaves the other digesting a different view of the same snapshot. Extracting the shared middle into one helper beside the other digest helpers in `crate::state` keeps the field order, so every digest value, response and existing test is unchanged.

## Evidence

Copy 1, `services/.../src/ops/create_workflow.rs:510-546`:

    fn request_fingerprint(input: &CreateWorkflowInput) -> String {
        let mut fields: Vec<String> = vec![
            input.name.clone(),
            input.role_arn.clone(),
            input.description.clone().unwrap_or_default(),
            input.trigger_mode.clone().unwrap_or_default(),
            input.engine_version.map(|version| version.to_string()).unwrap_or_default(),
            input.definition_s3_location.bucket.clone(),
            input.definition_s3_location.object_key.clone(),
            input.definition_s3_location.version_id.clone().unwrap_or_default(),
        ];
        match input.code.as_ref() { ... None => fields.push(String::new()), }
        ... if let Some(logging) = ... { fields.push(logging.log_group_name.clone()); }
        if let Some(network) = ... { fields.extend(...security_group_ids...); fields.extend(...subnet_ids...); }

Copy 2, `services/.../src/ops/update_workflow.rs:363-395`, the same sequence with `input.workflow_arn` in place of `input.name` and no encryption fields:

    fn request_fingerprint(input: &UpdateWorkflowInput) -> String {
        let mut fields: Vec<String> = vec![
            input.workflow_arn.clone(),
            input.role_arn.clone(),
            input.description.clone().unwrap_or_default(),
            ...
        ];
        match input.code.as_ref() { ... None => fields.push(String::new()), }
        if let Some(logging) = input.logging_configuration.as_ref() { fields.push(logging.log_group_name.clone()); }
        if let Some(network) = input.network_configuration.as_ref() { ... }

Both end in the single shared primitive `crate::state::digest` (`state.rs:818-827`), so only the field walk is duplicated.

Tests that cover both copies and must stay green: `tests/all/workflow_lifecycle.rs:457-513` (`a_repeated_client_token_replays_the_first_create`, which depends on create's digest) and `tests/all/workflow_update_versions.rs:341` (`repeated_updates_keep_every_version_and_only_the_last_is_latest`, which depends on update's digest feeding `mint_workflow_version`).

## Steps to Reproduce

1. Open services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/ops/create_workflow.rs at request_fingerprint (line 510) and src/ops/update_workflow.rs at request_fingerprint (line 363).
2. Compare the two field walks: identical from RoleArn through the NetworkConfiguration id lists, differing only in the leading identity field and in create's encryption fields.
3. Note that adding a configuration member (as EncryptionConfiguration already shows) requires editing both copies, and neither references the other.
