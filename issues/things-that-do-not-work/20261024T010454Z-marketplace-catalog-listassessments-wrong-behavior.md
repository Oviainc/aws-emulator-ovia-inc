Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# No code path ever creates an assessment, so ListAssessments is permanently empty and DescribeAssessment permanently 404

_Filed: 2026-10-24T01:04:54Z_
_Operation: ListAssessments | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The assessment surface is implemented as a read-only projection of a store that nothing ever writes. `ListAssessments` therefore always answers 200 with an empty `AssessmentSummaryList`, and `DescribeAssessment` always answers 404, no matter what the caller has published. On AWS an assessment is the result of a framework evaluation (AMI Security / Container Security) that the service performs when a product version is submitted through a change set, so after a successful APPLY change set against an AMI or container product, `ListAssessments` reports an assessment for that entity/change set and `DescribeAssessment` reports its `AssessmentResult`, `FrameworkSummary` and `ControlAssessments`. The emulator never produces one, so the whole assessment workflow (list → describe → page the control results) is unreachable through the API; only in-crate tests that poke `store.assessments` directly can see it.

## Evidence

Spec: `ListAssessments` ("An assessment is the result of evaluating a product or change set against a framework, such as AMI Security or Container Security") and `DescribeAssessment` are modeled, paginated read operations over assessments keyed by `AssessmentIdentifier`/`AssessmentTargetFilter.EntityId`/`ChangeSetId` — i.e. assessments are expected to be produced by change-set activity on products.
State: `src/state.rs` — `pub assessments: BTreeMap<String, AssessmentRecord>` is documented "Nothing creates one … so no call creates a record here, and both operations answer from a store that is consequently empty (`ListAssessments` an empty list, `DescribeAssessment` the modelled `ResourceNotFoundException`)". `settle()`/`apply_changes()` never insert an assessment.
Tests: `tests/all/assessments.rs` lines ~659-665 assert the list stays empty after a full change-set lifecycle, and line ~675 `emulator_holding(..)` seeds `store.assessments` directly because no request can create one.

## Steps to Reproduce

1. POST /StartChangeSet with Catalog=AWSMarketplace and a change that creates/updates an AmiProduct (Intent=APPLY); wait for DescribeChangeSet to report SUCCEEDED
2. POST /ListAssessments with {"Catalog":"AWSMarketplace"} and observe {"AssessmentSummaryList":[]}
3. POST /ListAssessments with AssessmentTargetFilter={"EntityId":"<the created entity>"} — still empty
4. POST /DescribeAssessment for any assessment id and observe 404 ResourceNotFoundException: the assessment surface can never report a result
