Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__sagemaker.sagemaker-create-model-invalid-s3-artifact-path-negative s3__sagemaker.sagemaker-create-model-missing-s3-artifact-negative s3__sagemaker.sagemaker-model-artifact-s3-key-not-found-negative s3__sagemaker.sagemaker-model-creation-s3-modeldata-missing-object-negative s3__sagemaker.sagemaker-model-creation-cross-account-bucket-policy-negative s3__sagemaker.sagemaker-batch-transform-reads-s3-input-positive sagemaker.create-model-package-malformed-image-uri-rejected sagemaker.create-notebook-instance-nonexistent-role-arn-accepted-then-fails sagemaker.missing-permission-describe-artifact-rejected sagemaker.create-model-package-client-token-replay-no-second-version
Issue-Key: s3__sagemaker.sagemaker-create-model-invalid-s3-artifact-path-negative|run#8|AssertionError|assert raised, "CreateModel succeeded against a nonexistent S3 ModelDataUrl -- it must fail"
Issue-Key: s3__sagemaker.sagemaker-create-model-missing-s3-artifact-negative|_client_error#21|AssertionError|raise AssertionError(f"expected fn() to raise a ClientError, but it SUCCEEDED and returned {result!r}")
Issue-Key: s3__sagemaker.sagemaker-model-artifact-s3-key-not-found-negative|_client_error#11|AssertionError|raise AssertionError("the call succeeded, but AWS refuses it")
Issue-Key: s3__sagemaker.sagemaker-model-creation-s3-modeldata-missing-object-negative|run#25|AssertionError|raise AssertionError(
Issue-Key: s3__sagemaker.sagemaker-model-creation-cross-account-bucket-policy-negative|run#15|AssertionError|raise AssertionError(
Issue-Key: s3__sagemaker.sagemaker-batch-transform-reads-s3-input-positive|run#10|AssertionError|assert raised, (
Issue-Key: sagemaker.create-model-package-malformed-image-uri-rejected|_error_code_and_message#4|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: sagemaker.create-notebook-instance-nonexistent-role-arn-accepted-then-fails|run#8|AssertionError|assert bad_status == "Failed", (
Issue-Key: sagemaker.missing-permission-describe-artifact-rejected|_error#8|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: sagemaker.create-model-package-client-token-replay-no-second-version|_error#16|AssertionError|raise AssertionError(
Issue-Commit: f973f78f74c99aad4e85f2736f1fde89ed7327e3

# SageMaker accepts requests real AWS refuses: no S3 artifact existence or bucket-policy check, no transform-input check, no image-URI or role validation, no ClientToken idempotency, no IAM authorization (7 causes)

> _Note: this issue was identified against the code base at commit `f973f78f74c99aad4e85f2736f1fde89ed7327e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateModel

## Description
Seven independent over-permissive accepts: the emulator returns 200 where recorded AWS rejects the request synchronously. Each is its own root cause and its own work item; this issue bundles SEVEN causes and all seven must be closed before submitting.

Cause 1 of 7: CreateModel accepts a ModelDataUrl whose S3 object does not exist.
AWS: Smithy: CreateModel.PrimaryContainer.ModelDataUrl targets Url (@length 0..1024, @pattern ^(https|s3)://([^/]+)/?(.*)$) and its only modeled error is ResourceLimitExceeded; recorded AWS validates the artifact at request time and rejects a ModelDataUrl pointing at a nonexistent key/prefix with 400 ValidationException before any model object exists.
Emulator: returns HTTP 200 with a ModelArn and creates the model; a later DescribeModel shows it.
Why: the caller is told a deployable model exists when its artifact does not; the failure resurfaces much later (at endpoint creation/inference) instead of at the call the client can handle, and the "no side effect on rejection" contract is broken.
Fix: CreateModel's handler plus its artifact-existence validation against S3 state (services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_model.rs).
Traces: s3__sagemaker.sagemaker-create-model-invalid-s3-artifact-path-negative, s3__sagemaker.sagemaker-create-model-missing-s3-artifact-negative, s3__sagemaker.sagemaker-model-artifact-s3-key-not-found-negative, s3__sagemaker.sagemaker-model-creation-s3-modeldata-missing-object-negative.

Cause 2 of 7: CreateModel accepts an artifact the bucket policy explicitly denies to the execution role.
AWS: recorded behavior only - CreateModel's synchronous access check reads the artifact as the ExecutionRoleArn, so an explicit Deny in the bucket policy wins over the role's identity-based Allow and the call is rejected 400 ValidationException with no model created.
Emulator: returns 200 and creates the model; no authorization evaluation happens.
Why: a permission-boundary regression (a bucket policy that is supposed to fence off an artifact) is invisible, so security-relevant negative tests pass against the emulator and fail on AWS.
Fix: CreateModel's handler plus the S3 authorization evaluation (bucket policy + role policy, explicit-deny precedence) used by its artifact access check.
Traces: s3__sagemaker.sagemaker-model-creation-cross-account-bucket-policy-negative.

Cause 3 of 7: CreateTransformJob accepts an S3Prefix input under which there are zero objects.
AWS: Smithy: CreateTransformJob.TransformInput.DataSource.S3DataSource requires S3DataType + S3Uri and the operation models ResourceNotFound/ResourceInUse/ResourceLimitExceeded (400); recorded AWS inspects the input path's contents and refuses a prefix with no objects rather than starting a job that can never transform anything.
Emulator: returns 200 and creates the transform job.
Whip/Why: a client's data-staging bug is accepted silently; the job is admitted with no input and the caller has no synchronous signal.
Fix: CreateTransformJob's handler plus its input-path content check against S3 state.
Traces: s3__sagemaker.sagemaker-batch-transform-reads-s3-input-positive.

Cause 4 of 7: CreateModelPackage accepts a malformed container image URI.
AWS: Smithy: InferenceSpecification.Containers[].Image targets ContainerImage (@length 0..255, @pattern ^[\S]+$); a value containing spaces violates the pattern and is rejected 400 ValidationException with the standard "1 validation error detected: ..." message.
Emulator: returns HTTP 200 with a ModelPackageArn and registers the version.
Why: the registry accumulates versions that can never be deployed, and the modeled constraint is not enforced at all for this operation (CreateModel does enforce it), so clients cannot rely on validation being uniform.
Fix: CreateModelPackage's request validation for InferenceSpecification container images (services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_model_package.rs).
Traces: sagemaker.create-model-package-malformed-image-uri-rejected.

Cause 5 of 7: CreateNotebookInstance accepts a RoleArn for a role that does not exist and drives the instance to InService.
AWS: Smithy: CreateNotebookInstance.RoleArn is required (@pattern ^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$); recorded AWS does not leave such an instance running - provisioning against a nonexistent role fails (the instance ends in Failed / the request is refused), never reaching InService.
Emulator: the instance reports NotebookInstanceStatus 'InService'.
Why: a broken role reference is reported as a healthy instance, so the caller's error handling and the Failed-state branch never run.
Fix: CreateNotebookInstance's handler plus the role-existence check and the notebook-instance status machine.
Traces: sagemaker.create-notebook-instance-nonexistent-role-arn-accepted-then-fails.

Cause 6 of 7: DescribeArtifact applies no IAM authorization.
AWS: Smithy: DescribeArtifact requires ArtifactArn and models ResourceNotFound (400); recorded AWS refuses the call for a principal with no sagemaker permissions (AccessDeniedException, 400) and returns no artifact data.
Emulator: returns 200 with the full artifact (ArtifactName, Source, Properties, LineageGroupArn) regardless of the calling principal.
Why: the emulator cannot be used to validate least-privilege or deny-by-default behavior; lineage metadata is readable by principals AWS would refuse.
Fix: the authorization gate in front of the lineage read operations (DescribeArtifact's handler and the shared principal/permission evaluation).
Traces: sagemaker.missing-permission-describe-artifact-rejected.

Cause 7 of 7: a spent CreateModelPackage ClientToken is not honored, so a retried registration adds a second version.
AWS: Smithy: CreateModelPackage.ClientToken carries @idempotencyToken (@length 1..36, @pattern ^[a-zA-Z0-9-]+$); recorded AWS treats a replay of the same token as the same request - it returns the original ModelPackageArn (or refuses) and never creates a second registry version.
Emulator: the replay succeeds and returns a new model-package version ARN.
Why: an ordinary SDK retry silently doubles registry versions, corrupting version numbering and any approval workflow keyed on version.
Fix: CreateModelPackage's handler plus idempotency-token recording in model-package state.
Traces: sagemaker.create-model-package-client-token-replay-no-second-version.

## Evidence
Cause 1: "AssertionError: CreateModel succeeded against a nonexistent S3 ModelDataUrl -- it must fail"; "AssertionError: expected fn() to raise a ClientError, but it SUCCEEDED and returned {'ModelArn': 'arn:aws:sagemaker:us-west-2:000000000000:model/sim2real-sm-model-bad-...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}"; "AssertionError: the call succeeded, but AWS refuses it"; "AssertionError: CreateModel with a ModelDataUrl pointing at a nonexistent S3 object was expected to be rejected synchronously, but it was accepted".
Cause 2: "AssertionError: CreateModel accepted an artifact the bucket policy explicitly denies to the execution role; real AWS refuses it synchronously (Deny wins over the role's own identity-based Allow)".
Cause 3: "AssertionError: CreateTransformJob succeeded pointing at an S3 prefix with ZERO objects under it -- SageMaker must actually inspect the input path's contents, not accept it blindly".
Cause 4: "AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ModelPackageArn': 'arn:aws:sagemaker:us-west-2:000000000000:model-package/sim2real-behav-mp-bad-...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}".
Cause 5: "AssertionError: expected provisioning against a nonexistent role to fail, got 'InService'".
Cause 6: "AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ArtifactName': ..., 'ArtifactArn': ..., 'Source': {...}, 'ArtifactType': 'DataSet', ... 'HTTPStatusCode': 200 ...}".
Cause 7: "AssertionError: reusing a spent ClientToken SUCCEEDED (over-permissive) -- a retried registration would silently add a registry version: {'ModelPackageArn': '.../sim2real-behav-mpg-.../1', ... 'HTTPStatusCode': 200 ...}".

## Steps to Reproduce
1. Create a bucket but no artifact object. CreateModel(ModelName=..., ExecutionRoleArn=<valid role>, PrimaryContainer={'Image': <valid ecr uri>, 'ModelDataUrl': 's3://<bucket>/missing/model.tar.gz'}) -> expect 400 ValidationException and no model; observe 200 + ModelArn.
2. Upload an artifact, then attach a bucket policy with an explicit Deny on s3:GetObject for the execution role, and repeat CreateModel against that artifact -> expect 400 ValidationException; observe 200.
3. CreateTransformJob(TransformInput={'DataSource': {'S3DataSource': {'S3DataType': 'S3Prefix', 'S3Uri': 's3://<bucket>/empty-prefix/'}}}, ...) -> expect a synchronous refusal; observe 200.
4. CreateModelPackage(ModelPackageGroupName=..., InferenceSpecification={'Containers': [{'Image': 'this is not an ecr image uri'}], ...}) -> expect 400 ValidationException; observe 200.
5. CreateNotebookInstance(NotebookInstanceName=..., InstanceType='ml.t3.medium', RoleArn='arn:aws:iam::<acct>:role/does-not-exist') then poll DescribeNotebookInstance -> expect a failure/Failed status; observe InService.
6. Call DescribeArtifact with credentials of a principal holding no sagemaker permissions -> expect 400 AccessDeniedException; observe 200 with the artifact.
7. CreateModelPackage(..., ClientToken=T) twice with identical input -> expect no second version; observe a second ModelPackageArn.
