Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the AWS Proton emulator

_Filed: 2026-10-21T21:28:25Z_
_Operation: service-com-amazonaws-proton-awsproton20200720 | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 90 operations in `services/service-com-amazonaws-proton-awsproton20200720/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no Proton client can do anything. Implement a FIRST SMALL SLICE: the environment-template + environment lifecycle, which is Proton's primary resource chain.

### Happy path to implement (in order)

1. **CreateEnvironmentTemplate** — input `name` (required, 1..100, `^[0-9A-Za-z]+[0-9A-Za-z_\-]*$`), `displayName`, `description`, `encryptionKey`, `provisioning` (`CUSTOMER_MANAGED`), `tags`. Must persist a new template and return `environmentTemplate` with the required members computed from the stored record: `name`, `arn` (`arn:aws:proton:<region>:<account>:environment-template/<name>`), `createdAt`, `lastModifiedAt`, plus the optional `displayName`/`description`/`provisioning`/`recommendedVersion` as stored. Re-creating an existing name must fail `ConflictException` (HTTP 409).
2. **GetEnvironmentTemplate** — input `name`; returns the SAME `environmentTemplate` record that CreateEnvironmentTemplate stored (same arn, same createdAt, same description). Unknown name ⇒ `ResourceNotFoundException` (HTTP 404).
3. **CreateEnvironment** — input `name` (required), `templateName` (required), `templateMajorVersion` (required), `templateMinorVersion`, `spec` (required), `description`, `protonServiceRoleArn`, `environmentAccountConnectionId`, `provisioningRepository`, `componentRoleArn`, `codebuildRoleArn`, `tags`. Must look up `templateName` in the stored template map and return `ResourceNotFoundException` (404) when it is absent; otherwise persist an Environment and return `environment` with every required member filled from state: `name`, `arn`, `createdAt`, `lastDeploymentAttemptedAt`, `lastDeploymentSucceededAt`, `templateName`, `templateMajorVersion`, `templateMinorVersion`, `deploymentStatus` (`SUCCEEDED` is acceptable for the emulator — deployment is instantaneous), plus the stored `description`/`spec`/`provisioning`/role ARNs. Duplicate `name` ⇒ `ConflictException` (409).
4. **GetEnvironment** — input `name`; returns the stored `environment` verbatim (the `spec`, `description` and ARN created in step 3 must come back). Unknown name ⇒ `ResourceNotFoundException` (404).
5. **ListEnvironments** — input `nextToken` (`^[A-Za-z0-9+=/]+$`), `maxResults` (1..100), `environmentTemplates` (list of `EnvironmentTemplateFilter`); output required `environments` (list<EnvironmentSummary>) + `nextToken`. Must project each stored Environment into an `EnvironmentSummary` (name, arn, createdAt, lastDeploymentAttemptedAt, lastDeploymentSucceededAt, templateName, templateMajorVersion, templateMinorVersion, deploymentStatus), honour `maxResults` as a real page size, emit an opaque `nextToken` only when more items remain and resume from it, and apply the `environmentTemplates` filter (majorVersion/templateName) instead of ignoring it.
6. **DeleteEnvironment** — input `name`; removes the record and returns the deleted `environment`. A following GetEnvironment must return `ResourceNotFoundException` (404), and ListEnvironments must no longer include it. Delete of an absent name is `@idempotent` and must not 500.

### Rules

- NEVER STUBS: each of the six handlers must compute its response from the shared engine state reached via `crate::state::service_state(&*self.engine)` — no canned structs, no echoing the input back without storing it, no random ARNs that are not persisted. A create followed by a get/list must return exactly what was created.
- Modeled validation must be enforced before mutation: required members present, `name`/`templateName` length 1..100 and pattern, `templateMajorVersion`/`templateMinorVersion` pattern `^(0|([1-9]{1}\d*))$`, `spec` 1..51200, `maxResults` 1..100 ⇒ otherwise `ValidationException` (HTTP 400).
- Error shapes must be the modeled ones with their modeled HTTP statuses: `ValidationException` 400, `ResourceNotFoundException` 404, `ConflictException` 409.

## State Model

Add to `ComAmazonawsProtonAwsproton20200720State` in `src/state.rs`:

- `environment_templates: BTreeMap<String /* name */, EnvironmentTemplateRecord>` where `EnvironmentTemplateRecord { name: String, arn: String, created_at: SystemTime, last_modified_at: SystemTime, display_name: Option<String>, description: Option<String>, encryption_key: Option<String>, provisioning: Option<String>, recommended_version: Option<String>, tags: Vec<(String, String)> }`.
- `environments: BTreeMap<String /* name */, EnvironmentRecord>` where `EnvironmentRecord { name: String, arn: String, created_at: SystemTime, last_deployment_attempted_at: SystemTime, last_deployment_succeeded_at: SystemTime, template_name: String, template_major_version: String, template_minor_version: String, deployment_status: String, deployment_status_message: Option<String>, description: Option<String>, spec: Option<String>, provisioning: Option<String>, proton_service_role_arn: Option<String>, environment_account_connection_id: Option<String>, environment_account_id: Option<String>, component_role_arn: Option<String>, codebuild_role_arn: Option<String>, provisioning_repository: Option<RepositoryBranchRecord>, tags: Vec<(String, String)> }`.

`BTreeMap` keyed by name gives both O(1)-ish lookup for Get/Delete and the deterministic ordering ListEnvironments needs for stable pagination tokens (the token can encode the last-returned name).

All other Proton operations (services, service instances, components, repositories, sync configs, deployments, tagging, account settings) stay at 501 and are tracked as separate gap issues.

## Evidence

`services/service-com-amazonaws-proton-awsproton20200720/src/ops/` contains 90 handler files and all 90 match `^//! SCAFFOLDED, NOT IMPLEMENTED:` (grep over the ops dir), i.e. every operation returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` ⇒ HTTP 501 with zero persisted state. Input/output members and error statuses above are taken from the crate's OPERATIONS.md (CreateEnvironment L264-269, GetEnvironment L271-276, DeleteEnvironment L285-290, ListEnvironments L292-297, CreateEnvironmentTemplate L299-304, GetEnvironmentTemplate L306-311) and from the Smithy model shapes `Environment`, `EnvironmentSummary`, `EnvironmentTemplate`, `EnvironmentTemplateSummary` (required members and ARN/timestamp fields).

## Steps to Reproduce

1. Start the emulator and point an AWS Proton client at it.
2. Call CreateEnvironmentTemplate(name="my-env-tmpl", displayName="My Env", description="demo", provisioning="CUSTOMER_MANAGED") — today: HTTP 501 NotImplemented; expected: 200 with environmentTemplate{name, arn, createdAt, lastModifiedAt, displayName, description}.
3. Call GetEnvironmentTemplate(name="my-env-tmpl") and verify the arn/createdAt/description are identical to the create response (proves state, not an echo).
4. Call CreateEnvironment(name="dev", templateName="my-env-tmpl", templateMajorVersion="1", spec="proton: EnvironmentSpec\n", description="dev env") — expect 200 with environment{name="dev", arn, createdAt, templateName, templateMajorVersion, templateMinorVersion, deploymentStatus}.
5. Call CreateEnvironment again with name="dev" — expect ConflictException (HTTP 409), not 200 and not 501.
6. Call CreateEnvironment(name="other", templateName="does-not-exist", templateMajorVersion="1", spec="x") — expect ResourceNotFoundException (HTTP 404).
7. Call GetEnvironment(name="dev") and verify description and spec round-trip exactly as submitted.
8. Call ListEnvironments(maxResults=1) with two environments created — expect exactly 1 EnvironmentSummary plus a nextToken; pass that nextToken back and expect the second environment and no duplicate.
9. Call ListEnvironments(environmentTemplates=[{templateName="my-env-tmpl", majorVersion="1"}]) and verify only matching environments are returned.
10. Call DeleteEnvironment(name="dev") — expect 200 with the deleted environment; then GetEnvironment(name="dev") must return ResourceNotFoundException (404) and ListEnvironments must not include "dev".
11. Call CreateEnvironment(name="bad name!", templateName="my-env-tmpl", templateMajorVersion="1", spec="x") — expect ValidationException (HTTP 400).
