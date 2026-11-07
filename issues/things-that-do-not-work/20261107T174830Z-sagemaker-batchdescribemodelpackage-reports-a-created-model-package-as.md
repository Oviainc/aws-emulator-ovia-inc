Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-model-package-unicode-description-round-trip
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-model-package-unicode-description-round-trip
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|BatchDescribeModelPackage|2xx->2xx|$.ModelPackageSummaries.arn:aws:sagemaker:us-west-2:000000000000:model-package/sim2real-mpg-f3375f31-09ce-469b-ab1d-bc11d969eb30/1.ModelPackageStatus
Issue-Commit: b82edd9f383ff8a9220a50cbdb5a6cdc19307e8f

# SageMaker BatchDescribeModelPackage reports a created model package as Pending instead of Completed

> _Note: this issue was identified against the code base at commit `b82edd9f383ff8a9220a50cbdb5a6cdc19307e8f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchDescribeModelPackage

## Description
One net-new wire divergence on the SageMaker model-package batch-read path. The step answers 200 with the right shape, but the per-ARN summary carries a stored status that never settles.

Related open issue, which does NOT own this cluster (the replay reports it unowned): 'SageMaker model-package and algorithm image digest is regenerated, and ModelPackageStatus never settles' (20261102T105242Z-sagemaker-model-package-and-algorithm-image-digest-is.md). Its prose describes the same stuck-status fault as its cause 4, but its replay pointer lists fixture ids with no step key, so this fixture#step binding is not covered by it and a fixer replaying that issue never exercises this step. Filing it here so the binding exists.

Cause 1 of 1: BatchDescribeModelPackage returns ModelPackageStatus 'Pending' for a model package that creation has finished for.
AWS: BatchDescribeModelPackage returns a ModelPackageSummaries map keyed by model-package ARN, and each ModelPackageSummary carries a required ModelPackageStatus (Smithy: ModelPackageSummary.ModelPackageStatus -> ModelPackageStatus, required; enum values [Pending, InProgress, Completed, Failed, Deleting]). CreateModelPackage (Smithy: CreateModelPackage, output ModelPackageArn; errors ConflictException, ResourceLimitExceeded) registers the package, and recorded behavior shows that a package created with no outstanding validation work settles to 'Completed', so the batch summary reports 'Completed'. The model enumerates the states but does not itself say which is terminal; that a successfully created package reaches 'Completed' is recorded behavior only.
Emulator: the summary for the created package reports ModelPackageStatus 'Pending' and keeps reporting it for the life of the resource — the initial state value is written at create and never advanced. Status 200 and the rest of the summary match, so this is purely the stored status.
Why: a client that batch-polls model packages and waits for 'Completed' before approving, deploying or attaching them to an endpoint config polls forever; a client that treats any non-'Completed' status as not-yet-ready, or as failed, rejects a package that AWS considers good. Because BatchDescribeModelPackage reads the same stored field as DescribeModelPackage, both the single and batch pollers hang.
Fix: the model-package status machine behind CreateModelPackage must advance the package's stored status to 'Completed' once creation completes (instead of leaving the initial 'Pending'); BatchDescribeModelPackage's output projection should keep reading the stored value, not a constant.
Traces: sagemaker-model-package-unicode-description-round-trip#task#3.

## Evidence
Cause 1 — fixture sagemaker-model-package-unicode-description-round-trip, step task#3, BatchDescribeModelPackage, expected status 200 and observed status 200.

Divergence detail reported by the replay:
  $.ModelPackageSummaries.<model-package-arn>.ModelPackageStatus: value_mismatch (expected 'Completed', got 'Pending')

The diverging key in the recording is $.ModelPackageSummaries.arn:aws:sagemaker:us-west-2:000000000000:model-package/sim2real-mpg-f3375f31-09ce-469b-ab1d-bc11d969eb30/1.ModelPackageStatus. Of that path, the fixed part is the member path ModelPackageSummaries.<key>.ModelPackageStatus; the ARN key itself (account id, region, group name, version suffix) is a per-run value taken from the request and the emulator's state and is not part of the requirement. The requirement is the enum value: 'Completed' for a package whose creation has finished, where the emulator serves 'Pending'.

Divergence key: service=sagemaker|BatchDescribeModelPackage|status=200->200|keys=$.ModelPackageSummaries.<arn>.ModelPackageStatus. affected_count 1; stub_path empty and owned_by empty, i.e. no stub and no open issue carries this replay key.

Smithy consulted (services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json): BatchDescribeModelPackage input ModelPackageArnList (required, @length 1..100), output ModelPackageSummaries map + BatchDescribeModelPackageErrorMap; ModelPackageSummary.ModelPackageStatus required, target ModelPackageStatus enum = [Pending, InProgress, Completed, Failed, Deleting].

## Steps to Reproduce
Replay the wire corpus against the emulator (replay_wire_corpus). Representative and only step: fixture sagemaker-model-package-unicode-description-round-trip, step task#3.

Manually: create a model package group, then CreateModelPackage into it (with a unicode ModelPackageDescription, as the fixture does — the description is incidental), capture the returned ModelPackageArn, then call BatchDescribeModelPackage with ModelPackageArnList=[that ARN] and read ModelPackageSummaries[<arn>].ModelPackageStatus. AWS reports 'Completed'; the emulator reports 'Pending' on this and every subsequent call.
