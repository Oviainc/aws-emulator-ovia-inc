Issue-Source: trace-sweep-divergence
Issue-Trace: iam-access-key-error-paths
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iam-access-key-error-paths iam-access-key-status-update-not-found-error iam-access-key-update-not-found iam-self-user-access-key-not-found iam-account-alias-create-mismatch-delete-error iam-account-alias-delete-nonexistent-error iam-account-alias-lifecycle-and-guard-clauses iam-account-settings-alias-and-password-policy iam-account-properties-and-summary iam-acquire-role-backup-service-lifecycle iam-role-and-user-tag-lifecycle iam-role-lifecycle-inline-managed-policy-and-boundary iam-service-linked-role-supported-services iam-cred-create-user-name-length-boundary iam-cred-list-users-max-items-1001 iam-cred-list-users-max-items-zero iam-cred-password-policy-min-length-out-of-range iam-cred-password-policy-reuse-prevention-out-of-range iam-credential-and-certificate-ops-user-not-found iam-signing-certificate-upload-and-status-update-errors iam-credential-not-found-errors iam-signing-certificate-error-paths iam-signing-certificate-lifecycle iam-signing-certificate-not-found-errors iam-server-certificate-rename-and-service-specific-credential-status-not-found-errors iam-service-specific-credential-error-paths iam-service-specific-credential-not-found iam-service-specific-credential-guard-clauses iam-delegation-request-guard-clauses-and-create-attempt iam-error-paths-nonexistent-user iam-put-user-policy-overwrite-round-trip-and-not-found iam-user-mutation-guard-clauses-on-nonexistent-principal iam-user-mutation-not-found-error-paths iam-user-policy-and-boundary-not-found-errors iam-user-policy-and-permissions-boundary-not-found iam-user-policy-not-found-error-paths iam-user-policy-operations-not-found-errors iam-user-policy-operations-reject-nonexistent-user iam-get-user-policy-document-round-trip-and-not-found iam-user-inline-policy-round-trip-and-missing-user-error iam-filtered-empty-listings iam-group-lifecycle-with-inline-and-attached-policies iam-get-context-keys-for-principal-policy iam-get-group-membership-and-not-found-contract iam-user-group-membership iam-get-user-self-and-not-found iam-user-error-paths-nonexistent iam-user-identity-and-credential-ops-not-found iam-user-not-found-error-paths iam-user-operations-not-found-errors iam-user-ops-not-found-errors iam-group-inline-and-managed-policies iam-group-membership-not-found iam-remove-user-from-group-not-found-error iam-identity-providers-saml-and-oidc-batch6 iam-instance-profile-and-mfa-device-untag iam-instance-profile-role-association-and-tags iam-list-attached-group-policies-contract iam-list-user-tags-contract iam-login-profile-and-password-not-found-errors iam-login-profile-error-paths-nonexistent-user iam-login-profile-missing-user-errors iam-login-profile-not-found-and-change-password-errors iam-login-profile-not-found-error-paths iam-pg-login-profile-lifecycle iam-managed-policy-untag-lifecycle iam-managed-policy-version-lifecycle-tags-and-entities iam-pg-list-policy-versions-marker-walk iam-managed-policy-versioning-and-tags iam-mfa-device-lifecycle-batch7 iam-oidc-provider-non-https-url-rejected iam-permissions-boundary-and-policy-simulation iam-pg-list-role-tags-marker-walk iam-pg-list-roles-marker-walk iam-pg-login-profile-duplicate-create iam-saml-and-oidc-provider-untag-lifecycle iam-server-certificate-error-paths iam-server-certificate-rename-and-not-found iam-server-certificate-rename-lifecycle iam-server-certificate-tag-lifecycle iam-service-access-analysis-and-role-template-reports iam-service-specific-credential-lifecycle iam-signing-certificate-guard-clauses
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|UpdateAccessKey|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|DeleteAccountAlias|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|PutAccountProperties|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|GetRole|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|CreateUser|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|ListUsers|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|UpdateAccountPasswordPolicy|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|UploadSigningCertificate|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|UpdateSigningCertificate|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|UpdateServiceSpecificCredential|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|DeleteServerCertificate|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|DeleteServiceSpecificCredential|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|GetDelegationRequest|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|PutUserPolicy|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|GetUserPolicy|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|DeleteUserPolicy|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|ListUserPolicies|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|AttachUserPolicy|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|DetachUserPolicy|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|ListAttachedUserPolicies|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|ListGroupsForUser|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|GetContextKeysForPrincipalPolicy|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|GetGroup|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|GetUser|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|DeleteGroup|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|AddUserToGroup|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|CreateGroup|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|GetGroupPolicy|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|RemoveUserFromGroup|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|CreateSAMLProvider|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|GetSAMLProvider|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|GetOpenIDConnectProvider|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|RemoveClientIDFromOpenIDConnectProvider|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|DeleteSAMLProvider|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|UntagInstanceProfile|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|DeleteInstanceProfile|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|GetInstanceProfile|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|ListAttachedGroupPolicies|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|ListUserTags|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|GetLoginProfile|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|DeleteLoginProfile|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|UntagPolicy|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|CreatePolicyVersion|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|DeletePolicyVersion|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|GetPolicy|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|GetMFADevice|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|CreateOpenIDConnectProvider|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|SimulatePrincipalPolicy|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|ListRoleTags|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|ListRoles|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|CreateLoginProfile|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|TagRole|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|UntagRole|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|TagUser|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|UntagUser|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|CreateRole|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|PutRolePolicy|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|UntagSAMLProvider|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|UpdateServerCertificate|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|GetServerCertificate|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|GetHumanReadableSummary|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|GenerateServiceLastAccessedDetails|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|CreateServiceSpecificCredential|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|ListServiceSpecificCredentials|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|ResetServiceSpecificCredential|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|ListSigningCertificates|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|DeleteSigningCertificate|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|DeleteUser|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: iam|DeleteAccessKey|4xx->4xx|$.ErrorResponse.RequestId
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# IAM error responses omit the ResponseMetadata RequestId element

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAccessKey

## Description
Cause 1 of 1: the awsQuery XML error envelope the emulator writes has no <RequestId> child, on every IAM operation and every error class. 69 replayed divergence clusters (covering ~250 steps across 4xx and 409 paths) differ ONLY in this element; the status, the Error/Type, the Error/Code and (except where separately filed) the Error/Message all match.

AWS: IAM is an awsQuery protocol service. Every IAM error response is <ErrorResponse><Error><Type/><Code/><Message/></Error><RequestId>...</RequestId></ErrorResponse>; the RequestId element is always present and carries the request's id (a UUID generated per request - its value is per-run and is NOT part of the requirement, only its presence and position are). The recorded responses for e.g. UpdateAccessKey/NoSuchEntity, DeleteAccountAlias/NoSuchEntity, PutUserPolicy/NoSuchEntity and CreateGroup/EntityAlreadyExists all carry it. Smithy: the IAM model is awsQuery with awsQueryError codes on every error shape (e.g. NoSuchEntityException -> code NoSuchEntity, httpResponseCode 404; EntityAlreadyExistsException -> EntityAlreadyExists, 409), and awsQuery error serialization includes RequestId.

Emulator: the element is absent - the replay reports `$.ErrorResponse.RequestId: missing (expected '<uuid>', got None)` for every one of these clusters, at every status the corpus exercises (400, 404, 409).

Why: a client that logs or correlates by request id, or an SDK/middleware that reads ResponseMetadata.RequestId off a failed call, gets a null. Retry/telemetry wrappers that assert on the field raise instead of surfacing the service error, so the real IAM error code never reaches application code.

Fix: the IAM awsQuery error serializer - the one place that builds the <ErrorResponse> document for all operations. It must emit <RequestId> after </Error> with the per-request id, the same id the success path puts in ResponseMetadata/RequestId (success responses already carry it, so only the error branch is wrong).

Traces: representative iam-access-key-error-paths#task#3 (UpdateAccessKey, 404 NoSuchEntity); the remaining 68 cluster representatives are listed in trace_ids and span 68 distinct operations/fixtures.

## Evidence
Expected (recorded AWS, UpdateAccessKey on an unknown AccessKeyId): HTTP 404, body <ErrorResponse><Error><Type>Sender</Type><Code>NoSuchEntity</Code><Message>The Access Key with id ... cannot be found.</Message></Error><RequestId>606706b2-8d4c-4a2e-92fb-146ca7b44cd6</RequestId></ErrorResponse>.
Actual (emulator): HTTP 404 with the same Error block but no RequestId child - replay detail `$.ErrorResponse.RequestId: missing (expected '606706b2-8d4c-4a2e-92fb-146ca7b44cd6', got None)`.
The same single detail line is the ONLY divergence in 69 clusters, e.g. DeleteAccountAlias/404 (iam-account-alias-create-mismatch-delete-error#task#2), ListUsers/400 (iam-cred-list-users-max-items-1001#task#3), CreateGroup/409 (iam-group-lifecycle-with-inline-and-attached-policies#task#13), GetMFADevice/400 (iam-mfa-device-lifecycle-batch7#task#5).

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay fixture iam-access-key-error-paths step task#3: UpdateAccessKey with an AccessKeyId that does not exist.
3. Read the 404 body: the <ErrorResponse> has <Error> but no <RequestId>.
4. Repeat with any of the fixture#step bindings in trace_ids - every IAM error path reproduces it.
