Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the greengrassv2 emulator

_Filed: 2026-10-21T21:08:24Z_
_Operation: service-com-amazonaws-greengrassv2-greengrassv2 | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 29 operations in `services/service-com-amazonaws-greengrassv2-greengrassv2/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented`, and `src/state.rs` holds no fields (`// TODO: add service-specific state fields`). No client workflow can succeed.

Implement a FIRST, SMALL, stateful slice: the component-publish-then-deploy loop, which is the core Greengrass V2 workflow. Protocol is `restJson1`, signing name `greengrass`.

Happy path, in order (field names from OPERATIONS.md):

1. **CreateComponentVersion** — `POST /greengrass/v2/createComponentVersion -> 201`. Input: `inlineRecipe` (blob; the JSON/YAML recipe carrying `ComponentName` and `ComponentVersion`), optional `tags`, `clientToken` (@idempotencyToken). Must parse the recipe to derive `componentName`/`componentVersion`, store the recipe bytes verbatim, mint `arn` matching `^arn:[^:]*:greengrass:[^:]*:(aws|[0-9]+):components:[^:]+:versions:[^:]+$`, and return required `componentName`, `componentVersion`, `creationTimestamp`, `status` (CloudComponentStatus with `componentState` e.g. `DEPLOYABLE`). Re-creating the same name+version must raise `ConflictException` (409).
2. **DescribeComponent** — `GET /greengrass/v2/components/{arn}/metadata -> 200`. Must look the stored version up by ARN and return `arn`, `componentName`, `componentVersion`, `creationTimestamp`, `status`, `tags` as persisted; unknown ARN -> `ResourceNotFoundException` (404).
3. **GetComponent** — `GET /greengrass/v2/components/{arn} -> 200`. Must return the stored recipe blob in `recipe` plus the required `recipeOutputFormat` (echoing the `recipeOutputFormat` query value, default JSON) and `tags`; unknown ARN -> 404.
4. **ListComponents** — `GET /greengrass/v2/components -> 200`. Must enumerate stored components as `components: list<Component>` where each entry is `{arn (name-level ARN, no `:versions:` segment), componentName, latestVersion: {arn, componentVersion, creationTimestamp, description, publisher, platforms}}`, honour `scope=PRIVATE|PUBLIC` (created components are PRIVATE), and honour `maxResults` (1..100) with an opaque `nextToken` that resumes where the previous page ended.
5. **CreateDeployment** — `POST /greengrass/v2/deployments -> 201`. Input: required `targetArn` (`^arn:[^:]*:iot:[^:]*:[0-9]+:(thing|thinggroup)/.+$`), `deploymentName`, `components` (map name -> ComponentDeploymentSpecification with required `componentVersion`), `deploymentPolicies`, `iotJobConfiguration`, `tags`, `clientToken`. Must persist the whole request, assign a fresh `deploymentId` and `iotJobId`/`iotJobArn`, set `deploymentStatus = ACTIVE`, mark it `isLatestForTarget = true` and clear that flag on the target's previous deployment. A malformed `targetArn` -> `ValidationException` (400).
6. **GetDeployment** — `GET /greengrass/v2/deployments/{deploymentId} -> 200`. Must return the persisted `targetArn`, `deploymentId`, `revisionId`, `deploymentName`, `deploymentStatus`, `components`, `deploymentPolicies`, `iotJobConfiguration`, `iotJobId`, `iotJobArn`, `creationTimestamp`, required `isLatestForTarget`, `parentTargetArn`, `tags` — exactly what CreateDeployment stored; unknown id -> 404.
7. **DeleteComponent** — `DELETE /greengrass/v2/components/{arn} -> 204`. Must remove the stored version so the subsequent DescribeComponent/GetComponent return `ResourceNotFoundException` (404) and ListComponents no longer shows it.

Every one of these must compute its response from persisted state. No canned bodies, no echoing the input back without storing it, no minting an ID that is never recorded. A create followed by a read must return what was created, and a delete must be observable by the next reader.

## State Model

Add to `ComAmazonawsGreengrassv2Greengrassv2State`:

- `components: BTreeMap<String /* componentName */, ComponentEntry>` where
  `ComponentEntry { name: String, name_arn: String, versions: BTreeMap<String /* componentVersion */, ComponentVersionEntry> }`
- `ComponentVersionEntry { arn: String, component_name: String, component_version: String, recipe: Vec<u8>, recipe_format: RecipeOutputFormat, description: Option<String>, publisher: Option<String>, platforms: Vec<ComponentPlatform>, creation_timestamp: SystemTime, status: CloudComponentStatus /* componentState, message, errors, vendorGuidance */, scope: ComponentVisibilityScope, tags: BTreeMap<String,String> }`
- `deployments: BTreeMap<String /* deploymentId */, DeploymentEntry>` where
  `DeploymentEntry { deployment_id: String, revision_id: String, deployment_name: Option<String>, target_arn: String, parent_target_arn: Option<String>, components: BTreeMap<String, ComponentDeploymentSpecification>, deployment_policies: Option<DeploymentPolicies>, iot_job_configuration: Option<DeploymentIoTJobConfiguration>, iot_job_id: String, iot_job_arn: String, deployment_status: DeploymentStatus, is_latest_for_target: bool, creation_timestamp: SystemTime, tags: BTreeMap<String,String> }`
- `latest_deployment_by_target: BTreeMap<String /* targetArn */, String /* deploymentId */>` so `isLatestForTarget` and `historyFilter=LATEST_ONLY` can be answered from state.
- `idempotency_tokens: BTreeMap<String /* clientToken */, String /* resource id or ARN */>` so a replayed `clientToken` on CreateComponentVersion / CreateDeployment returns the original result instead of creating a duplicate.

The remaining 22 operations (core-device, client-device association, connectivity-info, service-role, tagging, ListDeployments/ListComponentVersions/ListEffectiveDeployments/ListInstalledComponents, CancelDeployment, DeleteDeployment, ResolveComponentCandidates, GetComponentVersionArtifact) stay at 501 for this slice and are tracked as separate gap issues.

## Evidence

`services/service-com-amazonaws-greengrassv2-greengrassv2/OPERATIONS.md` lists all 29 operations with their routes, success codes and members — CreateComponentVersion (`POST /greengrass/v2/createComponentVersion -> 201`, required output `componentName`/`componentVersion`/`creationTimestamp`/`status`), DescribeComponent (`GET /greengrass/v2/components/{arn}/metadata`), GetComponent (required `recipe` + `recipeOutputFormat`), ListComponents (@paginated inputToken=nextToken, items=components, maxResults 1..100), CreateDeployment (`POST /greengrass/v2/deployments -> 201`, required `targetArn`), GetDeployment (required `isLatestForTarget`), DeleteComponent (`DELETE /greengrass/v2/components/{arn} -> 204`). Shape detail from the Smithy model: `Component{arn, componentName, latestVersion}`, `ComponentLatestVersion{arn, componentVersion, creationTimestamp, description, publisher, platforms}`, `Deployment{..., isLatestForTarget default false}`, `ComponentDeploymentSpecification{componentVersion required, configurationUpdate, runWith}`, `CloudComponentStatus{componentState, message, errors, vendorGuidance, vendorGuidanceMessage}`. Actual: `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 29 handler files, and `src/state.rs` declares `ComAmazonawsGreengrassv2Greengrassv2State` with only `// TODO: add service-specific state fields`, so no operation holds state. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI greengrassv2 client at it.
2. CreateComponentVersion with an inlineRecipe declaring ComponentName=com.example.Hello, ComponentVersion=1.0.0 — expect 201 with componentName/componentVersion echoed from the parsed recipe, a creationTimestamp, a `:components:com.example.Hello:versions:1.0.0` ARN and a CloudComponentStatus; today it returns HTTP 501 NotImplemented.
3. DescribeComponent with the returned ARN — expect 200 carrying the same componentName, componentVersion, creationTimestamp and status that were persisted.
4. GetComponent with the same ARN — expect 200 whose `recipe` blob is byte-identical to the inlineRecipe submitted, with recipeOutputFormat=JSON.
5. ListComponents?scope=PRIVATE&maxResults=1 — expect the component present with latestVersion.componentVersion=1.0.0; add a second component and verify maxResults=1 returns one item plus a nextToken that pages to the second.
6. CreateDeployment with targetArn=arn:aws:iot:us-east-1:123456789012:thing/MyCore and components={"com.example.Hello":{"componentVersion":"1.0.0"}} — expect 201 with a deploymentId and iotJobId/iotJobArn.
7. GetDeployment with that deploymentId — expect 200 returning the stored targetArn, deploymentName, components map, deploymentStatus=ACTIVE and isLatestForTarget=true.
8. CreateDeployment again for the same targetArn, then GetDeployment the first deploymentId — expect isLatestForTarget=false on the superseded deployment.
9. DeleteComponent with the component version ARN — expect 204, then DescribeComponent on the same ARN returns ResourceNotFoundException (404) and ListComponents no longer lists it.
10. CreateDeployment with targetArn="not-an-arn" — expect ValidationException (400), not a 2xx or 501.
