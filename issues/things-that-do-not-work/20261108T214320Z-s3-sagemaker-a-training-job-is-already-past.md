Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__sagemaker.cli-sagemaker-describe-training-job-status-transitions
Issue-Key: s3__sagemaker.cli-sagemaker-describe-training-job-status-transitions|run#18|AssertionError|assert saw_pre_artifact_absence, "never observed a pre-upload poll to correlate against (job finished too fast)"
Issue-Commit: 622f38417b8d75b0cfd881842d16d49c14a6c801

# s3__sagemaker: a training job is already past the artifact-upload phase on the very first DescribeTrainingJob — the Starting/Downloading/Training phases never exist, so SecondaryStatus can never be correlated with the artifact's appearance in S3

> _Note: this issue was identified against the code base at commit `622f38417b8d75b0cfd881842d16d49c14a6c801`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTrainingJob

## Description
One net-new root cause, grounded in one failing check that the replay report marks neither ALREADY FILED nor OWNED.

Related open issue, which does NOT cover it: 20261108T162008Z-s3-sagemaker-8-net-new-causes-sagemaker-job.md (its Cause 6 describes this family), but its replay key for this check is `s3__sagemaker.cli-sagemaker-describe-training-job-status-transitions|run#16|AssertionError|assert keys == []` — the artifact-appeared-while-Starting assertion. The emulator now fails one assertion later and in the opposite direction: no pre-upload poll is observable at all, so the report reports this check as unowned.

=== Cause 1 of 1: an accepted CreateTrainingJob settles terminal before the first DescribeTrainingJob, reporting a post-upload SecondaryStatus with no InProgress pre-upload phase ===
AWS: Smithy (services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json) defines SecondaryStatus with the in-progress phases a training job passes through and documents them on SecondaryStatusTransition.Status: "Starting - Starting the training job", "Downloading - An optional stage for algorithms that support File training input mode… data is being downloaded to the ML storage volumes", "Training - Training is in progress", "Uploading - Training is complete and the model artifacts are being uploaded to the S3 location", then "Completed". Each phase is recorded as an element of DescribeTrainingJob.SecondaryStatusTransitions with its own StartTime/EndTime. A job therefore spends observable time with TrainingJobStatus=InProgress and SecondaryStatus in {Starting, Downloading, Training} BEFORE Uploading, and the model artifact under OutputDataConfig.S3OutputPath/<job-name>/output/ does not exist until the Uploading phase (recorded behavior for the artifact's visibility).
Emulator: the first `aws sagemaker describe-training-job --training-job-name <job>` issued immediately after CreateTrainingJob already reports a SecondaryStatus in the post-artifact set {Uploading, Completed, MaxRuntimeExceeded, Failed, Stopped} and TrainingJobStatus reaches Completed in that same first poll, so a poll loop never once sees a pre-upload phase: `AssertionError: never observed a pre-upload poll to correlate against (job finished too fast)`. The job is reported as finished without ever having been InProgress in a pre-upload phase, and SecondaryStatusTransitions therefore records no Starting->Downloading->Training->Uploading progression.
Why: a client that polls SecondaryStatus to decide when the model artifact exists (download/package/deploy chains), that renders the phase timeline from SecondaryStatusTransitions, or that asserts an in-progress phase before completion, gets a job whose entire lifecycle is one terminal observation. Ordering guarantees real clients depend on (artifact not readable before Uploading; job observable as InProgress) cannot be exercised at all.
Fix: the SageMaker training-job state machine and the DescribeTrainingJob projection of SecondaryStatus/SecondaryStatusTransitions — an accepted job must become InProgress and pass through the documented pre-upload phases observably, publishing the artifact to the S3 output path no earlier than the Uploading phase, with one SecondaryStatusTransitions element per phase.
Traces: s3__sagemaker.cli-sagemaker-describe-training-job-status-transitions.

## Evidence
s3__sagemaker.cli-sagemaker-describe-training-job-status-transitions (mismatch). Job names, buckets and prefixes below are per-run generated values.

The check creates a data bucket and an execution role with S3 access, calls `aws sagemaker create-training-job` with the region's built-in XGBoost image, then polls `aws sagemaker describe-training-job` every 2 s. On every poll whose SecondaryStatus is NOT in {Uploading, Completed, MaxRuntimeExceeded, Failed, Stopped} it lists <output-prefix>/<job-name>/ and requires the listing to be empty (recording that a pre-upload observation happened); it breaks when TrainingJobStatus is terminal.

Expected: at least one poll with SecondaryStatus in {Starting, Downloading, Training} and an empty output prefix, then TrainingJobStatus=Completed with model.tar.gz present.
Actual:
  File "check_s3__sagemaker_cli_sagemaker_describe_training_job_status_transitions.py", line 89, in run
    assert saw_pre_artifact_absence, "never observed a pre-upload poll to correlate against (job finished too fast)"
  AssertionError: never observed a pre-upload poll to correlate against (job finished too fast)
The preceding assertion (`described["TrainingJobStatus"] == "Completed"`) passed, so the job did report Completed — it reported a post-upload SecondaryStatus on the first describe, with no InProgress pre-upload phase in between.

## Steps to Reproduce
1. Clone the repo and boot the emulator (run_behavioral_suite).
2. Create an S3 bucket with a training CSV under a train/ prefix and a SageMaker execution role trusting sagemaker.amazonaws.com with read/write on that bucket.
3. `aws sagemaker create-training-job` with the region's built-in XGBoost image (e.g. 246618743249.dkr.ecr.us-west-2.amazonaws.com/sagemaker-xgboost:1.5-1), an S3Prefix input channel, and OutputDataConfig.S3OutputPath s3://<bucket>/output-root.
4. Immediately poll `aws sagemaker describe-training-job --training-job-name <job>` every 2 s, recording TrainingJobStatus, SecondaryStatus and ListObjectsV2 on output-root/<job>/.
5. On AWS the first polls show TrainingJobStatus=InProgress with SecondaryStatus Starting/Downloading/Training and an empty output prefix, and SecondaryStatusTransitions accumulates those phases before Uploading/Completed. The emulator's first poll already reports a post-upload SecondaryStatus and TrainingJobStatus=Completed, so no pre-upload phase is ever observable.
