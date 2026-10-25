Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.alias-exists-on-confirm-signup cognito-idp.code-mismatch-vs-expired-precision cognito-idp.confirm-sign-up-wrong-code-rejected cognito-idp.confirm-signup-wrong-code-rejected cognito-idp.code-delivery-failure-error cognito-idp.get-user-attribute-verification-code-rejects-unverifiable-attribute cognito-idp.feature-unavailable-in-tier-domain-mfa-logging cognito-idp.create-user-pool-client-client-credentials-requires-secret cognito-idp.admin-delete-user-and-list-users-race-no-partial-state
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.alias-exists-on-confirm-signup|run#11|AssertionError|assert code == "CodeMismatchException", (
Issue-Key: cognito-idp.code-mismatch-vs-expired-precision|run#10|AssertionError|assert c1 == "CodeMismatchException", (
Issue-Key: cognito-idp.confirm-sign-up-wrong-code-rejected|run#7|AssertionError|assert code == "ExpiredCodeException", f"expected ExpiredCodeException, got {code}"
Issue-Key: cognito-idp.confirm-signup-wrong-code-rejected|run#10|AssertionError|assert code == "CodeMismatchException", f"expected CodeMismatchException, got {code}"
Issue-Key: cognito-idp.code-delivery-failure-error|run#8|AssertionError|assert resend_code in (None, "LimitExceededException"), (
Issue-Key: cognito-idp.get-user-attribute-verification-code-rejects-unverifiable-attribute|run#10|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code}"
Issue-Key: cognito-idp.feature-unavailable-in-tier-domain-mfa-logging|run#11|AssertionError|assert code2 == "FeatureUnavailableInTierException", (
Issue-Key: cognito-idp.create-user-pool-client-client-credentials-requires-secret|_error#11|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: cognito-idp.admin-delete-user-and-list-users-race-no-partial-state|_wait_for#14|AssertionError|raise AssertionError(f"timed out after {timeout_s}s waiting for {reason}")
Issue-Commit: afc21d5623f1a8f7411b21bbb87a0dff6c2e7024

# cognito-idp: six causes — ConfirmSignUp, ResendConfirmationCode, GetUserAttributeVerificationCode, UpdateUserPoolDomain and ListUsers answer NotImplemented, and CreateUserPoolClient accepts client_credentials on a secret-less client

> _Note: this issue was identified against the code base at commit `afc21d5623f1a8f7411b21bbb87a0dff6c2e7024`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ConfirmSignUp

## Description
Six distinct root causes in the cognito-idp emulator, each grounded in failing behavioral checks that the replay report did NOT mark as ALREADY FILED or OWNED. All six must be closed before this item is submitted.

=== Cause 1 of 6: ConfirmSignUp answers NotImplemented, so no confirmation-code outcome is reachable ===
AWS: Smithy ConfirmSignUp — required input ClientId/Username/ConfirmationCode; modeled errors include CodeMismatchException (400), ExpiredCodeException (400), AliasExistsException (400), NotAuthorizedException (403), UserNotFoundException (404), TooManyFailedAttemptsException (400). A wrong code checked against an outstanding code is CodeMismatchException; a code checked where none is outstanding is ExpiredCodeException. Against a live alias collision real Cognito still answers CodeMismatchException, never AliasExistsException (recorded behavior only, per the alias check's own notes).
Emulator: every ConfirmSignUp call is refused with error code NotImplemented ("operation ConfirmSignUp is scaffolded but not implemented in this emulator").
Why: a client can never complete self-service sign-up, and every negative confirmation-code path is indistinguishable from an emulator gap — code-specific rejection cannot be told apart from a blanket refusal, so sign-up state machines built on it cannot be tested at all.
Fix: the ConfirmSignUp handler (services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/confirm_sign_up.rs) and the per-user confirmation-code state it must consult. An open stub work item exists for this operation covering only cognito-idp.op-contract-confirmSignUp; the four checks below are additional and were not marked already-filed.
Traces: cognito-idp.alias-exists-on-confirm-signup, cognito-idp.code-mismatch-vs-expired-precision, cognito-idp.confirm-sign-up-wrong-code-rejected, cognito-idp.confirm-signup-wrong-code-rejected.

=== Cause 2 of 6: ResendConfirmationCode answers NotImplemented ===
AWS: Smithy ResendConfirmationCode — output CodeDeliveryDetails; modeled errors include CodeDeliveryFailureException (400), LimitExceededException (400), InvalidEmailRoleAccessPolicyException (400), InvalidSmsRoleAccessPolicyException (400), UserNotFoundException (404). A call either succeeds with CodeDeliveryDetails or is refused with one of those modeled errors (a daily-quota refusal is LimitExceededException).
Emulator: error code NotImplemented.
Why: a client resending a confirmation code gets neither a delivery receipt nor any modeled delivery/quota failure, so code-delivery failure handling can never be exercised.
Fix: the ResendConfirmationCode handler (.../src/ops/resend_confirmation_code.rs) and the code-delivery path it shares with SignUp.
Traces: cognito-idp.code-delivery-failure-error.

=== Cause 3 of 6: GetUserAttributeVerificationCode answers NotImplemented, so the unverifiable-attribute rejection never happens ===
AWS: Smithy GetUserAttributeVerificationCode — required AccessToken + AttributeName, output CodeDeliveryDetails; modeled errors include InvalidParameterException (400), NotAuthorizedException (403), UserNotConfirmedException (400). Requesting a code for a non-verifiable attribute (e.g. family_name, as opposed to email/phone_number) is refused with InvalidParameterException and produces no CodeDeliveryDetails.
Emulator: error code NotImplemented for both the negative attribute and the verifiable-attribute positive control.
Why: a client cannot verify email/phone at all, and an invalid attribute request is not refused with the modeled 400 — callers that branch on InvalidParameterException see an unknown error code instead.
Fix: the GetUserAttributeVerificationCode handler (.../src/ops/get_user_attribute_verification_code.rs) and its attribute-verifiability validation. The open item 20261024T114943Z-cognito-idp-getuserattributeverificationcode-stub.md names the same operation but does not carry this check's replay key.
Traces: cognito-idp.get-user-attribute-verification-code-rejects-unverifiable-attribute.

=== Cause 4 of 6: UpdateUserPoolDomain answers NotImplemented instead of the tier refusal ===
AWS: Smithy UpdateUserPoolDomain — required Domain + UserPoolId; modeled errors include FeatureUnavailableInTierException (403), InvalidParameterException (400), ConcurrentModificationException (400), ResourceNotFoundException (404). Asking for a managed-login/domain capability the pool's feature tier does not include is refused with FeatureUnavailableInTierException (HTTP 403).
Emulator: error code NotImplemented.
Why: tier-gated configuration appears to be an emulator gap rather than a 403 tier refusal, so a client's tier-upgrade branch is never taken and over-permissive tier behavior cannot be detected.
Fix: the UpdateUserPoolDomain handler (.../src/ops/update_user_pool_domain.rs) and the user-pool feature-tier gate. The open item 20261024T114943Z-cognito-idp-updateuserpooldomain-stub.md names the same operation but does not carry this check's replay key.
Traces: cognito-idp.feature-unavailable-in-tier-domain-mfa-logging.

=== Cause 5 of 6: CreateUserPoolClient accepts AllowedOAuthFlows=[client_credentials] on a client with no secret ===
AWS: Smithy CreateUserPoolClient models InvalidOAuthFlowException (400, "This exception is thrown when the specified OAuth flow is not valid") alongside GenerateSecret (default false) and AllowedOAuthFlows. The client_credentials flow requires a client secret, so GenerateSecret=false together with AllowedOAuthFlows=[client_credentials] is refused with InvalidOAuthFlowException / HTTP 400 and no app client is created. Recorded message example: "client_credentials flow can not be selected if client does not have a client secret."
Emulator: the call SUCCEEDS with HTTP 200 and returns a UserPoolClient with AllowedOAuthFlows=['client_credentials'] and no ClientSecret, and the client persists in ListUserPoolClients.
Why: this is an over-permissive acceptance — an application ships a public (secret-less) client configured for machine-to-machine client_credentials that real Cognito would have refused at create time, and the misconfiguration is only discovered against real AWS.
Fix: CreateUserPoolClient input validation (.../src/ops/create_user_pool_client.rs) — reject the client_credentials flow when the client has no secret, before any state is written.
Traces: cognito-idp.create-user-pool-client-client-credentials-requires-secret.

=== Cause 6 of 6: ListUsers answers NotImplemented, so a concurrent delete/list observation can never be made ===
AWS: Smithy ListUsers — required UserPoolId, optional Filter (length 0..256), paginated on PaginationToken/Users; modeled errors InvalidParameterException (400), ResourceNotFoundException (404), NotAuthorizedException (403). A filtered ListUsers returns the matching fully-populated user rows (Username, UserStatus, Enabled, UserCreateDate) and, after AdminDeleteUser, zero rows.
Emulator: ListUsers is refused with NotImplemented, so the check's polling thread collects no samples and the check fails with "timed out after 5.0s waiting for the poller to observe the user present before AdminDeleteUser runs".
Why: a client cannot enumerate users at all; consistency of AdminDeleteUser against concurrent reads is unobservable and any code paging users fails outright.
Fix: the ListUsers handler (.../src/ops/list_users.rs), its Filter parsing and its pagination. The open item 20261024T114943Z-cognito-idp-listusers-stub.md names the same operation but does not carry this check's replay key — this check failed on an assertion in the harness, not on the stub call, so it was not marked already-filed.
Traces: cognito-idp.admin-delete-user-and-list-users-race-no-partial-state.

## Evidence
Cause 1 — expected CodeMismatchException / ExpiredCodeException from ConfirmSignUp, got error code NotImplemented:
  "expected confirming sign-up with a guessed code for a phone number already claimed as a verified alias elsewhere to be rejected with CodeMismatchException -- never silently confirmed, got NotImplemented"
  "ConfirmSignUp: expected CodeMismatchException for a wrong code, got NotImplemented"
  "expected ExpiredCodeException, got NotImplemented"
  "expected CodeMismatchException, got NotImplemented"

Cause 2 — "ResendConfirmationCode: expected success or a documented daily-quota LimitExceededException, got NotImplemented"

Cause 3 — "expected InvalidParameterException, got NotImplemented" (GetUserAttributeVerificationCode with a non-verifiable attribute)

Cause 4 — "UpdateUserPoolDomain: expected FeatureUnavailableInTierException, got NotImplemented"

Cause 5 — expected InvalidOAuthFlowException / HTTP 400; actual HTTP 200 with a created client:
  AssertionError: expected a rejection, but the call SUCCEEDED and returned {'UserPoolClient': {'UserPoolId': 'us-west-2_...', 'ClientName': 'sim2real-behav-cip-client-...', 'ClientId': '...', 'AllowedOAuthFlows': ['client_credentials'], 'AllowedOAuthFlowsUserPoolClient': True, ...}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}  (no ClientSecret present)

Cause 6 — AssertionError: timed out after 5.0s waiting for the poller to observe the user present before AdminDeleteUser runs; the poller's only call is list_users(UserPoolId=..., Filter='username = "..."'), which the emulator refuses with NotImplemented.

## Steps to Reproduce
Boot the emulator and replay the behavioral suite, or by hand against the cognito-idp endpoint:
1. Cause 1: CreateUserPool + CreateUserPoolClient, SignUp a user, then ConfirmSignUp with an arbitrary wrong ConfirmationCode. Expect HTTP 400 CodeMismatchException (ExpiredCodeException when no code is outstanding); observe NotImplemented.
2. Cause 2: SignUp an unconfirmed user, then ResendConfirmationCode for it. Expect 200 with CodeDeliveryDetails (or 400 LimitExceededException at quota); observe NotImplemented.
3. Cause 3: create a confirmed user, sign in for an AccessToken, call GetUserAttributeVerificationCode with AttributeName='family_name'. Expect HTTP 400 InvalidParameterException; observe NotImplemented.
4. Cause 4: on a pool whose feature tier excludes the capability, call UpdateUserPoolDomain. Expect HTTP 403 FeatureUnavailableInTierException; observe NotImplemented.
5. Cause 5: CreateResourceServer with scope 'read', then CreateUserPoolClient with GenerateSecret=false, AllowedOAuthFlows=['client_credentials'], AllowedOAuthFlowsUserPoolClient=true, AllowedOAuthScopes=['<rs>/read']. Expect HTTP 400 InvalidOAuthFlowException and no client created; observe HTTP 200 and a persisted secret-less client. (The same call with GenerateSecret=true is the positive control and must still succeed.)
6. Cause 6: AdminCreateUser, then poll ListUsers with Filter='username = "<name>"' while AdminDeleteUser runs. Expect populated rows then zero rows; observe NotImplemented on every ListUsers call.

Replay these check ids to confirm: cognito-idp.alias-exists-on-confirm-signup, cognito-idp.code-mismatch-vs-expired-precision, cognito-idp.confirm-sign-up-wrong-code-rejected, cognito-idp.confirm-signup-wrong-code-rejected, cognito-idp.code-delivery-failure-error, cognito-idp.get-user-attribute-verification-code-rejects-unverifiable-attribute, cognito-idp.feature-unavailable-in-tier-domain-mfa-logging, cognito-idp.create-user-pool-client-client-credentials-requires-secret, cognito-idp.admin-delete-user-and-list-users-race-no-partial-state
