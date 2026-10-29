Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.get-open-id-connect-provider-contract iam.op-contract-create-openid-connect-provider iam.e2e-oidc-invalid-url-scheme-rejected iam.op-contract-deleteOpenIDConnectProvider iam.op-contract-attach-role-policy iam.op-contract-attach-user-policy iam.op-contract-tag-user iam.change-password-invalid-user-type-rejected iam.service-linked-role-immutability-rejected iam.service-specific-credential-unsupported-service-rejected iam.unsupported-service-specific-credential-rejected iam.iam-entity-names-case-insensitive-unique
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.get-open-id-connect-provider-contract|run#12|AssertionError|assert isinstance(thumbprints, list) and len(thumbprints) >= 1, f"expected a non-empty ThumbprintList, got {thumbprints!r}"
Issue-Key: iam.op-contract-create-openid-connect-provider|run#9|AssertionError|assert len(thumbprints) >= 1, "expected IAM to auto-fetch a thumbprint"
Issue-Key: iam.e2e-oidc-invalid-url-scheme-rejected|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: iam.op-contract-deleteOpenIDConnectProvider|_error#12|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: iam.op-contract-attach-role-policy|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: iam.op-contract-attach-user-policy|_error#10|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: iam.op-contract-tag-user|_error#8|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: iam.change-password-invalid-user-type-rejected|run#18|AssertionError|assert error.get("Code") == "NoSuchEntity", f"expected NoSuchEntity, got {error!r}"
Issue-Key: iam.service-linked-role-immutability-rejected|run#6|AssertionError|assert code_delete == "UnmodifiableEntity", (
Issue-Key: iam.service-specific-credential-unsupported-service-rejected|run#8|AssertionError|assert code == "NoSuchEntity", f"expected NoSuchEntity, got {code}"
Issue-Key: iam.unsupported-service-specific-credential-rejected|run#6|AssertionError|assert code == "NoSuchEntity", f"expected NoSuchEntity, got {code}"
Issue-Key: iam|GetPolicy|NoSuchEntity
Issue-Commit: f5ba8024c64455531a4bd3d9779b46ec0fe104e4

# iam: 10 net-new validation and accept/reject root causes — OIDC thumbprint never derived, idempotent OIDC delete, AttachUserPolicy/TagUser accept what AWS refuses, service-linked-role immutability, policy ARNs with a path unreadable

> _Note: this issue was identified against the code base at commit `f5ba8024c64455531a4bd3d9779b46ec0fe104e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateOpenIDConnectProvider

## Description
Ten distinct net-new root causes where IAM accepts or classifies a request differently from AWS. Each must be closed before this issue is submitted.

Cause 1 of 10: CreateOpenIDConnectProvider never derives a thumbprint, so the provider has an empty ThumbprintList.
AWS: when ThumbprintList is omitted, IAM fetches the provider's TLS certificate chain and stores the derived thumbprint, so the created provider and GetOpenIDConnectProvider both report a non-empty ThumbprintList. Smithy: CreateOpenIDConnectProvider's ThumbprintList is optional and the operation models OpenIdIdpCommunicationErrorException (OpenIdIdpCommunicationError, 400) for a failed fetch — the fetch is part of the operation.
Emulator: the provider is created with ThumbprintList == [] and no communication error is raised.
Why: a client that relies on IAM to pin the IdP's certificate gets an unpinned provider, and any read-back assertion on the thumbprint fails.
Fix: the thumbprint-derivation step of the create_open_id_connect_provider handler (and the stored provider record it writes).
Traces: iam.get-open-id-connect-provider-contract, iam.op-contract-create-openid-connect-provider

Cause 2 of 10: CreateOpenIDConnectProvider's non-https Url refusal carries an emulator-authored message where the recording carries none.
AWS: in the recording the refusal for a Url that does not begin with https:// carries no Message member at all.
Emulator: adds 'Invalid Url <Url>: the URL must begin with https://' — text AWS does not emit for this refusal.
Why: a client comparing the refusal against AWS's sees a message where AWS sends none.
Fix: the Url validation refusal in the create_open_id_connect_provider handler must reproduce the recorded refusal shape rather than author its own sentence.
Traces: iam.e2e-oidc-invalid-url-scheme-rejected

Cause 3 of 10: DeleteOpenIDConnectProvider on an already-deleted provider succeeds instead of refusing.
AWS: the second delete is rejected — the ARN names no entity. Smithy: DeleteOpenIDConnectProvider models NoSuchEntityException (NoSuchEntity, HTTP 404).
Emulator: answers HTTP 200 for the repeat delete.
Why: a client cannot distinguish 'deleted by me' from 'never existed', and a delete-twice guard never fires.
Fix: the existence check in the delete_open_id_connect_provider handler.
Traces: iam.op-contract-deleteOpenIDConnectProvider

Cause 4 of 10: AttachRolePolicy's reserved-policy refusal is re-worded.
AWS: attaching an AWS service-role-reserved managed policy to a customer role is refused with the message 'Cannot attach AWS reserved policy to an IAM role.' Smithy: AttachRolePolicy models PolicyNotAttachableException (PolicyNotAttachable, HTTP 400).
Emulator: answers 'Policy <PolicyArn> is not attachable.' — generic text that is not AWS's sentence for this class.
Why: message-matching clients and operators diverge.
Fix: the PolicyNotAttachable message construction in the attach_role_policy handler.
Traces: iam.op-contract-attach-role-policy

Cause 5 of 10: AttachUserPolicy accepts a managed policy AWS refuses to attach.
AWS: the attach is refused (PolicyNotAttachable, HTTP 400 — the policy is not attachable to a user). Smithy: AttachUserPolicy models PolicyNotAttachableException.
Emulator: answers HTTP 200 and records the attachment.
Why: a user ends up carrying a policy AWS would never attach, so the emulated account diverges structurally from a real one.
Fix: the attachability pre-flight check in the attach_user_policy handler.
Traces: iam.op-contract-attach-user-policy

Cause 6 of 10: TagUser accepts a tag AWS rejects.
AWS: the call is refused. Smithy: TagUser models InvalidInputException (InvalidInput, 400) and LimitExceededException (LimitExceeded, 409) for tag-shape and tag-count violations.
Emulator: answers HTTP 200 and stores the tag.
Why: tag constraints AWS enforces at write time are silently accepted, so state the real service forbids is reachable.
Fix: the Tags validation in the tag_user handler.
Traces: iam.op-contract-tag-user

Cause 7 of 10: ChangePassword under a role session reports InvalidUserType instead of NoSuchEntity.
AWS: calling ChangePassword with credentials that are not an IAM user's is refused with code NoSuchEntity (HTTP 404). Smithy: ChangePassword models both NoSuchEntityException (NoSuchEntity, 404) and InvalidUserTypeException (InvalidUserType, 400), and the recording shows NoSuchEntity for this case.
Emulator: answers InvalidUserType with 'The request was rejected because the type of user for the transaction was incorrect.'
Why: a client branching on NoSuchEntity treats an InvalidUserType refusal as an unexpected failure.
Fix: the caller-type classification in the change_password handler.
Traces: iam.change-password-invalid-user-type-rejected

Cause 8 of 10: service-linked roles are not protected — DeleteRole and UpdateAssumeRolePolicy answer NoSuchEntity instead of UnmodifiableEntity.
AWS: a service-linked role under /aws-service-role/ cannot be deleted or have its trust policy modified by the account; the refusal is UnmodifiableEntity (HTTP 400). Smithy: DeleteRole models UnmodifiableEntityException (UnmodifiableEntity, 400) alongside NoSuchEntityException.
Emulator: both operations answer NoSuchEntity, i.e. they claim the role does not exist.
Why: a client distinguishing 'protected' from 'absent' gets the wrong answer, and tooling that guards on UnmodifiableEntity never sees it.
Fix: the service-linked-role guard in the delete_role and update_assume_role_policy handlers (the role record must be found and then refused as unmodifiable).
Traces: iam.service-linked-role-immutability-rejected

Cause 9 of 10: CreateServiceSpecificCredential for an unknown service answers NotSupportedService instead of NoSuchEntity.
AWS: an unknown ServiceName is refused with code NoSuchEntity (HTTP 404). Smithy: the operation models NoSuchEntityException (NoSuchEntity, 404) AND ServiceNotSupportedException (NotSupportedService, 404); the recording shows AWS answering NoSuchEntity for a service name that does not exist, reserving NotSupportedService for a real service that does not offer the credential type.
Emulator: answers NotSupportedService for a name that is not a service at all.
Why: a client branching on NoSuchEntity mis-handles the refusal, and the two codes carry different meanings.
Fix: the ServiceName resolution/classification in the create_service_specific_credential handler.
Traces: iam.service-specific-credential-unsupported-service-rejected, iam.unsupported-service-specific-credential-rejected

Cause 10 of 10: a managed policy created under a Path cannot be read back by the ARN CreatePolicy returned.
AWS: CreatePolicy returns an Arn of the form arn:aws:iam::<account>:policy<Path><PolicyName>, and GetPolicy / CreatePolicyVersion on exactly that ARN answer 200.
Emulator: GetPolicy on the returned ARN answers 404 NoSuchEntity 'Policy <arn> was not found.' — the stored key and the minted ARN disagree, and the ARN itself repeats the name segment (…:policy/<name>/<name>).
Why: a policy the emulator says it created is unreadable and unversionable through the identifier it handed the client, so every path-qualified policy is a dead ARN.
Fix: the ARN minting and the policy-store key in the create_policy handler and the lookup in get_policy / create_policy_version (they must agree on the path-qualified ARN form).
Traces: iam.iam-entity-names-case-insensitive-unique

## Evidence
Cause 1 — `expected a non-empty ThumbprintList, got []` and `expected IAM to auto-fetch a thumbprint`.
Cause 2 — `expected message None, got 'Invalid Url http://<host>: the URL must begin with https://'`.
Cause 3 — `expected the call to be rejected, but it succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}`.
Cause 4 — expected `'Cannot attach AWS reserved policy to an IAM role.'`, got `'Policy arn:aws:iam::aws:policy/aws-service-role/AWSSupportServiceRolePolicy is not attachable.'`.
Cause 5 — `expected a rejection, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}`.
Cause 6 — `real AWS rejects this call, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}`.
Cause 7 — expected code `NoSuchEntity`, got `{'Type': 'Sender', 'Code': 'InvalidUserType', 'Message': 'The request was rejected because the type of user for the transaction was incorrect.'}`.
Cause 8 — `expected UnmodifiableEntity from DeleteRole on a service-linked role, got NoSuchEntity`; `expected UnmodifiableEntity from UpdateAssumeRolePolicy, got 'NoSuchEntity'`.
Cause 9 — `expected NoSuchEntity, got NotSupportedService` (both checks).
Cause 10 — `NoSuchEntityException: An error occurred (NoSuchEntity) when calling the GetPolicy operation: Policy arn:aws:iam::000000000000:policy/<name>/<name> was not found.` — note the duplicated name segment in the ARN the emulator itself minted.

## Steps to Reproduce
(1) CreateOpenIDConnectProvider with no ThumbprintList and read ThumbprintList back from the response and GetOpenIDConnectProvider. (2) CreateOpenIDConnectProvider with a http:// Url. (3) DeleteOpenIDConnectProvider twice. (4) AttachRolePolicy with arn:aws:iam::aws:policy/aws-service-role/AWSSupportServiceRolePolicy. (5) AttachUserPolicy with the same reserved policy. (6) TagUser with a tag AWS rejects. (7) ChangePassword using assumed-role-session credentials. (8) DeleteRole and UpdateAssumeRolePolicy on a service-linked role under /aws-service-role/. (9) CreateServiceSpecificCredential with a ServiceName that is not an AWS service. (10) CreatePolicy with a Path, then GetPolicy on the Arn the response returned.
