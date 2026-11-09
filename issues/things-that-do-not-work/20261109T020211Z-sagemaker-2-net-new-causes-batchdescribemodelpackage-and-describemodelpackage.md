Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.batch-describe-model-package-matches-individual-creates sagemaker.create-model-package-client-token-replay-no-second-version
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker.batch-describe-model-package-matches-individual-creates|run#16|AssertionError|assert summary["ModelPackageStatus"] == exp["ModelPackageStatus"], (
Issue-Key: sagemaker.create-model-package-client-token-replay-no-second-version|_assert_reuse_refused#16|AssertionError|assert message == _reuse_message(named_group), (
Issue-Commit: 81c00d0cdf8c8d837a6edaf703850a5ccd4fe73e

# sagemaker: 2 net-new causes — BatchDescribeModelPackage and DescribeModelPackage disagree on ModelPackageStatus for the same package, and the spent-ClientToken refusal is reworded to 'in the last 8 hours'

> _Note: this issue was identified against the code base at commit `81c00d0cdf8c8d837a6edaf703850a5ccd4fe73e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchDescribeModelPackage

## Description
Two net-new behavioral root causes on the SageMaker model-package surface. Neither failing check is marked ALREADY FILED or OWNED by the replay, so no open issue carries their replay keys.

Cause 1 of 2: BatchDescribeModelPackage and DescribeModelPackage report different ModelPackageStatus values for the same model package at the same instant.
AWS: both reads project the one stored status of the package. Smithy: BatchDescribeModelPackage input ModelPackageArnList (required, @length 1..100), output ModelPackageSummaries (map keyed by model-package ARN) + BatchDescribeModelPackageErrorMap; ModelPackageSummary.ModelPackageStatus is required and targets the ModelPackageStatus enum [Pending, InProgress, Completed, Failed, Deleting]; DescribeModelPackage returns the same ModelPackageStatus for the same resource. The model enumerates the states but does not say which is terminal; that a created package settles to 'Completed' and that the batch and single reads always agree is recorded behavior only. The consistency requirement is unconditional: the two operations read one field of one resource, so whatever the status is, both must report it.
Emulator: immediately after CreateModelPackage, DescribeModelPackage(ModelPackageName=<arn>) reports ModelPackageStatus 'Pending' while BatchDescribeModelPackage(ModelPackageArnList=[<arn>]) reports 'Completed' for that very ARN. Status 200 and every other projected member (ModelPackageGroupName, ModelPackageVersion, ModelPackageDescription, ModelApprovalStatus, InferenceSpecification.Containers[0].Image) agree; only the status differs, i.e. the batch projection does not read the stored status the single describe serves — and the stored value never advances off its initial 'Pending'.
Why: a client that registers a package and then polls for readiness gets a different answer depending on which API it polls: a DescribeModelPackage poller waits forever on 'Pending' (or treats the package as not-ready/failed and refuses to approve or deploy it), while a BatchDescribeModelPackage poller is told 'Completed' for a package whose stored state says otherwise. Code that cross-checks the two reads (a reconciler, a drift detector) sees phantom disagreement and flaps.
Fix: the model-package status machine behind CreateModelPackage must advance the single stored status to the terminal value once registration completes, and BatchDescribeModelPackage's output projection must read that stored field rather than substituting a constant, so the batch and single reads are always equal. Related open issues describe the same status field from the other direction but do NOT own this check: 20261107T174830Z-sagemaker-batchdescribemodelpackage-reports-a-created-model-package-as.md (Batch serving 'Pending' where AWS serves 'Completed') and 20261102T105242Z-sagemaker-model-package-and-algorithm-image-digest-is.md (ModelPackageStatus never settles).
Traces: sagemaker.batch-describe-model-package-matches-individual-creates.

Cause 2 of 2: the CreateModelPackage spent-ClientToken refusal message is emulator-worded.
AWS: CreateModelPackage carries an idempotency token (Smithy: ClientToken, @idempotencyToken, @length 1..36, pattern ^[a-zA-Z0-9-]+$) and refuses a reuse of a spent token with ConflictException (Smithy: CreateModelPackage errors ConflictException, ResourceLimitExceeded; httpError 400), whose Message is the fixed sentence 'Specified idempotent client token in Model package with Group:<model-package-group-name> was used with different request parameters within the idempotency window.' The fixed text is every word except <model-package-group-name>, which is the ModelPackageGroupName the request carried.
Emulator: the refusal arrives, and the leading clause and the interpolated group name are right, but the message ends '... was used with different request parameters in the last 8 hours.' instead of '... within the idempotency window.' — the emulator substitutes a concrete window duration for AWS's fixed wording.
Why: a client that matches this refusal (to distinguish a token collision from a parameter conflict and decide whether to mint a fresh token or fail) does not match, and any human-facing surface asserts a window length AWS does not state in the message, so an operator reasons about an 8-hour window the service never promised.
Fix: the CreateModelPackage handler's idempotency-conflict path must emit AWS's fixed sentence, interpolating only the request's ModelPackageGroupName.
Traces: sagemaker.create-model-package-client-token-replay-no-second-version.

## Evidence
Cause 1 — check sagemaker.batch-describe-model-package-matches-individual-creates, [mismatch]:
  assert summary["ModelPackageStatus"] == exp["ModelPackageStatus"]
  AssertionError: arn:aws:sagemaker:us-west-2:000000000000:model-package/<group>/1: expected ModelPackageStatus 'Pending', got 'Completed'
where the expected value is what DescribeModelPackage(ModelPackageName=<same arn>) returned moments earlier in the same check, and the observed value is the one BatchDescribeModelPackage put in ModelPackageSummaries[<arn>]. The ARN (account, region, group name, version suffix) is a per-run value; the requirement is that the two operations report the same enum value for one package, and that a finished registration reports the terminal 'Completed' on both.

Cause 2 — check sagemaker.create-model-package-client-token-replay-no-second-version, [mismatch]:
  assert message == _reuse_message(named_group)
  expected: 'Specified idempotent client token in Model package with Group:<group> was used with different request parameters within the idempotency window.'
  got:      'Specified idempotent client token in Model package with Group:<group> was used with different request parameters in the last 8 hours.'
The refusal itself (and the group name interpolated into it) matched; only the trailing fixed clause diverges. <group> is the ModelPackageGroupName from the request, not part of the template.

Smithy consulted (services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json): BatchDescribeModelPackage input ModelPackageArnList required @length 1..100, output ModelPackageSummaries + BatchDescribeModelPackageErrorMap, ModelPackageSummary.ModelPackageStatus required -> enum [Pending, InProgress, Completed, Failed, Deleting]; CreateModelPackage ClientToken @idempotencyToken @length 1..36 pattern ^[a-zA-Z0-9-]+$, errors ConflictException (400) and ResourceLimitExceeded (400).

## Steps to Reproduce
Replay the behavioral suite (run_behavioral_suite) and re-run the two checks listed in trace_ids.

Cause 1 manually: CreateModelPackageGroup, push a minimal image to an ECR repository, then CreateModelPackage into the group with InferenceSpecification.Containers=[{Image: <that image>}] and ModelApprovalStatus=PendingManualApproval; capture ModelPackageArn. Call DescribeModelPackage(ModelPackageName=<arn>) and BatchDescribeModelPackage(ModelPackageArnList=[<arn>]) back to back and compare ModelPackageStatus. AWS returns the same value from both (settling to 'Completed'); the emulator returns 'Pending' from the describe and 'Completed' from the batch.

Cause 2 manually: CreateModelPackage into a named group with an explicit ClientToken (and SkipModelValidation='All'), then replay the byte-identical request with the same ClientToken and read the error Message. AWS ends the sentence with 'within the idempotency window.'; the emulator ends it with 'in the last 8 hours.'
