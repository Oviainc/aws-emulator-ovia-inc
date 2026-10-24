Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.create-user-profile-missing-domain-rejected sagemaker.e2e-ai-workload-config-delete-succeeds-while-benchmark-job-references-it sagemaker.batch-describe-model-package-matches-individual-creates sagemaker.feature-group-offline-store-permission-denied-create-failed-rejected sagemaker.search-suggestions-returns-matching-property-names s3__sagemaker.cfn-deploy-sagemaker-notebook-instance-s3-bucket-access-positive
Issue-Key: sagemaker.create-user-profile-missing-domain-rejected|run#8|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Key: sagemaker.e2e-ai-workload-config-delete-succeeds-while-benchmark-job-references-it|run#37|AssertionError|assert running_delete_code == "ValidationException", (
Issue-Key: sagemaker.batch-describe-model-package-matches-individual-creates|run#20|AssertionError|assert bogus_error["ErrorCode"] == "EntityNotFoundError", (
Issue-Key: sagemaker.feature-group-offline-store-permission-denied-create-failed-rejected|run#19|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r} ({message})"
Issue-Key: sagemaker.search-suggestions-returns-matching-property-names|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: s3__sagemaker|CreateStack|ValidationError
Issue-Commit: 7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a

# SageMaker control-plane gaps: CreateUserProfile and DeleteAIBenchmarkJob are 501 stubs, BatchDescribeModelPackage mis-codes a missing ARN, CreateFeatureGroup skips the offline-store access probe, GetSearchSuggestions mis-cases the facet prefix, and CFN rejects AWS::SageMaker::NotebookInstanceLifecycleConfig (6 causes)

> _Note: this issue was identified against the code base at commit `7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUserProfile

## Description
Six independent defects, none of them named by an already-filed stub issue or by an open issue's replay key. Each is its own root cause and its own work item; this issue bundles SIX causes and all six must be closed before submitting.

Cause 1 of 6: CreateUserProfile is a scaffolded stub and answers NotImplemented.
AWS: Smithy: CreateUserProfile requires DomainId (@pattern ^d-(-*[a-z0-9]){1,61}$) and UserProfileName, returns UserProfileArn and models ResourceInUse/ResourceLimitExceeded (HTTP 400). Recorded AWS resolves the Domain while creating the profile, so a well-formed but nonexistent DomainId is rejected 400 ValidationException and nothing is created; against a real Domain that has reached InService the same call succeeds.
Emulator: every call answers the scaffold error NotImplemented (HTTP 501).
Why: Studio user profiles cannot be created at all, and a client branching on ValidationException instead sees an unmodeled code that SDK retry logic treats as a server fault.
Fix: the operation's handler plus Domain lookup/state (the handler is still carrying the "SCAFFOLDED, NOT IMPLEMENTED" marker).
Traces: sagemaker.create-user-profile-missing-domain-rejected. Handler: services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_user_profile.rs (module header line 4 marks it SCAFFOLDED, NOT IMPLEMENTED).

Cause 2 of 6: DeleteAIBenchmarkJob is a scaffolded stub and answers NotImplemented.
AWS: Smithy: DeleteAIBenchmarkJob requires AIBenchmarkJobName (AIEntityName, @length 1..63, @pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$), returns AIBenchmarkJobArn and models ResourceNotFound (400). Recorded AWS refuses the delete of a job that is still InProgress with 400 ValidationException and leaves the job untouched.
Emulator: NotImplemented (HTTP 501) for every call, so neither the refusal nor the delete happens.
Why: AI benchmark jobs have no delete path - the in-use guard is unobservable, the referencing-resource lifecycle cannot be exercised, and the unmodeled 501 breaks error handling.
Fix: the operation's handler plus the job-status guard and benchmark-job state removal.
Traces: sagemaker.e2e-ai-workload-config-delete-succeeds-while-benchmark-job-references-it. Handler: services/service-com-amazonaws-sagemaker-sagemaker/src/ops/delete_ai_benchmark_job.rs (module header line 4 marks it SCAFFOLDED, NOT IMPLEMENTED).

Cause 3 of 6: BatchDescribeModelPackage reports a nonexistent ARN with ErrorCode 'ValidationException' instead of AWS's 'EntityNotFoundError'.
AWS: Smithy: BatchDescribeModelPackage models NO operation-level errors; a per-item failure is reported inside BatchDescribeModelPackageErrorMap as BatchDescribeModelPackageError{ErrorCode, ErrorResponse} (both required). Recorded AWS returns HTTP 200 with the valid summaries intact and ErrorCode 'EntityNotFoundError' for an ARN that does not exist.
Emulator: the same per-item map entry carries ErrorCode 'ValidationException'.
Why: the per-item error code is the only machine-readable discriminator in this partial-failure response (there is no modeled exception to key on), so a client that branches on EntityNotFoundError to re-register or skip a package takes the wrong branch - it reads a not-found as a malformed request.
Fix: the per-item error construction in BatchDescribeModelPackage's handler (use the batch error code, not the service's generic validation code).
Traces: sagemaker.batch-describe-model-package-matches-individual-creates. Handler: services/service-com-amazonaws-sagemaker-sagemaker/src/ops/batch_describe_model_package.rs.

Cause 4 of 6: CreateFeatureGroup accepts an OfflineStoreConfig whose RoleArn has no S3 access - no synchronous rejection at all.
AWS: Smithy: CreateFeatureGroup takes OfflineStoreConfig (S3StorageConfig) + RoleArn and models only ResourceInUse/ResourceLimitExceeded; recorded AWS probes the offline-store bucket with the supplied role before creating anything and rejects the request 400 ValidationException, creating no feature group, while the identical call with the same role granted S3 access succeeds.
Emulator: the call returns success (the check's error probe reports code None / empty message), so the feature group is created despite the role being unable to reach the offline store.
Why: an over-permissive accept - a misconfigured offline store is reported as healthy, the caller's ValidationException branch never runs, and least-privilege regressions are invisible. Distinct from the already-filed over-permissive bundle 20261022T210606Z-sagemaker-accepts-requests-real-aws-refuses-no-s3.md, which names CreateModel/CreateTransformJob/CreateModelPackage/CreateNotebookInstance/DescribeArtifact but not CreateFeatureGroup's offline-store probe, and distinct from 20261023T183034Z-sagemaker-feature-store-data-plane-is-not-event.md, which is about record ordering and offline-store propagation after a successful create.
Fix: CreateFeatureGroup's validation - probe the OfflineStoreConfig S3 location as the supplied RoleArn (bucket policy + role policy, explicit-deny precedence) and refuse before writing feature-group state.
Traces: sagemaker.feature-group-offline-store-permission-denied-create-failed-rejected. Handler: services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_feature_group.rs.

Cause 5 of 6: GetSearchSuggestions emits the facet prefix in the model's casing ('Parameters.<key>') instead of the lowercase form AWS answers ('parameters.<key>').
AWS: recorded behavior only (the model leaves PropertyNameSuggestion.PropertyName unconstrained): a trial component's Parameters entry is suggested as 'parameters.<key>' - lowercase facet prefix, the form Search's Filter.Name accepts - and a hint matching that key returns exactly that property name.
Emulator: the suggestion is built as "{container}.{key}" with the container taken verbatim from the SearchRecord member name, so the answer is 'Parameters.<key>' (and likewise 'Tags.<key>', 'Model.Tags.<key>'); a caller polling for the AWS-cased property never sees it (the check exhausts its poll budget and the harness kills it at 60 s).
Why: the property name is not a cosmetic string - it is fed straight back into Search's Filter.Name, so a console/autocomplete client built on AWS's casing issues a filter name the emulator's own Search will not match, and any assertion on the suggested name fails even though the underlying data is indexed.
Fix: the property-name rendering in GetSearchSuggestions' handler (the facet prefix used when offering stored keys and the modeled member names it offers alongside them), not the state.
Traces: sagemaker.search-suggestions-returns-matching-property-names. Handler: services/service-com-amazonaws-sagemaker-sagemaker/src/ops/get_search_suggestions.rs - `push("Parameters", parameter)` at line 383 (and the sibling Tags/InputArtifacts/OutputArtifacts/CustomerMetadataProperties pushes), plus the modeled-member offer loop at lines 480-485.

Cause 6 of 6: CloudFormation refuses a template declaring AWS::SageMaker::NotebookInstanceLifecycleConfig as an unrecognized resource type, so CreateStack fails outright.
AWS: recorded behavior only - AWS::SageMaker::NotebookInstanceLifecycleConfig is a supported CloudFormation resource type; CreateStack accepts the template and the stack reaches CREATE_COMPLETE with the lifecycle config provisioned and attachable to an AWS::SageMaker::NotebookInstance.
Emulator: CreateStack is rejected 400 ValidationError "Template format error: Unrecognized resource types: [AWS::SageMaker::NotebookInstanceLifecycleConfig]" - the type is absent from the resource-type registry, so the stack never even starts.
Why: a template that AWS deploys is refused at submission time, so nothing downstream of it can be tested and the failure is reported as the caller's template being malformed. Note the open issue 20261022T210606Z-sagemaker-cloudformation-declared-sagemaker-resources-are-never-provisioned-and.md lists this check id but covers a different cause (resources silently not provisioned after a SUCCESSFUL stack create, and missing OutputValue); this one is the template-type registry rejecting the type before provisioning is ever reached, and the report did not mark the check as owned.
Fix: the CloudFormation resource-type registry plus a resource provider for AWS::SageMaker::NotebookInstanceLifecycleConfig (create/update/delete into SageMaker notebook-instance-lifecycle-config state and resolve its Ref/GetAtt).
Traces: s3__sagemaker.cfn-deploy-sagemaker-notebook-instance-s3-bucket-access-positive.

## Evidence
Cause 1: "AssertionError: expected ValidationException, got NotImplemented" on CreateUserProfile with a well-formed, nonexistent DomainId.
Cause 2: "AssertionError: expected DeleteAIBenchmarkJob on an InProgress job to be rejected with ValidationException, got 'NotImplemented'".
Cause 3: "AssertionError: expected ErrorCode EntityNotFoundError for a non-existent ARN, got 'ValidationException'" (read from BatchDescribeModelPackageErrorMap; the valid entries were returned).
Cause 4: "AssertionError: expected ValidationException, got None ()" - the create of a feature group whose OfflineStoreConfig role has no S3 permissions raised nothing.
Cause 5: "Error: Check timed out after 60.0s" while polling GetSearchSuggestions(Resource='ExperimentTrialComponent', SuggestionQuery={'PropertyNameQuery': {'PropertyNameHint': 'Sim2RealCustomHP<suffix>'}}) for the property name 'parameters.Sim2RealCustomHP<suffix>'; the handler constructs that name as 'Parameters.<key>' (services/.../get_search_suggestions.rs line 383), so the predicate can never become true.
Cause 6: "botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::SageMaker::NotebookInstanceLifecycleConfig]".

## Steps to Reproduce
1. CreateUserProfile(DomainId='d-000000000000', UserProfileName='sim2real-up') -> expect 400 ValidationException; observe 501 NotImplemented.
2. Create an AI workload config + CreateAIBenchmarkJob, then DeleteAIBenchmarkJob(AIBenchmarkJobName=<the InProgress job>) -> expect 400 ValidationException and the job untouched; observe 501 NotImplemented.
3. CreateModelPackageGroup + CreateModelPackage (one version), then BatchDescribeModelPackage(ModelPackageArnList=[<real version ARN>, <syntactically valid but nonexistent ARN>]) -> expect 200 with the real summary plus BatchDescribeModelPackageErrorMap[<bogus>].ErrorCode == 'EntityNotFoundError'; observe 'ValidationException'.
4. Create a role with NO s3 permissions, then CreateFeatureGroup(..., OfflineStoreConfig={'S3StorageConfig': {'S3Uri': 's3://<bucket>/offline/'}}, RoleArn=<that role>) -> expect 400 ValidationException and no feature group; observe success. Grant the same role S3 access and repeat -> succeeds (positive control).
5. CreateTrialComponent(TrialComponentName=X, Parameters={'Sim2RealCustomHP<suffix>': {'NumberValue': 42.0}}), then GetSearchSuggestions(Resource='ExperimentTrialComponent', SuggestionQuery={'PropertyNameQuery': {'PropertyNameHint': 'Sim2RealCustomHP<suffix>'}}) -> expect PropertyNameSuggestions containing 'parameters.Sim2RealCustomHP<suffix>'; observe 'Parameters.Sim2RealCustomHP<suffix>'.
6. CreateStack with a template declaring AWS::SageMaker::NotebookInstanceLifecycleConfig (plus an AWS::SageMaker::NotebookInstance referencing it) -> expect the stack to be accepted; observe 400 ValidationError "Unrecognized resource types: [AWS::SageMaker::NotebookInstanceLifecycleConfig]".
