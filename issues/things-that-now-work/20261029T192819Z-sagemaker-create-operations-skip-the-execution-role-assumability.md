Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-ai-benchmark-job-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-ai-benchmark-job-lifecycle sagemaker-ai-benchmark-job-workflow sagemaker-ai-workload-benchmark-and-recommendation sagemaker-automl-job-and-v2-workflow sagemaker-automl-job-v2-create sagemaker-cluster-scheduler-and-quota-lifecycle sagemaker-cluster-scheduler-config-and-compute-quota-error-paths sagemaker-cluster-scheduler-quota-lifecycle sagemaker-cluster-scheduler-quota-notfound sagemaker-endpoint-model-and-config-ops-reject-invalid-input sagemaker-hyperparameter-tuning-job-lifecycle sagemaker-mlflow-app-error-paths sagemaker-mlflow-app-lifecycle sagemaker-image-version-lifecycle
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|CreateAIBenchmarkJob|4xx->2xx|$.__type
Issue-Key: sagemaker|CreateAIBenchmarkJob|4xx->2xx|$.message
Issue-Key: sagemaker|CreateAIBenchmarkJob|4xx->2xx|status
Issue-Key: sagemaker|CreateAutoMLJobV2|4xx->2xx|$.__type
Issue-Key: sagemaker|CreateAutoMLJobV2|4xx->2xx|$.message
Issue-Key: sagemaker|CreateAutoMLJobV2|4xx->2xx|status
Issue-Key: sagemaker|CreateCluster|4xx->2xx|$.__type
Issue-Key: sagemaker|CreateCluster|4xx->2xx|$.message
Issue-Key: sagemaker|CreateCluster|4xx->2xx|status
Issue-Key: sagemaker|DeleteCluster|4xx->4xx|$.Message
Issue-Key: sagemaker|DeleteCluster|4xx->4xx|$.__type
Issue-Key: sagemaker|CreateModel|4xx->2xx|$.__type
Issue-Key: sagemaker|CreateModel|4xx->2xx|$.message
Issue-Key: sagemaker|CreateModel|4xx->2xx|status
Issue-Key: sagemaker|CreateHyperParameterTuningJob|4xx->2xx|$.__type
Issue-Key: sagemaker|CreateHyperParameterTuningJob|4xx->2xx|$.message
Issue-Key: sagemaker|CreateHyperParameterTuningJob|4xx->2xx|status
Issue-Key: sagemaker|StopHyperParameterTuningJob|4xx->2xx|$
Issue-Key: sagemaker|StopHyperParameterTuningJob|4xx->2xx|status
Issue-Key: sagemaker|DescribeHyperParameterTuningJob|4xx->2xx|$.Message
Issue-Key: sagemaker|DescribeHyperParameterTuningJob|4xx->2xx|$.__type
Issue-Key: sagemaker|DescribeHyperParameterTuningJob|4xx->2xx|status
Issue-Key: sagemaker|CreateMlflowApp|4xx->2xx|$.__type
Issue-Key: sagemaker|CreateMlflowApp|4xx->2xx|$.message
Issue-Key: sagemaker|CreateMlflowApp|4xx->2xx|status
Issue-Key: sagemaker|DescribeImage|2xx->2xx|$.FailureReason
Issue-Key: sagemaker|DescribeImage|2xx->2xx|$.ImageStatus
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# SageMaker create operations skip the execution-role assumability check

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAIBenchmarkJob

## Description
This issue bundles 10 causes that share one root fault: every SageMaker create path that takes a RoleArn/ExecutionRoleArn accepts a role that cannot be assumed by sagemaker.amazonaws.com. AWS validates assumability synchronously and refuses with 400 ValidationException; the emulator creates the resource and answers 200, which then makes the whole downstream lifecycle of that fixture diverge. Fix belongs in one shared role-assumability guard invoked from each create handler's validation stage (and, for Image, in the image-status machine that must land CREATE_FAILED).

Cause 1 of 10: CreateAIBenchmarkJob. AWS: recorded behavior only (Smithy CreateAIBenchmarkJob declares no ValidationException in its error list, so the recording is the authority): 400 with __type 'ValidationException' and the fixed message 'Unable to assume the provided RoleArn. Ensure the role exists and its trust policy allows this service to assume it.' (no variable parts). Emulator: 200 with a created job. Why: a client that expects a refusal for an unusable role gets a job that can never run, and its error handling is never exercised. Fix: CreateAIBenchmarkJob handler validation. Traces: sagemaker-ai-benchmark-job-lifecycle#task#2 (also sagemaker-ai-benchmark-job-workflow#task#2, sagemaker-ai-workload-benchmark-and-recommendation#task#4).

Cause 2 of 10: CreateAutoMLJobV2. AWS: recorded behavior only — 400 ValidationException, message fixed text 'Could not assume role <RoleArn>. Please ensure that the role exists and allows principal 'sagemaker.amazonaws.com' to assume the role.' where <RoleArn> is the request's RoleArn. Emulator: 200. Why: as above. Fix: CreateAutoMLJobV2 handler validation. Traces: sagemaker-automl-job-and-v2-workflow#task#2 (also sagemaker-automl-job-v2-create#task#1).

Cause 3 of 10: CreateCluster. AWS: recorded behavior only — 400 ValidationException, fixed text 'SageMaker HyperPod cannot assume the execution role <role arn>. Please ensure that the execution role allows the action sts:AssumeRole for the service principal sagemaker.amazonaws.com.' with the role ARN taken from InstanceGroups[].ExecutionRole. Emulator: 200 with a cluster in Creating. Fix: CreateCluster handler validation. Traces: sagemaker-cluster-scheduler-and-quota-lifecycle#task#1 (also sagemaker-cluster-scheduler-config-and-compute-quota-error-paths#task#1, sagemaker-cluster-scheduler-quota-lifecycle#task#1, sagemaker-cluster-scheduler-quota-notfound#task#2).

Cause 4 of 10: DeleteCluster after that rejected create. AWS: because the create was refused, the cluster does not exist, so DeleteCluster answers 400 with __type 'ResourceNotFound' and message 'Cluster with name <ClusterName> not found' (fixed text, name from the request). Emulator: 400 with __type 'ConflictException' and 'Cluster "<cluster arn>" cannot be deleted while it is in Creating status.' Why: the client sees the wrong error class and a non-existent resource reported as busy. Fix: CreateCluster validation (root) plus the DeleteCluster not-found/state precedence. Traces: sagemaker-cluster-scheduler-and-quota-lifecycle#task#2 (also sagemaker-cluster-scheduler-config-and-compute-quota-error-paths#task#6, sagemaker-cluster-scheduler-quota-lifecycle#task#6).

Cause 5 of 10: CreateModel. AWS: recorded behavior only — 400 ValidationException, fixed text 'The execution role ARN "<ExecutionRoleArn>" is invalid. Please ensure that the role exists and that its trust relationship policy allows the action "sts:AssumeRole" for the service principal "sagemaker.amazonaws.com".' Emulator: 200. Fix: CreateModel handler validation. Traces: sagemaker-endpoint-model-and-config-ops-reject-invalid-input#task#2.

Cause 6 of 10: CreateHyperParameterTuningJob. AWS: recorded behavior only — 400 ValidationException, fixed text "SageMaker is unable to perform: sts:AssumeRole on the role. Check if you have a trust relationship for 'sagemaker.amazonaws.com' to assume the role." Emulator: 200. Fix: CreateHyperParameterTuningJob handler validation. Traces: sagemaker-hyperparameter-tuning-job-lifecycle#task#1.

Cause 7 of 10: StopHyperParameterTuningJob for a job AWS never created. AWS: 400 with __type 'ResourceNotFound' and message "Amazon SageMaker can't find a tuning job called <HyperParameterTuningJobName>. Type another name." Emulator: 200 with an empty body. Fix: same create guard, plus the Stop handler's not-found path. Traces: sagemaker-hyperparameter-tuning-job-lifecycle#task#2.

Cause 8 of 10: DescribeHyperParameterTuningJob for that job. AWS: 400 ResourceNotFound with the same "Amazon SageMaker can't find a tuning job called <name>. Type another name." text. Emulator: 200 describing the job it should not have created. Fix: as cause 6/7. Traces: sagemaker-hyperparameter-tuning-job-lifecycle#task#4.

Cause 9 of 10: CreateMlflowApp. AWS: recorded behavior only — 400 ValidationException with the same sts:AssumeRole trust-relationship text as cause 6. Emulator: 200. Fix: CreateMlflowApp handler validation. Traces: sagemaker-mlflow-app-error-paths#task#1 (also sagemaker-mlflow-app-lifecycle#task#1).

Cause 10 of 10: DescribeImage after CreateImage with an unassumable role. AWS: the image settles to ImageStatus 'CREATE_FAILED' and carries FailureReason 'Unable to assume role with RoleArn <RoleArn>'. Emulator: ImageStatus 'CREATED' and no FailureReason. Why: a client polling for CREATE_FAILED waits for a terminal failure that never arrives. Fix: CreateImage/DescribeImage status machine plus the shared role guard. Traces: sagemaker-image-version-lifecycle#task#3.

## Evidence
Cause 1: expected status 400 {'__type':'ValidationException','message':'Unable to assume the provided RoleArn. Ensure the role exists and its trust policy allows this service to assume it.'}; actual 200 with a created job. Cause 2: expected 400 ValidationException "Could not assume role arn:aws:iam::<acct>:role/<name>. Please ensure that the role exists and allows principal 'sagemaker.amazonaws.com' to assume the role."; actual 200. Cause 3: expected 400 ValidationException 'SageMaker HyperPod cannot assume the execution role <arn>. Please ensure that the execution role allows the action sts:AssumeRole for the service principal sagemaker.amazonaws.com.'; actual 200. Cause 4: expected __type 'ResourceNotFound' / 'Cluster with name <name> not found'; actual __type 'ConflictException' / 'Cluster "<arn>" cannot be deleted while it is in Creating status.'. Cause 5: expected 400 ValidationException 'The execution role ARN "<arn>" is invalid. ...'; actual 200. Causes 6-8: expected 400 ValidationException sts:AssumeRole text, then 400 ResourceNotFound "Amazon SageMaker can't find a tuning job called <name>. Type another name." twice; actual 200/200/200. Cause 9: expected 400 ValidationException; actual 200. Cause 10: expected ImageStatus 'CREATE_FAILED' + FailureReason 'Unable to assume role with RoleArn <arn>'; actual ImageStatus 'CREATED', FailureReason absent. Smithy: CreateTrainingJob/CreateTransformJob-style create ops model only ResourceInUse/ResourceLimitExceeded/ResourceNotFound, so the ValidationException wording above is taken from the recorded responses.

## Steps to Reproduce
Replay the wire corpus (replay_wire_corpus). The representative step is fixture sagemaker-ai-benchmark-job-lifecycle step task#2; the other nine causes are the fixture#step bindings listed in trace_ids. Each is a create call whose RoleArn/ExecutionRoleArn names a role that does not grant sts:AssumeRole to sagemaker.amazonaws.com (or a non-existent role); the emulator answers 200 instead of the recorded 400.
