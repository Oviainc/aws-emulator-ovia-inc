Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.e2e-training-job-artifact-propagates-through-model-to-endpoint sagemaker.e2e-training-job-to-served-endpoint sagemaker.e2e-training-to-endpoint-inference-flow sagemaker.endpoint-lifecycle-config-to-inservice-to-delete sagemaker.model-endpoint-config-update-flow sagemaker.training-job-managed-completion-transition sagemaker.training-job-server-managed-status-progression sagemaker.deploy-endpoint-stack-reaches-inservice-and-serves sagemaker.processing-job-managed-terminal-state sagemaker.hyperparameter-tuning-job-managed-best-job-population sagemaker.automl-job-server-side-orchestration-progress sagemaker.scaling-configuration-recommendation-for-existing-endpoint sagemaker.notebook-lifecycle-config-applied-server-side sagemaker.batch-replace-cluster-nodes-stale-reference-rejected sagemaker.get-search-suggestions-reflects-created-resource sagemaker.search-suggestions-reflects-created-resource-properties sagemaker.search-suggestions-return-relevant-property-names sagemaker.get-search-suggestions-matches-resource-schema sagemaker.deploy-endpoint-cfn-serves-inference-component
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker.e2e-training-job-artifact-propagates-through-model-to-endpoint|run#19|AssertionError|assert train_desc["TrainingJobStatus"] == "Completed", (
Issue-Key: sagemaker.e2e-training-job-to-served-endpoint|_run_linear_learner_training_job#3|AssertionError|raise AssertionError(
Issue-Key: sagemaker.e2e-training-to-endpoint-inference-flow|run#13|AssertionError|assert train_desc["TrainingJobStatus"] == "Completed", train_desc.get("FailureReason")
Issue-Key: sagemaker.endpoint-lifecycle-config-to-inservice-to-delete|run#12|AssertionError|assert training_desc["TrainingJobStatus"] == "Completed", (
Issue-Key: sagemaker.model-endpoint-config-update-flow|_train_tiny_xgboost_model#4|AssertionError|raise AssertionError(
Issue-Key: sagemaker.training-job-managed-completion-transition|run#12|AssertionError|assert final["TrainingJobStatus"] == "Completed", (
Issue-Key: sagemaker.training-job-server-managed-status-progression|run#14|AssertionError|assert final.get("FailureReason") == "AlgorithmError: , exit code: 1", (
Issue-Key: sagemaker.deploy-endpoint-stack-reaches-inservice-and-serves|_run_linear_learner_training_job#4|AssertionError|raise AssertionError(
Issue-Key: sagemaker.processing-job-managed-terminal-state|run#10|AssertionError|assert final["ProcessingJobStatus"] == "Completed", (
Issue-Key: sagemaker.hyperparameter-tuning-job-managed-best-job-population|run#14|AssertionError|assert final["HyperParameterTuningJobStatus"] == "Completed", (
Issue-Key: sagemaker.automl-job-server-side-orchestration-progress|run#20|AssertionError|assert final_status == "Completed", f"expected the AutoML job to complete, got {final_status}"
Issue-Key: sagemaker.scaling-configuration-recommendation-for-existing-endpoint|run#17|AssertionError|assert description["Status"] == "COMPLETED", (
Issue-Key: sagemaker.notebook-lifecycle-config-applied-server-side|run#17|Timeout|time.sleep(15)
Issue-Key: sagemaker.batch-replace-cluster-nodes-stale-reference-rejected|_wait_for_node_presence#13|Timeout|time.sleep(15)
Issue-Key: sagemaker.get-search-suggestions-reflects-created-resource|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Key: sagemaker.search-suggestions-reflects-created-resource-properties|run#9|Timeout|time.sleep(5)
Issue-Key: sagemaker.search-suggestions-return-relevant-property-names|_poll_until#7|Timeout|time.sleep(interval_s)
Issue-Key: sagemaker.get-search-suggestions-matches-resource-schema|run#10|AssertionError|assert "ExperimentName" not in schema_field_suggestions, (
Issue-Key: sagemaker|DescribeEndpoint|ValidationException
Issue-Commit: 066b7cda09b446bd4bd9be1c3ecfcbe16940769e

# SageMaker performs no job, notebook-hook, cluster-node or search-index work: every training/processing/tuning/AutoML/recommendation job ends Failed and GetSearchSuggestions never indexes (10 causes)

> _Note: this issue was identified against the code base at commit `066b7cda09b446bd4bd9be1c3ecfcbe16940769e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTrainingJob

## Description
The emulator ACCEPTS every asynchronous SageMaker job and then drives it straight to a terminal Failed state with an emulator-authored FailureReason that announces the emulator's own missing runtime, instead of running the work and reporting the status progression AWS reports. Ten causes below, each a distinct runtime/indexer the service never performs. Several open issues name the training-job case for OTHER replay keys (20261025T095709Z-sagemaker-training-jobs-never-run-an-accepted-createtrainingjob.md, 20261025T101032Z-sagemaker-training-jobs-never-execute-every-accepted-createtrainingjob.md, 20261025T224154Z, 20261029T203709Z-sagemaker-an-accepted-createtrainingjob-is-never-executed-the.md, 20261024T160943Z-sagemaker-runs-no-job-work-processing-pipeline-step.md, 20261022T210606Z-sagemaker-cloudformation-declared-sagemaker-resources-are-never-provisioned-and.md); none of them carries the replay keys listed here, and causes 6-10 are not covered by any of them.

Across causes 1-5 the FailureReason is itself a divergence independent of the status: the text is emulator prose about the emulator ("This emulator has no container runtime..."). AWS's FailureReason for a job that genuinely fails describes the JOB's failure (algorithm exit status, data access denial, resource limit) and is empty on a job that Completed. An emulator-authored reason is never a value AWS can produce.

Cause 1 of 10 — training jobs never execute.
AWS: a valid CreateTrainingJob progresses TrainingJobStatus InProgress -> Completed with SecondaryStatus advancing through the canonical phases (Starting, Downloading, Training, Uploading, Completed), writes model.tar.gz under OutputDataConfig.S3OutputPath, and reports no FailureReason.
Emulator: DescribeTrainingJob reports TrainingJobStatus='Failed' with FailureReason "This emulator has no container runtime, so the training job's algorithm image was never started and no model artifacts were written to the output S3 location".
Why: every downstream flow (CreateModel from the artifact, endpoint deploy, inference) is unreachable, and a client's success branch is never exercised.
Fix: the training-job execution/state machine behind create_training_job and describe_training_job.
Traces: sagemaker.e2e-training-job-artifact-propagates-through-model-to-endpoint, sagemaker.e2e-training-job-to-served-endpoint, sagemaker.e2e-training-to-endpoint-inference-flow, sagemaker.endpoint-lifecycle-config-to-inservice-to-delete, sagemaker.model-endpoint-config-update-flow, sagemaker.training-job-managed-completion-transition, sagemaker.deploy-endpoint-stack-reaches-inservice-and-serves.

Cause 2 of 10 — the training job's FailureReason does not attribute the job's own failure.
AWS: a job whose algorithm exits non-zero reports a FailureReason describing THAT (the container's exit status), not the platform.
Emulator: reports the same fixed no-container-runtime prose for a job deliberately built to exit non-zero, so a genuine failure and a platform gap are indistinguishable.
Why: failure-attribution logic keyed on FailureReason cannot classify anything.
Fix: the training-job execution path's failure attribution.
Traces: sagemaker.training-job-server-managed-status-progression.

Cause 3 of 10 — processing jobs never execute.
AWS: a trivial (exit 0) processing job reaches ProcessingJobStatus='Completed'.
Emulator: 'Failed' with FailureReason "This emulator has no container runtime, so the processing job's container image was never started".
Fix: the processing-job execution/state machine.
Traces: sagemaker.processing-job-managed-terminal-state.

Cause 4 of 10 — hyperparameter tuning jobs never launch training jobs.
AWS: a tuning job launches its training jobs, evaluates the objective metric and populates BestTrainingJob, reaching 'Completed'.
Emulator: 'Failed' with FailureReason "This emulator does not run training jobs: no training job was launched for this hyperparameter tuning job, so no objective metric was evaluated and no model artifacts were written to the output S3 location."
Fix: the tuning-job orchestration behind create_hyper_parameter_tuning_job.
Traces: sagemaker.hyperparameter-tuning-job-managed-best-job-population.

Cause 5 of 10 — AutoML jobs never progress.
AWS: an AutoML job progresses to 'Completed' and produces candidates.
Emulator: ends 'Failed'.
Fix: the AutoML job orchestration.
Traces: sagemaker.automl-job-server-side-orchestration-progress.

Cause 6 of 10 — inference recommendations jobs never run a load test.
AWS: an inference recommendations job reaches COMPLETED and yields measured recommendations that GetScalingConfigurationRecommendation can read.
Emulator: the job ends 'FAILED' with FailureReason "This emulator runs no model inference container and generates no benchmark traffic, so no load test was performed and no inference recommendation was measured."
Why: the whole Inference Recommender surface has no reachable success path.
Fix: the inference-recommendations job execution/state machine.
Traces: sagemaker.scaling-configuration-recommendation-for-existing-endpoint.

Cause 7 of 10 — a notebook instance's lifecycle-config OnCreate hook is stored but never executed.
AWS: a NotebookInstance created with a NotebookInstanceLifecycleConfig runs the OnCreate script server-side before reaching InService, and the script's stdout appears in the CloudWatch Logs stream SageMaker emits for the hook.
Emulator: the config is stored, the hook never runs, no log stream content is emitted, and the check exhausts its wait (reported as a 60s timeout on the replay).
Why: a lifecycle config can be registered but its effect can never be observed, so provisioning automation that depends on it silently does nothing.
Fix: the notebook-instance lifecycle-config execution path (and the log stream it must write).
Traces: sagemaker.notebook-lifecycle-config-applied-server-side.

Cause 8 of 10 — HyperPod cluster nodes never reach a Running state.
AWS: a node added by BatchAddClusterNodes appears in ListClusterNodes and advances to Running, after which BatchDeleteClusterNodes removes it and BatchReplaceClusterNodes naming only the removed NodeLogicalId is refused 400 ValidationException with "All provided nodes do not exist. Please check the input and try again."
Emulator: the added node never advances, so the wait for node presence/Running never completes (reported as a 60s timeout on the replay) and the replace refusal is unreachable.
Why: the entire cluster-node lifecycle has no observable progression.
Fix: the cluster-node provisioning state machine behind batch_add_cluster_nodes / list_cluster_nodes.
Traces: sagemaker.batch-replace-cluster-nodes-stale-reference-rejected.

Cause 9 of 10 — GetSearchSuggestions never indexes a created resource's properties.
AWS: after a resource is created, GetSearchSuggestions with a PropertyNameHint returns the matching indexed property names (Tags.<key>, HyperParameters.<key>, Metrics.<key>) for that resource type.
Emulator: returns nothing for a just-created resource, so the polling checks exhaust their wait (reported as 60s timeouts on the replay).
Why: search-assisted UIs and discovery code get no suggestions at all.
Fix: the search-suggestion index that get_search_suggestions reads, and the create paths that must populate it. The open issue 20261029T070612Z-sagemaker-attachclusternodevolume-s-cluster-not-found-message-is.md mentions this surface but does not carry the replay keys below.
Traces: sagemaker.get-search-suggestions-reflects-created-resource, sagemaker.search-suggestions-reflects-created-resource-properties, sagemaker.search-suggestions-return-relevant-property-names.

Cause 10 of 10 — GetSearchSuggestions suggests fixed top-level schema fields, and a CloudFormation-declared endpoint/inference component is never provisioned.
AWS: GetSearchSuggestions suggests only INDEXED properties (Tags/HyperParameters/Metrics sub-paths), never fixed top-level schema fields such as ExperimentName. Separately, a CloudFormation stack declaring an AWS::SageMaker::Endpoint plus inference component provisions them, so DescribeEndpoint resolves the stack-created endpoint.
Emulator: suggests 'ExperimentName' (a fixed schema field); and DescribeEndpoint on the CFN-created endpoint is refused 400 ValidationException 'Could not find endpoint "<endpoint-name>".' — the declared resource was never created.
Why: the suggestion list is wrong in kind, and CFN-driven SageMaker deployment produces no resources.
Fix: the get-search-suggestions property filter; and the CloudFormation resource provisioner for AWS::SageMaker::Endpoint / InferenceComponent.
Traces: sagemaker.get-search-suggestions-matches-resource-schema, sagemaker.deploy-endpoint-cfn-serves-inference-component.

## Evidence
Replayed against the emulator, representative evidence per cause:

1. sagemaker.training-job-managed-completion-transition: expected the tiny valid training job to complete, got Failed (This emulator has no container runtime, so the training job's algorithm image was never started and no model artifacts were written to the output S3 location)
2. sagemaker.training-job-server-managed-status-progression: unexpected FailureReason for the /bin/false job: "This emulator has no container runtime, so the training job's algorithm image was never started and no model artifacts were written to the output S3 location"
3. sagemaker.processing-job-managed-terminal-state: expected the trivial (exit 0) processing job to complete, got Failed (This emulator has no container runtime, so the processing job's container image was never started)
4. sagemaker.hyperparameter-tuning-job-managed-best-job-population: expected the tiny tuning job to complete, got Failed (This emulator does not run training jobs: no training job was launched for this hyperparameter tuning job, so no objective metric was evaluated and no model artifacts were written to the output S3 location.)
5. sagemaker.automl-job-server-side-orchestration-progress: expected the AutoML job to complete, got Failed
6. sagemaker.scaling-configuration-recommendation-for-existing-endpoint: inference recommendations job ended in 'FAILED': This emulator runs no model inference container and generates no benchmark traffic, so no load test was performed and no inference recommendation was measured.
7. sagemaker.notebook-lifecycle-config-applied-server-side: Check timed out after 60.0s while polling CloudWatch Logs for the OnCreate hook marker (check asserts "the lifecycle config was stored but not executed" otherwise)
8. sagemaker.batch-replace-cluster-nodes-stale-reference-rejected: Check timed out after 60.0s while waiting for the added NodeLogicalId to become present/Running in ListClusterNodes
9. sagemaker.get-search-suggestions-reflects-created-resource / search-suggestions-reflects-created-resource-properties / search-suggestions-return-relevant-property-names: Check timed out after 60.0s polling GetSearchSuggestions for the created resource's indexed property names
10. sagemaker.get-search-suggestions-matches-resource-schema: 'ExperimentName' is a fixed top-level schema field, not an indexed Tags/HyperParameters/Metrics property, but it was suggested: ['ExperimentName']; sagemaker.deploy-endpoint-cfn-serves-inference-component: botocore.exceptions.ClientError: An error occurred (ValidationException) when calling the DescribeEndpoint operation: Could not find endpoint "<endpoint-name>".

## Steps to Reproduce
Re-replay the checks in trace_ids. Causes 1-2: CreateTrainingJob with a valid ResourceConfig/AlgorithmSpecification and poll DescribeTrainingJob for TrainingJobStatus/SecondaryStatus and FailureReason; then repeat with an algorithm that exits non-zero and compare the FailureReason. Cause 3: CreateProcessingJob with a trivial exit-0 container and poll DescribeProcessingJob. Cause 4: CreateHyperParameterTuningJob with one tiny training definition and poll for BestTrainingJob. Cause 5: CreateAutoMLJob and poll for Completed. Cause 6: CreateInferenceRecommendationsJob and poll its status. Cause 7: CreateNotebookInstanceLifecycleConfig + CreateNotebookInstance referencing it, then look for the OnCreate hook output in the emitted CloudWatch Logs stream. Cause 8: CreateCluster, BatchAddClusterNodes, then poll ListClusterNodes for the node to become Running. Cause 9: create an Experiment with tags, then poll GetSearchSuggestions(Resource='Experiment', SuggestionQuery.PropertyNameQuery.PropertyNameHint=<tag key>). Cause 10: GetSearchSuggestions and check for fixed schema fields in the suggestions; and deploy a CloudFormation stack declaring an AWS::SageMaker::Endpoint with an inference component, then DescribeEndpoint.
