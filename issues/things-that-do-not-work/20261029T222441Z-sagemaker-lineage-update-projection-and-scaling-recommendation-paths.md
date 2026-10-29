Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.add-association-self-reference-rejected sagemaker.trial-create-describe-delete sagemaker.trial-create-describe-delete-lifecycle sagemaker.code-repository-create-describe-delete sagemaker.action-create-describe-delete sagemaker.model-package-unicode-description-round-trip sagemaker.cluster-node-add-and-cluster-delete-lifecycle sagemaker.get-lineage-group-policy-absent-by-default sagemaker.get-lineage-group-policy-missing sagemaker.get-lineage-group-policy-not-found sagemaker.delete-algorithm-conflict-while-referenced-accepted sagemaker.e2e-experiment-trial-training-job-metrics-flow sagemaker.get-scaling-configuration-recommendation-endpoint-not-found sagemaker.get-scaling-configuration-recommendation-for-valid-endpoint sagemaker.get-scaling-configuration-recommendation-matches-endpoint sagemaker.get-scaling-configuration-recommendation-not-found sagemaker.get-scaling-configuration-recommendation-not-found-for-missing-endpoint sagemaker.get-scaling-configuration-recommendation-unknown-endpoint-not-found sagemaker.scaling-configuration-recommendation-for-live-endpoint sagemaker.scaling-configuration-recommendation-not-found-for-unknown-endpoint
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|AddAssociation|ValidationException
Issue-Key: sagemaker.trial-create-describe-delete|run#20|AssertionError|assert update_code == "ResourceNotFound", (
Issue-Key: sagemaker.trial-create-describe-delete-lifecycle|run#18|AssertionError|assert update_code == "ResourceNotFound", (
Issue-Key: sagemaker.code-repository-create-describe-delete|run#13|AssertionError|assert update_code == "ValidationException", update_code
Issue-Key: sagemaker.action-create-describe-delete|run#19|AssertionError|assert update_code == "ResourceNotFound", (
Issue-Key: sagemaker.model-package-unicode-description-round-trip|run#9|AssertionError|assert pkg_arn not in error_map, f"expected {pkg_arn} to describe cleanly, got error {error_map.get(pkg_arn)}"
Issue-Key: sagemaker.cluster-node-add-and-cluster-delete-lifecycle|run#13|KeyError|assert described["NodeProvisioningMode"] == "Continuous", described.get("NodeProvisioningMode")
Issue-Key: sagemaker.get-lineage-group-policy-absent-by-default|run#3|AssertionError|raise AssertionError(
Issue-Key: sagemaker.get-lineage-group-policy-missing|run#3|AssertionError|raise AssertionError(
Issue-Key: sagemaker.get-lineage-group-policy-not-found|run#3|AssertionError|raise AssertionError(
Issue-Key: sagemaker.delete-algorithm-conflict-while-referenced-accepted|run#26|AssertionError|assert job_described["AlgorithmSpecification"]["AlgorithmName"] == algo_referenced_arn, (
Issue-Key: sagemaker.e2e-experiment-trial-training-job-metrics-flow|run#20|AssertionError|assert matches, f"no trial component auto-associated with training job {training_job_arn!r}: {components}"
Issue-Key: sagemaker.get-scaling-configuration-recommendation-endpoint-not-found|run#6|AssertionError|assert code_message == f"Inference recommendations job {bogus_job_name} was not found.", f"unexpected message: {code_message!r}"
Issue-Key: sagemaker.get-scaling-configuration-recommendation-for-valid-endpoint|run#6|AssertionError|assert message == f"Inference recommendations job {bogus_job_name} was not found.", f"unexpected GetScalingConfigurationRecommendation message: {message!r}"
Issue-Key: sagemaker.get-scaling-configuration-recommendation-matches-endpoint|run#5|AssertionError|assert message == (
Issue-Key: sagemaker.get-scaling-configuration-recommendation-not-found|run#6|AssertionError|assert code_message == f"Inference recommendations job {bogus_job_name} was not found.", f"unexpected message: {code_message!r}"
Issue-Key: sagemaker.get-scaling-configuration-recommendation-not-found-for-missing-endpoint|run#6|AssertionError|assert code_message == f"Inference recommendations job {missing_job_name} was not found.", f"unexpected message: {code_message!r}"
Issue-Key: sagemaker.get-scaling-configuration-recommendation-unknown-endpoint-not-found|run#6|AssertionError|assert code_message == f"Inference recommendations job {fake_job_name} was not found.", f"unexpected message: {code_message!r}"
Issue-Key: sagemaker.scaling-configuration-recommendation-for-live-endpoint|run#6|AssertionError|assert shape_message == expected_shape, f"expected message {expected_shape!r}, got {shape_message!r}"
Issue-Key: sagemaker.scaling-configuration-recommendation-not-found-for-unknown-endpoint|run#7|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 066b7cda09b446bd4bd9be1c3ecfcbe16940769e

# SageMaker lineage, update, projection and scaling-recommendation paths diverge: self-association refused, Update* stubs, missing response members, no default lineage group (10 causes)

> _Note: this issue was identified against the code base at commit `066b7cda09b446bd4bd9be1c3ecfcbe16940769e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetScalingConfigurationRecommendation

## Description
Ten root causes in the lineage/experiment, Update*, response-projection and inference-recommendation surfaces. This issue is net-new relative to the open issues 20261029T192819Z-sagemaker-update-operations-are-accepted-but-never-mutate.md, 20261029T192819Z-sagemaker-describe-responses-drop-modeled-defaults-and-echoed.md and 20261029T192819Z-sagemaker-lifecycle-identity-and-round-trip-fidelity-defects.md, none of which carries the replay keys below.

Cause 1 of 10 — AddAssociation REFUSES a self-referencing lineage edge that AWS accepts (over-strict).
AWS: Smithy: AddAssociation errors = [ResourceLimitExceeded, ResourceNotFound] only — ValidationException is not modeled on this operation at all, and SourceArn/DestinationArn carry no "must differ" constraint. Recorded behavior: AWS accepts SourceArn == DestinationArn and records a genuine self-loop, returning both ARNs.
Emulator: 400 ValidationException with Message 'The source and the destination of an association must be different entities; both were "<entity-arn>".' — an invented gate and an unmodeled error for this operation.
Why: a legal lineage write is refused, and the error code the client sees is one AWS never returns from AddAssociation.
Fix: remove the source/destination inequality gate from the add-association handler's validation.
Traces: sagemaker.add-association-self-reference-rejected.

Cause 2 of 10 — UpdateTrial is an unimplemented scaffold stub.
AWS: UpdateTrial on a deleted trial is refused ResourceNotFound (400) naming the trial.
Emulator: 400 NotImplemented, "operation UpdateTrial is scaffolded but not implemented in this emulator".
Why: the mutation half of the trial lifecycle is unavailable, so a deleted trial is only unreadable, never unusable.
Fix: implement the update-trial handler.
Traces: sagemaker.trial-create-describe-delete, sagemaker.trial-create-describe-delete-lifecycle.

Cause 3 of 10 — UpdateCodeRepository is an unimplemented scaffold stub.
AWS: UpdateCodeRepository on a deleted name is refused 400 ValidationException with Message "Cannot find CodeRepository: arn:aws:sagemaker:<region>:<account>:code-repository/<name>" — fixed text, variable the code-repository ARN.
Emulator: 400 NotImplemented.
Why: the code-repository lifecycle has no mutation path.
Fix: implement the update-code-repository handler.
Traces: sagemaker.code-repository-create-describe-delete.

Cause 4 of 10 — UpdateAction is an unimplemented scaffold stub on the lineage-action lifecycle.
AWS: UpdateAction on a deleted action is refused ResourceNotFound with Message "Action '<action-arn>' does not exist."
Emulator: 400 NotImplemented.
Why: same as cause 2 for Actions. NOTE: a scaffold-stub issue naming UpdateAction (services/service-com-amazonaws-sagemaker-sagemaker/src/ops/update_action.rs) was auto-filed for the sibling check sagemaker.action-lifecycle-transitions; closing that stub also closes this cause, and the replay key below is recorded here only so the fixer re-replays it.
Fix: implement the update-action handler.
Traces: sagemaker.action-create-describe-delete.

Cause 5 of 10 — DescribeModelPackage REFUSES a model package that carries no InferenceSpecification.
AWS: InferenceSpecification is optional on CreateModelPackage; a package created without it describes cleanly (the member is simply absent from the response).
Emulator: 400 ValidationException with Message 'Model package "<model-package-arn>" carries no InferenceSpecification, which a model package summary reports.' — the emulator refuses its OWN stored record.
Why: a successfully registered model package becomes permanently unreadable, so registry listing/approval flows dead-end.
Fix: the describe-model-package handler's response projection (treat InferenceSpecification as optional) rather than raising from it.
Traces: sagemaker.model-package-unicode-description-round-trip.

Cause 6 of 10 — DescribeCluster/instance-group projection omits NodeProvisioningMode.
AWS: the cluster description reports the cluster's node provisioning mode; the member is present on a cluster created with one.
Emulator: the member is absent from the response, so a client reading it raises KeyError: 'NodeProvisioningMode'.
Why: a client cannot tell how the HyperPod cluster provisions nodes, and a required-by-contract read crashes.
Fix: the describe-cluster handler's response projection.
Traces: sagemaker.cluster-node-add-and-cluster-delete-lifecycle.

Cause 7 of 10 — the default per-account lineage group is never created.
AWS: SageMaker auto-creates one lineage group per account/region (named sagemaker-<region>-<account>), so ListLineageGroups returns at least that entry and GetLineageGroupPolicy can be addressed against it.
Emulator: ListLineageGroups returns an empty list in a fresh account/region, so every lineage-group-policy path is unreachable.
Why: lineage-governance code has no group to address; the absence is indistinguishable from a permissions failure.
Fix: the lineage-group state seeding (create the default group on first use of the account/region), and the list-lineage-groups handler that reads it.
Traces: sagemaker.get-lineage-group-policy-absent-by-default, sagemaker.get-lineage-group-policy-missing, sagemaker.get-lineage-group-policy-not-found.

Cause 8 of 10 — DescribeTrainingJob echoes AlgorithmSpecification.AlgorithmName as the bare name, not the algorithm ARN.
AWS: a training job created against a named Algorithm records and reports that algorithm's ARN in its own AlgorithmSpecification, so the reference survives the algorithm's deletion.
Emulator: reports the bare name ('<algorithm-name>'), so after the algorithm is deleted the job no longer identifies what it ran.
Why: lineage/audit over completed jobs loses the algorithm identity.
Fix: the create-training-job handler's AlgorithmSpecification resolution and the describe-training-job projection that reads it.
Traces: sagemaker.delete-algorithm-conflict-while-referenced-accepted.

Cause 9 of 10 — a training job with an ExperimentConfig auto-associates no trial component.
AWS: a training job submitted with an ExperimentConfig has a trial component created for it and associated to the named trial, so ListTrialComponents(SourceArn=<training-job-arn>) returns it.
Emulator: returns [] — no trial component is ever created for the job.
Why: experiment tracking records nothing for training runs, so metrics/lineage flows have no subject.
Fix: the create-training-job handler's ExperimentConfig handling and the trial-component state it must write.
Traces: sagemaker.e2e-experiment-trial-training-job-metrics-flow.

Cause 10 of 10 — GetScalingConfigurationRecommendation refuses on the wrong key and with the wrong texts.
AWS: Smithy: input InferenceRecommendationsJobName (@required), optional RecommendationId and EndpointName; errors = [ResourceNotFound] (client, 400). Recorded behavior, three distinct templates: (a) when neither RecommendationId nor EndpointName is given, ValidationException "Neither RecommendationId or EndpointName were provided in the input. Please provide a RecommendationId or EndpointName in the request."; (b) when the named job does not exist, "Inference recommendations job <job-name> was not found."; (c) when the job exists, an EndpointName naming a live endpoint resolves rather than being refused. Fixed text as quoted; the only variable part is the job name from the request.
Emulator: answers 'Inference recommendations job <job-name> does not exist.' (wrong wording in (b), and raised even for cases where the endpoint-keyed lookup should resolve), and 'Specify either RecommendationId or EndpointName.' instead of AWS's two-sentence text in (a).
Why: a caller cannot distinguish a missing job from a missing endpoint, and neither refusal matches AWS's wording.
Fix: the get-scaling-configuration-recommendation handler's lookup order (job -> recommendation/endpoint) and its refusal rendering.
Traces: sagemaker.get-scaling-configuration-recommendation-endpoint-not-found, sagemaker.get-scaling-configuration-recommendation-for-valid-endpoint, sagemaker.get-scaling-configuration-recommendation-matches-endpoint, sagemaker.get-scaling-configuration-recommendation-not-found, sagemaker.get-scaling-configuration-recommendation-not-found-for-missing-endpoint, sagemaker.get-scaling-configuration-recommendation-unknown-endpoint-not-found, sagemaker.scaling-configuration-recommendation-for-live-endpoint, sagemaker.scaling-configuration-recommendation-not-found-for-unknown-endpoint.

## Evidence
Replayed against the emulator, representative evidence per cause:

1. sagemaker.add-association-self-reference-rejected: botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the AddAssociation operation: The source and the destination of an association must be different entities; both were "arn:aws:sagemaker:<region>:<account>:artifact/<id>". — the check asserts AWS ACCEPTS this edge; Smithy models no ValidationException on AddAssociation.
2. sagemaker.trial-create-describe-delete: expected UpdateTrial on a deleted trial to raise ResourceNotFound, got 'NotImplemented'
3. sagemaker.code-repository-create-describe-delete: assert update_code == "ValidationException" -> AssertionError: NotImplemented (expected Message 'Cannot find CodeRepository: arn:aws:sagemaker:<region>:<account>:code-repository/<name>')
4. sagemaker.action-create-describe-delete: expected UpdateAction on a deleted action to fail ResourceNotFound, got NotImplemented
5. sagemaker.model-package-unicode-description-round-trip: expected <arn> to describe cleanly, got error {'ErrorCode': 'ValidationException', 'ErrorResponse': 'Model package "<arn>" carries no InferenceSpecification, which a model package summary reports.'}
6. sagemaker.cluster-node-add-and-cluster-delete-lifecycle: KeyError: 'NodeProvisioningMode'
7. sagemaker.get-lineage-group-policy-absent-by-default: no lineage group visible to this account/region; the default per-account lineage group ('sagemaker-<region>-<account>') is normally auto-created but ListLineageGroups returned none here
8. sagemaker.delete-algorithm-conflict-while-referenced-accepted: expected the training job's own AlgorithmSpecification to still name the deleted algorithm's ARN, got '<algorithm-name>'
9. sagemaker.e2e-experiment-trial-training-job-metrics-flow: no trial component auto-associated with training job '<training-job-arn>': []
10. sagemaker.scaling-configuration-recommendation-for-live-endpoint: expected message 'Neither RecommendationId or EndpointName were provided in the input. Please provide a RecommendationId or EndpointName in the request.', got 'Specify either RecommendationId or EndpointName.'; sagemaker.scaling-configuration-recommendation-not-found-for-unknown-endpoint: expected 'Inference recommendations job <name> was not found.', got 'Inference recommendations job <name> does not exist.'; sagemaker.get-scaling-configuration-recommendation-for-valid-endpoint: unexpected message 'Inference recommendations job <name> does not exist.'

## Steps to Reproduce
Re-replay the checks in trace_ids. Cause 1: CreateArtifact, then AddAssociation with SourceArn == DestinationArn. Causes 2-4: create the trial / code repository / action, delete it, then call UpdateTrial / UpdateCodeRepository / UpdateAction on the dead name. Cause 5: CreateModelPackage with no InferenceSpecification, then DescribeModelPackage. Cause 6: CreateCluster, then DescribeCluster and read NodeProvisioningMode. Cause 7: ListLineageGroups in a fresh account/region. Cause 8: CreateAlgorithm, CreateTrainingJob naming it, DeleteAlgorithm, then DescribeTrainingJob and read AlgorithmSpecification. Cause 9: CreateTrainingJob with an ExperimentConfig, then ListTrialComponents(SourceArn=<training-job-arn>). Cause 10: CreateInferenceRecommendationsJob, then GetScalingConfigurationRecommendation with (a) neither RecommendationId nor EndpointName, (b) a job name that does not exist, (c) a live EndpointName.
