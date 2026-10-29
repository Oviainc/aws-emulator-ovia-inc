Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-account-recovery-and-attribute-verification
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-account-recovery-and-attribute-verification cognito-idp-admin-auth-challenge-flow cognito-idp-admin-managed-device-and-mfa-settings cognito-idp-admin-password-reset-and-user-settings cognito-idp-admin-user-auth-workflow cognito-idp-admin-user-group-and-account-linking-lifecycle cognito-idp-admin-user-signup-confirm-and-mfa cognito-idp-app-client-error-paths cognito-idp-auth-challenge-and-token-refresh cognito-idp-auth-event-feedback-requires-advanced-security-addon cognito-idp-client-credentials-m2m-flow cognito-idp-create-import-job-malformed-role-arn-error cognito-idp-device-and-pool-diagnostics cognito-idp-group-lifecycle cognito-idp-identity-provider-management cognito-idp-list-users-in-group cognito-idp-list-users-in-pool cognito-idp-m2m-client-credentials-token cognito-idp-managed-login-branding-crud cognito-idp-managed-login-branding-lifecycle cognito-idp-mfa-and-log-delivery-configuration cognito-idp-mfa-setup-and-preference-workflow cognito-idp-pool-schema-and-ui-customization cognito-idp-provisioned-limit-and-signing-certificate cognito-idp-resource-server-and-scoped-client cognito-idp-resource-tagging-and-custom-attributes cognito-idp-risk-configuration-without-addon cognito-idp-self-service-account-deletion-lifecycle cognito-idp-self-service-mfa-preference-and-auth-factors cognito-idp-self-service-password-challenge-and-token-lifecycle cognito-idp-self-service-signup-and-token-session cognito-idp-self-service-signup-confirm-and-forgot-password cognito-idp-terms-document-lifecycle cognito-idp-terms-documents-error-paths cognito-idp-terms-documents-lifecycle cognito-idp-terms-lifecycle-create-describe-update-delete cognito-idp-terms-not-found-errors cognito-idp-terms-two-types-per-client cognito-idp-ui-customization-lifecycle cognito-idp-user-device-management-errors cognito-idp-user-import-job-create-describe-list cognito-idp-user-import-job-lifecycle cognito-idp-user-import-readiness-and-lookup cognito-idp-user-mfa-settings-and-refresh-token-exchange cognito-idp-user-pool-and-client-lifecycle cognito-idp-user-pool-client-secret-management cognito-idp-user-pool-core-lifecycle cognito-idp-user-pool-domain-delete-notfound-error cognito-idp-user-pool-group-membership-lifecycle cognito-idp-user-pool-lifecycle-and-provisioned-limits cognito-idp-user-pool-list-lifecycle cognito-idp-user-pool-replica-errors cognito-idp-user-pool-replica-lifecycle cognito-idp-user-session-mfa-and-refresh cognito-idp-user-settings-and-mfa cognito-idp-webauthn-credential-management cognito-idp-admin-user-core-lifecycle cognito-idp-attribute-management-edge-cases-and-invalid-input cognito-idp-risk-configuration-invalid-parameter-error cognito-idp-risk-configuration-lifecycle cognito-idp-self-service-profile-view-update-and-attribute-verification cognito-idp-user-pool-update-reflects-auto-verified-attributes
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|CreateUserPool|2xx->2xx|$.UserPool.AccountRecoverySetting
Issue-Key: cognito-idp|CreateUserPool|2xx->2xx|$.UserPool.EmailConfiguration
Issue-Key: cognito-idp|CreateUserPool|2xx->2xx|$.UserPool.IssuerConfiguration
Issue-Key: cognito-idp|CreateUserPool|2xx->2xx|$.UserPool.KeyConfiguration
Issue-Key: cognito-idp|CreateUserPool|2xx->2xx|$.UserPool.LambdaConfig
Issue-Key: cognito-idp|CreateUserPool|2xx->2xx|$.UserPool.SchemaAttributes[*].NumberAttributeConstraints.MinValue
Issue-Key: cognito-idp|CreateUserPool|2xx->2xx|$.UserPool.SchemaAttributes[*].StringAttributeConstraints.MaxLength
Issue-Key: cognito-idp|CreateUserPool|2xx->2xx|$.UserPool.SchemaAttributes[*].StringAttributeConstraints.MinLength
Issue-Key: cognito-idp|CreateUserPool|2xx->2xx|$.UserPool.UserAttributeUpdateSettings
Issue-Key: cognito-idp|CreateUserPool|2xx->2xx|$.UserPool.VerificationMessageTemplate
Issue-Key: cognito-idp|CreateUserPool|2xx->2xx|$.UserPool.SchemaAttributes[*].StringAttributeConstraints
Issue-Key: cognito-idp|CreateUserPool|2xx->2xx|$.UserPool.UserPoolAddOns.AdvancedSecurityAdditionalFlows
Issue-Key: cognito-idp|DescribeUserPool|2xx->2xx|$.UserPool.AccountRecoverySetting
Issue-Key: cognito-idp|DescribeUserPool|2xx->2xx|$.UserPool.EmailConfiguration
Issue-Key: cognito-idp|DescribeUserPool|2xx->2xx|$.UserPool.IssuerConfiguration
Issue-Key: cognito-idp|DescribeUserPool|2xx->2xx|$.UserPool.KeyConfiguration
Issue-Key: cognito-idp|DescribeUserPool|2xx->2xx|$.UserPool.LambdaConfig
Issue-Key: cognito-idp|DescribeUserPool|2xx->2xx|$.UserPool.SchemaAttributes[*]
Issue-Key: cognito-idp|DescribeUserPool|2xx->2xx|$.UserPool.SchemaAttributes[*].NumberAttributeConstraints.MinValue
Issue-Key: cognito-idp|DescribeUserPool|2xx->2xx|$.UserPool.SchemaAttributes[*].StringAttributeConstraints
Issue-Key: cognito-idp|DescribeUserPool|2xx->2xx|$.UserPool.SchemaAttributes[*].StringAttributeConstraints.MaxLength
Issue-Key: cognito-idp|DescribeUserPool|2xx->2xx|$.UserPool.SchemaAttributes[*].StringAttributeConstraints.MinLength
Issue-Key: cognito-idp|DescribeUserPool|2xx->2xx|$.UserPool.UserAttributeUpdateSettings
Issue-Key: cognito-idp|DescribeUserPool|2xx->2xx|$.UserPool.VerificationMessageTemplate
Issue-Key: cognito-idp|DescribeUserPool|2xx->2xx|$.UserPool.UserPoolTags.Environment
Issue-Key: cognito-idp|ListUserPools|2xx->2xx|$.UserPools[Name="sim2real-probe-cognito-idp-pool-61f0536e"].LambdaConfig
Issue-Key: cognito-idp|ListUserPools|2xx->2xx|$.UserPools[Name="sim2real-probe-cognito-idp-pool-61f0536e"].ReplicaRegions
Issue-Key: cognito-idp|ListUserPools|2xx->2xx|$.UserPools[Name="sim2real-probe-cognito-idp-pool-a-a14840ad"].LambdaConfig
Issue-Key: cognito-idp|ListUserPools|2xx->2xx|$.UserPools[Name="sim2real-probe-cognito-idp-pool-a-a14840ad"].ReplicaRegions
Issue-Key: cognito-idp|ListUserPools|2xx->2xx|$.UserPools[Name="sim2real-probe-cognito-idp-pool-b-a14840ad"].LambdaConfig
Issue-Key: cognito-idp|ListUserPools|2xx->2xx|$.UserPools[Name="sim2real-probe-cognito-idp-pool-b-a14840ad"].ReplicaRegions
Issue-Commit: e5bbef2d2760924537f10016f55a30b12c0b7775

# cognito-idp: user pool reads omit server-set default blocks and the default schema diverges

> _Note: this issue was identified against the code base at commit `e5bbef2d2760924537f10016f55a30b12c0b7775`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUserPool

## Description
AWS: CreateUserPool/DescribeUserPool return a UserPoolType in which the server materialises every configuration block it defaults, even when the request omitted it, and in which SchemaAttributes is the service's own default schema. The emulator stores only what the request carried, so the read model is missing those server-set blocks and its default schema differs from the service's.
Emulator: the blocks listed below come back absent (`got None`) and several default schema attributes carry the emulator's generic string constraints instead of the service's.
Why: clients round-trip a Describe result back into UpdateUserPool, or assert on it in IaC drift checks. A missing AccountRecoverySetting/VerificationMessageTemplate/LambdaConfig block reads as 'feature not configured', so a drift check reports a change that does not exist, and a round-trip update silently clears recovery mechanisms. A wrong default schema makes attribute-length validation disagree with AWS.
Fix: CreateUserPool/DescribeUserPool/ListUserPools state and serializer — materialise the service defaults at pool creation and project them on read; seed the default schema from the service's standard attribute set rather than a generic String(0..2048) template.

Cause 1 of 10: AccountRecoverySetting is omitted. AWS returns the default `{"RecoveryMechanisms":[{"Name":"verified_email","Priority":1},{"Name":"verified_phone_number","Priority":2}]}` when the request sets none. Fixture cognito-idp-account-recovery-and-attribute-verification, step setup#1 (57 recorded steps in this cluster).
Cause 2 of 10: EmailConfiguration is omitted. AWS returns `{"EmailSendingAccount":"COGNITO_DEFAULT"}`. Same representative step; also fixture cognito-idp-attribute-management-edge-cases-and-invalid-input, step setup#1.
Cause 3 of 10: IssuerConfiguration `{"Type":"ORIGINAL"}` and KeyConfiguration `{"KeyType":"AWS_OWNED_KEY"}` are omitted. Fixture cognito-idp-admin-user-core-lifecycle, step task#1.
Cause 4 of 10: LambdaConfig `{}` and UserAttributeUpdateSettings `{"AttributesRequireVerificationBeforeUpdate":[]}` are omitted; AWS always emits both, empty. Fixture cognito-idp-self-service-profile-view-update-and-attribute-verification, step setup#1.
Cause 5 of 10: VerificationMessageTemplate `{"DefaultEmailOption":"CONFIRM_WITH_CODE"}` is omitted. Fixture cognito-idp-attribute-management-edge-cases-and-invalid-input, step setup#1.
Cause 6 of 10: UserPoolAddOns.AdvancedSecurityAdditionalFlows is omitted when the pool is created with an add-on; AWS returns `{"CustomAuthMode":"AUDIT"}`. Fixture cognito-idp-risk-configuration-invalid-parameter-error, step setup#1.
Cause 7 of 10: the default schema's per-attribute constraints are wrong — the emulator stamps String(MinLength 0, MaxLength 2048) and omits NumberAttributeConstraints.MinValue on every attribute, where AWS gives standard attributes their own bounds (e.g. MinLength '1' on the first standard attribute, MinLength/MaxLength '10' on the birthdate-class attribute, MinValue '0' on the number attribute) and emits `StringAttributeConstraints: {}` for attributes that have none. Fixture cognito-idp-pool-schema-and-ui-customization, step task#4.
Cause 8 of 10: the `identities` standard attribute is missing from the default schema, so the whole SchemaAttributes list is shifted (position 8 holds `picture` where AWS holds `identities` with `StringAttributeConstraints: {}`). Fixture cognito-idp-user-pool-core-lifecycle, step task#2.
Cause 9 of 10: ListUserPools omits LambdaConfig `{}` and ReplicaRegions from each UserPoolDescriptionType; AWS emits both. Fixtures cognito-idp-user-pool-core-lifecycle step task#5 and cognito-idp-user-pool-list-lifecycle step task#3.
Cause 10 of 10: DescribeUserPool omits UserPoolTags that CreateUserPool accepted, so a tagged pool reads back untagged. Fixture cognito-idp-user-pool-lifecycle-and-provisioned-limits, step task#6.

## Evidence
Expected vs actual (CreateUserPool, cognito-idp-account-recovery-and-attribute-verification#setup#1, status 200 both sides):
  $.UserPool.AccountRecoverySetting: expected {'RecoveryMechanisms':[{'Name':'verified_email','Priority':1},{'Name':'verified_phone_number','Priority':2}]}, got None
  $.UserPool.EmailConfiguration: expected {'EmailSendingAccount':'COGNITO_DEFAULT'}, got None
  $.UserPool.IssuerConfiguration: expected {'Type':'ORIGINAL'}, got None
  $.UserPool.KeyConfiguration: expected {'KeyType':'AWS_OWNED_KEY'}, got None
  $.UserPool.LambdaConfig: expected {}, got None
  $.UserPool.UserAttributeUpdateSettings: expected {'AttributesRequireVerificationBeforeUpdate': []}, got None
  $.UserPool.VerificationMessageTemplate: expected {'DefaultEmailOption':'CONFIRM_WITH_CODE'}, got None
  $.UserPool.SchemaAttributes[0].StringAttributeConstraints.MinLength: expected '1', got '0'
  $.UserPool.SchemaAttributes[13].StringAttributeConstraints.MinLength/MaxLength: expected '10'/'10', got '0'/'2048'
  $.UserPool.SchemaAttributes[19].NumberAttributeConstraints.MinValue: expected '0', got None
DescribeUserPool (cognito-idp-user-pool-core-lifecycle#task#2): $.UserPool.SchemaAttributes[8]: expected the `identities` attribute with StringAttributeConstraints {}, got `picture`.
ListUserPools (cognito-idp-user-pool-core-lifecycle#task#5): LambdaConfig expected {} got None; ReplicaRegions expected ['us-west-2'] got None.
DescribeUserPool (cognito-idp-user-pool-lifecycle-and-provisioned-limits#task#6): $.UserPool.UserPoolTags.Environment expected 'probe', got None.

## Steps to Reproduce
1. Replay cognito-idp-account-recovery-and-attribute-verification step setup#1 (CreateUserPool with no recovery/email/verification configuration) and diff the UserPool against the recording.
2. Replay cognito-idp-pool-schema-and-ui-customization step task#4 and cognito-idp-user-pool-core-lifecycle step task#2 (DescribeUserPool) and diff SchemaAttributes element-by-element.
3. Replay cognito-idp-user-pool-core-lifecycle step task#5 and cognito-idp-user-pool-list-lifecycle step task#3 (ListUserPools).
4. Replay cognito-idp-user-pool-lifecycle-and-provisioned-limits step task#6 and check UserPoolTags.
