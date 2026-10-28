Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codebuild.delete-build-batch-in-progress-rejected
Issue-Shape-Id: com.amazonaws.codebuild#CodeBuild_20161006
Issue-Key: codebuild.delete-build-batch-in-progress-rejected|run#14|AssertionError|assert message == f"Cannot delete BuildBatch arn:aws:codebuild:{ctx.region}:{ctx.account}:build-batch/{batch_id}, as it is still in progress", (
Issue-Commit: b0e56fd651260a31352d5eb473a95bc0dbdec9ab

# CodeBuild DeleteBuildBatch: the IN_PROGRESS refusal message is rewritten — it names the batch by bare id instead of AWS's "Cannot delete BuildBatch <batch-arn>, as it is still in progress"

> _Note: this issue was identified against the code base at commit `b0e56fd651260a31352d5eb473a95bc0dbdec9ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteBuildBatch

## Description
ONE net-new root cause in the `codebuild` behavioral suite, in the CodeBuild service crate services/service-com-amazonaws-codebuild-codebuild-20161006. Every other failing codebuild / codebuild__* / cloudwatch__codebuild check in this replay was marked OWNED by an existing open issue or ALREADY FILED and is excluded here.

Cause 1 of 1: DeleteBuildBatch now correctly REFUSES a still-IN_PROGRESS batch with the modeled error code, but emits its own refusal sentence instead of AWS's.

AWS: Smithy DeleteBuildBatch(id @required, NonEmptyString @length min 1) -> { statusCode, buildsDeleted, buildsNotDeleted }; the single modeled error is InvalidInputException (errorKind client, httpError 400). Deleting a build batch that has not yet completed is refused with 400 InvalidInputException and the message template
  `Cannot delete BuildBatch <build-batch-arn>, as it is still in progress`
The fixed text is `Cannot delete BuildBatch ` ... `, as it is still in progress`. The only variable part is <build-batch-arn>, which is the batch's FULL ARN as the service constructs it — `arn:aws:codebuild:<region>:<account-id>:build-batch/<batch-id>` — with region and account taken from the caller's request/credentials and <batch-id> (the `<project-name>:<uuid>` form) taken from the request's `id`. The batch is left intact and still IN_PROGRESS, and the same batch deletes cleanly once it is actually stopped and complete (the check's positive control).

Emulator: the status and error code are right (400 InvalidInputException), but the message is a different sentence that names the batch by its bare id and never builds the ARN: `Build batch <batch-id> is still in progress and cannot be deleted`. Concretely, where AWS would say `Cannot delete BuildBatch arn:aws:codebuild:<region>:<account>:build-batch/<project>:<uuid>, as it is still in progress`, the emulator answered `Build batch <project>:<uuid> is still in progress and cannot be deleted` — no `Cannot delete BuildBatch ` prefix, no `, as it is still in progress` suffix, and the identifier is the raw id rather than the resource ARN.

Why: InvalidInputException is DeleteBuildBatch's ONLY modeled error, so the code alone cannot tell a client which precondition failed — in-progress batch, malformed id, or unknown id all arrive as the same 400 InvalidInputException. Clients and teardown/cleanup scripts therefore discriminate on the message text, and tooling that scrapes the ARN out of the refusal to re-target a StopBuildBatch or to log the offending resource gets nothing usable, because the emulator's message carries no ARN at all. Anything string-matching AWS's wording treats the refusal as an unrecognised failure and either aborts the teardown or retries the delete forever.

Fix: the in-progress precondition's error construction in the DeleteBuildBatch handler (services/service-com-amazonaws-codebuild-codebuild-20161006/src/ops/delete_build_batch.rs) must emit AWS's sentence and format the batch's full ARN from region, account and the batch id rather than interpolating the bare id into a locally-worded message. The code (InvalidInputException) and the leave-the-batch-intact effect are already correct and must not change.

Traces: codebuild.delete-build-batch-in-progress-rejected. Note that open issue 20261025T014438Z-codebuild-8-further-divergences-getresourcepolicy-updatewebhook-deletebuildbatch-stubs.md lists this same check id under its Cause 4, but its recorded replay key for it is the NotImplemented stub failure (`assert code == "InvalidInputException"`); that part now passes and the failure has moved to the message assertion, which no open issue covers.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1 against the emulator built from this checkout: 47/106 passed, 53 mismatched, 6 errored.

[mismatch] codebuild.delete-build-batch-in-progress-rejected
  check_codebuild_delete_build_batch_in_progress_rejected.py, line 65, in run
    assert message == f"Cannot delete BuildBatch arn:aws:codebuild:{ctx.region}:{ctx.account}:build-batch/{batch_id}, as it is still in progress"
  AssertionError: expected the exact InvalidInputException message AWS returns, got 'Build batch sim2real-cbbatchneg-7aa4009f8169:c2600cc7-f412-8aee-b311-7f2989efd1f6 is still in progress and cannot be deleted'

The preceding assertion in the same check — `assert code == "InvalidInputException"` — PASSED, so the divergence is isolated to the message text, not to the status or the error code.

Message breakdown (template, not one run's values):
  Expected fixed text : "Cannot delete BuildBatch " + <arn> + ", as it is still in progress"
  Expected variable   : <arn> = arn:aws:codebuild:<region>:<account-id>:build-batch/<batch-id>, where <region>/<account-id> come from the caller and <batch-id> is the request's `id` member (shape <project-name>:<uuid>).
  Emulator fixed text : "Build batch " + <batch-id> + " is still in progress and cannot be deleted" — wrong prefix, wrong suffix, and the resource is identified by bare id with no ARN.
The project name and uuid in the quoted detail above are this run's generated values filling <batch-id>; they are not part of the requirement.

Smithy model consulted: services/service-com-amazonaws-codebuild-codebuild-20161006/smithy-model/com-amazonaws-codebuild-codebuild-20161006.json — DeleteBuildBatch input {id: NonEmptyString, required, length min 1}, output {statusCode, buildsDeleted, buildsNotDeleted}, errors [InvalidInputException (client, httpError 400)].

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Create an IAM role assumable by codebuild.amazonaws.com.
3. CreateProject with source {type: NO_SOURCE, buildspec: a version 0.2 buildspec carrying a `batch:` section with a single `build-list` identifier}, artifacts {type: NO_ARTIFACTS}, environment {LINUX_CONTAINER / BUILD_GENERAL1_SMALL}, serviceRole=<role arn>, and buildBatchConfig {serviceRole: <role arn>, combineArtifacts: false}.
4. StartBuildBatch(projectName=<project>) and take buildBatch.id; poll BatchGetBuildBatches(ids=[<id>]) until buildBatchStatus == IN_PROGRESS.
5. Call DeleteBuildBatch(id=<id>) while the batch is still IN_PROGRESS and read Error.Code and Error.Message off the ClientError.
   Expect: 400 InvalidInputException with message "Cannot delete BuildBatch arn:aws:codebuild:<region>:<account>:build-batch/<id>, as it is still in progress".
   Observe: 400 InvalidInputException with message "Build batch <id> is still in progress and cannot be deleted".
6. Confirm the rest of the check's expectations still hold (they do): BatchGetBuildBatches still returns the batch as IN_PROGRESS, ListBuildBatches still lists its id, and after StopBuildBatch + the batch reporting complete=true, DeleteBuildBatch succeeds and the id leaves ListBuildBatches.
7. Or simply replay the suite (run_behavioral_suite) and read the `codebuild` section for codebuild.delete-build-batch-in-progress-rejected.
