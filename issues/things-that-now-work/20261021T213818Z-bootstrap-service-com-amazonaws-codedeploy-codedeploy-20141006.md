Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the codedeploy emulator

_Filed: 2026-10-21T21:38:18Z_
_Operation: service-com-amazonaws-codedeploy-codedeploy-20141006 | Class: missing_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in `services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and answers HTTP 501 `NotImplemented`, holding no state. Implement a FIRST stateful slice covering the core CodeDeploy application/deployment lifecycle so a realistic client script works end to end.

### Happy path to implement (in order)

1. **CreateApplication** — input `applicationName` (required, len 1..100, pattern `^[A-Za-z0-9+=,.@_-]*$`), `computePlatform` (ECS|Lambda|Server), `tags`. Output `applicationId` — must be a freshly generated ID that is STORED, not echoed. A second create with the same name must fail with `ApplicationAlreadyExistsException`; a name violating the length/pattern constraints must fail with `InvalidApplicationNameException`; an empty/absent name with `ApplicationNameRequiredException`.
2. **CreateDeploymentGroup** — input `applicationName` (required), `deploymentGroupName` (required), `serviceRoleArn` (required), optional `deploymentConfigName`, `ec2TagFilters`, `autoRollbackConfiguration`, `deploymentStyle`, `tags`. Output `deploymentGroupId` (generated + stored). Unknown application ⇒ `ApplicationDoesNotExistException`; duplicate group name within the app ⇒ `DeploymentGroupAlreadyExistsException`; unknown `deploymentConfigName` ⇒ `DeploymentConfigDoesNotExistException` (pre-seed the AWS built-in configs `CodeDeployDefault.OneAtATime`, `CodeDeployDefault.AllAtOnce`, `CodeDeployDefault.HalfAtATime` so `GetDeploymentConfig`/validation resolve them).
3. **CreateDeployment** — input `applicationName` (required), `deploymentGroupName`, `revision` (RevisionLocation), `deploymentConfigName`, `description`, `ignoreApplicationStopFailures`, `updateOutdatedInstancesOnly`, `fileExistsBehavior`, `autoRollbackConfiguration`. Output `deploymentId` — generate a `d-XXXXXXXXX`-style ID, persist a full DeploymentInfo record, and index it under (application, deployment group). Unknown app ⇒ `ApplicationDoesNotExistException`; unknown group ⇒ `DeploymentGroupDoesNotExistException`; missing revision ⇒ `RevisionRequiredException`.
4. **GetDeployment** — input `deploymentId` (required). Output `deploymentInfo` (DeploymentInfo) reconstructed from the stored record: at minimum `deploymentId`, `applicationName`, `deploymentGroupName`, `deploymentConfigName` (the effective one, defaulted when the request omitted it), `revision` (byte-identical to what CreateDeployment stored), `status`, `creator` (`user`), `createTime`, `startTime`, `completeTime`, `description`, `ignoreApplicationStopFailures`, `updateOutdatedInstancesOnly`, `computePlatform` (inherited from the application). Unknown ID ⇒ `DeploymentDoesNotExistException`. Deployments created by the emulator should reach a terminal `Succeeded` status (either immediately or after the first read) so client waiters terminate; never report a hardcoded status unrelated to the stored record.
5. **GetApplication / ListApplications** — `GetApplication(applicationName)` returns `application` (ApplicationInfo: `applicationId`, `applicationName`, `createTime`, `computePlatform`, `linkedToGitHub`) from the stored record, `ApplicationDoesNotExistException` otherwise. `ListApplications` returns the stored names (@paginated via `nextToken`; a token-driven second page must continue, not restart).
6. **ListDeployments** — optional `applicationName`, `deploymentGroupName`, `includeOnlyStatuses`, `createTimeRange`, `nextToken`. Output `deployments` (deployment IDs, newest first) filtered by the stored application/group/status; `nextToken` paging must work. Unknown app/group ⇒ the corresponding `*DoesNotExistException`.
7. **DeleteApplication** — input `applicationName` (required). Removes the application together with its deployment groups and revision registrations; a following `GetApplication` must then fail with `ApplicationDoesNotExistException`, and `ListApplications` must no longer list it. (Deleting a non-existent application is a no-op success in CodeDeploy.)

All seven must compute their responses from persisted state in the crate's shared engine store (`crate::state::service_state(&*self.engine)`), reached through `self.engine` — no canned bodies, no echoed inputs, no generated IDs that are not stored. Delete the `SCAFFOLDED, NOT IMPLEMENTED` marker line from each file you implement.

## State Model

In `ComAmazonawsCodedeployCodedeploy20141006State`:

- `applications: HashMap<String /*applicationName*/, Application>` where `Application { application_id: String, application_name: String, compute_platform: Option<ComputePlatform>, create_time: SystemTime, linked_to_git_hub: bool, github_account_name: Option<String>, tags: Vec<Tag>, deployment_groups: HashMap<String /*deploymentGroupName*/, DeploymentGroup>, revisions: Vec<RegisteredRevision> }`
- `DeploymentGroup { deployment_group_id: String, deployment_group_name: String, application_name: String, service_role_arn: String, deployment_config_name: String, ec2_tag_filters: Vec<EC2TagFilter>, on_premises_instance_tag_filters: Vec<TagFilter>, auto_scaling_groups: Vec<String>, auto_rollback_configuration: Option<AutoRollbackConfiguration>, deployment_style: Option<DeploymentStyle>, alarm_configuration: Option<AlarmConfiguration>, load_balancer_info: Option<LoadBalancerInfo>, tags: Vec<Tag>, last_successful_deployment: Option<String>, last_attempted_deployment: Option<String> }`
- `deployments: HashMap<String /*deploymentId*/, Deployment>` where `Deployment { deployment_id: String, application_name: String, deployment_group_name: Option<String>, deployment_config_name: String, revision: Option<RevisionLocation>, previous_revision: Option<RevisionLocation>, status: DeploymentStatus, creator: DeploymentCreator, description: Option<String>, ignore_application_stop_failures: bool, update_outdated_instances_only: bool, file_exists_behavior: Option<FileExistsBehavior>, auto_rollback_configuration: Option<AutoRollbackConfiguration>, compute_platform: Option<ComputePlatform>, external_id: Option<String>, create_time: SystemTime, start_time: Option<SystemTime>, complete_time: Option<SystemTime> }`
- `deployment_order: Vec<String>` — creation order of deployment IDs, so `ListDeployments` can return newest-first deterministically and page stably.
- `deployment_configs: HashMap<String /*deploymentConfigName*/, DeploymentConfig { deployment_config_id: String, deployment_config_name: String, minimum_healthy_hosts: Option<MinimumHealthyHosts>, compute_platform: Option<ComputePlatform>, create_time: SystemTime, is_builtin: bool }>` — pre-seeded with the `CodeDeployDefault.*` built-ins.
- `next_id: u64` or a counter used to mint `applicationId` / `deploymentGroupId` / `d-` deployment IDs.

Operations outside this slice (deployment targets/instances, on-premises instance registration, tagging APIs, GitHub tokens, blue/green controls such as ContinueDeployment / StopDeployment / SkipWaitTimeForInstanceTermination, PutLifecycleEventHookExecutionStatus, the BatchGet* family, UpdateApplication / UpdateDeploymentGroup) stay at 501 for now and will be filed as individual gap issues.

## Evidence

Spec/model: `services/service-com-amazonaws-codedeploy-codedeploy-20141006/OPERATIONS.md` lines 82-104 (CreateApplication → `applicationId`, with `ApplicationAlreadyExistsException`/`InvalidApplicationNameException`; CreateDeploymentGroup → `deploymentGroupId` with required `applicationName`/`deploymentGroupName`/`serviceRoleArn`; CreateDeployment → `deploymentId`), lines 142-158 (GetApplication → `application` (ApplicationInfo), GetDeployment → `deploymentInfo` (DeploymentInfo) with `DeploymentDoesNotExistException`), lines 198-231 (ListApplications and ListDeployments are @paginated with `nextToken`), lines 106-110 (DeleteApplication). Field inventories for ApplicationInfo and DeploymentInfo taken from the Smithy model via smithy_query (`applicationId`/`applicationName`/`createTime`/`computePlatform`/`linkedToGitHub`; `deploymentId`/`applicationName`/`deploymentGroupName`/`deploymentConfigName`/`revision`/`status`/`creator`/`createTime`/`startTime`/`completeTime`/`description`).

Implementation: all 47 files under `src/ops/` are scaffolds; e.g. `src/ops/create_deployment.rs:4` carries `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented`. No tests exercise any CodeDeploy behavior (only the generated `tests/state_roundtrip.rs` state-API example). No existing issue mentions codedeploy.

## Steps to Reproduce

1. Start the emulator and point the AWS CLI/SDK at it: `aws --endpoint-url http://localhost:<port> deploy create-application --application-name demo-app --compute-platform Server` → expect 200 with a non-empty `applicationId`; today returns HTTP 501 NotImplemented
2. `aws deploy get-application --application-name demo-app` → expect `application.applicationName == demo-app`, `application.computePlatform == Server`, and the same `applicationId` returned by create
3. `aws deploy list-applications` → expect `applications` to contain `demo-app`
4. `aws deploy create-deployment-group --application-name demo-app --deployment-group-name demo-dg --service-role-arn arn:aws:iam::123456789012:role/CodeDeployRole --deployment-config-name CodeDeployDefault.OneAtATime` → expect 200 with a stored `deploymentGroupId`; repeat the same call → expect `DeploymentGroupAlreadyExistsException` (HTTP 400)
5. `aws deploy create-deployment --application-name demo-app --deployment-group-name demo-dg --s3-location bucket=my-bucket,key=app.zip,bundleType=zip --description 'first deploy'` → expect 200 with a `deploymentId`
6. `aws deploy get-deployment --deployment-id <id>` → expect `deploymentInfo` echoing the stored application/group/config, the exact S3 revision supplied, `creator=user`, a `createTime`, and a status that eventually reaches `Succeeded`
7. `aws deploy list-deployments --application-name demo-app --deployment-group-name demo-dg` → expect `deployments` to contain `<id>`; `aws deploy list-deployments --application-name demo-app --deployment-group-name nope` → expect `DeploymentGroupDoesNotExistException`
8. `aws deploy get-deployment --deployment-id d-NOTREAL1` → expect `DeploymentDoesNotExistException` (HTTP 400), not 501
9. `aws deploy delete-application --application-name demo-app` then `aws deploy get-application --application-name demo-app` → expect `ApplicationDoesNotExistException`, and `list-applications` no longer lists `demo-app`
