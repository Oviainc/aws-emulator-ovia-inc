Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.create-cluster-scheduler-config-nonexistent-cluster-rejected sagemaker.create-endpoint-malformed-config-name-rejected sagemaker.create-model-malformed-container-image-rejected sagemaker.create-model-malformed-image-uri-rejected sagemaker.create-endpoint-config-invalid-instance-type-rejected sagemaker.create-endpoint-config-malformed-instance-type-rejected sagemaker.create-endpoint-config-missing-model-rejected sagemaker.create-endpoint-nonexistent-config-rejected sagemaker.endpoint-config-endpoint-full-lifecycle sagemaker.model-create-describe-delete sagemaker.model-create-describe-delete-404 sagemaker.model-create-describe-delete-lifecycle sagemaker.create-training-job-invalid-instance-type-rejected sagemaker.endpoint-resource-limit-enforced-server-side
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker.create-cluster-scheduler-config-nonexistent-cluster-rejected|run#9|AssertionError|assert malformed_message == expected_malformed, (
Issue-Key: sagemaker.create-endpoint-malformed-config-name-rejected|run#19|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: sagemaker.create-model-malformed-container-image-rejected|run#8|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: sagemaker.create-model-malformed-image-uri-rejected|run#7|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: sagemaker.create-endpoint-config-invalid-instance-type-rejected|run#14|AssertionError|assert not_found_message == expected_not_found, (
Issue-Key: sagemaker.create-endpoint-config-malformed-instance-type-rejected|run#15|AssertionError|assert absent_message == expected_absent, (
Issue-Key: sagemaker.create-endpoint-config-missing-model-rejected|run#8|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: sagemaker.create-endpoint-nonexistent-config-rejected|run#14|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: sagemaker.endpoint-config-endpoint-full-lifecycle|run#24|AssertionError|assert gone_message == expected, f"expected message {expected!r}, got {gone_message!r}"
Issue-Key: sagemaker.model-create-describe-delete|run#17|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: sagemaker.model-create-describe-delete-404|run#20|AssertionError|assert message == expected_message, f"expected the message {expected_message!r}, got {message!r}"
Issue-Key: sagemaker.model-create-describe-delete-lifecycle|run#20|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: sagemaker.create-training-job-invalid-instance-type-rejected|run#9|AssertionError|assert message == (
Issue-Key: sagemaker.endpoint-resource-limit-enforced-server-side|_error#11|AssertionError|raise AssertionError(f"expected rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: d2e37a828af0176472a4a50a2ff1b55038dac16f

# SageMaker error fidelity: ValidationException messages carry regex anchors and ARNs instead of request names, enum rejection text diverges, and CreateEndpoint enforces no hosting quota

> _Note: this issue was identified against the code base at commit `d2e37a828af0176472a4a50a2ff1b55038dac16f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEndpoint (bundle also covers CreateEndpointConfig, CreateModel, DescribeModel, DescribeEndpoint, CreateTrainingJob, CreateClusterSchedulerConfig)

## Description
Four independent fidelity defects in SageMaker's rejection path. Each is its own root cause and its own work item; this issue bundles FOUR causes and all four must be closed before submitting.

Cause 1 of 4: the constraint-violation message echoes the Smithy @pattern verbatim, including the ^ and $ anchors.
AWS: the validation-error message is built from the request and the member's constraint, in the fixed form "1 validation error detected: Value '<value>' at '<memberPath>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>", and <pattern> is reported WITHOUT the leading ^ and trailing $ (Smithy: CreateEndpoint.EndpointConfigName @pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$ is reported as [a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}; CreateClusterSchedulerConfig.ClusterArn @pattern ^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:cluster/[a-z0-9]{12}$ is reported without anchors; CreateModel.PrimaryContainer.Image @pattern ^[\S]+$ is reported as [\S]+). Status 400, code ValidationException.
Emulator: same status and code, but the message prints the anchored pattern ("... pattern: ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$").
Why: clients and test suites that parse or compare the documented constraint text (and operators reading it) get a pattern string that is not the one AWS reports; the message rule here is known, so the anchors are part of the contract, not an environment detail.
Fix: the shared request-validation message formatter for the sagemaker service (strip the anchors when rendering a @pattern into the ValidationException message), not the individual handlers.
Traces: sagemaker.create-cluster-scheduler-config-nonexistent-cluster-rejected, sagemaker.create-endpoint-malformed-config-name-rejected, sagemaker.create-model-malformed-container-image-rejected, sagemaker.create-model-malformed-image-uri-rejected.

Cause 2 of 4: not-found messages name the resolved ARN instead of the name supplied in the request.
AWS: recorded behavior only (no modeled error - Smithy models only ResourceLimitExceeded for CreateEndpoint/CreateModel). A reference to a missing model/endpoint-config/endpoint is rejected 400 ValidationException with the message built from the request's name: 'Could not find model "<ModelName>".', 'Could not find endpoint configuration "<EndpointConfigName>".', 'Could not find endpoint "<EndpointName>".'
Emulator: same 400/ValidationException, but substitutes the full ARN it synthesized for the name, e.g. 'Could not find endpoint configuration "arn:aws:sagemaker:<region>:<account>:endpoint-config/<name>".'
Why: the message rule is known (the message quotes the identifier the caller passed); echoing an ARN leaks a synthesized identifier the caller never supplied and breaks consumers that extract the offending name from the message.
Fix: the not-found error construction in the endpoint/endpoint-config/model handlers and the shared state lookup helper that formats these messages - pass through the request's name rather than the ARN.
Traces: sagemaker.create-endpoint-config-invalid-instance-type-rejected, sagemaker.create-endpoint-config-malformed-instance-type-rejected, sagemaker.create-endpoint-config-missing-model-rejected, sagemaker.create-endpoint-nonexistent-config-rejected, sagemaker.endpoint-config-endpoint-full-lifecycle, sagemaker.model-create-describe-delete, sagemaker.model-create-describe-delete-404, sagemaker.model-create-describe-delete-lifecycle.

Cause 3 of 4: an unsupported training InstanceType is rejected with the generic enum-set message instead of AWS's supported-instance-type message.
AWS: Smithy: CreateTrainingJob.ResourceConfig.InstanceType targets the TrainingInstanceType enum; recorded AWS rejects an unsupported value with 400 ValidationException and the message "1 validation error detected: Value 'ml.totally.bogus' at 'resourceConfig.instanceType' failed to satisfy constraint: Member must be a supported instance type".
Emulator: 400 ValidationException, but the message is the generic enum rendering "... failed to satisfy constraint: Member must satisfy enum value set: [ml.m4.xlarge, ml.m4.2xlarge, ...]" listing the whole enum.
Why: the message rule for this member is known and differs from the generic enum rule; a client surfacing the error to a user shows a multi-kilobyte enum dump that real AWS never sends.
Fix: the InstanceType validation in CreateTrainingJob's validation layer (special-case the instance-type enum's message), services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_training_job.rs.
Traces: sagemaker.create-training-job-invalid-instance-type-rejected.

Cause 4 of 4: CreateEndpoint applies no hosting-quota check and accepts an endpoint config whose variant instance type has zero account quota.
AWS: Smithy: CreateEndpoint's only modeled error is ResourceLimitExceeded (HTTP 400). Recorded: an endpoint config whose ProductionVariant requests an instance type the account has no hosting quota for (e.g. ml.p4de.24xlarge at default quota 0) is rejected 400 ResourceLimitExceeded and no endpoint object is created, while the same request for a commonly-available type succeeds.
Emulator: returns HTTP 200 with an EndpointArn and creates the endpoint.
Why: this is an over-permissive accept: a client that relies on the server-side quota rejection believes a deployment it cannot have was admitted, and cost/limit handling paths never run.
Fix: CreateEndpoint's handler plus the per-instance-type hosting-quota check in endpoint state, services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_endpoint.rs.
Traces: sagemaker.endpoint-resource-limit-enforced-server-side.

Related but distinct open issues (they do not cover any cause above): the two router AmbiguousService issues for sagemaker signing-name collisions.

## Evidence
Cause 1: "AssertionError: expected message \"...pattern: [a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}\", got \"...pattern: ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$\"" (create-endpoint-malformed-config-name-rejected); same anchor divergence for 'clusterArn' and for 'primaryContainer.image' ([\S]+ vs ^[\S]+$).
Cause 2: "AssertionError: expected message 'Could not find endpoint configuration \"sim2real-epc-missing-...\".', got 'Could not find endpoint configuration \"arn:aws:sagemaker:us-west-2:000000000000:endpoint-config/sim2real-epc-missing-...\".'"; also 'Could not find model "<name>".' vs '... "arn:aws:sagemaker:...:model/<name>".' and 'Could not find endpoint "<name>".' vs the endpoint ARN form.
Cause 3: "AssertionError: unexpected ValidationException message: \"1 validation error detected: Value 'ml.totally.bogus' at 'resourceConfig.instanceType' failed to satisfy constraint: Member must satisfy enum value set: [ml.m4.xlarge, ...]\"" where the check asserts "... Member must be a supported instance type".
Cause 4: "AssertionError: expected rejection, but the call SUCCEEDED and returned {'EndpointArn': 'arn:aws:sagemaker:us-west-2:000000000000:endpoint/sim2real-ep-overquota-...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}".

## Steps to Reproduce
1. CreateEndpoint(EndpointName=<valid>, EndpointConfigName='bad_config!name') -> 400 ValidationException; compare the reported pattern to AWS's unanchored form. Repeat with CreateClusterSchedulerConfig(ClusterArn='not-an-arn-at-all-0123456789') and CreateModel(PrimaryContainer={'Image':'this is not an ecr image uri', ...}).
2. CreateEndpoint(EndpointName=<valid>, EndpointConfigName=<never created>) -> message quotes a synthesized endpoint-config ARN instead of the name passed. Same for CreateEndpointConfig referencing a missing ModelName, and DescribeModel/DescribeEndpoint after delete.
3. CreateTrainingJob with ResourceConfig.InstanceType='ml.totally.bogus' (otherwise valid request) -> ValidationException whose message dumps the enum value set instead of "Member must be a supported instance type".
4. CreateModel + CreateEndpointConfig with ProductionVariants[0].InstanceType='ml.p4de.24xlarge', then CreateEndpoint against that config -> expect 400 ResourceLimitExceeded and no endpoint; the emulator returns 200 with an EndpointArn.
