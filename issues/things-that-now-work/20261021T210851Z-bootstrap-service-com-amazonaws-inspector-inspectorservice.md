Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the Amazon Inspector (classic) emulator

_Filed: 2026-10-21T21:08:51Z_
_Operation: service-com-amazonaws-inspector-inspectorservice | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 37 handlers in `services/service-com-amazonaws-inspector-inspectorservice/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), and `src/state.rs` holds `ComAmazonawsInspectorInspectorserviceState { /* TODO: add service-specific state fields */ }` — zero state. No client workflow can run.

Implement a FIRST SLICE of SIX operations statefully — the Inspector-classic core loop of resource group -> assessment target -> assessment template -> assessment run -> read -> delete. Wire protocol is `awsJson1_1`: `POST /` with `X-Amz-Target: InspectorService.<OperationName>` (see README/OPERATIONS.md).

1. **CreateResourceGroup** — input `resourceGroupTags` (list<ResourceGroupTag>, 1..10, required). Mint and PERSIST a resource group keyed by a generated ARN of the form `arn:aws:inspector:<region>:<account>:resourcegroup/0-<8 chars>`; return `resourceGroupArn`. The tags must be retrievable later by DescribeResourceGroups.
2. **CreateAssessmentTarget** — input `assessmentTargetName` (1..140, required), `resourceGroupArn` (optional). Reject a `resourceGroupArn` that is not in the store with `NoSuchEntityException` (400). Store an AssessmentTarget {arn, name, resourceGroupArn, createdAt, updatedAt} under a generated `.../target/0-<8 chars>` ARN; return `assessmentTargetArn`. Duplicate target names must fail per service semantics (`InvalidInputException`).
3. **CreateAssessmentTemplate** — input `assessmentTargetArn` (required), `assessmentTemplateName` (required), `durationInSeconds` (180..86400, required), `rulesPackageArns` (required), `userAttributesForFindings` (optional). Unknown `assessmentTargetArn` -> `NoSuchEntityException`; `durationInSeconds` out of the modeled range -> `InvalidInputException`. Persist an AssessmentTemplate {arn, name, assessmentTargetArn, durationInSeconds, rulesPackageArns, userAttributesForFindings, assessmentRunCount: 0, lastAssessmentRunArn: None, createdAt} with ARN `<targetArn>/template/0-<8 chars>`; return `assessmentTemplateArn`.
4. **StartAssessmentRun** — input `assessmentTemplateArn` (required), `assessmentRunName` (optional, default derived from the template name). Unknown template -> `NoSuchEntityException`. Create an AssessmentRun {arn (`<templateArn>/run/0-<8 chars>`), name, assessmentTemplateArn, state, durationInSeconds and rulesPackageArns COPIED FROM THE TEMPLATE, userAttributesForFindings, createdAt, startedAt, stateChangedAt, dataCollected, stateChanges, notifications, findingCounts}; increment the template's `assessmentRunCount` and set its `lastAssessmentRunArn`. Return `assessmentRunArn`.
5. **DescribeAssessmentRuns** — input `assessmentRunArns` (1..10, required). Return `assessmentRuns` built from the stored runs (name, template ARN, state, durationInSeconds, rulesPackageArns, timestamps all matching what StartAssessmentRun recorded) and put every unknown ARN into `failedItems` keyed by that ARN with `failureCode: ITEM_DOES_NOT_EXIST` / `retryable: false` — not an empty map, and not an echo of the input.
6. **DeleteAssessmentTarget** — input `assessmentTargetArn` (required). Unknown ARN -> `NoSuchEntityException`. Remove the target AND cascade-remove its templates and runs, so a following DescribeAssessmentTargets/DescribeAssessmentRuns reports those ARNs in `failedItems` rather than returning them.

No canned, hardcoded or echoed responses: every field in every response above must be computed from the persisted state the earlier call wrote, and generated ARNs/IDs must be stored, not minted-and-discarded. Required output members (`failedItems`, `assessmentRuns`, ...) must always be present in the JSON body. Each implemented handler must drop its `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line.

## State Model

Add to `ComAmazonawsInspectorInspectorserviceState` (all reached via `crate::state::service_state(&*self.engine)`, never a fresh engine):

- `resource_groups: HashMap<String /*arn*/, ResourceGroup { arn, tags: Vec<ResourceGroupTag{key, value}>, created_at }>`
- `assessment_targets: HashMap<String /*arn*/, AssessmentTarget { arn, name, resource_group_arn: Option<String>, created_at, updated_at }>`
- `assessment_templates: HashMap<String /*arn*/, AssessmentTemplate { arn, name, assessment_target_arn, duration_in_seconds: i32, rules_package_arns: Vec<String>, user_attributes_for_findings: Vec<Attribute>, last_assessment_run_arn: Option<String>, assessment_run_count: i32, created_at }>`
- `assessment_runs: HashMap<String /*arn*/, AssessmentRun { arn, name, assessment_template_arn, state, duration_in_seconds, rules_package_arns, user_attributes_for_findings, created_at, started_at, completed_at: Option<_>, state_changed_at, data_collected: bool, state_changes: Vec<AssessmentRunStateChange>, notifications: Vec<AssessmentRunNotification>, finding_counts: HashMap<Severity, i32> }>`
- `next_id: u64` (or an RNG) used to mint the `0-xxxxxxxx` ARN suffixes deterministically per store.

The remaining 31 operations (ListAssessmentTargets/Templates/Runs, DescribeAssessmentTargets/Templates/ResourceGroups, ListFindings/DescribeFindings, Stop/DeleteAssessmentRun, tags, event subscriptions, exclusions, agents, rules packages, cross-account role) stay 501 for now and are tracked as individual gap issues.

## Evidence

OPERATIONS.md (crate root, generated from `smithy-model/com-amazonaws-inspector-inspectorservice.json`) lists all 37 operations with the input/output members and errors cited above: CreateResourceGroup (`resourceGroupTags` -> `resourceGroupArn`), CreateAssessmentTarget (`assessmentTargetName`, `resourceGroupArn` -> `assessmentTargetArn`, errors include NoSuchEntityException 400), CreateAssessmentTemplate (`assessmentTargetArn`, `assessmentTemplateName`, `durationInSeconds` range 180..86400, `rulesPackageArns` -> `assessmentTemplateArn`), StartAssessmentRun (`assessmentTemplateArn`, `assessmentRunName` -> `assessmentRunArn`), DescribeAssessmentRuns (`assessmentRunArns` 1..10 -> required `assessmentRuns` + required `failedItems`), DeleteAssessmentTarget (`assessmentTargetArn` -> no members, AssessmentRunInProgressException/NoSuchEntityException). `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-inspector-inspectorservice/src/ops/` matches every handler file (36 wired ops + SKIPPED_STUBS.md), and `src/state.rs` has no fields, proving nothing is implemented and no state exists. Inspector-classic ARN hierarchy (target -> template -> run) and `failedItems`/`ITEM_DOES_NOT_EXIST` semantics are well-established service behavior.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI inspector client (classic Inspector, awsJson1_1, X-Amz-Target: InspectorService.<Op>) at it.
2. CreateResourceGroup with resourceGroupTags=[{key:'Name',value:'web'}] -> expect 200 with a resourceGroupArn; today: HTTP 501 NotImplemented.
3. CreateAssessmentTarget with assessmentTargetName='demo-target' and that resourceGroupArn -> expect 200 with assessmentTargetArn.
4. CreateAssessmentTemplate with that assessmentTargetArn, assessmentTemplateName='demo-template', durationInSeconds=3600, rulesPackageArns=['arn:aws:inspector:us-east-1:316112463485:rulespackage/0-gEjTy7T7'] -> expect 200 with assessmentTemplateArn.
5. StartAssessmentRun with that assessmentTemplateArn and assessmentRunName='run-1' -> expect 200 with assessmentRunArn under the template ARN.
6. DescribeAssessmentRuns with [thatRunArn, 'arn:aws:inspector:us-east-1:123456789012:target/0-xxxxxxxx/template/0-yyyyyyyy/run/0-zzzzzzzz'] -> expect assessmentRuns[0].name=='run-1', assessmentTemplateArn/durationInSeconds/rulesPackageArns equal to what the template was created with, and failedItems containing the bogus ARN with failureCode ITEM_DOES_NOT_EXIST.
7. DeleteAssessmentTarget with the target ARN -> expect 200 empty body; re-run DescribeAssessmentRuns and DescribeAssessmentTargets and expect the deleted ARNs to appear only in failedItems.
8. Call CreateAssessmentTemplate with durationInSeconds=10 and with an unknown assessmentTargetArn -> expect InvalidInputException (400) and NoSuchEntityException (400) respectively, not 501.
