Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-osc-dashboards-client-callback-diagnosis
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-osc-dashboards-client-callback-diagnosis cognito-idp-osc-dashboards-callback-repoint-to-alb cognito-idp-self-service-mfa-preference-and-auth-factors cognito-idp-acr-traj-sms-mfa-config-mistakes cognito-idp-acr-traj-recovery-setting-update-mistakes cognito-idp-acr-traj-forgot-password-falls-to-email cip-chal-mfa-setup-required cognito-idp-self-service-password-challenge-and-token-lifecycle
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|DescribeUserPoolDomain|2xx->2xx|$.DomainDescription.AWSAccountId
Issue-Key: cognito-idp|DescribeUserPoolDomain|2xx->2xx|$.DomainDescription.Domain
Issue-Key: cognito-idp|DescribeUserPoolDomain|2xx->2xx|$.DomainDescription.ManagedLoginVersion
Issue-Key: cognito-idp|DescribeUserPoolDomain|2xx->2xx|$.DomainDescription.UserPoolId
Issue-Key: cognito-idp|DescribeUserPoolClient|2xx->2xx|$.UserPoolClient.CallbackURLs[*]
Issue-Key: cognito-idp|DescribeUserPoolClient|2xx->2xx|$.UserPoolClient.LogoutURLs
Issue-Key: cognito-idp|GetUserAuthFactors|2xx->2xx|$.ConfiguredUserAuthFactors[*]
Issue-Key: cognito-idp|GetUserAuthFactors|2xx->2xx|$.PreferredMfaSetting
Issue-Key: cognito-idp|GetUserAuthFactors|2xx->2xx|$.UserMFASettingList
Issue-Key: cognito-idp|GetUserPoolMfaConfig|2xx->2xx|$.MfaConfiguration
Issue-Key: cognito-idp|DescribeUserPool|2xx->2xx|$.UserPool.AccountRecoverySetting.RecoveryMechanisms[*]
Issue-Key: cognito-idp|ForgotPassword|2xx->2xx|$.CodeDeliveryDetails.AttributeName
Issue-Key: cognito-idp|ForgotPassword|2xx->2xx|$.CodeDeliveryDetails.DeliveryMedium
Issue-Key: cognito-idp|InitiateAuth|2xx->2xx|$.Session
Issue-Related: 20261029T184956Z-cognito-idp-app-client-reads-omit-validity-defaults.md
Issue-Related: 20261029T184956Z-cognito-idp-code-delivery-destination-masking-keeps-the.md
Issue-Related: 20261029T184956Z-cognito-idp-hosted-ui-branding-and-domain-reads.md
Issue-Commit: 7923db9e5c097e7a9eee0c015b735053c707d1d4

# cognito-idp: response payloads diverge — read paths omit or misreport pool, client, domain and MFA state

> _Note: this issue was identified against the code base at commit `7923db9e5c097e7a9eee0c015b735053c707d1d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeUserPoolDomain

## Description
Eight independent causes (all cognito-idp). Every one is a 200 response whose body reports state AWS does not: a modeled member is omitted entirely, or the value served is not the state the preceding write established. Net-new: the open issues 'cognito-idp: app client reads omit validity defaults and reorder request lists' (20261029T184956Z-cognito-idp-app-client-reads-omit-validity-defaults.md), 'cognito-idp: hosted-UI, branding and domain reads omit server-generated fields' (20261029T184956Z-cognito-idp-hosted-ui-branding-and-domain-reads.md), 'cognito-idp: code-delivery Destination masking keeps the domain suffix' (20261029T184956Z-cognito-idp-code-delivery-destination-masking-keeps-the.md) and 'cognito-idp: (Admin)InitiateAuth responses diverge — refresh token shape and missing MFA_SETUP challenge' (20261030T114740Z-cognito-idp-admin-initiateauth-responses-diverge-refresh-token.md) own the AuthSessionValidity/RefreshTokenValidity/TokenValidityUnits, RefreshToken and Destination-masking clusters; none of them carries the replay keys below.

Cause 1 of 8: DescribeUserPoolDomain serves an empty DomainDescription for a domain that exists.
AWS: DescribeUserPoolDomain answers 200 with DomainDescription populated — UserPoolId, AWSAccountId, Domain, S3Bucket, CloudFrontDistribution, Version, Status (DomainStatusType, ACTIVE once provisioned) and ManagedLoginVersion. Smithy: DomainDescriptionType members UserPoolId, AWSAccountId, Domain, S3Bucket, CloudFrontDistribution, Version, Status, CustomDomainConfig, ManagedLoginVersion, Routing. The concrete S3 asset bucket and CloudFront hostname are server-generated per environment; what is required is that the members are present and internally consistent (Domain echoing the created prefix, UserPoolId naming the attached pool, Status ACTIVE), not this run's literal values.
Emulator: answers 200 with DomainDescription containing none of these members (every one reads None).
Why: a client resolving a pool's hosted-UI/managed-login endpoint, or waiting for Status ACTIVE after CreateUserPoolDomain, gets no endpoint and no status at all and cannot proceed or poll.
Fix: DescribeUserPoolDomain's handler and serializer plus the domain state record that CreateUserPoolDomain writes (the server-generated S3 bucket, CloudFront distribution, version and status must be materialized at create time).
Traces: cognito-idp-osc-dashboards-client-callback-diagnosis#task#7.

Cause 2 of 8: DescribeUserPoolClient reports the wrong CallbackURLs and drops LogoutURLs.
AWS: DescribeUserPoolClient echoes the app client's currently stored CallbackURLs and LogoutURLs exactly as the last successful Create/UpdateUserPoolClient set them, and serializes LogoutURLs whenever the client has any. Smithy: UserPoolClientType members CallbackURLs and LogoutURLs (lists of RedirectUrlType); DescribeUserPoolClient output UserPoolClient -> UserPoolClientType.
Emulator: serves a CallbackURLs[0] that is not the value in force at that point in the trajectory (it reports the later-repointed URL) and omits LogoutURLs entirely.
Why: a client reading back a client's OAuth redirect configuration — to diagnose a redirect_mismatch, or to repoint it safely — sees a callback list it never wrote and concludes the client has no logout URLs, so the sign-out redirect it configured is invisible and a read-modify-write wipes it.
Fix: UpdateUserPoolClient's state write (ordering/replacement of the URL lists) and DescribeUserPoolClient's serializer (emit LogoutURLs when present). The validity-default members in the same cluster are owned by 20261029T184956Z-cognito-idp-app-client-reads-omit-validity-defaults.md.
Traces: cognito-idp-osc-dashboards-callback-repoint-to-alb#task#6.

Cause 3 of 8: GetUserAuthFactors reports the wrong configured auth factors.
AWS: GetUserAuthFactors returns ConfiguredUserAuthFactors listing exactly the factors the user has configured, in AWS's order; the recorded response has EMAIL_OTP in the position the emulator fills with SMS_OTP. Smithy: ConfiguredUserAuthFactorsListType @length 0..8 of AuthFactorType [PASSWORD, EMAIL_OTP, SMS_OTP, WEB_AUTHN, SOFTWARE_TOKEN].
Emulator: serves SMS_OTP where the recording has EMAIL_OTP, i.e. the factor list is derived from something other than the user's MFA preference/verified-attribute state.
Why: a client that offers the user a choice of sign-in factors offers SMS to a user who has only email configured, and the resulting challenge cannot be answered.
Fix: GetUserAuthFactors' handler — derive ConfiguredUserAuthFactors from the user's SetUserMFAPreference state and verified attributes, preserving AWS's factor order.
Traces: cognito-idp-self-service-mfa-preference-and-auth-factors#task#10.

Cause 4 of 8: GetUserAuthFactors omits PreferredMfaSetting, UserMFASettingList and the remaining factors.
AWS: after the user's MFA preference is set, GetUserAuthFactors returns PreferredMfaSetting (the preferred method name, e.g. SMS_MFA) and UserMFASettingList (the enabled methods), alongside the full ConfiguredUserAuthFactors list. Smithy: GetUserAuthFactors output members Username (@required), PreferredMfaSetting, UserMFASettingList, ConfiguredUserAuthFactors.
Emulator: omits PreferredMfaSetting and UserMFASettingList entirely and truncates ConfiguredUserAuthFactors (the entries past the first are absent).
Why: a client cannot tell which MFA method is preferred or even enabled, so it cannot render the user's security settings or pick a default challenge.
Fix: GetUserAuthFactors' serializer plus the user-level MFA state it reads (the preference write must be observable here, as it is from GetUser).
Traces: cognito-idp-self-service-mfa-preference-and-auth-factors#task#12.

Cause 5 of 8: GetUserPoolMfaConfig reports MFA as enabled after a refused SetUserPoolMfaConfig.
AWS: GetUserPoolMfaConfig reports MfaConfiguration as the pool's last successfully applied value; a SetUserPoolMfaConfig that AWS refused leaves it unchanged, recorded here as OFF. Smithy: GetUserPoolMfaConfig output MfaConfiguration -> UserPoolMfaType [OFF, ON, OPTIONAL].
Emulator: reports OPTIONAL — the value from the request AWS rejected, which the emulator committed.
Why: a client reading the pool's MFA configuration back sees MFA enabled with no method behind it, so it routes sign-in through a challenge that cannot be served. This is the read-side evidence that a refused write was persisted; the gate itself is filed separately under 'cognito-idp: user-pool request-precondition gates reach the wrong verdict'.
Fix: SetUserPoolMfaConfig must not mutate pool state on the refusal path; GetUserPoolMfaConfig then reports the unchanged value.
Traces: cognito-idp-acr-traj-sms-mfa-config-mistakes#task#7.

Cause 6 of 8: DescribeUserPool reports the wrong AccountRecoverySetting recovery mechanism.
AWS: DescribeUserPool echoes AccountRecoverySetting.RecoveryMechanisms as the last accepted update left them — here {Name: verified_phone_number, Priority: 1}. Smithy: AccountRecoverySettingType.RecoveryMechanisms @length 1..2 of RecoveryOptionType, Name -> RecoveryOptionNameType [verified_email, verified_phone_number, admin_only].
Emulator: reports {Name: admin_only, Priority: 1} — a mechanism the trajectory did not establish.
Why: a client inspecting a pool to decide whether self-service password recovery is available reads admin_only and disables its forgot-password flow for a pool where AWS would deliver a code.
Fix: UpdateUserPool's AccountRecoverySetting state write and DescribeUserPool's projection of it (a refused or partial update must not substitute admin_only).
Traces: cognito-idp-acr-traj-recovery-setting-update-mistakes#task#7.

Cause 7 of 8: ForgotPassword picks SMS/phone_number where AWS falls back to email.
AWS: ForgotPassword selects the delivery medium from the pool's AccountRecoverySetting priority order restricted to what the user actually has verified, and reports it in CodeDeliveryDetails — here AttributeName 'email', DeliveryMedium 'EMAIL', with Destination the masked email address. Smithy: CodeDeliveryDetailsType members Destination, DeliveryMedium (DeliveryMediumType), AttributeName; the fallback rule itself is recorded behavior only.
Emulator: reports AttributeName 'phone_number', DeliveryMedium 'SMS' and a masked phone number, i.e. it uses the higher-priority mechanism without checking that the user can receive on it.
Why: the client tells the user to expect an SMS that is never deliverable, and a confirmation step keyed on the delivery medium takes the wrong branch. (The masking format of Destination itself is owned by 20261029T184956Z-cognito-idp-code-delivery-destination-masking-keeps-the.md; the divergence here is the chosen attribute and medium.)
Fix: ForgotPassword's handler — the recovery-mechanism selection must skip mechanisms the user has not verified before composing CodeDeliveryDetails.
Traces: cognito-idp-acr-traj-forgot-password-falls-to-email#task#11.

Cause 8 of 8: InitiateAuth issues a Session token that is not AWS-shaped.
AWS: when InitiateAuth returns a challenge it issues an opaque Session token; the recordings show a long base64url-style token (hundreds of characters) of the kind every challenge response carries, and the value is not asserted literally — only its shape class is. Smithy: SessionType is a @sensitive string with @length min 20 max 4096 (no @pattern), used for both InitiateAuth.Session input and output.
Emulator: returns a 64-character lowercase-hex string. It satisfies the length bound but not the recorded token shape. Hypothesis: the session id is a raw hex digest of internal state rather than an encoded session blob.
Why: a client or proxy that validates/round-trips the challenge session against AWS's token shape rejects it, and anything sizing a buffer or a store column from observed AWS sessions is calibrated against a value an order of magnitude smaller.
Fix: the InitiateAuth (and RespondToAuthChallenge) challenge-session serializer / session-token minting, not the challenge state machine. Same family as the RefreshToken shape cause in 20261030T114740Z-cognito-idp-admin-initiateauth-responses-diverge-refresh-token.md, which does not carry this step.
Traces: cip-chal-mfa-setup-required#task#7.

## Evidence
Cause 1 — DescribeUserPoolDomain, cognito-idp-osc-dashboards-client-callback-diagnosis#task#7 (200->200): $.DomainDescription.AWSAccountId, .CloudFrontDistribution, .Domain, .ManagedLoginVersion, .S3Bucket, .Status, .UserPoolId, .Version all expected present, all got None.
Cause 2 — DescribeUserPoolClient, cognito-idp-osc-dashboards-callback-repoint-to-alb#task#6 (200->200): $.UserPoolClient.CallbackURLs[0] value_mismatch (expected the then-current ES dashboards URL, got the later ALB URL); $.UserPoolClient.LogoutURLs missing (expected a one-element list, got None).
Cause 3 — GetUserAuthFactors, cognito-idp-self-service-mfa-preference-and-auth-factors#task#10 (200->200): $.ConfiguredUserAuthFactors[1] expected 'EMAIL_OTP', got 'SMS_OTP'.
Cause 4 — GetUserAuthFactors, cognito-idp-self-service-mfa-preference-and-auth-factors#task#12 (200->200): $.ConfiguredUserAuthFactors[1] expected 'EMAIL_OTP' got None; [2] expected 'SMS_OTP' got None; $.PreferredMfaSetting expected 'SMS_MFA' got None; $.UserMFASettingList expected ['SMS_MFA'] got None.
Cause 5 — GetUserPoolMfaConfig, cognito-idp-acr-traj-sms-mfa-config-mistakes#task#7 (200->200): $.MfaConfiguration expected 'OFF', got 'OPTIONAL'.
Cause 6 — DescribeUserPool, cognito-idp-acr-traj-recovery-setting-update-mistakes#task#7 (200->200): $.UserPool.AccountRecoverySetting.RecoveryMechanisms[0] expected {'Name':'verified_phone_number','Priority':1}, got {'Priority':1,'Name':'admin_only'}.
Cause 7 — ForgotPassword, cognito-idp-acr-traj-forgot-password-falls-to-email#task#11 (200->200): $.CodeDeliveryDetails.AttributeName expected 'email' got 'phone_number'; .DeliveryMedium expected 'EMAIL' got 'SMS'; .Destination expected a masked email got a masked phone number.
Cause 8 — InitiateAuth, cip-chal-mfa-setup-required#task#7 (200->200, 2 steps incl. cognito-idp-self-service-password-challenge-and-token-lifecycle#task#5): $.Session shape-asserted against the recorded long opaque token, got a 64-char hex string.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: replay cognito-idp-osc-dashboards-client-callback-diagnosis step task#7 (DescribeUserPoolDomain after the domain is created).
3. Cause 2: replay cognito-idp-osc-dashboards-callback-repoint-to-alb step task#6 (DescribeUserPoolClient between the callback-URL updates).
4. Causes 3 and 4: replay cognito-idp-self-service-mfa-preference-and-auth-factors steps task#10 and task#12 (GetUserAuthFactors after SetUserMFAPreference).
5. Cause 5: replay cognito-idp-acr-traj-sms-mfa-config-mistakes step task#7 (GetUserPoolMfaConfig after the refused SetUserPoolMfaConfig at task#4).
6. Cause 6: replay cognito-idp-acr-traj-recovery-setting-update-mistakes step task#7 (DescribeUserPool after the recovery-setting updates).
7. Cause 7: replay cognito-idp-acr-traj-forgot-password-falls-to-email step task#11 (ForgotPassword for a user with no verified phone).
8. Cause 8: replay cip-chal-mfa-setup-required step task#7 (InitiateAuth returning a challenge) and inspect $.Session.
9. Compare each 200 body member-by-member against the recorded response.
