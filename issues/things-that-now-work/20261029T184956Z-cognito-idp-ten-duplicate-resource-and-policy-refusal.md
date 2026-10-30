Issue-Source: trace-sweep-divergence
Issue-Trace: cip-cfg-alias-email-duplicate-signup-and-alias-claim
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cip-cfg-alias-email-duplicate-signup-and-alias-claim cip-cfg-case-insensitive-username-resolution cip-cfg-password-policy-rejects-missing-lowercase cip-cfg-password-policy-rejects-missing-numbers cip-cfg-password-policy-rejects-missing-symbols cip-cfg-password-policy-rejects-missing-uppercase cip-cfg-username-attributes-email-rejects-non-email-username cognito-idp-self-service-signup-and-token-session cognito-idp-self-service-signup-confirm-and-forgot-password cip-autherr-flow-not-enabled-admin-user-password cognito-idp-admin-password-reset-and-user-settings cognito-idp-attribute-management-edge-cases-and-invalid-input cognito-idp-group-lifecycle cognito-idp-user-pool-group-membership-lifecycle cognito-idp-identity-provider-management cognito-idp-managed-login-branding-crud cognito-idp-managed-login-branding-error-paths cognito-idp-managed-login-branding-errors cognito-idp-pool-schema-and-ui-customization cognito-idp-user-pool-domain-duplicate-error cognito-idp-user-pool-domain-errors cognito-idp-user-pool-replica-lifecycle
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|SignUp|4xx->4xx|$.message
Issue-Key: cognito-idp|SignUp|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|AdminInitiateAuth|4xx->4xx|$.message
Issue-Key: cognito-idp|AdminInitiateAuth|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|AdminSetUserPassword|4xx->4xx|$.message
Issue-Key: cognito-idp|AdminSetUserPassword|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|DeleteUserAttributes|4xx->4xx|$.message
Issue-Key: cognito-idp|DeleteUserAttributes|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|CreateGroup|4xx->4xx|$.message
Issue-Key: cognito-idp|CreateGroup|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|CreateIdentityProvider|4xx->4xx|$.message
Issue-Key: cognito-idp|CreateIdentityProvider|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|CreateManagedLoginBranding|4xx->4xx|$.message
Issue-Key: cognito-idp|CreateManagedLoginBranding|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|AddCustomAttributes|4xx->4xx|$.message
Issue-Key: cognito-idp|AddCustomAttributes|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|CreateUserPoolDomain|4xx->4xx|$.message
Issue-Key: cognito-idp|CreateUserPoolDomain|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|CreateUserPoolReplica|4xx->4xx|$.message
Issue-Key: cognito-idp|CreateUserPoolReplica|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: e5bbef2d2760924537f10016f55a30b12c0b7775

# cognito-idp: ten duplicate-resource and policy refusal messages are emulator prose

> _Note: this issue was identified against the code base at commit `e5bbef2d2760924537f10016f55a30b12c0b7775`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SignUp

## Description
Ten recorded refusals that raise the right error class with the wrong message. For each cause the AWS message is given with its fixed text and its variable slots named; the emulator substitutes its own sentence, usually by interpolating request values AWS does not interpolate. An error message is part of the contract: clients log it, surface it to end users and in several cases match on it, so emulator-authored prose is a defect.

Cause 1 of 10: SignUp for an existing username. AWS: UsernameExistsException, message `User already exists` — entirely fixed text, with NO username interpolated (AWS deliberately does not echo it). Emulator: `User account already exists for <username>`. Fix: the SignUp duplicate branch. Fixture cip-cfg-alias-email-duplicate-signup-and-alias-claim, step task#5 (9 recorded steps).
Cause 2 of 10: AdminInitiateAuth with an auth flow the client does not allow. AWS: message `Auth flow not enabled for this client` — fixed text, with NO flow name interpolated. Emulator: `<FLOW_NAME> flow not enabled for this client`. Fix: the AdminInitiateAuth flow gate. Fixture cip-autherr-flow-not-enabled-admin-user-password, step task#6.
Cause 3 of 10: AdminSetUserPassword with a password shorter than the policy minimum. AWS: InvalidPasswordException, message `Password does not conform to policy: Password not long enough` — the prefix `Password does not conform to policy: ` is fixed and the suffix is the specific policy clause that failed. Emulator: `Password did not conform with password policy: Password not long enough`. Fix: the password-policy evaluator's message template. Fixture cognito-idp-admin-password-reset-and-user-settings, step task#4.
Cause 4 of 10: DeleteUserAttributes on an immutable attribute. AWS: InvalidParameterException, message `Cannot modify the non-mutable attribute <attributeName>` — fixed text with the attribute name from the request. Emulator: `Attribute <name> cannot be deleted: this user pool's schema declares it immutable`. Fix: the attribute-mutability check. Fixture cognito-idp-attribute-management-edge-cases-and-invalid-input, step task#8.
Cause 5 of 10: CreateGroup for an existing group name. AWS: GroupExistsException, message `A group with the name already exists.` — fixed text, NO group name interpolated. Emulator: `A group named <name> already exists.`. Fix: the CreateGroup duplicate branch. Fixture cognito-idp-group-lifecycle, step task#5 (2 recorded steps).
Cause 6 of 10: CreateIdentityProvider for an existing provider. AWS: DuplicateProviderException, message `<ProviderName> already exists for tenant <userPoolId>.` — fixed text `already exists for tenant`, with the provider name and pool id from the request. Emulator: `Provider <name> already exists in user pool <poolId>.`. Fix: the CreateIdentityProvider duplicate branch. Fixture cognito-idp-identity-provider-management, step task#3.
Cause 7 of 10: CreateManagedLoginBranding when the client already has branding. AWS: ManagedLoginBrandingExistsException, message `A ManagedLoginBranding already exists for client <clientId>` — note no trailing period. Emulator: `Managed login branding already exists for Client <clientId>.`. Fix: the CreateManagedLoginBranding duplicate branch. Fixture cognito-idp-managed-login-branding-crud, step task#9 (4 recorded steps).
Cause 8 of 10: AddCustomAttributes for an attribute name already in the schema. AWS: InvalidParameterException, message `Entity Type: USER, Message: AttributeName: <name>, DataType: <TYPE>, Error: Attribute name is not unique` — a structured template whose slots are the attribute's bare name (without the `custom:` prefix) and its data type in upper case. Emulator: `Invalid attributes given, custom:<name> already exists in the schema of this user pool`. Fix: the AddCustomAttributes duplicate branch. Fixture cognito-idp-pool-schema-and-ui-customization, step task#5.
Cause 9 of 10: CreateUserPoolDomain for a domain already bound elsewhere. AWS: InvalidParameterException, message `Domain already associated with another user pool.` — fixed text, with NO domain or pool id interpolated (AWS does not disclose which pool holds it). Emulator: `Domain <domain> is already associated with user pool <poolId>.`, which leaks the owning pool. Fix: the CreateUserPoolDomain duplicate branch. Fixture cognito-idp-user-pool-domain-duplicate-error, step task#4 (2 recorded steps).
Cause 10 of 10: CreateUserPoolReplica in the pool's own region. AWS: InvalidParameterException, message `Replica must be created in a different region` — fixed text, no interpolation. Emulator: `User pool <poolId> is the primary replica in Region <region>, so it can't be replicated there.`. Fix: the CreateUserPoolReplica same-region branch. Fixture cognito-idp-user-pool-replica-lifecycle, step task#2.

## Evidence
All ten return the correct status/class but a different $.message. Representative diffs:
  SignUp: expected 'User already exists', got 'User account already exists for alias-user-one'.
  AdminInitiateAuth: expected 'Auth flow not enabled for this client', got 'ADMIN_USER_PASSWORD_AUTH flow not enabled for this client'.
  AdminSetUserPassword: expected 'Password does not conform to policy: Password not long enough', got 'Password did not conform with password policy: Password not long enough'.
  DeleteUserAttributes: expected 'Cannot modify the non-mutable attribute sub', got "Attribute sub cannot be deleted: this user pool's schema declares it immutable".
  CreateGroup: expected 'A group with the name already exists.', got 'A group named <name> already exists.'
  CreateIdentityProvider: expected 'Facebook already exists for tenant <poolId>.', got 'Provider Facebook already exists in user pool <poolId>.'
  CreateManagedLoginBranding: expected 'A ManagedLoginBranding already exists for client <clientId>', got 'Managed login branding already exists for Client <clientId>.'
  AddCustomAttributes: expected 'Entity Type: USER, Message: AttributeName: <name>, DataType: STRING, Error: Attribute name is not unique', got 'Invalid attributes given, custom:<name> already exists in the schema of this user pool'.
  CreateUserPoolDomain: expected 'Domain already associated with another user pool.', got 'Domain <domain> is already associated with user pool <poolId>.'
  CreateUserPoolReplica: expected 'Replica must be created in a different region', got "User pool <poolId> is the primary replica in Region <region>, so it can't be replicated there."

## Steps to Reproduce
1. Replay cip-cfg-alias-email-duplicate-signup-and-alias-claim step task#5 and read $.message.
2. Replay each remaining fixture#step named in the causes and diff $.message character-for-character against the recording.
