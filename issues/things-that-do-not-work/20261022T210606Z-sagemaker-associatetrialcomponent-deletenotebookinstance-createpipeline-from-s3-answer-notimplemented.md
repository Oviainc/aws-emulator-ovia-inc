Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.associate-trial-component-nonexistent-trial-rejected sagemaker.associate-trial-component-not-found sagemaker.notebook-instance-delete-while-inservice-rejected sagemaker.create-pipeline-missing-dependency-not-found sagemaker.create-hub-content-reference-nonexistent-hub-rejected sagemaker.create-hub-content-reference-permission-denied s3__sagemaker.cli-sagemaker-batch-transform-s3-workflow s3__sagemaker.sagemaker-batch-transform-writes-output-s3-positive sagemaker.model-package-empty-vs-absent-fields-preserved sagemaker.model-package-version-monotonic-and-server-assigned sagemaker.trial-component-timestamp-precision-preserved
Issue-Key: sagemaker.associate-trial-component-nonexistent-trial-rejected|run#8|AssertionError|assert code == "ResourceNotFound", f"expected ResourceNotFound, got {code}"
Issue-Key: sagemaker.associate-trial-component-not-found|run#8|AssertionError|assert code == "ResourceNotFound", f"expected ResourceNotFound, got {code}"
Issue-Key: sagemaker.notebook-instance-delete-while-inservice-rejected|run#12|AssertionError|assert code == "ValidationException", (
Issue-Key: sagemaker.create-pipeline-missing-dependency-not-found|run#11|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}: {message}"
Issue-Key: sagemaker|ListHubContents|ResourceNotFound
Issue-Key: s3__sagemaker.cli-sagemaker-batch-transform-s3-workflow|run#20|AssertionError|assert result["TransformJobStatus"] == "Completed", (
Issue-Key: s3__sagemaker.sagemaker-batch-transform-writes-output-s3-positive|run#17|AssertionError|assert desc["TransformJobStatus"] == "Completed", desc.get("FailureReason")
Issue-Key: sagemaker.model-package-empty-vs-absent-fields-preserved|run#11|AssertionError|assert "ModelPackageDescription" not in empty_direct, (
Issue-Key: sagemaker.model-package-version-monotonic-and-server-assigned|run#7|AssertionError|assert "ModelPackageName" not in desc, (
Issue-Key: sagemaker.trial-component-timestamp-precision-preserved|run#12|AssertionError|assert got_start == expected_start, (
Issue-Commit: f973f78f74c99aad4e85f2736f1fde89ed7327e3

# SageMaker: AssociateTrialComponent / DeleteNotebookInstance / CreatePipeline-from-S3 answer NotImplemented, the managed SageMakerPublicHub does not exist, transform jobs write no output, and three response fields are not normalized (8 causes)

> _Note: this issue was identified against the code base at commit `f973f78f74c99aad4e85f2736f1fde89ed7327e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateTrialComponent

## Description
Eight independent gaps that no already-filed stub issue names. Each is its own root cause and its own work item; this issue bundles EIGHT causes and all eight must be closed before submitting. (The report's own stub issues name other operations - CreateTrainingJob, CreateCluster, CreateFeatureGroup, PutModelPackageGroupPolicy, DeleteModelPackageGroup, StopNotebookInstance, StartPipelineExecution and so on - none of the three operations below.)

Cause 1 of 8: AssociateTrialComponent is a scaffolded stub.
AWS: Smithy: AssociateTrialComponent requires TrialComponentName and TrialName (both ExperimentEntityName, @length 1..120, @pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}$), returns TrialComponentArn + TrialArn, and models ResourceNotFound and ResourceLimitExceeded (HTTP 400). A TrialName that does not exist is rejected 400 ResourceNotFound.
Emulator: answers the scaffold error code NotImplemented for every call.
Why: trial components cannot be attached to a trial at all, and a client branching on ResourceNotFound sees an unmodeled code SDK retry logic treats as a server fault.
Fix: the operation's handler services/service-com-amazonaws-sagemaker-sagemaker/src/ops/associate_trial_component.rs plus trial/trial-component lookup in state.
Traces: sagemaker.associate-trial-component-nonexistent-trial-rejected, sagemaker.associate-trial-component-not-found.

Cause 2 of 8: DeleteNotebookInstance is a scaffolded stub.
AWS: Smithy: DeleteNotebookInstance requires NotebookInstanceName and declares no modeled errors; recorded AWS refuses the delete of an InService instance with 400 ValidationException (it must be Stopped first) and leaves the instance untouched, while the Stopped->delete path succeeds.
Emulator: NotImplemented for every call, so neither the refusal nor the delete happens.
Why: the notebook-instance state machine has no exit - instances cannot be removed, cleanup leaks them, and the "stop before delete" guard is unobservable.
Fix: services/service-com-amazonaws-sagemaker-sagemaker/src/ops/delete_notebook_instance.rs plus the status guard and notebook-instance state removal.
Traces: sagemaker.notebook-instance-delete-while-inservice-rejected.

Cause 3 of 8: CreatePipeline with PipelineDefinitionS3Location answers NotImplemented.
AWS: Smithy: CreatePipeline accepts either PipelineDefinition or PipelineDefinitionS3Location and models ConflictException, ResourceLimitExceeded and ResourceNotFound (400); a definition in S3 whose referenced step dependency is missing is rejected 400 ValidationException.
Emulator: "operation CreatePipeline with PipelineDefinitionS3Location is scaffolded but not implemented in this emulator" (code NotImplemented) - the member is gated off even though the inline-definition path works.
Why: the documented S3-backed definition workflow is unusable and the definition validation it would run never happens.
Fix: CreatePipeline's handler - fetch and validate the definition from PipelineDefinitionS3Location (services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_pipeline.rs).
Traces: sagemaker.create-pipeline-missing-dependency-not-found.

Cause 4 of 8: the AWS-managed public hub does not exist, so hub-content references cannot be made.
AWS: Smithy: ListHubContents takes HubName as name-or-ARN and models ResourceNotFound (400); CreateHubContentReference requires a SageMakerPublicHubContentArn matching ^arn:...:sagemaker:<region>:aws:hub-content/... , i.e. the service hosts an aws-owned public hub. Recorded AWS resolves HubName='SageMakerPublicHub' and returns its content summaries.
Emulator: ListHubContents(HubName='SageMakerPublicHub') is rejected 400 ResourceNotFound ('Could not find hub "arn:aws:sagemaker:<region>:000000000000:hub/SageMakerPublicHub".') - note it also resolves the managed hub into the caller's own account rather than the aws account the ARN pattern mandates.
Why: every JumpStart-style flow that references public hub content is blocked, and the not-found is misleading because the hub is service-managed, not caller-created.
Fix: seed the managed SageMakerPublicHub (under the aws account namespace) in hub state and resolve it in the hub handlers.
Traces: sagemaker.create-hub-content-reference-nonexistent-hub-rejected, sagemaker.create-hub-content-reference-permission-denied.

Cause 5 of 8: transform jobs complete without transforming anything - no output object is written.
AWS: Smithy: CreateTransformJob requires TransformInput/TransformOutput/TransformResources; recorded AWS runs the model container and, when the job reaches Completed, an output object exists under TransformOutput.S3OutputPath for the input records.
Emulator: the job is accepted but no records are transformed and no object appears under S3OutputPath, so the job never becomes an observably completed transform.
Why: batch-transform consumers read the output path and find nothing; a pipeline that gates on the transform's result cannot run, and "job completed" carries no meaning.
Fix: the transform-job execution path in SageMaker state - at minimum materialize the output objects (one per input record/file) under S3OutputPath and drive the job status accordingly.
Traces: s3__sagemaker.cli-sagemaker-batch-transform-s3-workflow, s3__sagemaker.sagemaker-batch-transform-writes-output-s3-positive.

Cause 6 of 8: an explicit empty ModelPackageDescription is stored instead of being normalized away.
AWS: Smithy: CreateModelPackage.ModelPackageDescription targets EntityDescription (@length 0..1024); recorded AWS normalizes an explicitly supplied empty string away, so DescribeModelPackage omits ModelPackageDescription entirely (absent, not '').
Emulator: DescribeModelPackage returns ModelPackageDescription=''.
Why: callers that distinguish absent from empty (round-trip and diff tooling, IaC drift detection) see a field AWS never returns.
Fix: input normalization in CreateModelPackage plus the DescribeModelPackage serializer (omit empty optional strings).
Traces: sagemaker.model-package-empty-vs-absent-fields-preserved.

Cause 7 of 8: DescribeModelPackage returns ModelPackageName for a versioned package.
AWS: recorded behavior only - a versioned model package has no independent name, so DescribeModelPackage on version N returns ModelPackageGroupName + ModelPackageVersion and omits ModelPackageName (even though the Smithy output marks it required/clientOptional).
Emulator: returns ModelPackageName=<group name> for version 1.
Why: the response conflates a group name with a package name, so clients keying on ModelPackageName treat a versioned package as an unversioned one.
Fix: the DescribeModelPackage serializer / model-package state (omit ModelPackageName when the package is versioned).
Traces: sagemaker.model-package-version-monotonic-and-server-assigned.

Cause 8 of 8: trial-component timestamps are not rounded to millisecond precision.
AWS: Smithy: CreateTrialComponent.StartTime/EndTime are timestamps; recorded AWS stores and returns them truncated to milliseconds - a supplied 12:00:00.123456 comes back as 12:00:00.123000.
Emulator: echoes the full microsecond value 12:00:00.123456.
Why: a client that compares what it sent with what AWS stores (or that reads the value into a millisecond-precision type) sees a value real AWS never returns.
Fix: timestamp normalization for trial-component StartTime/EndTime in the create/update handlers and state.
Traces: sagemaker.trial-component-timestamp-precision-preserved.

## Evidence
Cause 1: "AssertionError: expected ResourceNotFound, got NotImplemented" on AssociateTrialComponent (both checks).
Cause 2: "AssertionError: DeleteNotebookInstance on an InService instance: expected error code \"ValidationException\", got 'NotImplemented'".
Cause 3: "AssertionError: expected ValidationException, got NotImplemented: operation CreatePipeline with PipelineDefinitionS3Location is scaffolded but not implemented in this emulator".
Cause 4: "botocore.errorfactory.ResourceNotFound: An error occurred (ResourceNotFound) when calling the ListHubContents operation: Could not find hub \"arn:aws:sagemaker:us-west-2:000000000000:hub/SageMakerPublicHub\"." (both hub-content-reference checks).
Cause 5: "AssertionError: transform job did not complete: This emulator runs no model inference container, so no records were transformed and no output object was written to the job's S3OutputPath." and "AssertionError: This emulator runs no model inference container, so no records were transformed and no output object was written to the job's S3OutputPath."
Cause 6: "AssertionError: expected an explicit empty-string description to be normalized away (not stored) for arn:aws:sagemaker:us-west-2:000000000000:model-package/sim2real-mpg-.../1, got ''".
Cause 7: "AssertionError: a versioned model package has no independent name, so DescribeModelPackage must omit ModelPackageName entirely; version 1 returned ModelPackageName='sim2real-behav-mpg-...'".
Cause 8: "AssertionError: StartTime not rounded to millisecond precision: got datetime.datetime(2024, 3, 15, 12, 0, 0, 123456, ...), expected datetime.datetime(2024, 3, 15, 12, 0, 0, 123000, tzinfo=datetime.timezone.utc)".

## Steps to Reproduce
1. CreateTrialComponent(TrialComponentName=X), then AssociateTrialComponent(TrialComponentName=X, TrialName=<never created>) -> expect 400 ResourceNotFound; observe NotImplemented.
2. CreateNotebookInstance(...), wait for InService, DeleteNotebookInstance(NotebookInstanceName=...) -> expect 400 ValidationException and the instance untouched; observe NotImplemented.
3. Upload a pipeline definition to S3 and CreatePipeline(PipelineName=..., PipelineDefinitionS3Location={'Bucket':...,'ObjectKey':...}, RoleArn=..., ClientRequestToken=...) -> expect the definition to be fetched and validated (400 ValidationException for a missing step dependency); observe NotImplemented.
4. ListHubContents(HubName='SageMakerPublicHub', HubContentType='Model') -> expect the managed hub's contents; observe 400 ResourceNotFound naming a hub ARN in the caller's own account.
5. Put input records in S3, CreateModel + CreateTransformJob with TransformOutput.S3OutputPath -> poll to Completed and list the output path: expect transformed objects, observe none.
6. CreateModelPackage(..., ModelPackageDescription='') then DescribeModelPackage -> expect the member absent; observe ''.
7. DescribeModelPackage on <group>/1 -> expect no ModelPackageName; observe the group name.
8. CreateTrialComponent(StartTime=2024-03-15T12:00:00.123456Z) then DescribeTrialComponent -> expect .123000; observe .123456.
