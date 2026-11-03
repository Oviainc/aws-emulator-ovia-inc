Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.create-user-import-job-missing-trust-policy-rejected cognito-idp.refarch-cog-duplicate-recovery-priority-rejected cognito-idp.refarch-cog-forgot-password-before-password-set-rejected cognito-idp.refarch-cog-self-signup-rejected-when-admin-create-only cognito-idp.user-pool-client-http-callback-url-rejected cognito-idp.user-pool-client-oauth-flows-without-scopes-rejected cognito-idp.deep-cognito-idp-3-enforce-risk-ip-range-blocked cognito-idp.deep-cognito-idp-3-enforce-risk-user-context-ip-blocked cognito-idp.op-contract-update-provisioned-limit cognito-idp.provisioned-limit-large-numeric-value-precision-preserved cognito-idp.provisioned-limit-numeric-value-precision-round-trip cognito-idp.provisioned-limit-update-concurrent-modification-conflict cognito-idp.provisioned-limit-update-reflected cognito-idp.refarch-cog-verified-phone-still-recovers-by-email-without-sms-config
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.create-user-import-job-missing-trust-policy-rejected|_error#22|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: cognito-idp.refarch-cog-duplicate-recovery-priority-rejected|_error_code#4|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: cognito-idp.refarch-cog-forgot-password-before-password-set-rejected|_error_code#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: cognito-idp.refarch-cog-self-signup-rejected-when-admin-create-only|_error_response#7|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error response")
Issue-Key: cognito-idp.user-pool-client-http-callback-url-rejected|_error#7|AssertionError|raise AssertionError(f"the call was expected to be REFUSED but it succeeded: {result!r}")
Issue-Key: cognito-idp.user-pool-client-oauth-flows-without-scopes-rejected|_error#4|AssertionError|raise AssertionError(f"the call was expected to be REFUSED but it succeeded: {result!r}")
Issue-Key: cognito-idp.deep-cognito-idp-3-enforce-risk-ip-range-blocked|run#9|AssertionError|assert blocked == ("NotAuthorizedException", "Request not allowed due to security reasons."), (
Issue-Key: cognito-idp.deep-cognito-idp-3-enforce-risk-user-context-ip-blocked|run#16|AssertionError|assert blocked == ("NotAuthorizedException", "Request not allowed due to security reasons."), (
Issue-Key: cognito-idp.op-contract-update-provisioned-limit|_error#8|AssertionError|raise AssertionError(
Issue-Key: cognito-idp.provisioned-limit-large-numeric-value-precision-preserved|_error#9|AssertionError|raise AssertionError(f"expected UpdateProvisionedLimit to be rejected, but it succeeded: {result!r}")
Issue-Key: cognito-idp.provisioned-limit-numeric-value-precision-round-trip|run#7|AssertionError|raise AssertionError(
Issue-Key: cognito-idp.provisioned-limit-update-concurrent-modification-conflict|run#12|AssertionError|raise AssertionError(
Issue-Key: cognito-idp.provisioned-limit-update-reflected|run#10|AssertionError|raise AssertionError(
Issue-Key: cognito-idp.refarch-cog-verified-phone-still-recovers-by-email-without-sms-config|run#13|AssertionError|assert details["DeliveryMedium"] == "EMAIL", (
Issue-Commit: e56f1153b059bce6a929f0aeca74333a3bd5954e

# cognito-idp: nine validation/enforcement gates missing — calls AWS refuses (or routes differently) succeed unchanged (9 causes)

> _Note: this issue was identified against the code base at commit `e56f1153b059bce6a929f0aeca74333a3bd5954e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartUserImportJob

## Description
Nine net-new root causes in the cognito-idp control/data plane, all of the same family: a precondition AWS enforces is not evaluated at all, so the emulator answers 200 where AWS refuses (or picks the wrong path). Each cause is independent and must be closed separately.

NOTE on an adjacent open issue: 20261029T182116Z-cognito-idp-provisioned-limit-api-diverges-no-default.md describes the same UpdateProvisionedLimit no-op defect as its Cause 3, but it carries different replay keys; Cause 8 below is still failing under the check ids listed there and is included so the fixer re-replays them. Fixed issues 20261029T182116Z-cognito-idp-app-client-and-pool-configuration-validation.md and 20261029T184956Z-cognito-idp-revocation-provider-linking-risk-and-mfa.md touch neighbouring ground but do not cover any cause below (none of these is a wrongly-refused call, so none is a REOPENS).

Cause 1 of 9 — StartUserImportJob does not validate the job's CloudWatchLogsRoleArn trust policy.
AWS: StartUserImportJob models PreconditionNotMetException (400) (Smithy: StartUserImportJob errors = InvalidParameterException 400, NotAuthorizedException 403, OperationNotEnabledException 400, PreconditionNotMetException 400, ResourceNotFoundException 404, TooManyRequestsException 429). CreateUserImportJob accepts a role that does not trust cognito-idp.amazonaws.com, but Start is refused with PreconditionNotMetException and the message 'The configured Cloudwatch Logs Role is missing permissions or has an invalid trust policy.' — the whole message is fixed text, no request-derived part — and the job stays in status Created. Recorded behavior: the refusal persists far longer than the IAM propagation window a correctly-trusted role needs, and the identical Start succeeds once that same role's trust policy is repaired.
Emulator: the Start returns HTTP 200 with the job moved to Status 'Pending' (and a StartDate set), for a job whose CloudWatchLogsRoleArn trusts ec2.amazonaws.com only.
Why: a caller cannot distinguish a correctly wired import role from a mis-trusted one; an import that AWS would never start appears to be running, and the role-trust misconfiguration is never surfaced.
Fix: the StartUserImportJob handler's precondition validation (resolve the job's CloudWatchLogsRoleArn and require a trust policy admitting cognito-idp.amazonaws.com before advancing the job's state).
Traces: cognito-idp.create-user-import-job-missing-trust-policy-rejected.

Cause 2 of 9 — CreateUserPool accepts an AccountRecoverySetting with two mechanisms at the same Priority.
AWS: InvalidParameterException (Smithy: CreateUserPool errors include InvalidParameterException 400) with the message 'Invalid account recovery setting parameter. Account Recovery Setting cannot have duplicate priorities or recovery mechanisms.' — entirely fixed text — and no user pool is created.
Emulator: CreateUserPool succeeds and the pool is created with RecoveryMechanisms [{Priority:1,Name:verified_email},{Priority:1,Name:verified_phone_number}].
Why: an infrastructure template with a duplicated recovery priority deploys clean on the emulator and fails against AWS; worse, the pool's recovery ordering is then undefined.
Fix: AccountRecoverySetting validation in the CreateUserPool handler (reject duplicate Priority values and duplicate mechanism Names before the pool is stored).
Traces: cognito-idp.refarch-cog-duplicate-recovery-priority-rejected.

Cause 3 of 9 — ForgotPassword succeeds for a user still in FORCE_CHANGE_PASSWORD.
AWS: a user created by AdminCreateUser with no permanent password is in UserStatus FORCE_CHANGE_PASSWORD and cannot start self-service recovery: ForgotPassword is refused with NotAuthorizedException (Smithy: ForgotPassword errors include NotAuthorizedException 403) and the message 'User password cannot be reset in the current state.' — fixed text, no variable part — and the user's status is unchanged. Once AdminSetUserPassword(Permanent=true) moves the same user to CONFIRMED the identical call succeeds.
Emulator: the ForgotPassword call succeeds and returns CodeDeliveryDetails for the FORCE_CHANGE_PASSWORD user.
Why: an application that relies on the refusal to steer a first-login user into the NEW_PASSWORD_REQUIRED challenge instead sends that user down the reset-code path, which AWS would have blocked.
Fix: the user-state precondition in the ForgotPassword handler (gate on UserStatus before producing a code).
Traces: cognito-idp.refarch-cog-forgot-password-before-password-set-rejected.

Cause 4 of 9 — SignUp succeeds on a pool with AdminCreateUserConfig.AllowAdminCreateUserOnly = true.
AWS: NotAuthorizedException (Smithy: SignUp errors include NotAuthorizedException 403) with the message 'SignUp is not permitted for this user pool' — fixed text, no trailing period — and no user is created (ListUsers stays empty). AdminCreateUser on the same pool still succeeds, so the refusal is specific to the self-service API.
Emulator: SignUp returns 200 and self-registers the user on an admin-create-only pool.
Why: the pool's central self-signup prohibition is unenforced — any client holding the public ClientId can create accounts in a pool configured to forbid exactly that. This is a security-relevant gap, not only a fidelity one.
Fix: the AllowAdminCreateUserOnly gate in the SignUp handler.
Traces: cognito-idp.refarch-cog-self-signup-rejected-when-admin-create-only.

Cause 5 of 9 — UpdateUserPoolClient accepts a plain http:// callback URL for a non-localhost host.
AWS: InvalidParameterException (Smithy: UpdateUserPoolClient errors include InvalidParameterException 400) with the message '<callback URL> cannot use the HTTP protocol.' — the fixed text is ' cannot use the HTTP protocol.' and the variable part is the offending URL taken verbatim from the request's CallbackURLs — and the stored CallbackURLs are left unchanged. The https:// form of the same URL is accepted.
Emulator: the update returns 200 and stores CallbackURLs ['http://<host>/...'].
Why: a client can be left with an OAuth redirect over cleartext http, which AWS forbids outside localhost; the deployment only breaks when it reaches real AWS.
Fix: callback/logout URL scheme validation shared by the CreateUserPoolClient and UpdateUserPoolClient handlers.
Traces: cognito-idp.user-pool-client-http-callback-url-rejected.

Cause 6 of 9 — CreateUserPoolClient accepts an OAuth-enabled client with AllowedOAuthFlows but no AllowedOAuthScopes.
AWS: InvalidParameterException (Smithy: CreateUserPoolClient errors include InvalidParameterException 400, InvalidOAuthFlowException 400, ScopeDoesNotExistException 400) with the message 'AllowedOAuthFlows and AllowedOAuthScopes are required if user pool client is allowed to use OAuth flows.' — entirely fixed text — and no client is created (ListUserPoolClients stays empty).
Emulator: the create returns 200 with AllowedOAuthFlowsUserPoolClient true, AllowedOAuthFlows ['code','implicit'] and no AllowedOAuthScopes member at all.
Why: a hosted-UI/OAuth client is created in a state AWS treats as invalid; token requests against it then fail for an unrelated-looking reason.
Fix: the AllowedOAuthFlowsUserPoolClient consistency validation in the CreateUserPoolClient handler (and the same rule on update).
Traces: cognito-idp.user-pool-client-oauth-flows-without-scopes-rejected.

Cause 7 of 9 — the risk-configuration blocked-IP-range gate is not enforced on authentication.
AWS: with a pool-level RiskExceptionConfiguration whose BlockedIPRangeList contains the caller's address, authentication is refused with NotAuthorizedException (Smithy: InitiateAuth/AdminInitiateAuth errors include NotAuthorizedException 403) and the message 'Request not allowed due to security reasons.' — fixed text, no request-derived part — even when the password is correct. This holds both when the address is the request's own source and when it arrives in UserContextData.IpAddress from a client with EnablePropagateAdditionalUserContextData.
Emulator: the sign-in succeeds (the check reports 'returned OK') from an address inside the blocked range, in both the source-address and the propagated-UserContextData variants.
Why: a blocklist a tenant configured as a security control has no effect — traffic AWS would reject is authenticated and tokens are issued.
Fix: evaluate the pool's RiskExceptionConfiguration (BlockedIPRangeList, including the UserContextData.IpAddress path) in the authentication handlers before credentials are accepted.
Traces: cognito-idp.deep-cognito-idp-3-enforce-risk-ip-range-blocked, cognito-idp.deep-cognito-idp-3-enforce-risk-user-context-ip-blocked.

Cause 8 of 9 — UpdateProvisionedLimit accepts a RequestedLimitValue equal to the value already in effect.
AWS: re-requesting the current value is refused with InvalidParameterException (Smithy: UpdateProvisionedLimit errors = InvalidParameterException 400, NotAuthorizedException 403, ResourceNotFoundException 404, ServiceQuotaExceededException 400, TooManyRequestsException 429) and the message 'Requested value is the same as the current provisioned limit' — fixed text — and the value does not move. The same refusal is what makes the no-op distinguishable from a real raise in the concurrency and round-trip flows.
Emulator: the call returns 200 with Limit.ProvisionedLimitValue echoing the category's current/free value, so a redundant write is indistinguishable from an effective one. (Secondary observation from the same responses: the echoed ProvisionedLimitValue equals FreeLimitValue rather than the value the request asked for.)
Why: callers cannot detect a no-op update; idempotency and concurrent-modification handling written against AWS behaves differently here, and quota-management tooling silently believes it changed capacity.
Fix: the no-op/idempotency guard in the UpdateProvisionedLimit handler, before the state write.
Traces: cognito-idp.op-contract-update-provisioned-limit, cognito-idp.provisioned-limit-large-numeric-value-precision-preserved, cognito-idp.provisioned-limit-numeric-value-precision-round-trip, cognito-idp.provisioned-limit-update-concurrent-modification-conflict, cognito-idp.provisioned-limit-update-reflected.

Cause 9 of 9 — ForgotPassword delivers by SMS on a pool with no SmsConfiguration instead of falling through to the next recovery mechanism.
AWS: recovery walks AccountRecoverySetting.RecoveryMechanisms in Priority order, but a verified_phone_number mechanism is SKIPPED when the user pool has no SmsConfiguration, so recovery falls through to the next mechanism (verified_email) and ForgotPassword returns CodeDeliveryDetails.DeliveryMedium 'EMAIL' — even for a user whose phone_number IS verified. Recorded behavior: the pool in the check carries RecoveryMechanisms [{1,verified_phone_number},{2,verified_email}] and no SmsConfiguration, and AWS answers EMAIL.
Emulator: ForgotPassword reports CodeDeliveryDetails.DeliveryMedium 'SMS'.
Why: the code is reported as sent over a channel the pool cannot actually use, so a client waits for an SMS that can never arrive instead of checking email.
Fix: the recovery-mechanism selection in the ForgotPassword handler (skip verified_phone_number when the pool has no SmsConfiguration).
Traces: cognito-idp.refarch-cog-verified-phone-still-recovers-by-email-without-sms-config.

## Evidence
Verbatim from the replay (one line per cause):
1. StartUserImportJob on a job whose role trusts ec2.amazonaws.com: 'expected the call to be refused, but it succeeded: {"UserImportJob": {... "Status": "Pending", "CloudWatchLogsRoleArn": "arn:aws:iam::...:role/sim2real-badimport-badrole-..." ...}, "ResponseMetadata": {"HTTPStatusCode": 200 ...}}'. Expected PreconditionNotMetException with 'The configured Cloudwatch Logs Role is missing permissions or has an invalid trust policy.'
2. CreateUserPool with RecoveryMechanisms [{Priority:1,verified_email},{Priority:1,verified_phone_number}]: 'the call succeeded; real AWS rejects it, so there is no error code'. Expected InvalidParameterException 'Invalid account recovery setting parameter. Account Recovery Setting cannot have duplicate priorities or recovery mechanisms.'
3. ForgotPassword for a FORCE_CHANGE_PASSWORD user: 'the call succeeded; real AWS rejects it, so there is no error code'. Expected NotAuthorizedException 'User password cannot be reset in the current state.'
4. SignUp on an AllowAdminCreateUserOnly pool: 'the call succeeded; real AWS rejects it, so there is no error response'. Expected ('NotAuthorizedException', 'SignUp is not permitted for this user pool').
5. UpdateUserPoolClient CallbackURLs=['http://vpc-endpoint.example.com/_dashboards/app/home']: 'the call was expected to be REFUSED but it succeeded: {"UserPoolClient": {... "CallbackURLs": ["http://vpc-endpoint.example.com/_dashboards/app/home"] ...}, "HTTPStatusCode": 200}'. Expected InvalidParameterException '<url> cannot use the HTTP protocol.'
6. CreateUserPoolClient with AllowedOAuthFlowsUserPoolClient=True, AllowedOAuthFlows=['code','implicit'], no AllowedOAuthScopes: 'the call was expected to be REFUSED but it succeeded: {... "AllowedOAuthFlows": ["code", "implicit"], "AllowedOAuthFlowsUserPoolClient": True ...}'. Expected InvalidParameterException 'AllowedOAuthFlows and AllowedOAuthScopes are required if user pool client is allowed to use OAuth flows.'
7. 'ENFORCED pool: correct password from blocked-range IP 203.0.113.77 returned \'OK\', expected NotAuthorizedException' and 'propagating client: blocked-range UserContextData IP 203.0.113.77 returned \'OK\', expected NotAuthorizedException'. Expected ('NotAuthorizedException', 'Request not allowed due to security reasons.').
8. 'UpdateProvisionedLimit(RequestedLimitValue=50) succeeded when re-requesting the account\'s own current value -- real AWS rejects this as a redundant no-op'; 'a same-value UpdateProvisionedLimit(50) unexpectedly succeeded -- expected it to be rejected as a no-op request'; 'expected UpdateProvisionedLimit to be rejected, but it SUCCEEDED and returned {"Limit": {"LimitDefinition": {"LimitClass": "API_CATEGORY", "Attributes": {"Category": "UserCreation"}}, "ProvisionedLimitValue": 50, "FreeLimitValue": 50}, ... "HTTPStatusCode": 200}'. Expected InvalidParameterException 'Requested value is the same as the current provisioned limit'.
9. 'the priority-1 verified_phone_number mechanism is SKIPPED because the pool has no SmsConfiguration, so recovery falls through to priority-2 verified_email even for a user whose phone IS verified; Cognito reported DeliveryMedium \'SMS\''.

## Steps to Reproduce
1. CreateUserImportJob with a CloudWatchLogsRoleArn whose trust policy names ec2.amazonaws.com, upload the CSV to the pre-signed URL, then StartUserImportJob -> expect PreconditionNotMetException 'The configured Cloudwatch Logs Role is missing permissions or has an invalid trust policy.' and DescribeUserImportJob still 'Created'.
2. CreateUserPool with AccountRecoverySetting.RecoveryMechanisms = [{Priority:1,Name:verified_email},{Priority:1,Name:verified_phone_number}] -> expect InvalidParameterException naming the duplicate-priority rule, and no pool created.
3. AdminCreateUser (no TemporaryPassword), confirm AdminGetUser UserStatus is FORCE_CHANGE_PASSWORD, then ForgotPassword for that user -> expect NotAuthorizedException 'User password cannot be reset in the current state.'; after AdminSetUserPassword(Permanent=true) the same call must succeed with DeliveryMedium EMAIL.
4. CreateUserPool with AdminCreateUserConfig.AllowAdminCreateUserOnly=true, create a client, then SignUp -> expect NotAuthorizedException 'SignUp is not permitted for this user pool' and ListUsers empty; AdminCreateUser on the same pool must still succeed.
5. Create an OAuth client with CallbackURLs=['https://alb.example.com/...'], then UpdateUserPoolClient with CallbackURLs=['http://vpc-endpoint.example.com/...'] -> expect InvalidParameterException '<url> cannot use the HTTP protocol.' and the stored callback unchanged; the https form must be accepted.
6. CreateUserPoolClient with AllowedOAuthFlowsUserPoolClient=True, AllowedOAuthFlows=['code','implicit'], CallbackURLs set and AllowedOAuthScopes omitted -> expect InvalidParameterException and no client created.
7. SetRiskConfiguration with RiskExceptionConfiguration.BlockedIPRangeList covering 203.0.113.0/24, then authenticate with the correct password from 203.0.113.77 (and again with UserContextData.IpAddress=203.0.113.77 on a client with EnablePropagateAdditionalUserContextData) -> expect NotAuthorizedException 'Request not allowed due to security reasons.' both times.
8. GetProvisionedLimit for LimitClass API_CATEGORY / Category UserCreation, then UpdateProvisionedLimit with RequestedLimitValue equal to the returned ProvisionedLimitValue -> expect InvalidParameterException 'Requested value is the same as the current provisioned limit'.
9. CreateUserPool with no SmsConfiguration and RecoveryMechanisms [{1,verified_phone_number},{2,verified_email}], create a user whose phone_number and email are both verified, then ForgotPassword -> expect CodeDeliveryDetails.DeliveryMedium 'EMAIL'.
