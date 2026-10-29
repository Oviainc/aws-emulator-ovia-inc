Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-processing-job-lifecycle
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-processing-job-lifecycle sagemaker-processing-job-workflow sagemaker-training-job-lifecycle sagemaker-training-job-workflow sagemaker-endpoint-model-and-config-ops-reject-invalid-input sagemaker-generic-job-error-paths sagemaker-generic-job-lifecycle sagemaker-model-package-and-pipeline sagemaker-notebook-instance-error-paths
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|CreateProcessingJob|4xx->2xx|$.__type
Issue-Key: sagemaker|CreateProcessingJob|4xx->2xx|$.message
Issue-Key: sagemaker|CreateProcessingJob|4xx->2xx|status
Issue-Key: sagemaker|DescribeProcessingJob|4xx->2xx|$.__type
Issue-Key: sagemaker|DescribeProcessingJob|4xx->2xx|$.message
Issue-Key: sagemaker|DescribeProcessingJob|4xx->2xx|status
Issue-Key: sagemaker|CreateTrainingJob|4xx->2xx|$.__type
Issue-Key: sagemaker|CreateTrainingJob|4xx->2xx|$.message
Issue-Key: sagemaker|CreateTrainingJob|4xx->2xx|status
Issue-Key: sagemaker|DescribeTrainingJob|4xx->2xx|$.__type
Issue-Key: sagemaker|DescribeTrainingJob|4xx->2xx|$.message
Issue-Key: sagemaker|DescribeTrainingJob|4xx->2xx|status
Issue-Key: sagemaker|CreateEndpointConfig|4xx->2xx|$.__type
Issue-Key: sagemaker|CreateEndpointConfig|4xx->2xx|$.message
Issue-Key: sagemaker|CreateEndpointConfig|4xx->2xx|status
Issue-Key: sagemaker|CreateEndpoint|4xx->2xx|$.__type
Issue-Key: sagemaker|CreateEndpoint|4xx->2xx|$.message
Issue-Key: sagemaker|CreateEndpoint|4xx->2xx|status
Issue-Key: sagemaker|CreateJob|4xx->2xx|$.__type
Issue-Key: sagemaker|CreateJob|4xx->2xx|$.message
Issue-Key: sagemaker|CreateJob|4xx->2xx|status
Issue-Key: sagemaker|CreateModelPackage|4xx->2xx|$.__type
Issue-Key: sagemaker|CreateModelPackage|4xx->2xx|$.message
Issue-Key: sagemaker|CreateModelPackage|4xx->2xx|status
Issue-Key: sagemaker|DeleteModelPackage|4xx->2xx|$
Issue-Key: sagemaker|DeleteModelPackage|4xx->2xx|status
Issue-Key: sagemaker|CreateNotebookInstance|4xx->2xx|$.__type
Issue-Key: sagemaker|CreateNotebookInstance|4xx->2xx|$.message
Issue-Key: sagemaker|CreateNotebookInstance|4xx->2xx|status
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# SageMaker create paths accept references and documents AWS refuses

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProcessingJob

## Description
This issue bundles 10 causes that share one root fault: SageMaker create handlers run no admission validation on the request's role, cross-resource references, container image or configuration document, so requests AWS refuses with 400 are accepted with 200 and the fixture's later steps then see resources that should not exist. Fix belongs in the validation stage of each create handler, driven from the same shared reference/role resolver.

Cause 1 of 10: CreateProcessingJob with an unassumable RoleArn. AWS: recorded behavior only — 400 ValidationException, fixed text "Could not assume role <RoleArn>. Please ensure that the role exists and allows principal 'sagemaker.amazonaws.com' to assume the role." Emulator: 200. Fix: CreateProcessingJob validation. Traces: sagemaker-processing-job-lifecycle#task#1 (also sagemaker-processing-job-workflow#task#1).

Cause 2 of 10: DescribeProcessingJob for that refused job. AWS: 400 ValidationException 'Could not find requested job with name: <ProcessingJobName>'. Emulator: 200 describing the job it accepted. Fix: as cause 1, plus the Describe not-found path. Traces: sagemaker-processing-job-lifecycle#task#2.

Cause 3 of 10: CreateTrainingJob with an unassumable RoleArn. AWS: recorded behavior only — 400 ValidationException with the same "Could not assume role <RoleArn>. Please ensure that the role exists and allows principal 'sagemaker.amazonaws.com' to assume the role." text (Smithy CreateTrainingJob models only ResourceInUse/ResourceLimitExceeded/ResourceNotFound, and RoleArn is @required with pattern ^arn:aws[a-z\\-]*:iam::\\d{12}:role/...). Emulator: 200. Fix: CreateTrainingJob validation. Traces: sagemaker-training-job-lifecycle#task#1 (also sagemaker-training-job-workflow#task#1).

Cause 4 of 10: DescribeTrainingJob for that refused job. AWS: 400 ValidationException with the fixed text 'Requested resource not found.' Emulator: 200. Fix: as cause 3. Traces: sagemaker-training-job-lifecycle#task#2.

Cause 5 of 10: CreateEndpointConfig naming a model that does not exist. AWS: recorded behavior only — 400 ValidationException 'Could not find model "<ModelName>".' (the identifier is the request's ModelName, not an ARN). Emulator: 200. Why: a client's wiring error is silently accepted and surfaces much later. Fix: CreateEndpointConfig reference validation. Traces: sagemaker-endpoint-model-and-config-ops-reject-invalid-input#task#3.

Cause 6 of 10: CreateEndpoint naming a non-existent endpoint config. AWS: 400 ValidationException 'Could not find endpoint configuration "<EndpointConfigName>".' Emulator: 200. Fix: CreateEndpoint reference validation. Traces: sagemaker-endpoint-model-and-config-ops-reject-invalid-input#task#4.

Cause 7 of 10: CreateJob with a job-config document that violates the service schema. AWS: 400 ValidationException whose message is the schema diagnostic 'Job config document failed schema validation: object instance has properties which are not allowed by the schema: ["<unknown property>"]; object has missing required properties (["AgentConfig","InputDataConfig","ModelPackageConfig","OutputDataConfig","TrainingConfig"])' — the bracketed unknown-property list comes from the request, the required-property list is fixed. Emulator: 200. Fix: CreateJob document validation. Traces: sagemaker-generic-job-error-paths#task#1 (also sagemaker-generic-job-lifecycle#task#1).

Cause 8 of 10: CreateModelPackage referencing an ECR image the caller cannot pull. AWS: 400 ValidationException 'Access denied for registry ID: <registry id>, repository name: <repository>. Please check if your ECR image exists and has proper pull permissions for SageMaker.' with registry id and repository parsed from the container image URI. Emulator: 200. Fix: CreateModelPackage image-accessibility validation. Traces: sagemaker-model-package-and-pipeline#task#1.

Cause 9 of 10: DeleteModelPackage for the package AWS refused to create. AWS: 400 ValidationException 'ModelPackage <model package arn> does not exist.' Emulator: 200 with an empty body. Fix: as cause 8, plus the Delete not-found path. Traces: sagemaker-model-package-and-pipeline#task#3.

Cause 10 of 10: CreateNotebookInstance in a VPC subnet with no security group. AWS: 400 ValidationException with the fixed message 'Check if the security groups are specified' (no variable parts). Emulator: 200. Fix: CreateNotebookInstance network validation. Traces: sagemaker-notebook-instance-error-paths#task#1.

Related but not covering this: the open issue 'SageMaker accepts requests real AWS refuses: no S3 artifact existence or bucket-policy check, no transform-input check, no image-URI or role validation' describes the same family from a behavioral sweep but carries none of the replay keys above.

## Evidence
Cause 1: expected 400 {'__type':'ValidationException','message':"Could not assume role <role arn>. Please ensure that the role exists and allows principal 'sagemaker.amazonaws.com' to assume the role."}; actual 200. Cause 2: expected 400 ValidationException 'Could not find requested job with name: <name>'; actual 200. Cause 3: expected 400 ValidationException (assume-role text); actual 200. Cause 4: expected 400 ValidationException 'Requested resource not found.'; actual 200. Cause 5: expected 400 ValidationException 'Could not find model "<ModelName>".'; actual 200. Cause 6: expected 400 ValidationException 'Could not find endpoint configuration "<EndpointConfigName>".'; actual 200. Cause 7: expected 400 ValidationException 'Job config document failed schema validation: ...'; actual 200. Cause 8: expected 400 ValidationException 'Access denied for registry ID: <id>, repository name: <repo>. ...'; actual 200. Cause 9: expected 400 ValidationException 'ModelPackage <arn> does not exist.'; actual 200 empty body. Cause 10: expected 400 ValidationException 'Check if the security groups are specified'; actual 200.

## Steps to Reproduce
Replay the wire corpus. Representative: fixture sagemaker-processing-job-lifecycle step task#1. The other nine causes are the fixture#step bindings in trace_ids; each sends a create request whose role, referenced resource, container image or config document is invalid and is answered 200 instead of the recorded 400.
