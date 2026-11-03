Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-endpoint-model-and-config-ops-reject-invalid-input
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-endpoint-model-and-config-ops-reject-invalid-input sagemaker-model-package-and-pipeline
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|CreateEndpointConfig|4xx->2xx|$.__type
Issue-Key: sagemaker|CreateEndpointConfig|4xx->2xx|$.message
Issue-Key: sagemaker|CreateEndpointConfig|4xx->2xx|status
Issue-Key: sagemaker|CreateEndpoint|4xx->2xx|$.__type
Issue-Key: sagemaker|CreateEndpoint|4xx->2xx|$.message
Issue-Key: sagemaker|CreateEndpoint|4xx->2xx|status
Issue-Key: sagemaker|CreateModelPackage|4xx->2xx|$.__type
Issue-Key: sagemaker|CreateModelPackage|4xx->2xx|$.message
Issue-Key: sagemaker|CreateModelPackage|4xx->2xx|status
Issue-Key: sagemaker|DeleteModelPackage|4xx->2xx|$
Issue-Key: sagemaker|DeleteModelPackage|4xx->2xx|status
Issue-Commit: d24b6997be17cc686e45f4a7148f10449cf3bf59

# SageMaker endpoint and model-package creates skip referenced-resource existence checks, and DeleteModelPackage accepts an absent package

> _Note: this issue was identified against the code base at commit `d24b6997be17cc686e45f4a7148f10449cf3bf59`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEndpointConfig

## Description
Four net-new causes, all in the SageMaker control plane and all the same shape of defect: an operation that must resolve a resource the request NAMES (a Model, an EndpointConfig, an ECR inference image, a ModelPackage) performs no lookup at all, so a request real AWS refuses with 400 ValidationException is accepted with 200 and a fabricated ARN / empty body. Four causes must be closed before this issue is submitted.

Cause 1 of 4: CreateEndpointConfig does not resolve the ModelName named by a production variant.
AWS: Smithy (services/service-com-amazonaws-sagemaker-sagemaker) models CreateEndpointConfig with required EndpointConfigName and required ProductionVariants (ProductionVariantList, @length min 1 max 10); each variant's ModelName names a Model that must already exist in the account/region. The model declares only ResourceLimitExceeded, so the refusal class here is recorded behavior only: the recording shows HTTP 400 with __type 'ValidationException' and message 'Could not find model "<ModelName>".' Fixed text: 'Could not find model "' + '".' — the variable part is the ModelName taken verbatim from the request's ProductionVariants[*].ModelName (the recorded 'sim2real-probe-sagemaker-ep-model-0a08c223' is one run's value, not part of the requirement).
Emulator: answers HTTP 200 with an EndpointConfigArn; the response carries no __type and no message, i.e. the referenced-model lookup never happens.
Why: a client that builds an endpoint config against a model name that was never created (typo, wrong region, a create that failed earlier) is told it succeeded. The failure surfaces much later — or never, since the emulator's CreateEndpoint then also succeeds — so deployment pipelines that rely on this 400 as their guard rail ship a config that can never serve inference.
Fix: CreateEndpointConfig's handler/validation in the sagemaker service crate — resolve every ProductionVariants[].ModelName (and ShadowProductionVariants[].ModelName) against the Model store before persisting the config, and refuse with 400 ValidationException when one is absent.
Traces: sagemaker-endpoint-model-and-config-ops-reject-invalid-input#task#3.

Cause 2 of 4: CreateEndpoint does not resolve EndpointConfigName.
AWS: Smithy models CreateEndpoint with required EndpointName and required EndpointConfigName (EndpointConfigName, @length 0-63, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$); the named endpoint config must already exist. Modeled errors are ResourceLimitExceeded only, so the refusal class is recorded behavior only: HTTP 400, __type 'ValidationException', message 'Could not find endpoint configuration "<EndpointConfigName>".' Fixed text: 'Could not find endpoint configuration "' + '".'; the variable part is the EndpointConfigName from the request.
Emulator: answers HTTP 200 with an EndpointArn and no error body at all.
Why: an endpoint is created referring to a configuration that does not exist, so the emulator reports an endpoint resource with no variants behind it. Clients that treat the 400 as 'config not ready yet, retry/create it first' instead proceed into an inference call that cannot work, and negative tests asserting the refusal pass vacuously.
Fix: CreateEndpoint's handler/validation — look up EndpointConfigName in the endpoint-config state before creating the endpoint and refuse with 400 ValidationException when it is absent.
Traces: sagemaker-endpoint-model-and-config-ops-reject-invalid-input#task#4.

Cause 3 of 4: CreateModelPackage performs no check on the inference container image's ECR registry/repository.
AWS: Smithy models CreateModelPackage with InferenceSpecification (whose Containers carry the Image URI) and declares ConflictException and ResourceLimitExceeded. The image-accessibility refusal is recorded behavior only: when the Image URI names an ECR registry/repository SageMaker cannot pull from, AWS answers HTTP 400 with __type 'ValidationException' and message 'Access denied for registry ID: <registryId>, repository name: <repositoryName>. Please check if your ECR image exists and has proper pull permissions for SageMaker.' Fixed text: 'Access denied for registry ID: ', ', repository name: ', and the trailing 'Please check if your ECR image exists and has proper pull permissions for SageMaker.' sentence; the variable parts are the account id and repository name parsed out of the request's Image URI (the recorded 382416733822 / xgboost are that run's values).
Emulator: answers HTTP 200 with a ModelPackageArn; no __type, no message.
Why: a model package is registered around a container image the account cannot pull, so every downstream use (CreateModel, batch transform, endpoint deployment) is built on an unusable artifact while the one point where AWS catches it reports success. Cross-account image mistakes become silent.
Fix: CreateModelPackage's validation in the sagemaker service crate — parse the registry id and repository name from each InferenceSpecification container Image URI, check it against the emulator's ECR state/pull permission, and refuse with 400 ValidationException when it is not pullable.
Traces: sagemaker-model-package-and-pipeline#task#1.

Cause 4 of 4: DeleteModelPackage on an absent model package returns 200 with an empty body instead of refusing.
AWS: Smithy models DeleteModelPackage with required ModelPackageName (VersionedArnOrName, @length 1-176) and an empty output; the declared error is ConflictException. For a name/ARN that resolves to nothing, the recorded behavior is HTTP 400 with __type 'ValidationException' and message 'ModelPackage <modelPackageArn> does not exist.' Fixed text: 'ModelPackage ' + ' does not exist.'; the variable part is the fully-qualified model-package ARN derived from the request's ModelPackageName (region and account from the endpoint/caller), not the literal ARN in the recording.
Emulator: answers HTTP 200 with an empty JSON body ({}), i.e. delete is unconditionally idempotent.
Why: DeleteModelPackage is the same shape as the 'delete succeeded' response, so a client cannot distinguish 'I deleted it' from 'it was never there'. Cleanup logic that asserts the resource existed, and teardown steps that key off the 400 to decide whether an earlier create landed, both take the wrong branch.
Fix: DeleteModelPackage's handler — resolve ModelPackageName (plain name, ARN, and versioned forms) in state and refuse with 400 ValidationException when there is no such package, before performing the delete.
Traces: sagemaker-model-package-and-pipeline#task#3.

Related open issues that do NOT cover these four steps (the replay reports no owner for them): 20261022T210606Z-sagemaker-accepts-requests-real-aws-refuses-no-s3.md and 20261029T222441Z-sagemaker-control-plane-accepts-ten-classes-of-request.md describe the same family of missing create-time validation on other operations; 20261029T192819Z-sagemaker-delete-and-stop-paths-lack-not-found.md covers other delete/stop not-found gates but not DeleteModelPackage.

## Evidence
Replay of trace set simparity-wire-aws-v1 (94 fixtures, 475 steps, score 68.0%, emulator up and serving).

Cause 1 — CreateEndpointConfig, fixture sagemaker-endpoint-model-and-config-ops-reject-invalid-input step task#3:
  expected: status 400, $.__type = 'ValidationException', $.message = 'Could not find model "sim2real-probe-sagemaker-ep-model-0a08c223".'
  actual:   status 200, $.__type missing (None), $.message missing (None)

Cause 2 — CreateEndpoint, fixture sagemaker-endpoint-model-and-config-ops-reject-invalid-input step task#4:
  expected: status 400, $.__type = 'ValidationException', $.message = 'Could not find endpoint configuration "sim2real-probe-sagemaker-ep-config-0a08c223".'
  actual:   status 200, $.__type missing (None), $.message missing (None)

Cause 3 — CreateModelPackage, fixture sagemaker-model-package-and-pipeline step task#1:
  expected: status 400, $.__type = 'ValidationException', $.message = 'Access denied for registry ID: 382416733822, repository name: xgboost. Please check if your ECR image exists and has proper pull permissions for SageMaker.'
  actual:   status 200, $.__type missing (None), $.message missing (None)

Cause 4 — DeleteModelPackage, fixture sagemaker-model-package-and-pipeline step task#3:
  expected: status 400, body {'__type': 'ValidationException', 'message': 'ModelPackage arn:aws:sagemaker:us-west-2:000000000000:model-package/sim2real-probe-sagemaker-mpkg-443a8941 does not exist.'}
  actual:   status 200, empty_body {}

Model evidence (services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json): CreateEndpointConfig.ProductionVariants required (ProductionVariantList @length 1..10); CreateEndpoint.EndpointConfigName required (@length 0..63, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$); CreateModelPackage errors ConflictException, ResourceLimitExceeded; DeleteModelPackage.ModelPackageName required (VersionedArnOrName @length 1..176), output Unit, error ConflictException. No ValidationException is bound to these operations in the model, so the refusal class/text above is taken from the recorded responses.

## Steps to Reproduce
1. git clone the project and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1: replay fixture sagemaker-endpoint-model-and-config-ops-reject-invalid-input step task#3 — CreateEndpointConfig whose ProductionVariants name a model that was never created. Observe 200 + EndpointConfigArn instead of 400 ValidationException 'Could not find model "<ModelName>".'
4. Cause 2: same fixture, step task#4 — CreateEndpoint naming an EndpointConfigName that does not exist. Observe 200 + EndpointArn instead of 400 ValidationException 'Could not find endpoint configuration "<EndpointConfigName>".'
5. Cause 3: replay fixture sagemaker-model-package-and-pipeline step task#1 — CreateModelPackage whose InferenceSpecification container Image points at an ECR registry/repository the account cannot pull. Observe 200 + ModelPackageArn instead of 400 ValidationException 'Access denied for registry ID: <registryId>, repository name: <repositoryName>. Please check if your ECR image exists and has proper pull permissions for SageMaker.'
6. Cause 4: same fixture, step task#3 — DeleteModelPackage for a model package that does not exist. Observe 200 with an empty body instead of 400 ValidationException 'ModelPackage <arn> does not exist.'
