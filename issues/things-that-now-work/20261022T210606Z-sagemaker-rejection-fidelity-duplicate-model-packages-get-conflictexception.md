Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.create-model-package-duplicate-name-conflict sagemaker.create-model-package-duplicate-name-version-conflict-rejected sagemaker.create-model-package-duplicate-version-rejected sagemaker.action-create-delete-lifecycle sagemaker.action-create-describe-delete sagemaker.action-create-describe-delete-lifecycle sagemaker.action-lifecycle-create-describe-delete sagemaker.action-lifecycle-transitions sagemaker.create-action-delete-lifecycle sagemaker.create-model-card-duplicate-name-conflict-rejected sagemaker.duplicate-model-card-creation-rejected sagemaker.pipeline-create-describe-delete sagemaker.pipeline-create-describe-delete-lifecycle sagemaker.artifact-empty-vs-absent-property-preserved sagemaker.update-model-package-invalid-approval-status-rejected sagemaker.trial-component-unicode-metadata-round-trip s3__sagemaker.sagemaker-batch-transform-invalid-s3-uri-negative s3__sagemaker.sagemaker-batch-transform-writes-s3-output-positive s3__sagemaker.sagemaker-training-job-input-s3-uri-validation-rejected sagemaker.create-endpoint-referencing-deleting-config-wrong-state-rejected sagemaker.endpoint-config-create-describe-delete
Issue-Key: sagemaker.create-model-package-duplicate-name-conflict|run#10|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: sagemaker.create-model-package-duplicate-name-version-conflict-rejected|run#7|AssertionError|assert code == "ValidationException", f"{code}:{message}"
Issue-Key: sagemaker.create-model-package-duplicate-version-rejected|run#10|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}: {message}"
Issue-Key: sagemaker.action-create-delete-lifecycle|run#16|AssertionError|assert describe_err["Message"] == expected_describe, (
Issue-Key: sagemaker.action-create-describe-delete|run#17|AssertionError|assert message == f"Action '{arn}' does not exist.", f"unexpected DescribeAction message: {message!r}"
Issue-Key: sagemaker.action-create-describe-delete-lifecycle|run#16|AssertionError|assert describe_err["Message"] == expected_describe, (
Issue-Key: sagemaker.action-lifecycle-create-describe-delete|run#16|AssertionError|assert describe_err["Message"] == expected_describe, (
Issue-Key: sagemaker.action-lifecycle-transitions|run#16|AssertionError|assert describe_err["Message"] == expected_describe, (
Issue-Key: sagemaker.create-action-delete-lifecycle|run#13|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: sagemaker.create-model-card-duplicate-name-conflict-rejected|run#14|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: sagemaker.duplicate-model-card-creation-rejected|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: sagemaker.pipeline-create-describe-delete|run#16|AssertionError|assert message == f"Pipeline '{pipeline_arn}' does not exist.", message
Issue-Key: sagemaker.pipeline-create-describe-delete-lifecycle|_error#16|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: sagemaker.artifact-empty-vs-absent-property-preserved|run#7|AssertionError|assert err["Message"] == expected_message, (
Issue-Key: sagemaker.update-model-package-invalid-approval-status-rejected|run#3|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: sagemaker|CreateTrialComponent|ValidationException
Issue-Key: s3__sagemaker.sagemaker-batch-transform-invalid-s3-uri-negative|run#14|AssertionError|assert error.get("Message") == (
Issue-Key: s3__sagemaker.sagemaker-batch-transform-writes-s3-output-positive|run#11|AssertionError|assert error.get("Message") == (
Issue-Key: s3__sagemaker.sagemaker-training-job-input-s3-uri-validation-rejected|run#11|AssertionError|assert error == (
Issue-Key: sagemaker.create-endpoint-referencing-deleting-config-wrong-state-rejected|run#12|AssertionError|assert describe_config_message == expected_describe, (
Issue-Key: sagemaker.endpoint-config-create-describe-delete|run#21|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: f973f78f74c99aad4e85f2736f1fde89ed7327e3

# SageMaker rejection fidelity: duplicate model packages get ConflictException instead of ValidationException, not-found/conflict message templates diverge, @pattern anchors leak, enum set order differs, and a deleted pipeline is still readable (9 causes)

> _Note: this issue was identified against the code base at commit `f973f78f74c99aad4e85f2736f1fde89ed7327e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateModelPackage

## Description
Nine independent divergences in how SageMaker classifies and renders rejections, plus one state defect found with them. Each is its own root cause and its own work item; this issue bundles NINE causes and all nine must be closed before submitting.

Cause 1 of 9: a duplicate model package is rejected with the wrong error code.
AWS: Smithy models ConflictException and ResourceLimitExceeded for CreateModelPackage, but recorded AWS rejects a re-registration of an existing model package name/version with 400 ValidationException.
Emulator: 400 ConflictException ("Cannot create already existing model package \"<name>\".").
Why: a client branching on the error code to distinguish "already registered" from a retryable conflict takes the wrong branch.
Fix: the duplicate-detection path in CreateModelPackage's handler (classify as ValidationException), services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_model_package.rs.
Traces: sagemaker.create-model-package-duplicate-name-conflict, sagemaker.create-model-package-duplicate-name-version-conflict-rejected, sagemaker.create-model-package-duplicate-version-rejected.

Cause 2 of 9: the Action not-found message uses a different template than AWS.
AWS: Smithy: DescribeAction models ResourceNotFound (400) and takes ActionName as a name-or-ARN. The message is built from the request in the fixed form "Action '<action-arn>' does not exist." - the rule is known (the resolved ARN, single-quoted, then "does not exist.").
Emulator: same 400/ResourceNotFound, but the message is 'Could not find action "<name>".'
Why: the message rule is known and differs in both the identifier form and the sentence shape; consumers that extract the offending resource from the message get a bare name where AWS reports an ARN.
Fix: the not-found error construction for the lineage entities (Action) in their handlers / shared lookup helper.
Traces: sagemaker.action-create-delete-lifecycle, sagemaker.action-create-describe-delete, sagemaker.action-create-describe-delete-lifecycle, sagemaker.action-lifecycle-create-describe-delete, sagemaker.action-lifecycle-transitions, sagemaker.create-action-delete-lifecycle.

Cause 3 of 9: the duplicate-model-card conflict message omits the ARN and version.
AWS: Smithy: CreateModelCard models ConflictException (400). Recorded AWS builds the message from the resolved card and its version: "Modelcard arn:aws:sagemaker:<region>:<account>:model-card/<name> with version 1 already exists".
Emulator: same 400/ConflictException, message "Model Card with name <name> already exists. Choose a different name."
Why: the emulator's text hides the version dimension that AWS's model-card conflict is actually about, so a client cannot tell which version collided.
Fix: the conflict-error construction in CreateModelCard's handler.
Traces: sagemaker.create-model-card-duplicate-name-conflict-rejected, sagemaker.duplicate-model-card-creation-rejected.

Cause 4 of 9: the pipeline not-found message does not quote the ARN.
AWS: Smithy: DescribePipeline models ResourceNotFound (400). The message is built from the request's resolved pipeline ARN as "Pipeline '<pipeline-arn>' does not exist." (ARN in single quotes).
Emulator: same code, message "Pipeline <pipeline-arn> does not exist." - the quotes are missing.
Why: the message rule is known and the delimiters are part of it; message parsers that cut on the quotes extract nothing.
Fix: the not-found error construction in the pipeline handlers.
Traces: sagemaker.pipeline-create-describe-delete.

Cause 5 of 9: after DeletePipeline, DescribePipeline still returns the pipeline.
AWS: Smithy: DeletePipeline returns the PipelineArn and models ResourceNotFound/ConflictException; recorded AWS makes the pipeline unreadable afterwards - DescribePipeline is rejected 400 ResourceNotFound and a second DeletePipeline on the same name is likewise ResourceNotFound, and the name becomes reusable.
Emulator: DescribePipeline returns HTTP 200 with PipelineStatus 'Deleting' and the full definition, i.e. the delete leaves a readable record and never reaches a terminal removal.
Why: delete is not observable as delete - cleanup loops never terminate, name reuse cannot be verified, and clients that poll for the not-found condition hang.
Fix: DeletePipeline's handler plus pipeline state transition (remove the record, or make reads of a Deleting pipeline 400 ResourceNotFound as recorded).
Traces: sagemaker.pipeline-create-describe-delete-lifecycle.

Cause 6 of 9: a map-member constraint violation renders the wrong value and an incomplete constraint list.
AWS: Smithy: CreateArtifact/DescribeArtifact Properties targets LineageEntityParameters (@length 0..30) whose values are StringParameterValue (@length 0..2500, @pattern .*). Recorded AWS renders the rejected value as the WHOLE map ('{empty_prop=, present_prop=some-value}') at path 'properties', and lists all three constraints in the order "[Member must have length less than or equal to 4096, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: .*]".
Emulator: renders the single offending value ('') and only two constraints, in the opposite order, dropping the pattern entry.
Why: the message rule for map members is known (value = the rendered map, constraints = the full list); a client or operator reading the error cannot see which map was rejected.
Fix: the shared request-validation message formatter for map-typed members in the sagemaker service.
Traces: sagemaker.artifact-empty-vs-absent-property-preserved.

Cause 7 of 9: the enum-set rejection message lists the values in a different order.
AWS: recorded behavior only - UpdateModelPackage with an invalid ModelApprovalStatus is rejected 400 ValidationException whose message ends "Member must satisfy enum value set: [PendingManualApproval, Approved, Rejected]".
Emulator: same status and code, but the set is rendered in the Smithy declaration order "[Approved, Rejected, PendingManualApproval]".
Why: the message content here is fixed by the service (not by the request), so the order is part of the recorded contract; suites and clients comparing the documented value set see a different string.
Fix: the enum-rendering order used by the validation message formatter for ModelApprovalStatus (service order, not model order).
Traces: sagemaker.update-model-package-invalid-approval-status-rejected.

Cause 8 of 9: @pattern constraint messages still carry the ^ and $ anchors on members the existing issue does not name.
AWS: the validation message reports the pattern WITHOUT anchors (Smithy: CreateTrialComponent.DisplayName @pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}$ is reported as [a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}; TransformInput/TransformOutput and InputDataConfig S3Uri @pattern ^(https|s3)://([^/]+)/?(.*)$ is reported as (https|s3)://([^/]+)/?(.*)). Status 400, code ValidationException.
Emulator: prints the anchored pattern verbatim.
Why: the message rule is known, so the anchors are part of the contract; this is the same formatter defect already filed as Cause 1 of open issue 20261022T120642Z-sagemaker-error-fidelity-validationexception-messages-carry-regex-anchors.md, but that issue's replay key list does not include these four checks, so a fix verified only against it would leave these member paths unverified.
Fix: the shared @pattern rendering in the sagemaker validation message formatter (strip anchors), covering trial-component DisplayName and the transform/training S3Uri members.
Traces: sagemaker.trial-component-unicode-metadata-round-trip, s3__sagemaker.sagemaker-batch-transform-invalid-s3-uri-negative, s3__sagemaker.sagemaker-batch-transform-writes-s3-output-positive, s3__sagemaker.sagemaker-training-job-input-s3-uri-validation-rejected.

Cause 9 of 9: endpoint/endpoint-config not-found messages still echo a synthesized ARN on paths the existing issue does not name.
AWS: recorded behavior only - the message quotes the identifier the caller passed: 'Could not find endpoint configuration "<EndpointConfigName>".'
Emulator: substitutes the full ARN it synthesized, e.g. 'Could not find endpoint configuration "arn:aws:sagemaker:<region>:<account>:endpoint-config/<name>".'
Why: same known message rule as Cause 2 of the open error-fidelity issue above; these two checks are not in its replay key list, so they are reported here to keep them verified.
Fix: the not-found error construction in the endpoint-config/endpoint handlers and the shared state-lookup helper (pass the request's name through).
Traces: sagemaker.create-endpoint-referencing-deleting-config-wrong-state-rejected, sagemaker.endpoint-config-create-describe-delete.

## Evidence
Cause 1: "AssertionError: expected ValidationException, got 'ConflictException'"; "AssertionError: ConflictException:Cannot create already existing model package \"sim2real-mp-...\"."; "AssertionError: expected ValidationException, got ConflictException: Cannot create already existing model package ...".
Cause 2: "AssertionError: expected message \"Action 'arn:aws:sagemaker:us-west-2:000000000000:action/sim2real-action-...' does not exist.\", got 'Could not find action \"sim2real-action-...\".'" (six checks).
Cause 3: "AssertionError: expected message 'Modelcard arn:aws:sagemaker:us-west-2:000000000000:model-card/sim2real-behav-mc-... with version 1 already exists', got 'Model Card with name sim2real-behav-mc-... already exists. Choose a different name.'".
Cause 4: "AssertionError: Pipeline arn:aws:sagemaker:us-west-2:000000000000:pipeline/sim2real-pipeline-... does not exist." where the check asserts the message equals "Pipeline '<arn>' does not exist.".
Cause 5: "AssertionError: expected a rejection, but the call SUCCEEDED and returned {'PipelineArn': ..., 'PipelineStatus': 'Deleting', ... 'HTTPStatusCode': 200 ...}" from describe_pipeline after delete_pipeline.
Cause 6: "AssertionError: expected message \"... Value '{empty_prop=, present_prop=some-value}' at 'properties' ... [Member must have length less than or equal to 4096, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: .*]\", got \"... Value '' at 'properties' ... [Member must have length greater than or equal to 1, Member must have length less than or equal to 4096]\"".
Cause 7: "AssertionError: expected message \"... Member must satisfy enum value set: [PendingManualApproval, Approved, Rejected]\", got \"... [Approved, Rejected, PendingManualApproval]\"".
Cause 8: "... got \"... pattern: ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}$\"" (trial component displayName) and "{'Message': \"1 validation error detected: Value 'not-a-valid-s3-uri' at 'transformInput.dataSource.s3DataSource.s3Uri' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(https|s3)://([^/]+)/?(.*)$\", 'Code': 'ValidationException'}" (also transformOutput.s3OutputPath and inputDataConfig.1.member.dataSource.s3DataSource.s3Uri).
Cause 9: "AssertionError: expected message 'Could not find endpoint configuration \"sim2real-epc-doomed-...\".', got 'Could not find endpoint configuration \"arn:aws:sagemaker:us-west-2:000000000000:endpoint-config/sim2real-epc-doomed-...\".'".

## Steps to Reproduce
1. CreateModelPackageGroup + CreateModelPackage twice with the same name/version -> expect 400 ValidationException; observe ConflictException.
2. CreateAction then DeleteAction, then DescribeAction(ActionName=<name>) -> compare the message to "Action '<arn>' does not exist.".
3. CreateModelCard twice with the same ModelCardName -> compare the ConflictException message to "Modelcard <arn> with version 1 already exists".
4. CreatePipeline, DeletePipeline, DescribePipeline(PipelineName=<name>) -> expect 400 ResourceNotFound with "Pipeline '<arn>' does not exist."; observe 200 with PipelineStatus 'Deleting' (Cause 5) and, where it does 404, an unquoted ARN (Cause 4).
5. CreateArtifact with Properties={'empty_prop': '', 'present_prop': 'some-value'} -> compare the rendered value and constraint list.
6. UpdateModelPackage(ModelPackageArn=..., ModelApprovalStatus='Maybe') -> compare the enum value set order.
7. CreateTrialComponent(DisplayName='\u5b9f\u9a13-component-...') and CreateTransformJob/CreateTrainingJob with S3Uri='not-a-valid-s3-uri' -> the reported @pattern still has ^...$.
8. CreateEndpoint referencing a deleted/never-created EndpointConfigName, and DescribeEndpointConfig after delete -> the message quotes a synthesized ARN instead of the supplied name.
