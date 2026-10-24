Issue-Severity: HIGH
Issue-Gap-Class: validation
Issue-Commit: f76fdd8ab8ab91800165900d63e48ff1fef7e5fc

# CreateDeployment accepts a specifications map the published pattern schema rejects, and never enforces the modeled map-key constraints

_Filed: 2026-10-23T23:09:15Z_
_Operation: CreateDeployment | Class: validation_

> _Note: this issue was identified against the code base at commit `f76fdd8ab8ab91800165900d63e48ff1fef7e5fc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateDeployment performs only three checks before persisting a deployment: the workload exists, the pattern exists on that workload, and the deployment name is not currently held. The `specifications` map itself is never judged against anything.

Cause 1 — the published specification schema is not enforced. GetWorkloadDeploymentPattern reports, for every seeded pattern, a `specifications` schema whose `KeyPairName` and `VpcId` fields carry `required: "Yes"` and whose boolean fields (`CreateSecurityGroup`, `DisableDeploymentRollback`, `SaveDeploymentArtifacts`, `EnableEbsVolumeEncryption`) carry `allowedValues: ["Yes","No"]`. Expected (real Launch Wizard): a CreateDeployment whose specifications omit a required field, name a field the pattern does not publish, or give a value outside a field's allowedValues is refused with ValidationException (HTTP 400) and nothing is created — this is exactly why GetWorkloadDeploymentPattern publishes the schema with `required`/`allowedValues`. Actual: any non-empty map is accepted, e.g. `{"Foo":"bar"}` yields HTTP 200 with a deploymentId, and GetDeployment reports that map back as the deployment's specifications, with the required `KeyPairName`/`VpcId` absent. `dryRun: true`, documented as "checks whether you have the required permissions for the action, without actually making the request", likewise returns success for the same unusable input, so a client cannot pre-flight a deployment.

Cause 2 — map KEY constraints are never checked for this operation. The model types `DeploymentSpecifications` keys as `KeyString` (len 3..256, pattern `^[a-zA-Z0-9-:]+$`) and `Tags` keys as `TagKey` (len 1..128, pattern `^[a-zA-Z0-9 _.:/=+@-]+$`). This crate's own TagResource handler documents that the generated boundary walk does not carry a map's key constraints (it judges a key against the value shape), which is why TagResource adds `state::tag_key_refusal`. CreateDeployment has no such check, so a specification key like `"a"` (shorter than KeyString's min 3) or `"bad key!"` (outside its pattern), and a tag key like `""` or a 200-character tag key (both legal under `TagValue`, illegal under `TagKey`) are accepted with HTTP 200 and stored. The same tag key sent to TagResource on the resulting deployment is refused with ValidationException — so the emulator writes, through CreateDeployment, tag sets that its own tagging API declares invalid.

## Evidence

Spec: Smithy model — `DeploymentSpecifications` is map<KeyString(len 3..256, pattern ^[a-zA-Z0-9-:]+$), ValueString(len 1..1500)>; `Tags` is map<TagKey(len 1..128, pattern ^[a-zA-Z0-9 _.:/=+@-]+$), TagValue(len 0..256)>; `DeploymentSpecificationsField` carries `required` and `allowedValues`, and CreateDeployment's doc points callers at GetWorkloadDeploymentPattern "to retrieve the specifications required to create a deployment"; ValidationException is modeled at HTTP 400 for CreateDeployment.
Handler: services/service-com-amazonaws-launchwizard-launchwizard/src/ops/create_deployment.rs — handle() lines 163-239 contain only the workload lookup, the pattern lookup, the duplicate-name check, the dryRun short-circuit and the store ceiling; `input.specifications` is first touched at line 308 where it is moved into the stored record verbatim, and `input.tags` at line 311. No call to `state::constraint_refusal` or `state::tag_key_refusal` exists in this file (grep shows those helpers are used only by tag_resource.rs:170-176 and untag_resource.rs:165), and src/ops/types/create_deployment_types.rs declares no constrained twin.
Seeded schema the handler ignores: src/state.rs `common_specification_fields()` lines 659-711 (`KeyPairName`, `VpcId` required = true; Yes/No allowedValues on the boolean fields) and `workload_specification_fields()` lines 719-737.
Tests: tests/all/workload_deployment_patterns.rs::the_reported_specification_schema_drives_a_create_deployment (lines 477-575) only asserts the happy path (schema filled in correctly) plus the boundary refusal of an EMPTY map (`DeploymentSpecifications` @length min 1). No test submits a map missing a required field, an unknown field name, a value outside allowedValues, or a key/tag-key that violates the key shape, so this behavior is unasserted.

## Steps to Reproduce

1. Boot the emulator and point a launch-wizard client at it.
2. Call GetWorkloadDeploymentPattern(workloadName='SAP', deploymentPatternName='SapHanaSingle') and note that the reported specifications list marks KeyPairName and VpcId as required:'Yes' and publishes allowedValues ['Yes','No'] for CreateSecurityGroup.
3. Call CreateDeployment(workloadName='SAP', deploymentPatternName='SapHanaSingle', name='spec-gap', specifications={'Foo':'bar'}) — expect ValidationException (400); observe HTTP 200 with a deploymentId.
4. Call GetDeployment on that id and observe the stored specifications are {'Foo':'bar'} with no KeyPairName/VpcId.
5. Repeat with specifications={'CreateSecurityGroup':'Maybe', 'KeyPairName':'k', 'VpcId':'vpc-1'} — expect ValidationException for the value outside allowedValues; observe 200.
6. Repeat with specifications={'ab':'x'} (key shorter than KeyString min 3) and with specifications={'bad key!':'x'} (key outside the KeyString pattern) — expect ValidationException; observe 200.
7. Call CreateDeployment(..., tags={'': 'v'}) or with a 200-character tag key — expect ValidationException (TagKey is len 1..128); observe 200, then send the SAME tag key to TagResource on the created deployment's ARN and observe it IS refused with ValidationException.
