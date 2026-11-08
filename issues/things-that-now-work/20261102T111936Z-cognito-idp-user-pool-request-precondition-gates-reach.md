Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-acr-traj-enable-phone-recovery-update-mistakes
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-acr-traj-enable-phone-recovery-update-mistakes cognito-idp-acr-traj-recovery-setting-update-mistakes cognito-idp-recovery-priority-order-round-trips cognito-idp-unset-sms-configuration-stays-absent cognito-idp-acr-traj-sms-mfa-config-mistakes cognito-idp-acr-traj-forgot-password-falls-to-email cognito-idp-acr-traj-list-clients-paging
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|UpdateUserPool|4xx->2xx|$
Issue-Key: cognito-idp|UpdateUserPool|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|UpdateUserPool|4xx->2xx|status
Issue-Key: cognito-idp|CreateUserPool|4xx->2xx|$.__type
Issue-Key: cognito-idp|CreateUserPool|4xx->2xx|$.message
Issue-Key: cognito-idp|CreateUserPool|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|CreateUserPool|4xx->2xx|status
Issue-Key: cognito-idp|SetUserPoolMfaConfig|4xx->2xx|$.__type
Issue-Key: cognito-idp|SetUserPoolMfaConfig|4xx->2xx|$.message
Issue-Key: cognito-idp|SetUserPoolMfaConfig|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|SetUserPoolMfaConfig|4xx->2xx|status
Issue-Key: cognito-idp|ForgotPassword|4xx->2xx|$.__type
Issue-Key: cognito-idp|ForgotPassword|4xx->2xx|$.message
Issue-Key: cognito-idp|ForgotPassword|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|ForgotPassword|4xx->2xx|status
Issue-Key: cognito-idp|ListUserPoolClients|2xx->4xx|$.UserPoolClients
Issue-Key: cognito-idp|ListUserPoolClients|2xx->4xx|status
Issue-Commit: 7923db9e5c097e7a9eee0c015b735053c707d1d4

# cognito-idp: user-pool request-precondition gates reach the wrong verdict

> _Note: this issue was identified against the code base at commit `7923db9e5c097e7a9eee0c015b735053c707d1d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateUserPool

## Description
Five independent causes (all cognito-idp). Four user-pool write/precondition gates accept a request AWS refuses with HTTP 400 and answer 200 with an empty body, and one read gate refuses a request AWS accepts. All five are net-new: the open issue 'cognito-idp: ten request-validation and precondition gates accept calls AWS refuses' (20261029T184956Z-cognito-idp-ten-request-validation-and-precondition-gates.md) covers a different set of gates and does not carry any of the replay keys below.

Cause 1 of 5: UpdateUserPool accepts phone_number auto-verification with no SMS configuration.
AWS: UpdateUserPool refuses a request that puts phone_number into AutoVerifiedAttributes while the pool has no usable SmsConfiguration. Recorded response: HTTP 400, __type InvalidParameterException, message 'SMS configuration is required when phone_number is selected for auto verification' (that message is entirely fixed text — it carries no request or state values), plus the x-amzn-errortype header. Smithy: UpdateUserPool.AutoVerifiedAttributes -> VerifiedAttributesListType, UpdateUserPool.SmsConfiguration -> SmsConfigurationType, modeled error InvalidParameterException (@httpError 400); the cross-member precondition itself is recorded behavior only.
Emulator: answers HTTP 200 with an empty JSON body and no x-amzn-errortype header, and persists the auto-verified attribute.
Why: a client that relies on the refusal to detect a misconfigured pool sees the update succeed, then every phone-number verification and SMS-based recovery in that pool silently has no delivery path. Error handling for this misconfiguration can never be exercised.
Fix: UpdateUserPool's input validation / precondition check (the cross-member AutoVerifiedAttributes-vs-SmsConfiguration gate), before the pool state is written.
Traces: cognito-idp-acr-traj-enable-phone-recovery-update-mistakes#task#2 (cluster also covers that fixture's task#3 and task#5 and cognito-idp-acr-traj-recovery-setting-update-mistakes#task#2 and #task#5).

Cause 2 of 5: CreateUserPool accepts an AccountRecoverySetting with duplicate priorities or duplicate mechanisms.
AWS: CreateUserPool refuses an AccountRecoverySetting whose RecoveryMechanisms repeat a Priority value or repeat a mechanism Name. Recorded response: HTTP 400, __type InvalidParameterException, message 'Invalid account recovery setting parameter. Account Recovery Setting cannot have duplicate priorities or recovery mechanisms.' (all fixed text), x-amzn-errortype 'InvalidParameterException:'. Smithy: AccountRecoverySettingType.RecoveryMechanisms is a list of RecoveryOptionType with @length min 1 max 2 and member names from RecoveryOptionNameType [verified_email, verified_phone_number, admin_only]; the uniqueness rule across Priority/Name is recorded behavior only (the list is not @uniqueItems).
Emulator: answers HTTP 200 and creates the pool with the duplicated recovery setting.
Why: the pool is created in a state AWS cannot produce, so the subsequent forgot-password priority order is undefined and a client that validates its own recovery configuration by round-tripping it through CreateUserPool gets a false pass.
Fix: CreateUserPool's AccountRecoverySetting validation (duplicate-Priority and duplicate-Name check over RecoveryMechanisms).
Traces: cognito-idp-recovery-priority-order-round-trips#task#2 (cluster also covers cognito-idp-unset-sms-configuration-stays-absent#task#2).

Cause 3 of 5: SetUserPoolMfaConfig accepts MfaConfiguration ON/OPTIONAL with no MFA method enabled.
AWS: SetUserPoolMfaConfig refuses a request that turns MFA on without enabling at least one method (SmsMfaConfiguration, EmailMfaConfiguration or SoftwareTokenMfaConfiguration). Recorded response: HTTP 400, __type InvalidParameterException, message 'Invalid MFA Configuration given. SMS MFA, Email MFA, or Software Token MFA must be enabled.' (all fixed text), plus x-amzn-errortype. Smithy: SetUserPoolMfaConfig input members SmsMfaConfiguration/EmailMfaConfiguration/SoftwareTokenMfaConfiguration and MfaConfiguration -> UserPoolMfaType [OFF, ON, OPTIONAL]; modeled error InvalidParameterException (@httpError 400). The at-least-one-method precondition is recorded behavior only.
Emulator: answers HTTP 200 and stores MfaConfiguration as requested with no method enabled.
Why: the pool ends up advertising MFA with no way to challenge, so sign-in either skips the challenge or fails later with an unrelated error; a client's configuration-validation path never fires.
Fix: SetUserPoolMfaConfig's validation, before the MFA configuration is committed to pool state.
Traces: cognito-idp-acr-traj-sms-mfa-config-mistakes#task#4 (cluster also covers that fixture's task#6).

Cause 4 of 5: ForgotPassword succeeds for a user whose password cannot be reset in its current state.
AWS: when the target user's state does not permit a self-service reset, ForgotPassword refuses instead of sending a code. Recorded response: HTTP 400, __type NotAuthorizedException, message 'User password cannot be reset in the current state.' (all fixed text), plus x-amzn-errortype. Smithy: ForgotPassword models NotAuthorizedException (modeled @httpError 403; the recording shows the wire status is 400, and the recording governs); output is CodeDeliveryDetails only.
Emulator: answers HTTP 200 with no error members at all, i.e. it treats the reset as started.
Why: a client is told a reset code was sent for an account that can never complete the reset, so it waits for a code that does not exist instead of taking the documented remediation path (admin reset / confirm the user).
Fix: ForgotPassword's handler precondition on user status, ahead of code delivery.
Traces: cognito-idp-acr-traj-forgot-password-falls-to-email#task#9 (cluster also covers that fixture's task#10).

Cause 5 of 5: ListUserPoolClients refuses a request AWS answers with an empty page.
AWS: ListUserPoolClients answers HTTP 200 and always serializes UserPoolClients, as an empty list when the pool has no clients left to return for the requested page. Smithy: ListUserPoolClients.MaxResults -> QueryLimit @range 1..60, NextToken -> PaginationKey @length 1..131072 @pattern ^[\S]+$, output UserPoolClients -> UserPoolClientListType, @paginated(items: UserPoolClients).
Emulator: answers HTTP 400 for that same paging request and serializes no UserPoolClients member at all.
Why: a paginating client that walks a pool's clients to exhaustion gets a hard client error instead of a terminal empty page, so the loop aborts and the caller cannot distinguish 'no more clients' from a malformed request.
Fix: ListUserPoolClients' input validation / pagination handling (accept the recorded MaxResults/NextToken combination) and its serializer (always emit UserPoolClients, empty when there is nothing to return).
Traces: cognito-idp-acr-traj-list-clients-paging#task#3.

## Evidence
Cause 1 — UpdateUserPool, cognito-idp-acr-traj-enable-phone-recovery-update-mistakes#task#2 (5 steps in cluster): expected 400 {'__type':'InvalidParameterException','message':'SMS configuration is required when phone_number is selected for auto verification'} + x-amzn-errortype; actual 200 with body {} and no x-amzn-errortype.
Cause 2 — CreateUserPool, cognito-idp-recovery-priority-order-round-trips#task#2 (2 steps): expected 400 {'__type':'InvalidParameterException','message':'Invalid account recovery setting parameter. Account Recovery Setting cannot have duplicate priorities or recovery mechanisms.'}, x-amzn-errortype 'InvalidParameterException:'; actual 200.
Cause 3 — SetUserPoolMfaConfig, cognito-idp-acr-traj-sms-mfa-config-mistakes#task#4 (2 steps): expected 400 {'__type':'InvalidParameterException','message':'Invalid MFA Configuration given. SMS MFA, Email MFA, or Software Token MFA must be enabled.'}; actual 200.
Cause 4 — ForgotPassword, cognito-idp-acr-traj-forgot-password-falls-to-email#task#9 (2 steps): expected 400 {'__type':'NotAuthorizedException','message':'User password cannot be reset in the current state.'}; actual 200.
Cause 5 — ListUserPoolClients, cognito-idp-acr-traj-list-clients-paging#task#3: expected 200 with UserPoolClients []; actual 400 with UserPoolClients absent.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: replay fixture cognito-idp-acr-traj-enable-phone-recovery-update-mistakes step task#2 — UpdateUserPool adding phone_number to AutoVerifiedAttributes on a pool with no SmsConfiguration.
3. Cause 2: replay cognito-idp-recovery-priority-order-round-trips step task#2 — CreateUserPool with duplicate RecoveryMechanisms priorities.
4. Cause 3: replay cognito-idp-acr-traj-sms-mfa-config-mistakes step task#4 — SetUserPoolMfaConfig turning MFA on with no method enabled.
5. Cause 4: replay cognito-idp-acr-traj-forgot-password-falls-to-email step task#9 — ForgotPassword for a user that cannot self-reset.
6. Cause 5: replay cognito-idp-acr-traj-list-clients-paging step task#3 — ListUserPoolClients page that AWS answers empty.
7. Compare each response's HTTP status, __type, message and x-amzn-errortype header against the recorded step.
