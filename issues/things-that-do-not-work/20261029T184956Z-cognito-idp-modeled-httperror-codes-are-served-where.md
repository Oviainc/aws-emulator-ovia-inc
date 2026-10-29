Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-admin-user-auth-workflow
Issue-Step: teardown#12
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-admin-user-auth-workflow cognito-idp-admin-user-signup-confirm-and-mfa cognito-idp-openid-configuration-discovery cognito-idp-risk-configuration-lifecycle cognito-idp-user-import-job-lifecycle cognito-idp-user-pool-and-client-lifecycle cognito-idp-user-pool-client-lifecycle cognito-idp-user-pool-core-lifecycle cognito-idp-user-pool-lifecycle-and-provisioned-limits cognito-idp-user-pool-list-lifecycle cognito-idp-user-session-mfa-and-refresh cognito-idp-self-service-account-deletion-lifecycle cognito-idp-webauthn-invalid-token-errors cognito-idp-terms-document-lifecycle cognito-idp-terms-documents-crud cognito-idp-terms-documents-error-paths cognito-idp-terms-documents-lifecycle cognito-idp-terms-errors cognito-idp-terms-lifecycle cognito-idp-terms-lifecycle-create-describe-update-delete cognito-idp-terms-list-multiple cognito-idp-terms-not-found-errors cognito-idp-managed-login-branding-crud cognito-idp-managed-login-branding-errors cognito-idp-managed-login-branding-lifecycle cognito-idp-user-pool-replica-errors cognito-idp-user-pool-replica-lifecycle cognito-idp-user-pool-replica-notfound-errors cognito-idp-device-self-service-invalid-token-errors cognito-idp-self-service-ops-reject-invalid-access-token cognito-idp-invalid-inputs-and-missing-resources cognito-idp-user-mfa-settings-and-refresh-token-exchange cognito-idp-admin-device-management-error-paths cognito-idp-device-and-pool-diagnostics cognito-idp-user-pool-update-reflects-auto-verified-attributes
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|DescribeUserPool|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|DescribeUserPool|4xx->4xx|status
Issue-Key: cognito-idp|GetUser|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|GetUser|4xx->4xx|status
Issue-Key: cognito-idp|ListWebAuthnCredentials|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|ListWebAuthnCredentials|4xx->4xx|status
Issue-Key: cognito-idp|DescribeTerms|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|DescribeTerms|4xx->4xx|status
Issue-Key: cognito-idp|DescribeManagedLoginBranding|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|DescribeManagedLoginBranding|4xx->4xx|status
Issue-Key: cognito-idp|DeleteUserPoolReplica|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|DeleteUserPoolReplica|4xx->4xx|status
Issue-Key: cognito-idp|ConfirmDevice|4xx->4xx|status
Issue-Key: cognito-idp|SetUserMFAPreference|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|SetUserMFAPreference|4xx->4xx|status
Issue-Key: cognito-idp|AdminGetDevice|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|AdminGetDevice|4xx->4xx|status
Issue-Related: 20261029T184956Z-cognito-idp-every-error-response-omits-the-x.md
Issue-Commit: e5bbef2d2760924537f10016f55a30b12c0b7775

# cognito-idp: modeled httpError codes are served where AWS answers 400

> _Note: this issue was identified against the code base at commit `e5bbef2d2760924537f10016f55a30b12c0b7775`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeUserPool

## Description
AWS: cognito-idp answers HTTP 400 for its client-error classes, including ResourceNotFoundException, UserNotFoundException and NotAuthorizedException. The Smithy model annotates those shapes with @httpError(404), @httpError(404) and @httpError(403) respectively, but the recorded service does NOT use them: every recorded refusal in this corpus has expected_status 400 with the error class carried in the body's `__type` and in `x-amzn-errortype`. The HTTP status is therefore 400 and the modeled httpError must not be applied; recorded behavior wins over the trait here.
Emulator: the emulator applies the modeled @httpError, so ResourceNotFoundException/UserNotFoundException are served as 404 and NotAuthorizedException/FeatureUnavailableInTierException as 403.
Why: a client that branches on the HTTP status — API gateways, proxies, SDK retry classifiers and any code that treats 404 as 'route/endpoint missing' rather than 'resource missing' — takes a different path than it does against AWS. A 403 from a Cognito data-plane call is indistinguishable from a SigV4/credential failure, so credential-refresh logic fires on what is really a bad-token refusal.
Fix: the service's error-to-HTTP-status mapping (the awsJson1_1 error serializer for cognito-idp) must pin client errors to 400 instead of reading @httpError off the shape; the error class stays in `__type` and `x-amzn-errortype`. Note: the status fix alone does not close these steps — the same steps also miss `x-amzn-errortype` (filed separately as the serializer header issue), so both must land.

Cause 1 of 10: DescribeUserPool on a deleted pool answers 404; AWS answers 400 ResourceNotFoundException. Fixture cognito-idp-admin-user-auth-workflow, step teardown#12 (12 recorded steps in this cluster).
Cause 2 of 10: GetUser/DeleteUser for a deleted user answer 404; AWS answers 400 UserNotFoundException. Fixture cognito-idp-self-service-account-deletion-lifecycle, step task#9.
Cause 3 of 10: ListWebAuthnCredentials with an invalid access token answers 403; AWS answers 400 NotAuthorizedException. Fixture cognito-idp-webauthn-invalid-token-errors, step task#1.
Cause 4 of 10: DescribeTerms for an unknown terms id answers 404; AWS answers 400 ResourceNotFoundException. Fixture cognito-idp-terms-document-lifecycle, step task#10 (13 recorded steps).
Cause 5 of 10: DescribeManagedLoginBranding for an unknown branding id answers 404; AWS answers 400. Fixture cognito-idp-managed-login-branding-crud, step task#10.
Cause 6 of 10: DeleteUserPoolReplica for an unknown replica answers 404; AWS answers 400. Fixture cognito-idp-user-pool-replica-errors, step task#6.
Cause 7 of 10: ConfirmDevice with an invalid access token answers 403; AWS answers 400 NotAuthorizedException. Fixture cognito-idp-device-self-service-invalid-token-errors, step task#1.
Cause 8 of 10: SetUserMFAPreference with an invalid access token answers 403; AWS answers 400 NotAuthorizedException. Fixture cognito-idp-invalid-inputs-and-missing-resources, step task#6.
Cause 9 of 10: AdminGetDevice for an unknown device answers 404; AWS answers 400 ResourceNotFoundException. Fixture cognito-idp-admin-device-management-error-paths, step task#5.
Cause 10 of 10: DescribeUserPool after pool deletion answers 404 where only the status is asserted. Fixture cognito-idp-user-pool-update-reflects-auto-verified-attributes, step teardown#5.

The same status substitution appears on ~70 further recorded steps (AdminConfirmSignUp, AdminCreateUser, AdminForgetDevice, AdminUpdateDeviceStatus, CreateGroup, CreateResourceServer, CreateUserPoolClient, CreateUserPoolDomain, DeleteTerms, DescribeIdentityProvider, DescribeResourceServer, ForgetDevice, ForgotPassword, GetDevice, GetGroup, GetLogDeliveryConfiguration, GetUICustomization, ListResourceServers, ListUserPoolClients, ListUserPoolClientSecrets, ListUserPoolReplicas, ListUsersInGroup, SetLogDeliveryConfiguration, SetUICustomization, SetUserSettings, StartUserImportJob, StopUserImportJob, TagResource, UpdateDeviceStatus, UpdateManagedLoginBranding, UpdateTerms, UpdateUserPool, UpdateUserPoolDomain, UpdateUserPoolReplica, CompleteWebAuthnRegistration, DeleteWebAuthnCredential, StartWebAuthnRegistration).

## Evidence
Expected (recorded AWS): `status: 400` with body `__type` naming the error class.
Actual (emulator): `status: expected 400, got 404` for ResourceNotFoundException/UserNotFoundException paths and `status: expected 400, got 403` for NotAuthorizedException/FeatureUnavailableInTierException paths.
Smithy: ResourceNotFoundException @httpError(404), UserNotFoundException @httpError(404), NotAuthorizedException @httpError(403), FeatureUnavailableInTierException @httpError(403) — the recorded service does not honor these.

## Steps to Reproduce
1. Replay cognito-idp-admin-user-auth-workflow step teardown#12 (DescribeUserPool on a deleted pool) and read the HTTP status: emulator 404, AWS 400.
2. Replay cognito-idp-webauthn-invalid-token-errors step task#1 (ListWebAuthnCredentials with a bogus access token): emulator 403, AWS 400.
3. Repeat for each fixture#step listed under the causes.
