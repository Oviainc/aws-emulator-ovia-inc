Issue-Source: trace-sweep-divergence
Issue-Trace: transfer-as2-agreement-lifecycle
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transfer-as2-agreement-lifecycle transfer-as2-certificate-and-profile-lifecycle transfer-resource-tagging-across-types transfer-workflow-heterogeneous-step-order-and-defaults
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer|DescribeAgreement|2xx->2xx|$.Agreement.Tags
Issue-Key: transfer|DescribeProfile|2xx->2xx|$.Profile.CertificateIds
Issue-Key: transfer|DescribeProfile|2xx->2xx|$.Profile.Tags
Issue-Key: transfer|ListTagsForResource|2xx->2xx|$.Tags
Issue-Key: transfer|DescribeWorkflow|2xx->2xx|$.Workflow.OnExceptionSteps[*].__type
Issue-Key: transfer|DescribeWorkflow|2xx->2xx|$.Workflow.Steps[*].__type
Issue-Key: transfer|DescribeWorkflow|2xx->2xx|$.Workflow.Tags
Issue-Commit: 3b929b447a52a39dfc36e4a4cbd93099ed5ef8cf

# Transfer reads omit empty collection members and the WorkflowStep type discriminator

> _Note: this issue was identified against the code base at commit `3b929b447a52a39dfc36e4a4cbd93099ed5ef8cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAgreement

## Description
Four net-new divergences share one serializer fault: when a modelled collection member is EMPTY the emulator omits it from the JSON body, where the recorded service serialises it as an empty array; the workflow read additionally omits the per-step type discriminator the service emits. FOUR causes must be closed before submitting.

Cause 1 of 4 — DescribeAgreement omits Agreement.Tags when the agreement has no tags.
AWS: Smithy: DescribeAgreement -> DescribedAgreement{..., Tags -> Tags @length min 1 max 50}. The @length constraint applies to a tag list a caller SUPPLIES; in the response the service emits Tags as an empty array for an untagged agreement (recorded behavior: the recorded body carries "Tags": []).
Emulator: the member is absent from the body entirely.
Why: a client that reads Tags unconditionally (len(resp['Tags']) == 0, iterating the list) raises a KeyError / NoneType error instead of seeing no tags, so the distinction between 'no tags' and 'field not returned' is destroyed on every untagged agreement.
Fix: the DescribedAgreement serializer — serialise present-but-empty collections as [] rather than suppressing them.
Traces: transfer-as2-agreement-lifecycle#task#6 (the same omission also rides along on #task#8 of that fixture, whose primary cause is filed separately).

Cause 2 of 4 — DescribeProfile omits both Profile.CertificateIds and Profile.Tags when empty.
AWS: Smithy: DescribeProfile -> DescribedProfile{..., CertificateIds -> CertificateIds list, Tags -> Tags @length 1..50}. The recorded body for a profile created with neither certificates nor tags carries "CertificateIds": [] and "Tags": [].
Emulator: both members are absent.
Why: the same unconditional-read breakage, and here it is worse than cosmetic: CertificateIds is the member a caller inspects to decide whether a partner profile still has certificates attached, so an absent member is indistinguishable from an unsupported field.
Fix: the DescribedProfile serializer (same empty-collection rule).
Traces: transfer-as2-certificate-and-profile-lifecycle#task#8 (cluster also covers #task#11 of that fixture).

Cause 3 of 4 — ListTagsForResource omits Tags for an untagged resource.
AWS: Smithy: ListTagsForResource -> {Arn, NextToken, Tags -> Tags @length 1..50}; the operation is @paginated with items 'Tags'. The recorded body for a resource with no tags carries "Tags": [].
Emulator: the member is absent.
Why: Tags is the paginated result member; SDK paginators and the CLI build their result list from it, so an absent member makes an untagged resource return a differently shaped page than every other call, and callers that concatenate pages fail on the first untagged resource.
Fix: the ListTagsForResource serializer — always emit the paginated items member, empty when there is nothing to list.
Traces: transfer-resource-tagging-across-types#task#11.

Cause 4 of 4 — DescribeWorkflow omits every step's __type discriminator and omits Workflow.Tags.
AWS: Smithy: DescribeWorkflow -> DescribedWorkflow{Steps, OnExceptionSteps -> WorkflowSteps @length 0..8, Tags -> Tags @length 1..50}. Recorded behavior: the service emits each element of Steps and OnExceptionSteps with a "__type": "WorkflowStep" member alongside the step's own members, and emits "Tags": [] for an untagged workflow.
Emulator: no step carries __type (each of the three Steps entries and the single OnExceptionSteps entry lacks it) and Tags is absent.
Why: a client or recorded-response comparison that keys on __type to identify step elements sees untyped objects; combined with the missing Tags this makes a workflow read structurally different from the service's, so round-tripping a described workflow back into a create/update call drops information.
Fix: the DescribedWorkflow serializer — emit the per-element __type for WorkflowStep elements in both Steps and OnExceptionSteps, and emit empty Tags as [].
Traces: transfer-workflow-heterogeneous-step-order-and-defaults#task#2.

## Evidence
From replay of simparity-wire-aws-v1. All four steps answered HTTP 200 on both sides (expected 200, actual 200); the divergences are missing body members. Expected = recorded AWS response, Actual = emulator.

1. DescribeAgreement / transfer-as2-agreement-lifecycle#task#6 — '$.Agreement.Tags: missing (expected [], got None)'.
2. DescribeProfile / transfer-as2-certificate-and-profile-lifecycle#task#8 (cluster also covers #task#11) — '$.Profile.CertificateIds: missing (expected [], got None)' and '$.Profile.Tags: missing (expected [], got None)'.
3. ListTagsForResource / transfer-resource-tagging-across-types#task#11 — '$.Tags: missing (expected [], got None)'.
4. DescribeWorkflow / transfer-workflow-heterogeneous-step-order-and-defaults#task#2 — '$.Workflow.Steps[0].__type', '$.Workflow.Steps[1].__type', '$.Workflow.Steps[2].__type' and '$.Workflow.OnExceptionSteps[0].__type' all missing (expected 'WorkflowStep', got None); '$.Workflow.Tags: missing (expected [], got None)'.

Model grounding (services/service-com-amazonaws-transfer-transferservice/smithy-model/com-amazonaws-transfer-transferservice.json): DescribedAgreement.Tags, DescribedProfile.{CertificateIds,Tags}, DescribedWorkflow.{Steps,OnExceptionSteps,Tags} and ListTagsForResourceResponse.Tags are all modelled collection members; ListTagsForResource is @paginated with items=Tags. The empty-array-rather-than-omitted rendering and the per-step __type member are recorded behavior only — the model states neither.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Re-replay the four bound steps and diff the JSON bodies against the recordings:
   - transfer-as2-agreement-lifecycle#task#6 — DescribeAgreement on an untagged agreement; assert the body contains Tags == [].
   - transfer-as2-certificate-and-profile-lifecycle#task#8 — DescribeProfile on a profile with no certificates and no tags; assert CertificateIds == [] and Tags == [].
   - transfer-resource-tagging-across-types#task#11 — ListTagsForResource on an untagged resource ARN; assert Tags == [].
   - transfer-workflow-heterogeneous-step-order-and-defaults#task#2 — DescribeWorkflow; assert every Steps/OnExceptionSteps element carries __type == 'WorkflowStep' and that Tags == [].
