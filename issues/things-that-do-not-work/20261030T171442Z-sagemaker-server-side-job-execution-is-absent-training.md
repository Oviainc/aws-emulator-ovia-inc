Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.training-job-managed-completion-transition sagemaker.training-job-server-managed-status-progression sagemaker.e2e-training-job-to-served-endpoint sagemaker.e2e-training-to-endpoint-inference-flow sagemaker.e2e-training-job-artifact-propagates-through-model-to-endpoint sagemaker.endpoint-lifecycle-config-to-inservice-to-delete sagemaker.model-endpoint-config-update-flow sagemaker.deploy-endpoint-stack-reaches-inservice-and-serves sagemaker.processing-job-managed-terminal-state sagemaker.automl-job-server-side-orchestration-progress sagemaker.hyperparameter-tuning-job-managed-best-job-population sagemaker.scaling-configuration-recommendation-for-existing-endpoint sagemaker.deploy-endpoint-cfn-serves-inference-component
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker.training-job-managed-completion-transition|run#12|AssertionError|assert final["TrainingJobStatus"] == "Completed", (
Issue-Key: sagemaker.training-job-server-managed-status-progression|run#14|AssertionError|assert final.get("FailureReason") == "AlgorithmError: , exit code: 1", (
Issue-Key: sagemaker.e2e-training-job-to-served-endpoint|_run_linear_learner_training_job#3|AssertionError|raise AssertionError(
Issue-Key: sagemaker.e2e-training-to-endpoint-inference-flow|run#13|AssertionError|assert train_desc["TrainingJobStatus"] == "Completed", train_desc.get("FailureReason")
Issue-Key: sagemaker.e2e-training-job-artifact-propagates-through-model-to-endpoint|run#19|AssertionError|assert train_desc["TrainingJobStatus"] == "Completed", (
Issue-Key: sagemaker.endpoint-lifecycle-config-to-inservice-to-delete|run#12|AssertionError|assert training_desc["TrainingJobStatus"] == "Completed", (
Issue-Key: sagemaker.model-endpoint-config-update-flow|_train_tiny_xgboost_model#4|AssertionError|raise AssertionError(
Issue-Key: sagemaker.deploy-endpoint-stack-reaches-inservice-and-serves|_run_linear_learner_training_job#4|AssertionError|raise AssertionError(
Issue-Key: sagemaker.processing-job-managed-terminal-state|run#10|AssertionError|assert final["ProcessingJobStatus"] == "Completed", (
Issue-Key: sagemaker.automl-job-server-side-orchestration-progress|run#20|AssertionError|assert final_status == "Completed", f"expected the AutoML job to complete, got {final_status}"
Issue-Key: sagemaker.hyperparameter-tuning-job-managed-best-job-population|run#14|AssertionError|assert final["HyperParameterTuningJobStatus"] == "Completed", (
Issue-Key: sagemaker.scaling-configuration-recommendation-for-existing-endpoint|run#17|AssertionError|assert description["Status"] == "COMPLETED", (
Issue-Key: sagemaker|DescribeEndpoint|ValidationException
Issue-Commit: ea8447640442a063890341d04cda7629f55a3fd9

# SageMaker server-side job execution is absent: training, processing, AutoML-V2, hyperparameter-tuning and Inference-Recommender jobs all end Failed with emulator-authored reasons, and a CFN-declared Endpoint is never provisioned (6 causes)

> _Note: this issue was identified against the code base at commit `ea8447640442a063890341d04cda7629f55a3fd9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTrainingJob

## Description
Six distinct root causes, all in the SageMaker service, all grounded in failing behavioral checks that are NOT marked ALREADY FILED or OWNED in this replay. Several open issues describe neighbouring symptoms (20261025T095709Z-sagemaker-training-jobs-never-run-an-accepted-createtrainingjob.md, 20261025T101032Z-sagemaker-training-jobs-never-execute-every-accepted-createtrainingjob.md, 20261025T224154Z-sagemaker-createtrainingjob-is-accepted-but-never-executed-the.md, 20261029T203709Z-sagemaker-an-accepted-createtrainingjob-is-never-executed-the.md, 20261024T160943Z-sagemaker-runs-no-job-work-processing-pipeline-step.md, 20261022T210606Z-sagemaker-cloudformation-declared-sagemaker-resources-are-never-provisioned-and.md); none of them owns the replay keys below, so the causes are filed here and those issues are named for cross-reference. A fixer must close all six before submitting.

Cause 1 of 6 — CreateTrainingJob is accepted but never executed.
AWS: Smithy: CreateTrainingJob -> DescribeTrainingJob.TrainingJobStatus is an enum of InProgress|Completed|Failed|Stopping|Stopped; a valid job with a reachable algorithm image progresses InProgress -> Completed, DescribeTrainingJob.ModelArtifacts.S3ModelArtifacts names an object that exists under OutputDataConfig.S3OutputPath, and SecondaryStatus walks the canonical phases. When the algorithm container itself exits non-zero, AWS sets TrainingJobStatus=Failed with FailureReason of the form "AlgorithmError: <stderr tail>, exit code: <code>" (recorded behavior: the check that overrides the entrypoint to /bin/false asserts FailureReason == "AlgorithmError: , exit code: 1" — fixed text "AlgorithmError: " and ", exit code: ", variable parts are the container's stderr tail and its exit status).
Emulator: every accepted CreateTrainingJob reaches TrainingJobStatus=Failed with the emulator-authored FailureReason "This emulator has no container runtime, so the training job's algorithm image was never started and no model artifacts were written to the output S3 location", no SecondaryStatus progression and no artifact written, which is an AWS-impossible string in an AWS-shaped field.
Why: a client that trains a model and then feeds ModelArtifacts.S3ModelArtifacts into CreateModel/CreateEndpoint can never get past the first step; every downstream deploy/invoke path is unreachable, and a client that parses FailureReason for the AlgorithmError/exit-code form reads prose it cannot classify.
Fix: the CreateTrainingJob handler and the training-job state machine that drives it (services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_training_job.rs and the job-execution/state component behind DescribeTrainingJob) must drive the job to Completed with an artifact at the declared S3 output path, and when the job does fail must render FailureReason in AWS's AlgorithmError form.
Traces: sagemaker.training-job-managed-completion-transition, sagemaker.training-job-server-managed-status-progression, sagemaker.e2e-training-job-to-served-endpoint, sagemaker.e2e-training-to-endpoint-inference-flow, sagemaker.e2e-training-job-artifact-propagates-through-model-to-endpoint, sagemaker.endpoint-lifecycle-config-to-inservice-to-delete, sagemaker.model-endpoint-config-update-flow, sagemaker.deploy-endpoint-stack-reaches-inservice-and-serves.

Cause 2 of 6 — CreateProcessingJob is accepted but never executed.
AWS: Smithy: DescribeProcessingJob.ProcessingJobStatus enum InProgress|Completed|Failed|Stopping|Stopped; a processing container whose entrypoint exits 0 drives the job to Completed server-side with no further client calls.
Emulator: the job ends ProcessingJobStatus=Failed with FailureReason "This emulator has no container runtime, so the processing job's container image was never started".
Why: a client polling for Completed on a trivially successful processing job sees a terminal Failed, so any pipeline gated on processing success aborts.
Fix: the CreateProcessingJob handler and the processing-job state machine (src/ops/create_processing_job.rs plus the job state component).
Traces: sagemaker.processing-job-managed-terminal-state.

Cause 3 of 6 — CreateAutoMLJobV2 performs no candidate generation.
AWS: Smithy: CreateAutoMLJobV2 -> DescribeAutoMLJobV2.AutoMLJobStatus enum Completed|InProgress|Failed|Stopped|Stopping, with AutoMLJobSecondaryStatus walking candidate generation; a valid job over a >=500-row dataset reaches Completed autonomously.
Emulator: the job reaches AutoMLJobStatus=Failed instead of Completed.
Why: AutoML consumers poll for Completed and then read BestCandidate; they get a terminal failure and no candidate.
Fix: the CreateAutoMLJobV2 handler and its job orchestration (src/ops/create_auto_ml_job_v2.rs plus the AutoML state component).
Traces: sagemaker.automl-job-server-side-orchestration-progress.

Cause 4 of 6 — CreateHyperParameterTuningJob launches no child training jobs.
AWS: Smithy: DescribeHyperParameterTuningJob.HyperParameterTuningJobStatus enum Completed|InProgress|Failed|Stopped|Stopping, and BestTrainingJob is populated once the tuning job completes; AWS runs the child training jobs and evaluates the objective metric server-side.
Emulator: the tuning job ends Failed with the emulator-authored reason "This emulator does not run training jobs: no training job was launched for this hyperparameter tuning job, so no objective metric was evaluated and no model artifacts were written to the output S3 location." and BestTrainingJob is never populated.
Why: a client that tunes and then deploys BestTrainingJob's artifact has nothing to deploy.
Fix: the CreateHyperParameterTuningJob handler and the tuning orchestration that must create and drive child training jobs (src/ops/create_hyper_parameter_tuning_job.rs plus the job state component).
Traces: sagemaker.hyperparameter-tuning-job-managed-best-job-population.

Cause 5 of 6 — CreateInferenceRecommendationsJob never benchmarks, so no recommendation exists for GetScalingConfigurationRecommendation.
AWS: Smithy: DescribeInferenceRecommendationsJob.Status enum PENDING|IN_PROGRESS|COMPLETED|FAILED|STOPPING|STOPPED; an Advanced job deploys its own ephemeral endpoint, generates benchmark traffic and reaches COMPLETED with >=1 InferenceRecommendations entry, after which GetScalingConfigurationRecommendation on that job + RecommendationId returns a non-empty DynamicScalingConfiguration.
Emulator: the job ends Status=FAILED with "This emulator runs no model inference container and generates no benchmark traffic, so no load test was performed and no inference recommendation was measured.", so the recommendation lookup is unreachable.
Why: autoscaling consumers cannot obtain any recommendation; the operation is effectively dead.
Fix: the CreateInferenceRecommendationsJob handler and its benchmarking/state component, plus GetScalingConfigurationRecommendation's dependency on a completed job (src/ops/create_inference_recommendations_job.rs and src/ops/get_scaling_configuration_recommendation.rs).
Traces: sagemaker.scaling-configuration-recommendation-for-existing-endpoint.

Cause 6 of 6 — a CloudFormation stack declaring AWS::SageMaker::Model/EndpointConfig/Endpoint reports success but the Endpoint does not exist.
AWS: a CREATE_COMPLETE stack that declares an AWS::SageMaker::Endpoint has actually created it: DescribeEndpoint on the declared EndpointName returns 200 with EndpointStatus eventually InService, and CreateInferenceComponent can attach to it.
Emulator: after the stack completes, DescribeEndpoint on the stack-declared endpoint name is refused 400 ValidationException with message "Could not find endpoint \"<EndpointName>\"" (fixed text "Could not find endpoint " plus the quoted name from the request), i.e. the CFN-declared SageMaker resources were never provisioned.
Why: any infrastructure-as-code deployment reports success while the resource it promised is absent — the worst possible divergence for a deploy tool.
Fix: the CloudFormation resource providers for the AWS::SageMaker::* types (the CFN stack-provisioning component that must call through to the SageMaker create handlers), not DescribeEndpoint itself.
Traces: sagemaker.deploy-endpoint-cfn-serves-inference-component.

## Evidence
Replay output (verbatim tails):
- sagemaker.training-job-managed-completion-transition: AssertionError: expected the tiny valid training job to complete, got Failed (This emulator has no container runtime, so the training job's algorithm image was never started and no model artifacts were written to the output S3 location)
- sagemaker.training-job-server-managed-status-progression: assert final.get("FailureReason") == "AlgorithmError: , exit code: 1" -> AssertionError: unexpected FailureReason for the /bin/false job: "This emulator has no container runtime, so the training job's algorithm image was never started and no model artifacts were written to the output S3 location"
- sagemaker.e2e-training-job-to-served-endpoint / sagemaker.deploy-endpoint-stack-reaches-inservice-and-serves: AssertionError: training job did not complete: status=Failed reason=This emulator has no container runtime, ...
- sagemaker.e2e-training-job-artifact-propagates-through-model-to-endpoint: AssertionError: training job did not complete: Failed (This emulator has no container runtime, ...)
- sagemaker.e2e-training-to-endpoint-inference-flow: AssertionError: This emulator has no container runtime, ...
- sagemaker.endpoint-lifecycle-config-to-inservice-to-delete: AssertionError: expected training job <name> to reach Completed, got Failed (This emulator has no container runtime, ...)
- sagemaker.model-endpoint-config-update-flow: AssertionError: prerequisite training job did not complete: Failed (This emulator has no container runtime, ...)
- sagemaker.processing-job-managed-terminal-state: AssertionError: expected the trivial (exit 0) processing job to complete, got Failed (This emulator has no container runtime, so the processing job's container image was never started)
- sagemaker.automl-job-server-side-orchestration-progress: AssertionError: expected the AutoML job to complete, got Failed
- sagemaker.hyperparameter-tuning-job-managed-best-job-population: AssertionError: expected the tiny tuning job to complete, got Failed (This emulator does not run training jobs: no training job was launched for this hyperparameter tuning job, ...)
- sagemaker.scaling-configuration-recommendation-for-existing-endpoint: AssertionError: inference recommendations job ended in 'FAILED': This emulator runs no model inference container and generates no benchmark traffic, ...
- sagemaker.deploy-endpoint-cfn-serves-inference-component: botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the DescribeEndpoint operation: Could not find endpoint "<stack-declared endpoint name>".

Expected (AWS): Completed/COMPLETED terminal states with artifacts and recommendations as described per cause; a CFN-declared Endpoint resolvable via DescribeEndpoint; an AlgorithmError-form FailureReason when a container exits non-zero.

## Steps to Reproduce
1. CreateTrainingJob with a valid built-in-algorithm image, one ml.m5.large instance and an S3 output path, then poll DescribeTrainingJob: observe Failed with the emulator-authored FailureReason instead of Completed with ModelArtifacts.S3ModelArtifacts pointing at a written object.
2. Repeat with the algorithm entrypoint overridden to /bin/false: observe the same prose instead of FailureReason "AlgorithmError: , exit code: 1".
3. CreateProcessingJob whose container args are `exit 0`, poll DescribeProcessingJob: Failed instead of Completed.
4. CreateAutoMLJobV2 over a >=500-row CSV, poll DescribeAutoMLJobV2: Failed instead of Completed.
5. CreateHyperParameterTuningJob (Strategy=Random, one tiny child job), poll DescribeHyperParameterTuningJob: Failed, BestTrainingJob absent.
6. CreateInferenceRecommendationsJob (Advanced, MaxNumberOfTests=1), poll DescribeInferenceRecommendationsJob: FAILED, so GetScalingConfigurationRecommendation has no completed job to reference.
7. CreateStack with AWS::SageMaker::Model + EndpointConfig + Endpoint, wait for CREATE_COMPLETE, then DescribeEndpoint on the declared EndpointName: 400 ValidationException "Could not find endpoint \"<name>\"".
