Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.user-pool-client-callback-url-with-fragment-rejected cognito-idp.user-pool-client-code-flow-without-callback-urls-rejected cognito-idp.user-pool-domain-with-a-reserved-word-rejected cognito-idp.refarch-cog-admin-only-recovery-combined-with-email-rejected cognito-idp.refarch-cog-phone-recovery-with-mfa-required-rejected-without-sms-config cognito-idp.refarch-cog-unset-sms-configuration-stays-absent cognito-idp.deploy-user-pool-cfn-signup-behaves cognito-idp.invalid-lambda-response-error
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.user-pool-client-callback-url-with-fragment-rejected|_error#5|AssertionError|raise AssertionError(f"the call was expected to be REFUSED but it succeeded: {result!r}")
Issue-Key: cognito-idp.user-pool-client-code-flow-without-callback-urls-rejected|_error#5|AssertionError|raise AssertionError(f"the call was expected to be REFUSED but it succeeded: {result!r}")
Issue-Key: cognito-idp.user-pool-domain-with-a-reserved-word-rejected|_error#4|AssertionError|raise AssertionError(f"the call was expected to be REFUSED but it succeeded: {result!r}")
Issue-Key: cognito-idp.refarch-cog-admin-only-recovery-combined-with-email-rejected|_error#5|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: cognito-idp.refarch-cog-phone-recovery-with-mfa-required-rejected-without-sms-config|_error#4|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: cognito-idp.refarch-cog-unset-sms-configuration-stays-absent|run#8|AssertionError|assert pool.get("EmailConfiguration") == {"EmailSendingAccount": "COGNITO_DEFAULT"}, (
Issue-Key: cognito-idp.deploy-user-pool-cfn-signup-behaves|run#11|AssertionError|assert code == "InvalidPasswordException", f"expected InvalidPasswordException, got {code}"
Issue-Key: cognito-idp.invalid-lambda-response-error|_error#21|AssertionError|raise AssertionError("real AWS rejects this call, but it SUCCEEDED")
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# cognito-idp: user-pool and app-client configuration that AWS refuses is accepted, a defaulted member is omitted, and a password rejection is misclassified (8 causes)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUserPool

## Description
Eight distinct root causes in the cognito-idp control plane, all grounded in failing checks the replay report marked neither ALREADY FILED nor OWNED. Six are accept-what-AWS-refuses (overly permissive), one is a missing defaulted response member, one is an error-class misclassification.

Cause 1 of 8 — CreateUserPoolClient accepts a CallbackURL that contains a fragment.
AWS: recorded behavior only — a callback URL with a '#fragment' component is refused with a 400 client error; Smithy (cognito-idp) CreateUserPoolClient models InvalidParameterException (400) and InvalidOAuthFlowException (400), and CallbackURLs is a plain string list, so nothing about the envelope is wrong and only the value rule rejects it. The recording asserts that the call is REFUSED; the exact message is an example, not a requirement.
Emulator: HTTP 200, the client is created and echoes CallbackURLs ['https://alb.example.com/_dashboards/app/home#login'].
Why: the hosted UI cannot redirect to a URI with a fragment, so a configuration AWS rejects at create time deploys here and only fails later at sign-in, where the client has no error to key on.
Fix: CallbackURLs/LogoutURLs value validation in the CreateUserPoolClient handler (and the same path in UpdateUserPoolClient).
Traces: cognito-idp.user-pool-client-callback-url-with-fragment-rejected.

Cause 2 of 8 — CreateUserPoolClient accepts AllowedOAuthFlows ['code','implicit'] with an empty CallbackURLs list.
AWS: recorded behavior only — a client that enables the code or implicit grant with no callback URL is refused with a 400 (Smithy models InvalidOAuthFlowException and InvalidParameterException on CreateUserPoolClient). The recording asserts refusal.
Emulator: HTTP 200 with UserPoolClient.CallbackURLs [] and AllowedOAuthFlows ['code','implicit'] and AllowedOAuthFlowsUserPoolClient true.
Why: the redirect-based flows are unusable on the created client, and the cross-member rule that guards against it is absent, so the misconfiguration is invisible until a real authorize request fails.
Fix: the cross-member check coupling AllowedOAuthFlows to CallbackURLs in the CreateUserPoolClient handler's validation.
Traces: cognito-idp.user-pool-client-code-flow-without-callback-urls-rejected.

Cause 3 of 8 — CreateUserPoolDomain accepts a reserved-word domain prefix.
AWS: recorded behavior only — Cognito refuses reserved domain prefixes (e.g. ones containing 'aws', 'amazon', 'cognito') with InvalidParameterException (400); Smithy CreateUserPoolDomain models InvalidParameterException, and the Domain member's own pattern ^[a-z0-9](?:[a-z0-9\-]{0,61}[a-z0-9])?$ is satisfied, so this is a value rule above the schema.
Emulator: HTTP 200 with {'ManagedLoginVersion': 1}; the domain is created.
Why: the domain is globally namespaced in AWS, so a prefix AWS would never grant appears to succeed; a deployment that cannot exist in production passes here.
Fix: the reserved-prefix validation in the CreateUserPoolDomain handler.
Traces: cognito-idp.user-pool-domain-with-a-reserved-word-rejected.

Cause 4 of 8 — CreateUserPool accepts an AccountRecoverySetting that combines admin_only with another recovery mechanism.
AWS: InvalidParameterException (400). Smithy: CreateUserPool models InvalidParameterException; AccountRecoverySettingType.RecoveryMechanisms is a list of length 1..2 of declared enum members at distinct priorities, so the envelope is valid and the rule is a relationship between the VALUES — admin_only is exclusive. Recorded message (an example): 'Invalid account recovery setting parameter. Account Recovery Setting cannot use admin_only setting with any other recovery mechanisms.'
Emulator: the call succeeds and a pool is created.
Why: the pool simultaneously claims self-service recovery and claims to have disabled it; ForgotPassword then has to pick one, and an auditor reading the config draws the opposite conclusion from what the pool does.
Fix: AccountRecoverySetting validation in the CreateUserPool/UpdateUserPool handler.
Traces: cognito-idp.refarch-cog-admin-only-recovery-combined-with-email-rejected.

Cause 5 of 8 — CreateUserPool accepts MfaConfiguration='ON' without an SmsConfiguration and without phone_number auto-verification.
AWS: InvalidParameterException (400) — MfaConfiguration ON/OPTIONAL requires BOTH SmsConfiguration and phone_number in AutoVerifiedAttributes. Smithy: MfaConfiguration, SmsConfiguration and AutoVerifiedAttributes are three independent optional members of CreateUserPoolRequest, so every combination is a valid envelope and only the coupling rejects this one. Recorded message (an example): 'SMS configuration and Auto verification for phone_number are required when MFA is required/optional'.
Emulator: the call succeeds and a pool with MfaConfiguration ON and no SMS delivery path is created.
Why: a pool is produced that requires an SMS second factor it can never deliver, so every user is locked out at sign-in instead of the configuration being refused at create time.
Fix: the cross-member MFA/SMS validation in the CreateUserPool and UpdateUserPool handlers.
Traces: cognito-idp.refarch-cog-phone-recovery-with-mfa-required-rejected-without-sms-config.

Cause 6 of 8 — a pool created with no EmailConfiguration reports no EmailConfiguration at all, instead of the Cognito-default sender.
AWS: recorded behavior only — DescribeUserPool for a pool created without EmailConfiguration returns EmailConfiguration {'EmailSendingAccount': 'COGNITO_DEFAULT'}. Smithy EmailConfigurationType.EmailSendingAccount documents COGNITO_DEFAULT as 'When Amazon Cognito emails your users, it uses its built-in email functionality', which is the defaulted state; the member is optional in the model, so a response that omits it is schema-valid and only behavior can say which AWS produces.
Emulator: UserPool.EmailConfiguration is absent (None).
Why: the presence of the defaulted member is what tells a caller that email delivery works (and, paired with an absent SmsConfiguration, that SMS does not). With it omitted, an auditor reads 'no email configured' and concludes email recovery is dead when it is the only working path.
Fix: the CreateUserPool/DescribeUserPool representation — materialise the COGNITO_DEFAULT EmailConfiguration when none is supplied, while keeping SmsConfiguration genuinely absent.
Traces: cognito-idp.refarch-cog-unset-sms-configuration-stays-absent.

Cause 7 of 8 — a password that violates the pool's password policy is refused with InvalidParameterException instead of InvalidPasswordException.
AWS: Smithy SignUp models InvalidPasswordException (400) — 'This exception is thrown when Amazon Cognito encounters an invalid password' — as an error distinct from InvalidParameterException (400), and a password below the pool's Policies.PasswordPolicy.MinimumLength is the InvalidPasswordException case.
Emulator: error code InvalidParameterException.
Why: a client distinguishes 'your password is too weak, choose another' (InvalidPasswordException, re-prompt the user) from 'your request is malformed' (InvalidParameterException, a bug); collapsing them means the sign-up form shows the wrong remedy and retry logic keyed on the password class never fires.
Fix: the error mapping of the password-policy check in the SignUp handler (and the same policy evaluation shared with AdminCreateUser/AdminSetUserPassword).
Traces: cognito-idp.deploy-user-pool-cfn-signup-behaves.

Cause 8 of 8 — a PreSignUp trigger whose response cannot be parsed does not fail AdminCreateUser.
AWS: Smithy AdminCreateUser and SignUp both model InvalidLambdaResponseException (400); when a configured PreSignUp trigger returns output Cognito cannot interpret, the operation is refused with that error and no user is created (recorded message, an example: 'Unrecognizable lambda output'), and a subsequent AdminGetUser answers UserNotFoundException.
Emulator: AdminCreateUser succeeds — 'real AWS rejects this call, but it SUCCEEDED' — so the trigger's malformed response is silently ignored and the user exists.
Why: a broken trigger is invisible: every flow that depends on the trigger's verdict (validation, auto-confirmation, attribute injection) proceeds as if the trigger ran and approved it, which is an authorization bypass in anything built on PreSignUp.
Fix: the user-pool Lambda-trigger dispatch in the AdminCreateUser/SignUp handlers — validate the trigger response and map an uninterpretable one to InvalidLambdaResponseException, leaving no user behind. Related open issue 20261023T114649Z-cognito-idp-cognito-user-pool-lambda-triggers-are-not-invoked.md records trigger non-invocation for a PreSignUp that raises, for PreTokenGeneration V2_0 overrides and for a non-invokable trigger ARN; it carries neither this check's replay key nor the InvalidLambdaResponseException class, so a fixer could close it without re-running this one.
Traces: cognito-idp.invalid-lambda-response-error.

## Evidence
Cause 1: AssertionError: the call was expected to be REFUSED but it succeeded: {'UserPoolId': 'us-west-2_...', 'ClientId': '...', 'CallbackURLs': ['https://alb.example.com/_dashboards/app/home#login'], 'AllowedOAuthFlows': ['code', 'implicit'], ...} — HTTP 200.
Cause 2: AssertionError: the call was expected to be REFUSED but it succeeded: {'UserPoolClient': {..., 'CallbackURLs': [], 'AllowedOAuthFlows': ['code', 'implicit'], 'AllowedOAuthFlowsUserPoolClient': True, ...}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}.
Cause 3: AssertionError: the call was expected to be REFUSED but it succeeded: {'ManagedLoginVersion': 1, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}} — expected a 400 InvalidParameterException.
Cause 4: AssertionError: the call succeeded; real AWS rejects it, so there is no error code — expected InvalidParameterException for RecoveryMechanisms [admin_only + verified_email].
Cause 5: AssertionError: the call succeeded; real AWS rejects it, so there is no error code — expected InvalidParameterException for MfaConfiguration='ON' with no SmsConfiguration/phone auto-verification.
Cause 6: AssertionError: an unset EmailConfiguration is DEFAULTED to Cognito's own sender -- the reason the email recovery path works while the phone one cannot; got None. Expected {'EmailSendingAccount': 'COGNITO_DEFAULT'}.
Cause 7: AssertionError: expected InvalidPasswordException, got InvalidParameterException.
Cause 8: AssertionError: real AWS rejects this call, but it SUCCEEDED (at AdminCreateUser on a pool whose LambdaConfig.PreSignUp returns unparseable output); expected ('InvalidLambdaResponseException', 'Unrecognizable lambda output') followed by UserNotFoundException for the username.

## Steps to Reproduce
Cause 1: CreateUserPool, then CreateUserPoolClient with AllowedOAuthFlows ['code','implicit'] and CallbackURLs ['https://host/path#login'] -> expect a 400, observe 200.
Cause 2: CreateUserPoolClient with AllowedOAuthFlows ['code','implicit'], AllowedOAuthFlowsUserPoolClient=True and CallbackURLs=[] -> expect a 400, observe 200.
Cause 3: CreateUserPoolDomain with a reserved prefix (e.g. one containing 'aws'/'cognito') -> expect InvalidParameterException, observe 200 {'ManagedLoginVersion': 1}.
Cause 4: CreateUserPool with AccountRecoverySetting={'RecoveryMechanisms':[{'Priority':1,'Name':'admin_only'},{'Priority':2,'Name':'verified_email'}]} -> expect InvalidParameterException, observe success.
Cause 5: CreateUserPool with MfaConfiguration='ON' and no SmsConfiguration and no 'phone_number' in AutoVerifiedAttributes -> expect InvalidParameterException, observe success.
Cause 6: CreateUserPool with no EmailConfiguration, then DescribeUserPool -> expect EmailConfiguration {'EmailSendingAccount':'COGNITO_DEFAULT'}, observe the member absent.
Cause 7: CreateUserPool with Policies.PasswordPolicy.MinimumLength=8 (via CloudFormation or the API), then SignUp with a shorter password -> expect InvalidPasswordException, observe InvalidParameterException.
Cause 8: CreateUserPool, UpdateUserPool with LambdaConfig={'PreSignUp': <fn that returns a non-Cognito payload>}, then AdminCreateUser -> expect InvalidLambdaResponseException and no user, observe 200 and the user created.
