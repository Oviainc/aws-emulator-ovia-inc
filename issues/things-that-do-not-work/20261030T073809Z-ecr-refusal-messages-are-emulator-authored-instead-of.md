Issue-Source: trace-sweep-divergence
Issue-Trace: api-lifecycle-policy-preview-and-apply
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: api-lifecycle-policy-preview-and-apply api-repository-creation-template-lifecycle ecr-invalid-arn-format-error ecr-invalid-principal-arn-validation ecr-register-invalid-arn-format-error ecr-register-invalid-principal-arn-error ecr-layer-upload-lifecycle-and-errors
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr|PutLifecyclePolicy|4xx->4xx|$.message
Issue-Key: ecr|CreateRepositoryCreationTemplate|4xx->4xx|$.message
Issue-Key: ecr|RegisterPullTimeUpdateExclusion|4xx->4xx|$.message
Issue-Key: ecr|DeregisterPullTimeUpdateExclusion|4xx->4xx|$.message
Issue-Key: ecr|CompleteLayerUpload|4xx->4xx|$.message
Issue-Commit: 99a6bf4e8a9dd64f73b61f622317c845b92ac2b7

# ECR refusal messages are emulator-authored instead of the text ECR returns

> _Note: this issue was identified against the code base at commit `99a6bf4e8a9dd64f73b61f622317c845b92ac2b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutLifecyclePolicy

## Description
Five net-new root causes sharing one fault: the emulator refuses these ECR requests with the right HTTP status and the right error shape, but writes its own $.message prose instead of the message ECR serves for that error class. The status and code match in every case, so only the message diverges — and the message is part of the contract.

Cause 1 of 5: PutLifecyclePolicy's lifecycle-policy validation failure reports the inner enum mismatch instead of ECR's schema-level sentence.
AWS: Smithy: PutLifecyclePolicy.lifecyclePolicyText (LifecyclePolicyText, @length 100..30720), error InvalidParameterException (client, HTTP 400). Message wording is recorded behavior only; the recording shows HTTP 400 with message "Invalid parameter at 'LifecyclePolicyText' failed to satisfy constraint: 'Lifecycle policy validation failure: instance failed to match exactly one schema (matched 0 out of 4)\n'". Fixed text: "Invalid parameter at '", "' failed to satisfy constraint: 'Lifecycle policy validation failure: instance failed to match exactly one schema (matched 0 out of 4)\n'" including the trailing newline inside the quotes; the only variable part is the member name, LifecyclePolicyText, taken from the request. ECR validates the policy document against its JSON schema and reports the schema-level outcome (the rule did not match any of the alternative rule schemas), never the particular sub-constraint that failed.
Emulator: HTTP 400 whose message is "Invalid parameter at 'LifecyclePolicyText' failed to satisfy constraint: 'Lifecycle policy validation failure: instance value (\"notARealCountType\") not found in enum (possible values: [\"imageCountMoreThan\",\"sinceImagePushed\"])\n'" — it echoes the rejected value from the request and enumerates the accepted countType values, a sentence ECR does not produce.
Why: a client or test that asserts on the lifecycle-policy rejection sentence (the only machine-readable detail the 400 carries, since the code is the generic InvalidParameterException) passes against the emulator and fails against ECR, and anything parsing it for the failed sub-constraint gets data AWS never sends.
Fix: the PutLifecyclePolicy lifecycle-policy validator's error construction — report the schema-level match failure for a malformed rule instead of propagating the inner validator's enum diagnostic.
Traces: api-lifecycle-policy-preview-and-apply#task#9.

Cause 2 of 5: CreateRepositoryCreationTemplate's duplicate-prefix refusal opens with the wrong article/noun phrase.
AWS: Smithy: error TemplateAlreadyExistsException (client, HTTP 400, "The repository creation template already exists. Specify a unique prefix and try again." as shape documentation — documentation, not the runtime message). Recorded runtime message: "A repository creation template with prefix '<prefix>' already exists in the registry with id '<registryId>'". Fixed text: "A repository creation template with prefix '", "' already exists in the registry with id '", "'"; variable parts are the prefix from the request and the registry id from state (the caller's registry/account).
Emulator: same status and shape, but the message begins "The repository creation template with prefix '...'" — the shape's documentation sentence style rather than the runtime wording.
Why: idempotent template provisioning commonly matches on this message to decide "already exists, continue"; the differing leading phrase breaks that match.
Fix: the CreateRepositoryCreationTemplate handler's TemplateAlreadyExistsException message template (stop deriving it from the error shape's documentation).
Traces: api-repository-creation-template-lifecycle#task#3.

Cause 3 of 5: RegisterPullTimeUpdateExclusion's malformed-principalArn refusal invents a sentence instead of reporting the pattern constraint.
AWS: Smithy: RegisterPullTimeUpdateExclusion.principalArn targets PrincipalArn with @pattern ^arn:aws(-[a-z]+)*:iam::[0-9]{12}:(role|user)/[\w+=,.@-]+(/[\w+=,.@-]+)*$ and @length 0..200; error InvalidParameterException (client, HTTP 400). Recorded message: "Invalid parameter at 'principalArn' failed to satisfy constraint: 'Member must satisfy regular expression pattern: <pattern>'". Fixed text: "Invalid parameter at '", "' failed to satisfy constraint: 'Member must satisfy regular expression pattern: ", "'"; variable parts are the member name (principalArn) and the member's modeled @pattern — and notably NOT the rejected value, which AWS does not echo.
Emulator: HTTP 400 with "Invalid parameter at 'principalArn' failed to satisfy constraint: 'The principal ARN '<value>' must be well formed'" — an emulator-authored constraint sentence that also leaks the rejected input back to the caller.
Why: clients and conformance tests key on the "Member must satisfy regular expression pattern:" wording (and on the pattern itself) to distinguish a syntactic ARN rejection from a semantic one; the emulator's sentence matches neither, and its echoing of the input makes the message request-dependent where AWS's is not.
Fix: the input-validation layer for ECR's pattern-constrained members — build the InvalidParameterException message from the member name and the modeled @pattern.
Traces: ecr-invalid-arn-format-error#task#1 (cluster also covers ecr-invalid-principal-arn-validation#task#1, ecr-register-invalid-arn-format-error#task#1, ecr-register-invalid-principal-arn-error#task#1).

Cause 4 of 5: DeregisterPullTimeUpdateExclusion refuses a malformed principalArn with the same invented sentence.
AWS: Smithy: DeregisterPullTimeUpdateExclusion.principalArn targets PrincipalArn with the same @pattern; error InvalidParameterException (client, HTTP 400). Recorded message is the same template as Cause 3: "Invalid parameter at 'principalArn' failed to satisfy constraint: 'Member must satisfy regular expression pattern: <pattern>'", with the member name and modeled pattern as its only variable parts.
Emulator: HTTP 400 with "Invalid parameter at 'principalArn' failed to satisfy constraint: 'The principal ARN '<value>' must be well formed'".
Why: same as Cause 3 — the deregistration path is reached by a different handler, so a fix applied only to the register path leaves this one diverging.
Fix: the DeregisterPullTimeUpdateExclusion input validation (same shared pattern-message construction as Cause 3).
Traces: ecr-invalid-principal-arn-validation#task#2.

Cause 5 of 5: CompleteLayerUpload's already-exists refusal names the layer digest instead of the upload id, and mis-words the registry clause.
AWS: Smithy: CompleteLayerUpload inputs repositoryName, uploadId (@pattern UUID form), layerDigests; error LayerAlreadyExistsException (client, HTTP 400, documented "The image layer already exists in the associated repository."). Recorded runtime message: "Layer with upload id '<uploadId>' already exists in the repository with name '<repositoryName>' in registry with id '<registryId>'". Fixed text: "Layer with upload id '", "' already exists in the repository with name '", "' in registry with id '", "'"; variable parts are the uploadId from the request, the repositoryName from the request and the registry id from state. ECR identifies the already-completed upload by its UPLOAD ID, and the registry clause has no article ("in registry with id").
Emulator: HTTP 400 with "The layer with digest '<sha256 digest>' already exists in the repository with name '<repositoryName>' in the registry with id '<registryId>'" — a different leading phrase, the layer digest substituted for the upload id, and "in the registry with id".
Why: a push client that retries CompleteLayerUpload (the normal recovery when the first completion's response was lost) parses this message to recover the upload it already finished; keyed on the upload id it finds a digest instead, so retry/idempotency handling diverges.
Fix: the CompleteLayerUpload handler's LayerAlreadyExistsException message construction (and the layer state it reads the identifying upload id from).
Traces: ecr-layer-upload-lifecycle-and-errors#task#7.

Note: an existing open issue, 20261022T234922Z-ecr-completelayerupload-is-a-501-stub-cfn-provisioned.md, concerns CompleteLayerUpload answering 501; the operation now answers 400 and only its refusal message diverges, so Cause 5 is net-new and not covered by it.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from mainline (66 fixtures, 456 steps, 10 mismatched). All five clusters matched the expected HTTP status (400) and differed only at $.message:

1) api-lifecycle-policy-preview-and-apply#task#9 PutLifecyclePolicy — expected 400/got 400; expected message "Invalid parameter at 'LifecyclePolicyText' failed to satisfy constraint: 'Lifecycle policy validation failure: instance failed to match exactly one schema (matched 0 out of 4)\n'", got "Invalid parameter at 'LifecyclePolicyText' failed to satisfy constraint: 'Lifecycle policy validation failure: instance value (\"notARealCountType\") not found in enum (possible values: [\"imageCountMoreThan\",\"sinceImagePushed\"])\n'".
2) api-repository-creation-template-lifecycle#task#3 CreateRepositoryCreationTemplate — expected "A repository creation template with prefix '<prefix>' already exists in the registry with id '<registryId>'", got "The repository creation template with prefix '<prefix>' already exists in the registry with id '<registryId>'".
3) ecr-invalid-arn-format-error#task#1 RegisterPullTimeUpdateExclusion (cluster of 4: also ecr-invalid-principal-arn-validation#task#1, ecr-register-invalid-arn-format-error#task#1, ecr-register-invalid-principal-arn-error#task#1) — expected "Invalid parameter at 'principalArn' failed to satisfy constraint: 'Member must satisfy regular expression pattern: ^arn:aws(-[a-z]+)*:iam::[0-9]{12}:(role|user)/[\\w+=,.@-]+(/[\\w+=,.@-]+)*$'", got "Invalid parameter at 'principalArn' failed to satisfy constraint: 'The principal ARN '<value>' must be well formed'".
4) ecr-invalid-principal-arn-validation#task#2 DeregisterPullTimeUpdateExclusion — same expected pattern message, same invented "must be well formed" sentence.
5) ecr-layer-upload-lifecycle-and-errors#task#7 CompleteLayerUpload — expected "Layer with upload id '<uploadId>' already exists in the repository with name '<repositoryName>' in registry with id '<registryId>'", got "The layer with digest '<sha256:...>' already exists in the repository with name '<repositoryName>' in the registry with id '<registryId>'".

Model facts used: PrincipalArn @pattern ^arn:aws(-[a-z]+)*:iam::[0-9]{12}:(role|user)/[\w+=,.@-]+(/[\w+=,.@-]+)*$ @length 0..200; InvalidParameterException / TemplateAlreadyExistsException / LayerAlreadyExistsException all client errors at HTTP 400 with a single `message` member; PutLifecyclePolicy.lifecyclePolicyText @length 100..30720; CompleteLayerUpload.uploadId @pattern UUID form.

## Steps to Reproduce
1. Replay fixtures api-lifecycle-policy-preview-and-apply (task#9), api-repository-creation-template-lifecycle (task#3), ecr-invalid-arn-format-error (task#1), ecr-invalid-principal-arn-validation (task#2) and ecr-layer-upload-lifecycle-and-errors (task#7) against the emulator.
2. PutLifecyclePolicy with a rule whose countType is not a modeled value: the 400 message must be the schema-level 'instance failed to match exactly one schema (matched 0 out of 4)' sentence, not an enum diagnostic echoing the value.
3. CreateRepositoryCreationTemplate twice with the same prefix: the second 400 message must begin 'A repository creation template with prefix ...'.
4. Register/DeregisterPullTimeUpdateExclusion with a principalArn that does not match the modeled pattern: the 400 message must be 'Invalid parameter at 'principalArn' failed to satisfy constraint: 'Member must satisfy regular expression pattern: <modeled pattern>'' and must not echo the rejected value.
5. CompleteLayerUpload a second time for an upload already completed: the 400 message must be keyed on the upload id and read 'Layer with upload id '<uploadId>' already exists in the repository with name '<name>' in registry with id '<id>''.
