Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr.complete-layer-upload-empty-rejected ecr.deploy-ecr-lifecycle-policy-cfn-expires-image ecr.deploy-ecr-repository-cfn-enforces-immutable-tag-policy-rejected ecr.deploy-ecr-repository-cfn-then-push-image-succeeds ecr.deploy-ecr-repository-with-lifecycle-policy-behaves ecr.deploy-repository-lifecycle-policy-enforced ecr.e2e-signing-and-status-flow ecr.e2e-signing-configuration-drives-signing-status ecr.put-signing-configuration-invalid-signing-profile-rejected ecr.signing-configuration-rule-set-round-trip ecr.e2e-pull-through-cache-full-flow ecr.e2e-pull-through-cache-rule-consumption-flow ecr.e2e-pull-through-cache-validate-update-cycle ecr.pull-through-cache-rule-update-lifecycle ecr.validate-pull-through-cache-rule-confirms-configured-rule ecr.validate-pull-through-cache-rule-nonexistent-rejected ecr.validate-pull-through-cache-rule-secret-not-found-rejected ecr.update-pull-through-cache-rule-invalid-upstream-rejected ecr.update-pull-through-cache-rule-missing-secret-rejected ecr.update-pull-through-cache-rule-secret-not-found-rejected ecr.pull-through-cache-rule-create-defaults-and-describe-pagination ecr.op-contract-tag-resource ecr.tag-resource-reserved-prefix-rejected ecr.describe-registry-validation-rejected ecr.repository-policy-deny-blocks-layer-upload ecr.registry-scanning-configuration-lifecycle
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr.complete-layer-upload-empty-rejected|run#8|AssertionError|assert code == "EmptyUploadException", f"expected EmptyUploadException, got {code}"
Issue-Key: ecr|PutLifecyclePolicy|RepositoryNotFoundException
Issue-Key: ecr|DescribeRepositories|RepositoryNotFoundException
Issue-Key: ecr|BatchCheckLayerAvailability|RepositoryNotFoundException
Issue-Key: ecr|GetLifecyclePolicy|RepositoryNotFoundException
Issue-Key: ecr|InitiateLayerUpload|RepositoryNotFoundException
Issue-Key: signer|PutSigningProfile|ValidationException
Issue-Key: ecr.e2e-pull-through-cache-full-flow|run#6|AssertionError|assert validation["isValid"] is True, f"a freshly created rule must validate: {validation!r}"
Issue-Key: ecr.e2e-pull-through-cache-rule-consumption-flow|run#7|AssertionError|assert validation["isValid"] is True, validation
Issue-Key: ecr.e2e-pull-through-cache-validate-update-cycle|_parse_failure#15|AssertionError|raise AssertionError(f"failure was not the expected JSON shape: {raw!r}") from exc
Issue-Key: ecr.pull-through-cache-rule-update-lifecycle|run#10|AssertionError|assert validate_resp["isValid"] is True, validate_resp.get("failure")
Issue-Key: ecr.validate-pull-through-cache-rule-confirms-configured-rule|run#8|AssertionError|assert resp["isValid"] is True, resp
Issue-Key: ecr.validate-pull-through-cache-rule-nonexistent-rejected|run#7|AssertionError|assert ok_resp["isValid"] is True, ok_resp
Issue-Key: ecr.validate-pull-through-cache-rule-secret-not-found-rejected|run#14|AssertionError|assert "UNABLE_TO_ACCESS_SECRET" in broken_failure, (
Issue-Key: ecr.update-pull-through-cache-rule-invalid-upstream-rejected|_error#15|AssertionError|raise AssertionError(
Issue-Key: ecr.update-pull-through-cache-rule-missing-secret-rejected|_error#10|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: ecr.update-pull-through-cache-rule-secret-not-found-rejected|_error#13|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: ecr.pull-through-cache-rule-create-defaults-and-describe-pagination|run#7|AssertionError|assert "upstreamRegistry" not in created, (
Issue-Key: ecr.op-contract-tag-resource|run#10|AssertionError|assert code == "InvalidTagParameterException", f"expected InvalidTagParameterException, got {code}"
Issue-Key: ecr.tag-resource-reserved-prefix-rejected|run#10|AssertionError|assert code == "InvalidTagParameterException", f"expected InvalidTagParameterException, got {code}"
Issue-Key: ecr.describe-registry-validation-rejected|run#10|AssertionError|assert code == "SerializationException", f"expected SerializationException, got {code!r}: {payload_bad}"
Issue-Key: ecr.repository-policy-deny-blocks-layer-upload|_client_error#11|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: ecr.registry-scanning-configuration-lifecycle|_poll_until#16|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s (last value: {value!r})")
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# ECR: CompleteLayerUpload is a 501 stub, CFN-provisioned repositories never exist, signing profiles are unusable, and pull-through-cache validate/update skip their contracts (10 causes)

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CompleteLayerUpload

## Description
This issue bundles 10 DISTINCT root causes found in the ecr behavioral replay. All ten must be closed before submitting; each carries its own failing check ids. (The separately-filed ECR UploadLayerPart stub issue, 20261022T154613Z-ecr-eks-logs-ecr-uploadlayerpart-is-a-501-stub-and-a.md, is NOT one of them and its blocked checks are excluded here.)

Cause 1 of 10: CompleteLayerUpload is an unimplemented scaffold stub.
AWS: Smithy ecr#CompleteLayerUpload models EmptyUploadException (client error, HTTP 400, doc: "The specified layer upload does not contain any layer parts.") among its errors; completing an upload for which no UploadLayerPart call was made returns that error, and a later upload of real bytes on the same repository still succeeds.
Emulator: the CompleteLayerUpload call itself answers error code NotImplemented ("operation CompleteLayerUpload is scaffolded but not implemented in this emulator"), so neither the rejection nor the happy path exists. This is a different operation and handler from the already-filed UploadLayerPart stub.
Why: no client can finish a layer upload, so no image can ever be pushed through the documented three-call sequence, and a client probing the empty-upload boundary sees an unmodeled error code.
Fix: implement the CompleteLayerUpload handler in the ecr service crate (services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/ops/complete_layer_upload.rs) including the empty-parts rejection.
Traces: ecr.complete-layer-upload-empty-rejected.

Cause 2 of 10: a CloudFormation stack declaring AWS::ECR::Repository reaches CREATE_COMPLETE but no repository exists.
AWS: recorded behavior only (CloudFormation resource provisioning is not described by the ECR model). CreateStack + the stack_create_complete waiter returning means the AWS::ECR::Repository resource exists, and the plain ECR API calls on that RepositoryName then succeed.
Emulator: the waiter reports CREATE_COMPLETE, then every ECR call naming the declared repository fails with RepositoryNotFoundException (HTTP 400), e.g. on PutLifecyclePolicy, DescribeRepositories, BatchCheckLayerAvailability, GetLifecyclePolicy and InitiateLayerUpload: "The repository with name '<name>' does not exist in the registry with id '<account>'". So the stack creates nothing in ECR while reporting success.
Why: every deploy-then-use workflow (CFN/CDK/pipeline) believes its repository exists; the failure surfaces far from the stack, and no rollback or FAILED status warns the operator.
Fix: the CloudFormation resource provisioner must materialise AWS::ECR::Repository into ECR state (and fail the stack when it cannot), the same gap already filed for other resource types (e.g. 20261022T171541Z-codedeploy-..., 20261022T210606Z-sagemaker-...), none of which covers AWS::ECR::Repository.
Traces: ecr.deploy-ecr-lifecycle-policy-cfn-expires-image, ecr.deploy-ecr-repository-cfn-enforces-immutable-tag-policy-rejected, ecr.deploy-ecr-repository-cfn-then-push-image-succeeds, ecr.deploy-ecr-repository-with-lifecycle-policy-behaves, ecr.deploy-repository-lifecycle-policy-enforced.

Cause 3 of 10: Signer PutSigningProfile refuses a container-signing platform unless signingMaterial is supplied, so no ECR signing scenario can start.
AWS: Smithy signer#PutSigningProfile requires only profileName and platformId; signingMaterial is an OPTIONAL input member (certificate material is only needed by the code-signing platforms, not by the Notation OCI platforms used for container images). A PutSigningProfile with platformId=Notation-OCI-SHA384-ECDSA and no signingMaterial returns 200 with arn/profileVersion.
Emulator: returns ValidationException (HTTP 400), message e.g. "signing material cannot be null.", for exactly that request, so every ECR managed-signing check dies in setup before touching ECR.
Why: the whole ECR image-signing surface (PutSigningConfiguration, GetSigningConfiguration, DescribeImageSigningStatus) is unreachable, and a client that legitimately omits signingMaterial is refused a request AWS accepts.
Fix: drop the unconditional signingMaterial requirement in the signer PutSigningProfile handler's validation and gate it per platformId.
Traces: ecr.e2e-signing-and-status-flow, ecr.e2e-signing-configuration-drives-signing-status, ecr.put-signing-configuration-invalid-signing-profile-rejected, ecr.signing-configuration-rule-set-round-trip.

Cause 4 of 10: ValidatePullThroughCacheRule never reports a well-formed rule as valid and its failure payload is emulator prose instead of AWS's failure document.
AWS: Smithy ecr#ValidatePullThroughCacheRule returns isValid (boolean) and failure (PTCValidateFailure, a string); recorded behavior shows a freshly created, complete rule validating with isValid=true and no failure, and a rule whose credential secret was deleted validating with isValid=false and a failure document carrying a failure code (recorded: UNABLE_TO_ACCESS_SECRET).
Emulator: always returns HTTP 200 with isValid=false and a prose failure that describes the emulator itself, e.g. "...is complete, but authentication to the upstream registry could not be verified: this emulator makes no outbound connection to 'public.ecr.aws'" or "the upstream registry 'quay' requires authentication and the pull through cache rule for '<prefix>' stores neither a credentialArn nor a customRoleArn". The secret-deleted case reports the same no-outbound text instead of the recorded failure code.
Why: a client cannot distinguish a healthy rule from a broken one; an operator validating a rule they just created is told it is invalid, and the diagnosis names the emulator rather than any failure code they can branch on.
Fix: the ValidatePullThroughCacheRule handler must decide validity from stored rule state (prefix, upstream, credential/role presence and resolvability) and emit a failure document in AWS's shape; it must not report an outbound-connectivity excuse as the rule's verdict.
Traces: ecr.e2e-pull-through-cache-full-flow, ecr.e2e-pull-through-cache-rule-consumption-flow, ecr.e2e-pull-through-cache-validate-update-cycle, ecr.pull-through-cache-rule-update-lifecycle, ecr.validate-pull-through-cache-rule-confirms-configured-rule, ecr.validate-pull-through-cache-rule-nonexistent-rejected, ecr.validate-pull-through-cache-rule-secret-not-found-rejected.

Cause 5 of 10: UpdatePullThroughCacheRule performs no validation of credentialArn or customRoleArn and accepts requests AWS refuses.
AWS: Smithy ecr#UpdatePullThroughCacheRule models SecretNotFoundException, UnableToAccessSecretException and InvalidParameterException (all client, HTTP 400); customRoleArn is a modeled ARN member. Recorded behavior: an update naming a Secrets Manager secret that does not exist, or a customRoleArn that is not an ARN, is rejected.
Emulator: returns HTTP 200 and persists the value, e.g. accepting customRoleArn 'not-an-arn' and accepting credentialArn pointing at a deleted/never-created secret, so the rule is stored in a state AWS would never allow.
Why: a misconfiguration is silently accepted at write time and only surfaces (if at all) much later at pull time; the modeled rejection errors are unreachable.
Fix: add existence/format validation of credentialArn and customRoleArn in the UpdatePullThroughCacheRule handler and raise the modeled errors.
Traces: ecr.update-pull-through-cache-rule-invalid-upstream-rejected, ecr.update-pull-through-cache-rule-missing-secret-rejected, ecr.update-pull-through-cache-rule-secret-not-found-rejected.

Cause 6 of 10: CreatePullThroughCacheRule invents an upstreamRegistry member that the request never supplied.
AWS: Smithy ecr#CreatePullThroughCacheRule has upstreamRegistry as an OPTIONAL enum member on both input and output; recorded behavior shows the output omitting upstreamRegistry when the request omitted it.
Emulator: the response includes a derived value (recorded example: upstreamRegistry='k8s' for upstreamRegistryUrl registry.k8s.io) even though the member was absent from the request.
Why: clients that treat member presence as "the caller set it" (round-trip/idempotency and diff logic in IaC tools) see a field they never wrote.
Fix: the CreatePullThroughCacheRule serializer/state must echo only what was supplied rather than back-filling upstreamRegistry.
Traces: ecr.pull-through-cache-rule-create-defaults-and-describe-pagination.

Cause 7 of 10: TagResource reports tag validation failures with the wrong error code.
AWS: Smithy ecr#TagResource models InvalidTagParameterException (client, HTTP 400, doc: tag key max 128 chars, value max 256) distinctly from InvalidParameterException; an invalid tag (over-length key/value, reserved aws: prefix) is rejected with InvalidTagParameterException.
Emulator: returns InvalidParameterException for those same requests.
Why: clients that branch on the error code (retry/skip tagging vs fail the request) take the wrong branch; the modeled InvalidTagParameterException is never produced.
Fix: classify tag-shape failures as InvalidTagParameterException in the TagResource handler's validation.
Traces: ecr.op-contract-tag-resource, ecr.tag-resource-reserved-prefix-rejected.

Cause 8 of 10: a malformed JSON request body is answered with ValidationException instead of the protocol's SerializationException.
AWS: recorded behavior only (a body-shape failure is a protocol-level error and is not in the ecr model, which for DescribeRegistry declares only InvalidParameterException, ValidationException and ServerException). A DescribeRegistry request whose body is a JSON array rather than an object is rejected with error code SerializationException.
Emulator: answers code ValidationException (recorded payload {"__type": "ValidationException", "message": "expected object start, got array start"}).
Why: SDKs and proxies classify SerializationException as a client-side framing bug, not a per-member validation failure; the wrong code sends them down the wrong handling/retry path.
Fix: the JSON protocol request deserializer must raise SerializationException for a body that is not a structure, before per-operation validation runs.
Traces: ecr.describe-registry-validation-rejected.

Cause 9 of 10: SetRepositoryPolicy stores a repository policy document AWS refuses.
AWS: an ECR repository policy is scoped to the repository it is attached to, and real AWS refuses ANY statement carrying a Resource element — "*" and the repository's own ARN alike — with InvalidParameterException (HTTP 400; recorded message mentions an invalid repository policy). Smithy ecr#SetRepositoryPolicy models InvalidParameterException.
Emulator: returns HTTP 200 and echoes the policy, e.g. for a statement {"Effect":"Deny","Principal":{"AWS":"*"},"Action":"ecr:InitiateLayerUpload","Resource":"*"}.
Why: the policy a client believes it attached is not the policy AWS would have accepted; a document that is silently stored here is rejected in production, and a Deny written this way is also never enforced.
Fix: validate the policy document in the SetRepositoryPolicy handler (reject statements carrying Resource) and raise InvalidParameterException. See also the sibling finding filed for service ecr__ecs__iam, where the same handler accepts a Principal naming a nonexistent role.
Traces: ecr.repository-policy-deny-blocks-layer-upload.

Cause 10 of 10: a registry scanning configuration is never applied to repositories.
AWS: recorded behavior only. After PutRegistryScanningConfiguration installs a rule whose repository filter matches a repository, BatchGetRepositoryScanningConfiguration for that repository reports the configured scan frequency and the applied scan filters (scanOnPush true / the rule's frequency, appliedScanFilters non-empty).
Emulator: BatchGetRepositoryScanningConfiguration keeps reporting the default forever (recorded: scanOnPush=false, scanFrequency='MANUAL', appliedScanFilters=[]) for the whole 30 s observation window, i.e. the registry-level configuration has no effect on repository state.
Why: the scanning posture a client configures is invisible to every read that would confirm it, so a scanning rollout cannot be verified or audited.
Fix: apply the stored registry scanning configuration when answering BatchGetRepositoryScanningConfiguration (and when evaluating scanOnPush) in the ecr scanning state.
Traces: ecr.registry-scanning-configuration-lifecycle.

## Evidence
Cause 1 — expected EmptyUploadException, got error code NotImplemented from the CompleteLayerUpload call itself (check ecr.complete-layer-upload-empty-rejected).
Cause 2 — stack_create_complete waiter succeeded; next ECR call raised RepositoryNotFoundException, e.g. "An error occurred (RepositoryNotFoundException) when calling the PutLifecyclePolicy operation: The repository with name 'sim2real-ecr-lp-<id>' does not exist in the registry with id '000000000000'" (also seen on DescribeRepositories, BatchCheckLayerAvailability, GetLifecyclePolicy, InitiateLayerUpload).
Cause 3 — "An error occurred (ValidationException) when calling the PutSigningProfile operation: signing material cannot be null." for put_signing_profile(profileName=..., platformId='Notation-OCI-SHA384-ECDSA', tags=...); expected HTTP 200 with an arn.
Cause 4 — ValidatePullThroughCacheRule returned HTTP 200 {isValid: False, failure: "the stored configuration of the pull through cache rule for '<prefix>' is complete, but authentication to the upstream registry could not be verified: this emulator makes no outbound connection to 'public.ecr.aws'"} for a rule just created; expected isValid=true. For a rule whose secret was deleted, expected a failure document carrying failureCode UNABLE_TO_ACCESS_SECRET, got the same no-outbound prose.
Cause 5 — UpdatePullThroughCacheRule returned HTTP 200 with customRoleArn 'not-an-arn' persisted, and HTTP 200 for a credentialArn naming a secret that does not exist; expected a 400 rejection (SecretNotFoundException / UnableToAccessSecretException / InvalidParameterException).
Cause 6 — CreatePullThroughCacheRule response contained 'upstreamRegistry': 'k8s' although the request omitted upstreamRegistry; expected the member to be absent.
Cause 7 — expected InvalidTagParameterException, got InvalidParameterException (both checks).
Cause 8 — expected SerializationException, got ValidationException {'__type': 'ValidationException', 'message': 'expected object start, got array start'}.
Cause 9 — SetRepositoryPolicy SUCCEEDED (HTTP 200) returning the echoed policyText for a statement carrying "Resource": "*" and again for "Resource": <repositoryArn>; expected InvalidParameterException.
Cause 10 — BatchGetRepositoryScanningConfiguration last observation after PutRegistryScanningConfiguration: {'scanOnPush': False, 'scanFrequency': 'MANUAL', 'appliedScanFilters': []} (condition not met within 30.0s).

## Steps to Reproduce
Replay the behavioral suite and re-run the checks listed in trace_ids. Per cause:
1. CreateRepository, InitiateLayerUpload, then CompleteLayerUpload with a digest and no UploadLayerPart -> observe the error code.
2. cloudformation CreateStack with a template whose only resource is AWS::ECR::Repository {RepositoryName: X}, wait for stack_create_complete, then call ecr DescribeRepositories(repositoryNames=[X]).
3. signer PutSigningProfile(profileName=..., platformId='Notation-OCI-SHA384-ECDSA') with no signingMaterial.
4. CreatePullThroughCacheRule(ecrRepositoryPrefix=P, upstreamRegistryUrl='public.ecr.aws'), then ValidatePullThroughCacheRule(ecrRepositoryPrefix=P) -> inspect isValid/failure.
5. UpdatePullThroughCacheRule(ecrRepositoryPrefix=P, customRoleArn='not-an-arn'); and again with credentialArn naming a nonexistent ecr-pullthroughcache secret.
6. CreatePullThroughCacheRule without upstreamRegistry -> inspect the response keys.
7. TagResource on a repository ARN with a reserved-prefix or over-length tag -> inspect the error code.
8. POST a DescribeRegistry request whose JSON body is an array -> inspect the error code.
9. CreateRepository, then SetRepositoryPolicy with a statement containing a Resource element.
10. PutRegistryScanningConfiguration with a rule matching a repository, then BatchGetRepositoryScanningConfiguration for it.
