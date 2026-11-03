// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice service.
//!
//! One [`UserPool`] per created pool, each owning its app clients and its users,
//! beside the auth sessions and the tokens the sign-in operations issue. Every
//! operation computes its response from this state — a create stores a resource
//! and a later read renders the STORED resource, never the request that made it.

/// The user-pool Lambda-trigger dispatch: a pool's [`LambdaConfigType`] member becomes an
/// invocation of the function it names, with the event for the stage the invoking
/// operation reached.
pub mod lambda_triggers;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::ring::signature::KeyPair;
use ::sha2::Digest;

use crate::ops::types::{
    AccountRecoverySettingType, AccountTakeoverRiskConfigurationType, AcrLevelConfigType,
    AdminCreateUserConfigType,
    AnalyticsConfigurationType, AttributeType, AuthenticationResultType,
    ClientAuthenticationResultType, ClientSecretDescriptorType,
    CodeDeliveryDetailsType, CompromisedCredentialsRiskConfigurationType, CustomDomainConfigType,
    DeviceConfigurationType, DeviceSecretVerifierConfigType, DeviceType, DomainDescriptionType,
    EmailConfigurationType, EmailMfaConfigType, GroupType, IdentityProviderType,
    IssuerConfigurationType,
    KeyConfigurationType, LambdaConfigType, LimitDefinitionType, LimitType,
    LogConfigurationType, LogDeliveryConfigurationType,
    MFAOptionType, NewDeviceMetadataType,
    NumberAttributeConstraintsType, PasswordPolicyType, ProviderUserIdentifierType,
    RecoveryOptionType,
    RefreshTokenRotationType,
    ResourceServerScopeType, ResourceServerType, RiskConfigurationType,
    RiskExceptionConfigurationType, RoutingType,
    SchemaAttributeType, SignInPolicyType, SmsConfigurationType, SmsMfaConfigType,
    SoftwareTokenMfaConfigType, StringAttributeConstraintsType,
    TermsDescriptionType, TermsType, TokenValidityUnitsType, UserAttributeUpdateSettingsType,
    UserImportJobType,
    UserPoolAddOnsType,
    UserPoolClientType, UserPoolDescriptionType, UserPoolPolicyType, UserPoolReplicaType,
    UserPoolType, UserType,
    UsernameConfigurationType, VerificationMessageTemplateType, WebAuthnConfigurationType,
    WebAuthnCredentialDescription,
};

/// The region a request that carries no SigV4 credential scope falls back to, so
/// a minted pool id and ARN always name one region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The service's SigV4 / endpoint / ARN namespace (OPERATIONS.md "Wire protocol").
pub const SIGNING_NAME: &str = "cognito-idp";

/// The resource type a user pool's ARN names: a pool is
/// `arn:<partition>:cognito-idp:<region>:<account>:userpool/<UserPoolId>`, the ARN
/// form [`UserPool::arn`] carries and the one a tagging operation's `ResourceArn`
/// addresses a pool by.
pub const USER_POOL_RESOURCE_TYPE: &str = "userpool";

/// The most tags one user pool holds: "A user pool can have as many as 50 tags"
/// (`TagResource` in this service's model).
pub const MAX_TAGS_PER_USER_POOL: usize = 50;

/// The longest a `UserPoolId` is: `UserPoolIdType` is modelled with a maximum
/// length of 55 (`smithy_query(query="shape",shape="UserPoolIdType")`).
const USER_POOL_ID_MAX_LENGTH: usize = 55;

/// Amazon Cognito's default password policy for a new user pool: at least 8
/// characters, with an uppercase letter, a lowercase letter, a number and a
/// symbol required (Amazon Cognito Developer Guide, "Password policies"). The
/// model's `PasswordPolicyType.MinimumLength` range starts at 6, so 8 is a
/// default, not a bound.
const DEFAULT_PASSWORD_MIN_LENGTH: i32 = 8;

/// `PasswordPolicyType.TemporaryPasswordValidityDays`: "Defaults to `7`. If you
/// submit a value of `0`, Amazon Cognito treats it as a null value and sets
/// `TemporaryPasswordValidityDays` to its default value." (this service's model).
const DEFAULT_TEMPORARY_PASSWORD_VALIDITY_DAYS: i32 = 7;

/// `AdminCreateUserConfigType.UnusedAccountValidityDays`: "The default value for
/// this parameter is 7." (this service's model).
const DEFAULT_UNUSED_ACCOUNT_VALIDITY_DAYS: i32 = 7;

/// `CreateUserPoolClient.AccessTokenValidity`: "If you don't specify otherwise in
/// the configuration of your app client, your access tokens are valid for one
/// hour." (this service's model). `IdTokenValidity` is documented the same way.
const DEFAULT_ACCESS_TOKEN_TTL: Duration = Duration::from_secs(60 * 60);

/// `CreateUserPoolClient.RefreshTokenValidity`: "If you don't specify otherwise in
/// the configuration of your app client, your refresh tokens are valid for 30
/// days", and a submitted `0` is overridden "with the default value of 30 days"
/// (this service's model).
const DEFAULT_REFRESH_TOKEN_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// `CreateUserPoolClient.AuthSessionValidity` is "the duration, in minutes, of
/// that session token" and its modelled range is 3..15, so an app client that
/// sets no value gets the range's minimum.
const DEFAULT_AUTH_SESSION_VALIDITY_MINUTES: u64 = 3;

/// `CreateUserPoolClient.ExplicitAuthFlows`: "If you don't specify a value for
/// `ExplicitAuthFlows`, your app client supports `ALLOW_REFRESH_TOKEN_AUTH`,
/// `ALLOW_USER_SRP_AUTH`, and `ALLOW_CUSTOM_AUTH`." (this service's model).
pub const DEFAULT_EXPLICIT_AUTH_FLOWS: [&str; 3] = [
    "ALLOW_REFRESH_TOKEN_AUTH",
    "ALLOW_USER_SRP_AUTH",
    "ALLOW_CUSTOM_AUTH",
];

/// The `ExplicitAuthFlows` values that enable `AuthFlow=ADMIN_USER_PASSWORD_AUTH`:
/// the current one and the legacy one it "replaces"
/// (`CreateUserPoolClient.ExplicitAuthFlows` in this service's model).
pub const ADMIN_PASSWORD_AUTH_FLOWS: [&str; 2] =
    ["ALLOW_ADMIN_USER_PASSWORD_AUTH", "ADMIN_NO_SRP_AUTH"];

/// The `ExplicitAuthFlows` values that enable `AuthFlow=USER_PASSWORD_AUTH`:
/// "`ALLOW_USER_PASSWORD_AUTH`: Enable user password-based authentication", beside
/// the legacy `USER_PASSWORD_AUTH` the same member documents
/// (`CreateUserPoolClient.ExplicitAuthFlows` in this service's model).
pub const USER_PASSWORD_AUTH_FLOWS: [&str; 2] =
    ["ALLOW_USER_PASSWORD_AUTH", "USER_PASSWORD_AUTH"];

/// The `ExplicitAuthFlows` value that enables `AuthFlow=REFRESH_TOKEN_AUTH` and its
/// `REFRESH_TOKEN` alias: "`ALLOW_REFRESH_TOKEN_AUTH`: Enable authflow to refresh
/// tokens" (`CreateUserPoolClient.ExplicitAuthFlows` in this service's model), which
/// is also one of the three a client that sets no `ExplicitAuthFlows` gets.
pub const REFRESH_TOKEN_AUTH_FLOWS: [&str; 1] = ["ALLOW_REFRESH_TOKEN_AUTH"];

/// The `ExplicitAuthFlows` value that enables `AuthFlow=USER_AUTH`:
/// "`ALLOW_USER_AUTH`: Enable selection-based sign-in with `USER_AUTH`"
/// (`CreateUserPoolClient.ExplicitAuthFlows` in this service's model).
pub const USER_AUTH_FLOWS: [&str; 1] = ["ALLOW_USER_AUTH"];

/// The `ExplicitAuthFlows` value that configures an app client for
/// machine-to-machine authorization: "you must configure the app client with a
/// client secret and enable the `ALLOW_CLIENT_TOKEN_AUTH` authentication flow"
/// (`GetClientToken` in this service's model).
pub const CLIENT_TOKEN_AUTH_FLOW: &str = "ALLOW_CLIENT_TOKEN_AUTH";

/// The message an app-client configuration owes when its `ExplicitAuthFlows`
/// breaks the exclusivity rule [`CLIENT_TOKEN_AUTH_FLOW`] carries: the rule as
/// `GetClientToken` states it in this service's model, word for word. It names no
/// value from the request, so every such refusal carries this one sentence.
pub const EXCLUSIVE_AUTH_FLOW_MESSAGE: &str =
    "The ALLOW_CLIENT_TOKEN_AUTH flow is mutually exclusive with user authentication \
     flows. It must be the only authentication flow that you configure for the app \
     client.";

/// [`EXCLUSIVE_AUTH_FLOW_MESSAGE`] when `flows` breaks that rule, and `None` when
/// it configures an app client the service accepts.
///
/// "The `ALLOW_CLIENT_TOKEN_AUTH` flow is mutually exclusive with user
/// authentication flows. It must be the only authentication flow that you
/// configure for the app client" (`GetClientToken` in this service's model). So a
/// list naming it beside ANOTHER flow describes an app client that cannot exist,
/// while a list naming it alone — or not at all — is a configuration the service
/// accepts. A list that merely repeats it names no other flow, so it is accepted:
/// the rule is about which flows are configured, not how many times each is sent.
/// Both the create and the update path ask this one function, so the two cannot
/// disagree about what an app client may hold.
pub fn exclusive_auth_flow_violation(flows: &[String]) -> Option<&'static str> {
    let names_exclusive_flow = flows.iter().any(|flow| flow == CLIENT_TOKEN_AUTH_FLOW);
    let names_another_flow = flows.iter().any(|flow| flow != CLIENT_TOKEN_AUTH_FLOW);
    (names_exclusive_flow && names_another_flow).then_some(EXCLUSIVE_AUTH_FLOW_MESSAGE)
}

/// The message the ADMINISTRATIVE sign-in refuses with when the app client's
/// stored `ExplicitAuthFlows` do not enable the requested flow.
///
/// `ExplicitAuthFlows` is "the authentication flows that you want your user pool
/// client to support" (`CreateUserPoolClient.ExplicitAuthFlows` in this
/// service's model), so a request whose `AuthFlow` the stored list does not
/// enable asks for a flow this app client is not configured for. The
/// administrative API reports that in ONE fixed sentence: it interpolates
/// neither the flow asked for nor the app client it was asked of, so the refusal
/// is the same for every flow and every client id. The client-side sign-in and
/// the machine-to-machine grant word the same condition with the flow the
/// request asked for ([`auth_flow_not_enabled_message`]) — Amazon Cognito does
/// not word this refusal identically in every API, so each path carries the
/// sentence ITS OWN API answers.
pub const ADMIN_AUTH_FLOW_NOT_ENABLED_MESSAGE: &str = "Auth flow not enabled for this client";

/// The message the CLIENT-SIDE sign-in and the machine-to-machine grant refuse
/// with when the app client's stored `ExplicitAuthFlows` do not enable the
/// requested flow.
///
/// One sentence whose only variable part is the flow the request asked for —
/// `auth_flow`, the `AuthFlowType` member as SUBMITTED, so an alias is echoed as
/// the caller wrote it — and which says nothing else about the app client: the
/// sentence is the same for every client id, so this refusal, which an
/// unauthenticated caller can provoke, carries no identifier of the app client
/// it was sent to. The administrative sign-in's own wording is
/// [`ADMIN_AUTH_FLOW_NOT_ENABLED_MESSAGE`].
pub fn auth_flow_not_enabled_message(auth_flow: &str) -> String {
    format!("{auth_flow} flow not enabled for this client")
}

/// The message an operation refuses with when the app client the request names
/// does not have the FEATURE that operation needs switched on in its stored
/// configuration.
///
/// `UnsupportedOperationException` is "thrown when you attempt to perform an
/// operation that isn't enabled for the user pool client" (this service's model),
/// and Amazon Cognito reports it in ONE fixed sentence with no variable part at
/// all: it interpolates neither the feature asked for, nor the app client, nor the
/// pool, so the refusal reads the same for every client id and tells a caller
/// nothing about the client it addressed. A request such a gate refuses changes
/// nothing, so the sentence is also the whole of what the caller learns.
pub const CLIENT_FEATURE_NOT_ENABLED_MESSAGE: &str = "This feature is not enabled for this client";

/// The `ChallengeParameters` of a successful authentication response that leaves
/// NO challenge outstanding: the empty map.
///
/// `InitiateAuth`, `AdminInitiateAuth`, `RespondToAuthChallenge` and
/// `AdminRespondToAuthChallenge` all model `ChallengeParameters` as a
/// `ChallengeParametersType` map (this service's model), and a successful
/// response carries it whether or not a challenge is outstanding: the parameters
/// of the challenge the caller must answer, or no parameters at all once
/// authentication is complete. A caller therefore reads the map unconditionally
/// and finds it empty rather than absent, so every one of those operations asks
/// this one function for the completed case and none of them can drop the member.
#[must_use]
pub fn completed_challenge_parameters() -> HashMap<String, String> {
    HashMap::new()
}

/// The `ChallengeParameters` of an `MFA_SETUP` challenge for `username`: the user
/// it is for, and the MFA methods that user can set up to satisfy the pool's MFA
/// requirement.
///
/// `MFA_SETUP` is the challenge for a user that "must set up" a second factor
/// before a sign-in completes, and its parameters report the methods available to
/// set up under `MFAS_CAN_SETUP` as a JSON array of the `UserMFASettingList`
/// values they are reported under — "the possible values in this list are
/// `SMS_MFA`, `EMAIL_OTP`, and `SOFTWARE_TOKEN_MFA`" (this service's model), which
/// is the order they are listed in — beside the user the challenge belongs to,
/// named exactly as every other challenge of these operations names it
/// ([`new_password_required_challenge_parameters`]).
///
/// Both `InitiateAuth` and `AdminInitiateAuth` start this challenge, and both read
/// its parameters here, so the two cannot report it differently.
#[must_use]
pub fn mfa_setup_challenge_parameters(
    username: &str,
    settings_can_setup: &[String],
) -> HashMap<String, String> {
    let mut parameters = HashMap::new();
    parameters.insert(USER_ID_FOR_SRP.to_string(), username.to_string());
    parameters.insert(
        MFAS_CAN_SETUP.to_string(),
        ::serde_json::Value::Array(
            settings_can_setup
                .iter()
                .map(|setting| ::serde_json::Value::String(setting.clone()))
                .collect(),
        )
        .to_string(),
    );
    parameters
}

/// the user it is for, the attributes the challenge response must supply, and the
/// values the user already has.
///
/// `ChallengeResponses` documents the response this challenge takes as
/// `{"NEW_PASSWORD": "[new_password]", "USERNAME": "[username]"}` plus
/// `"userAttributes.[attribute_name]": "[attribute_value]"` (this service's model),
/// so the parameters report the names under that same `userAttributes.` prefix and
/// the current values as a JSON object.
///
/// `userAttributes` reports the user's OWN attributes: `sub` is minted by the pool
/// for every user, is immutable, and is not an attribute any challenge response can
/// carry — the same reason it is never among the required attributes a challenge asks
/// for ([`UserPool::unset_required_attributes`]) — so it is not reported among them,
/// and a user with no other attribute is reported with an empty object.
///
/// Both `InitiateAuth` and `AdminInitiateAuth` start this challenge, and both read
/// its parameters here, so the two cannot report it differently.
#[must_use]
pub fn new_password_required_challenge_parameters(
    username: &str,
    required_attributes: &[String],
    user_attributes: &BTreeMap<String, String>,
) -> HashMap<String, String> {
    let mut parameters = HashMap::new();
    parameters.insert(USER_ID_FOR_SRP.to_string(), username.to_string());
    parameters.insert(
        REQUIRED_ATTRIBUTES.to_string(),
        ::serde_json::Value::Array(
            required_attributes
                .iter()
                .map(|name| {
                    ::serde_json::Value::String(format!("{USER_ATTRIBUTES_PREFIX}{name}"))
                })
                .collect(),
        )
        .to_string(),
    );
    parameters.insert(
        USER_ATTRIBUTES.to_string(),
        ::serde_json::Value::Object(
            user_attributes
                .iter()
                .filter(|(name, _)| name.as_str() != SUB_ATTRIBUTE)
                .map(|(name, value)| (name.clone(), ::serde_json::Value::String(value.clone())))
                .collect(),
        )
        .to_string(),
    );
    parameters
}

/// The `ChallengeParameters` entry naming the user a challenge is outstanding for.
const USER_ID_FOR_SRP: &str = "USER_ID_FOR_SRP";

/// The `ChallengeParameters` entry listing the attributes a challenge response must
/// supply.
const REQUIRED_ATTRIBUTES: &str = "requiredAttributes";

/// The `ChallengeParameters` entry carrying the values the user already has.
const USER_ATTRIBUTES: &str = "userAttributes";

/// The `ChallengeParameters` entry of an `MFA_SETUP` challenge that lists the MFA
/// methods the user can set up to satisfy the pool's MFA requirement.
const MFAS_CAN_SETUP: &str = "MFAS_CAN_SETUP";

/// The `ChallengeResponses` key prefix an attribute value is submitted under:
/// `"userAttributes.[attribute_name]": "[attribute_value]"` (this service's model),
/// which is how `requiredAttributes` names the attributes it asks for — and how
/// `AdminRespondToAuthChallenge` reads them back off a response, so the asking and
/// the answering side cannot disagree.
pub const USER_ATTRIBUTES_PREFIX: &str = "userAttributes.";

/// The attribute every user pool mints for its users and no caller sets: "the user's
/// unique identifier" the pool assigns (`sub` in this service's standard schema).
pub const SUB_ATTRIBUTE: &str = "sub";

/// The scope an access token has to carry to authorize the user self-service
/// operations: each one is authorized "with a signed-in user's access token. It
/// must include the scope `aws.cognito.signin.user.admin`" (`GetUser`,
/// `UpdateUserAttributes` and the rest of the self-service surface in this
/// service's model), and `CreateUserPoolClient.AllowedOAuthScopes` names it as
/// "the `aws.cognito.signin.user.admin` scope" that "authorizes user
/// self-service operations".
pub const USER_ADMIN_SCOPE: &str = "aws.cognito.signin.user.admin";

/// The message a self-service operation owes a live access token whose scopes do
/// not authorize it ([`TokenRejection::MissingRequiredScopes`]). It names no
/// value from the request, so every such refusal carries this one sentence, and
/// every operation that checks a token's scopes reads it here so the two cannot
/// disagree.
pub const MISSING_REQUIRED_SCOPES_MESSAGE: &str = "Access Token does not have required scopes";

/// The message a self-service operation owes an access token this emulator issued
/// and has since REVOKED ([`TokenRejection::Revoked`]).
///
/// A sign-out "invalidates the identity, access, and refresh tokens that Amazon
/// Cognito issued to a user", after which "Amazon Cognito returns an `Access Token
/// has been revoked` error when your app attempts to authorize a user pools API
/// request with a revoked access token" (`GlobalSignOut` in this service's model).
/// It names no value from the request, so every such refusal carries this one
/// sentence, and it is told apart from the refusal of a value that resolves to no
/// access token at all: the caller holds a token this service did issue and has
/// since invalidated.
pub const ACCESS_TOKEN_REVOKED_MESSAGE: &str = "Access Token has been revoked";

/// The message a challenge response owes a `Session` that does not belong to the request
/// it arrived on: one started for another pool or app client, one naming another user than
/// the response does, or one whose challenge the stored user no longer owes.
///
/// It reports that the session resolves no challenge THIS request can answer and names
/// nothing of the request, so the refusal is the same whichever of those it was — and it
/// is told apart from the replay of a spent session
/// ([`SESSION_ALREADY_USED_MESSAGE`]), from a closed validity window
/// ([`SESSION_EXPIRED_MESSAGE`]) and from a value this emulator never handed out
/// ([`INVALID_AUTH_STATE_MESSAGE`]).
pub const SESSION_NOT_FOR_THIS_REQUEST_MESSAGE: &str = "Invalid session for the user.";

/// The message a challenge response owes a `Session` this emulator handed out
/// and an earlier response ALREADY answered.
///
/// A `Session` is "the session identifier that maintains the state of
/// authentication requests and challenge responses" (`RespondToAuthChallenge` in
/// this service's model): answering its challenge is what spends it, so the
/// state it maintained is gone and the same session cannot carry a second
/// sign-in. The refusal says exactly that, which is a different answer from the
/// one a value this service never handed out gets
/// ([`INVALID_AUTH_STATE_MESSAGE`]) and from the one a session whose validity
/// window closed gets ([`SESSION_EXPIRED_MESSAGE`]). It names no value from the
/// request, so every such refusal carries this one sentence.
pub const SESSION_ALREADY_USED_MESSAGE: &str =
    "Invalid session for the user, session can only be used once.";

/// The message a challenge response owes a `Session` whose validity window has
/// closed. `CreateUserPoolClient.AuthSessionValidity` is "the duration, in
/// minutes, of that session token" (this service's model), so a response past it
/// resumes a challenge that is no longer outstanding.
pub const SESSION_EXPIRED_MESSAGE: &str = "Invalid session for the user, session is expired.";

/// The message a challenge response owes a `Session` that resolves to no stored
/// challenge of this emulator at all.
///
/// `(Admin)RespondToAuthChallenge` declares `CodeMismatchException` — "thrown if
/// the provided code doesn't match what the server was expecting"
/// (`smithy_query(query="operation",operation="AdminRespondToAuthChallenge")`) —
/// and a response carries the state of its sign-in in the `Session` beside the
/// code, so a value that names no outstanding challenge is a mismatch of that
/// state rather than a statement about any user. The sentence names neither the
/// session nor the user, so every such refusal carries this one sentence.
pub const INVALID_AUTH_STATE_MESSAGE: &str = "Invalid code or auth state for the user.";

/// `UserPoolTierType` defaults to `ESSENTIALS` (`CreateUserPool.UserPoolTier`).
const DEFAULT_USER_POOL_TIER: &str = ESSENTIALS_USER_POOL_TIER;

/// The feature plan threat-protection (`userAuthEvents`) activity logs need:
/// `LogConfigurationType.EventSource` is "info-level logs about threat-protection
/// user activity in user pools with the Plus feature plan" (this service's
/// model); one of the modelled
/// [`crate::ops::types::USER_POOL_TIER_TYPE_VALUES`].
pub const PLUS_USER_POOL_TIER: &str = "PLUS";

/// `FeatureType`'s value for a setting that is switched on, which
/// `RefreshTokenRotationType.Feature` carries for an app client that rotates its
/// refresh tokens; its only other value is the one for a setting that is not
/// (`crate::ops::types::FEATURE_TYPE_VALUES`).
const FEATURE_ENABLED: &str = "ENABLED";

/// `UserPoolMfaType` is `OFF` for a pool that configures no MFA
/// (`MfaConfiguration`: "Sets multi-factor authentication (MFA) to be on, off, or
/// optional" in this service's model). It is also the mode in which no MFA factor
/// has to be deliverable, which a handler that configures factors asks about.
pub const MFA_OFF: &str = "OFF";

/// `UserPoolMfaType`'s value for a pool in which "MFA tokens are required for all
/// user sign-ins" (`MfaConfiguration` in this service's model), which is what
/// makes a second factor a precondition of completing a sign-in
/// ([`UserPool::requires_mfa`]).
pub const MFA_REQUIRED: &str = "ON";

/// Deletion protection is opt-in: "When active, `DeletionProtection` prevents
/// accidental deletion of your user pool" (this service's model), so a pool that
/// asks for none can be deleted.
const DEFAULT_DELETION_PROTECTION: &str = "INACTIVE";

/// `PreventUserExistenceErrors` "Defaults to `LEGACY`"
/// (`CreateUserPoolClient.PreventUserExistenceErrors` in this service's model),
/// which is the mode that answers `UserNotFoundException` for an absent user.
const DEFAULT_PREVENT_USER_EXISTENCE_ERRORS: &str = "LEGACY";

/// The message a `UserNotFoundException` carries for an operation that resolves
/// the user against a pool it names, or against the access token it is given.
///
/// `UserNotFoundException` is "the message returned when a user isn't found"
/// (this service's model) and the answer is the same sentence whatever was asked
/// for: it names neither the username nor the pool, so the error reports that
/// the lookup found nothing without telling the caller which identity it looked
/// for. A message built from the requested username would answer a different
/// sentence per username, which is what makes the refusal an existence oracle.
///
/// The public sign-up and password-recovery operations resolve the user from a
/// different pair and answer their own sentence —
/// [`USERNAME_CLIENT_NOT_FOUND_MESSAGE`].
pub const USER_NOT_FOUND_MESSAGE: &str = "User does not exist.";

/// The message a `UserNotFoundException` carries for an operation that resolves
/// the user from the `Username` and the `ClientId` of the request TOGETHER.
///
/// The public sign-up and password-recovery operations — the ones whose input
/// models a required `Username` beside a required `ClientId` and no
/// `UserPoolId`, so the app client is what names the pool to look in — report an
/// unresolved user as the combination of the two not being found, rather than as
/// a statement about the username alone. Like [`USER_NOT_FOUND_MESSAGE`] it is
/// one fixed sentence that quotes neither the username nor the client id.
pub const USERNAME_CLIENT_NOT_FOUND_MESSAGE: &str = "Username/client id combination not found.";

/// The message a device operation's `ResourceNotFoundException` carries for a
/// `DeviceKey` the signed-in user holds no confirmed device for. Like
/// [`USER_NOT_FOUND_MESSAGE`] it names nothing the request asked for, so the
/// refusal is the same sentence for every key.
pub const DEVICE_NOT_FOUND_MESSAGE: &str = "Device does not exist.";

/// The message `ConfirmDevice`'s `InvalidParameterException` carries for a
/// `DeviceKey` the signed-in user holds no unconfirmed device for. Like
/// [`DEVICE_NOT_FOUND_MESSAGE`] it names nothing the request asked for, so a key
/// another user or pool holds is reported exactly as one that was never minted.
pub const INVALID_DEVICE_KEY_MESSAGE: &str = "Invalid device key given.";

/// The message the refusal of a sign-in by a user Amazon Cognito is waiting on a
/// new password from carries ([`UserStatus::ResetRequired`]).
///
/// One fixed sentence, naming no username: every operation that reports that
/// state — the sign-in operations, the refresh-token exchange and the
/// self-service operations a signed-in user's access token authorizes — reports
/// it the same way.
pub const PASSWORD_RESET_REQUIRED_MESSAGE: &str = "Password reset required for the user";

/// The message the refusal of a password reset for a user whose registration is
/// not complete carries: the account has no password for a reset to apply to,
/// so the state the user is in is what the refusal reports, naming no username.
pub const PASSWORD_NOT_RESETTABLE_MESSAGE: &str =
    "User password cannot be reset in the current state.";

/// The message a passkey operation's `ResourceNotFoundException` carries for a
/// `CredentialId` the signed-in user holds no registered passkey for. Like
/// [`DEVICE_NOT_FOUND_MESSAGE`] it names nothing the request asked for, so the
/// refusal is the same sentence for every credential id — and so a credential
/// REGISTERED TO ANOTHER user is reported exactly as one that was never
/// registered at all, which is what keeps the refusal from telling a caller
/// whether somebody else holds that credential.
pub const WEB_AUTHN_CREDENTIAL_NOT_FOUND_MESSAGE: &str = "The credential does not exist";

/// The message a `WebAuthnNotEnabledException` carries on a pool that does not
/// admit the passkey factor. One fixed sentence, naming neither the pool nor the
/// operation that asked: every operation that gates on the feature — the
/// registration operations — reports it the same way.
pub const WEB_AUTHN_NOT_ENABLED_MESSAGE: &str = "WebAuthn not enabled for this pool.";

/// The message a device operation's `InvalidUserPoolConfigurationException`
/// carries on a pool that stores no `DeviceConfiguration`: "When you provide a
/// value for any property of `DeviceConfiguration`, you activate the device
/// remembering for the user pool" (this service's model), so a pool that
/// provided none remembers no device and has no device surface to serve.
pub const DEVICE_REMEMBERING_NOT_CONFIGURED_MESSAGE: &str =
    "Device tracking not currently enabled for this pool.";

/// The message a `ResourceNotFoundException` carries when the group an operation
/// names is not one the named pool holds.
///
/// Amazon Cognito answers the same sentence for every group name: the group
/// lookup reports that nothing was found and names no group, so the refusal
/// cannot be read as a statement about one particular group name. Every
/// operation that resolves a `GroupName` against a pool reports it the same way,
/// exactly as [`USER_NOT_FOUND_MESSAGE`] is reported for a username.
pub const GROUP_NOT_FOUND_MESSAGE: &str = "Group not found.";

/// The message a `ResourceNotFoundException` carries when the identity provider
/// an operation names is not one the named pool holds.
///
/// Amazon Cognito reports an unresolved user pool IdP by naming what the request
/// asked for — `asked_for` is the `ProviderName` an operation looked the IdP up
/// by, or the `IdpIdentifier` when it looked it up by identifier — beside the
/// pool it was looked up in, which the sentence calls the tenant. Every
/// operation that resolves an IdP against a pool reports it the same way, so a
/// read and a delete of the same absent provider answer the same sentence.
pub fn identity_provider_not_found_message(asked_for: &str, user_pool_id: &str) -> String {
    format!("Identity provider {asked_for} for tenantId {user_pool_id} does not exist.")
}

/// The `ProviderName` that names a user pool's OWN users rather than one of its
/// third-party identity providers: "To deactivate a local user, set
/// `ProviderName` to `Cognito`" (`AdminDisableProviderForUser`), and "the
/// `ProviderName` should be set to `Cognito` for users in Cognito user pools"
/// (`AdminLinkProviderForUser.DestinationUser`), both in this service's model.
pub const NATIVE_PROVIDER_NAME: &str = "Cognito";

/// The message an `InvalidParameterException` carries when a request names a
/// provider identity the pool holds no link for.
///
/// Deactivating a provider for a user acts on a LINK — the external identity's
/// link to a local profile, or, for the native `Cognito` provider, the links a
/// local profile is the destination of — so an account that is linked to
/// nothing has no link to act on, and the refusal is the same sentence whatever
/// the provider or the identity named.
pub const ACCOUNT_NOT_LINKED_MESSAGE: &str =
    "Account isn't linked to anyone else, verify that the account has been linked";

/// How many federated identities may link to one local user: "The maximum
/// number of federated identities linked to a user is five."
/// (`AdminLinkProviderForUser` in this service's model).
pub const MAX_LINKED_PROVIDER_IDENTITIES: usize = 5;

/// The message `AdminLinkProviderForUser`'s `InvalidParameterException` carries
/// for a `SourceUser` that already carries a link.
///
/// A federated identity links to ONE local profile — the operation "links an
/// existing user account ... to an identity from an external IdP" and the link
/// is what lets that identity sign in as the profile — so an identity the pool
/// already holds a link for is not one a further call can link, whichever
/// destination it names. One fixed sentence that names the two MEMBERS of the
/// request rather than the provider, the identity or the profile they carried,
/// so the refusal echoes nothing the caller submitted.
pub const SOURCE_USER_ALREADY_LINKED_MESSAGE: &str =
    "SourceUser is already linked to DestinationUser";

/// The message `AdminLinkProviderForUser`'s `InvalidParameterException` carries
/// for a `SourceUser` that already links to a DIFFERENT local profile than the
/// `DestinationUser` the request names.
///
/// A federated identity links to ONE local profile, so a request that re-points
/// a linked identity at another profile is refused: the identity is spoken for.
/// One fixed sentence that names the MEMBER whose value was already linked
/// rather than the provider, the identity or either profile, so the refusal
/// echoes nothing the caller submitted. A repeat of the link the identity
/// already holds carries [`SOURCE_USER_ALREADY_LINKED_MESSAGE`] instead: that
/// request names the profile the link already points at.
pub const SOURCE_USER_LINKED_TO_ANOTHER_USER_MESSAGE: &str =
    "SourceUser with the specified attribute value is already linked to another user";

/// The message `AdminLinkProviderForUser`'s `InvalidParameterException` carries
/// for a `SourceUser` that names one of the pool's OWN users rather than an
/// external identity.
///
/// `SourceUser` is "an external IdP account for a user who doesn't exist yet in
/// the user pool. This user must be a federated user (for example, a SAML or
/// Facebook user), not another native user"
/// (`smithy_query(query="doc",shape="AdminLinkProviderForUserRequest",member="SourceUser")`),
/// so an identifier that resolves to a user pool user who signs in with a
/// username and password identifies no external account to link. One fixed
/// sentence, which names neither the identifier nor the user it resolved to.
pub const INVALID_SOURCE_USER_MESSAGE: &str =
    "Invalid SourceUser: Cognito users with a username/password may not be passed in as a \
     SourceUser, only as a DestinationUser";

/// The message a `TermsExistsException` carries for a `TermsName` the app client
/// already holds terms documents under.
///
/// Terms document names "must be unique to the app client" (this service's
/// model), so the refusal names the name that collided and the app client it
/// collided within — the same sentence wherever that uniqueness is enforced, on
/// a create of a second document of that name and on a rename onto it alike.
pub fn terms_exists_message(terms_name: &str, client_id: &str) -> String {
    format!("Terms with name {terms_name} already exists for Client {client_id}.")
}

/// The message a `ResourceNotFoundException` carries when a request resolves no
/// set of terms documents — by its `TermsId` in the named pool, or by the app
/// client and `TermsName` that name it together.
///
/// Amazon Cognito answers the same sentence however the documents were looked
/// up: it reports that the lookup found no terms documents and quotes neither
/// the id, the name, the app client nor the pool, exactly as
/// [`USER_NOT_FOUND_MESSAGE`] and [`GROUP_NOT_FOUND_MESSAGE`] report their own
/// unresolved lookups. A message built from the request would word the refusal
/// differently per id or per name, so anything that groups refusals by message
/// would see one error per request rather than one not-found class — and the
/// lookup by app client would put a `@sensitive` `ClientId` in it.
///
/// The app-client lookup THIS pool fails to resolve at all is a different class
/// and keeps its own wording (the `describe_terms_by_client` handler): there the
/// app client, not the terms documents, is what does not exist.
pub const TERMS_NOT_FOUND_MESSAGE: &str = "Terms does not exist.";

// ---------------------------------------------------------------------------
// Password policy rejection wording
//
// A rejection says that the password missed the pool's policy and which
// requirement it missed, in a fixed clause per requirement: the clause is the
// same whatever the pool configured, so the refusal does not report the pool's
// policy values to a caller that has not read them.
// ---------------------------------------------------------------------------

/// Which wording a password-policy rejection carries, ahead of the clause for
/// the requirement the password missed.
///
/// Amazon Cognito does not word this refusal identically in every API that
/// judges a password: the administrative password set, the administrative create
/// and the paths where the user supplies its own password each answer their own
/// sentence. The clause for the requirement is the same in all of them, so only
/// the prefix is chosen here and every operation names the wording ITS OWN API
/// answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordPolicyWording {
    /// The wording `AdminSetUserPassword` answers.
    AdminSetsPassword,
    /// The wording `AdminCreateUser` answers for a `TemporaryPassword` the
    /// policy refuses.
    AdminCreatesUser,
    /// The wording the paths where the user itself supplies the password answer
    /// — a sign-up, a forgotten-password confirmation, a new password given in
    /// answer to a challenge.
    UserSuppliesPassword,
}

impl PasswordPolicyWording {
    /// The prefix this wording carries ahead of the requirement's clause.
    fn prefix(self) -> &'static str {
        match self {
            Self::AdminSetsPassword => "Password does not conform to policy: ",
            Self::AdminCreatesUser => "Password did not conform with password policy: ",
            Self::UserSuppliesPassword => "Password did not conform with policy: ",
        }
    }
}

/// The clause for a password shorter than `PasswordPolicyType.MinimumLength`.
const PASSWORD_TOO_SHORT: &str = "Password not long enough";

/// The clause for `PasswordPolicyType.RequireUppercase`.
const PASSWORD_NEEDS_UPPERCASE: &str = "Password must have uppercase characters";

/// The clause for `PasswordPolicyType.RequireLowercase`.
const PASSWORD_NEEDS_LOWERCASE: &str = "Password must have lowercase characters";

/// The clause for `PasswordPolicyType.RequireNumbers`.
const PASSWORD_NEEDS_NUMBERS: &str = "Password must have numeric characters";

/// The clause for `PasswordPolicyType.RequireSymbols`.
const PASSWORD_NEEDS_SYMBOLS: &str = "Password must have symbol characters";

/// The message a password-policy rejection worded as `wording` asks for
/// carries for the requirement `requirement` names.
fn password_policy_message(wording: PasswordPolicyWording, requirement: &str) -> String {
    format!("{}{requirement}", wording.prefix())
}

/// The message the refusal of a password the pool's history policy restricts
/// carries: a `PasswordHistoryPolicyViolationException` is "the message returned
/// when a user's new password matches a previous password and doesn't comply with
/// the password-history policy" (this service's model), so the refusal reports
/// that the submitted password is one the user has already had and names neither
/// the password nor how many the policy keeps. It carries no policy prefix: this
/// is not a `PasswordPolicyType` requirement the password missed
/// ([`PasswordPolicyWording`]), it is the one password-history refusal.
pub const PASSWORD_REUSED_MESSAGE: &str = "Password has previously been used";

// ---------------------------------------------------------------------------
// Threat protection (risk configuration)
// ---------------------------------------------------------------------------

/// The `AdvancedSecurityMode` that leaves threat protection switched off: one of
/// the modelled [`crate::ops::types::ADVANCED_SECURITY_MODE_TYPE_VALUES`], and
/// the value a pool that carries no `UserPoolAddOns` behaves as.
pub const ADVANCED_SECURITY_MODE_OFF: &str = "OFF";

/// The `AdvancedSecurityMode` under which threat protection ACTS on traffic: "to
/// configure automatic security responses to potentially unwanted traffic to
/// your user pool, set to `ENFORCED`" (`UserPoolAddOnsType` in this service's
/// model), as against `AUDIT`, which logs "user security information but take[s]
/// no action". One of the modelled
/// [`crate::ops::types::ADVANCED_SECURITY_MODE_TYPE_VALUES`].
pub const ADVANCED_SECURITY_MODE_ENFORCED: &str = "ENFORCED";

/// The `AdvancedSecurityEnabledModeType` that logs "user security information
/// but take[s] no action" (`UserPoolAddOnsType` in this service's model), and the
/// mode threat protection runs the additional authentication types in where the
/// request configures none for them: that enum admits no "off" value
/// ([`crate::ops::types::ADVANCED_SECURITY_ENABLED_MODE_TYPE_VALUES`]), so a pool
/// whose add-ons are configured reports the auditing mode for them.
pub const ADVANCED_SECURITY_MODE_AUDIT: &str = "AUDIT";

/// The message a `UserPoolAddOnNotEnabledException` carries.
///
/// `UserPoolAddOnNotEnabledException` is thrown "when user pool add-ons aren't
/// enabled" (this service's model), which is the state of a pool that carries no
/// threat-protection surface: "to change threat protection to audit-only or off,
/// update the value of `UserPoolAddOns` in an `UpdateUserPool` request"
/// (`SetRiskConfiguration`). The refusal reports that state of the pool and names
/// nothing the request asked for, so it is the same sentence for every pool and
/// app client.
pub const USER_POOL_ADD_ONS_NOT_ENABLED_MESSAGE: &str = "User pool add-ons is not enabled.";

// ---------------------------------------------------------------------------
// User pool domains
// ---------------------------------------------------------------------------

/// The managed login branding version for hosted UI (classic): "A value of `1`
/// indicates hosted UI (classic) and a version of `2` indicates managed login"
/// (`CreateUserPoolDomain.ManagedLoginVersion` in this service's model).
pub const HOSTED_UI_CLASSIC_BRANDING: i32 = 1;

/// The managed login branding version, which the same member documents as
/// needing "your user pool be configured for any feature plan other than
/// `Lite`".
pub const MANAGED_LOGIN_BRANDING: i32 = 2;

/// The only branding versions `ManagedLoginVersion` names, so a request asking
/// for any other value asks for a version that does not exist.
pub const MANAGED_LOGIN_VERSIONS: [i32; 2] = [HOSTED_UI_CLASSIC_BRANDING, MANAGED_LOGIN_BRANDING];

/// The branding version a domain created without one carries: the model names
/// only the two above, and hosted UI (classic) is the one that needs no feature
/// plan.
pub const DEFAULT_MANAGED_LOGIN_VERSION: i32 = HOSTED_UI_CLASSIC_BRANDING;

/// The feature plan managed login is NOT included in
/// (`CreateUserPoolDomain.ManagedLoginVersion`); one of the modelled
/// `UserPoolTierType` values.
pub const LITE_USER_POOL_TIER: &str = "LITE";

/// The feature plan email message MFA needs: `EmailMfaConfigType` is "To
/// activate this setting, your user pool must be in the Essentials tier or
/// higher" (this service's model); one of the modelled
/// [`crate::ops::types::USER_POOL_TIER_TYPE_VALUES`].
pub const ESSENTIALS_USER_POOL_TIER: &str = "ESSENTIALS";

/// The modelled feature plans ([`crate::ops::types::USER_POOL_TIER_TYPE_VALUES`])
/// in INCLUSION order, cheapest first: a plan includes every feature the plans
/// before it include, which is what "the Essentials tier or higher"
/// (`EmailMfaConfigType`) and "the Plus feature plan"
/// (`LogConfigurationType.EventSource`) each name a floor of.
const USER_POOL_TIER_ORDER: [&str; 3] = [
    LITE_USER_POOL_TIER,
    ESSENTIALS_USER_POOL_TIER,
    PLUS_USER_POOL_TIER,
];

/// Whether a pool on `tier` includes a feature whose floor is the plan
/// `minimum`: the plans are ordered by inclusion, so every plan from `minimum`
/// upwards includes it and every plan below it does not. An unmodelled `tier`
/// includes nothing beyond the cheapest plan, so a gated feature is refused
/// rather than granted.
#[must_use]
pub fn user_pool_tier_includes(tier: &str, minimum: &str) -> bool {
    let rank = |plan: &str| {
        USER_POOL_TIER_ORDER
            .iter()
            .position(|known| *known == plan)
            .unwrap_or(0)
    };
    rank(tier) >= rank(minimum)
}

/// The refusal a feature whose floor is the plan `minimum` owes a pool on
/// `tier`, or `None` when that pool's plan includes the feature.
///
/// Every operation that configures a plan-gated feature asks this one question,
/// so the boundary and the sentence it is reported with cannot disagree between
/// them. `feature` is the feature's display name, as
/// [`feature_unavailable_in_tier_message`] renders it.
#[must_use]
pub fn feature_plan_refusal(tier: &str, minimum: &str, feature: &str) -> Option<String> {
    (!user_pool_tier_includes(tier, minimum))
        .then(|| feature_unavailable_in_tier_message(tier, feature))
}

/// The message a `FeatureUnavailableInTierException` carries: a tier refusal
/// reports the pool's configured feature plan and the feature that plan does not
/// include, in one template, so a caller can parse the feature out of it and
/// tell its operator what to upgrade.
///
/// `tier` is the pool's stored `UserPoolTier` (a `UserPoolTierType` value) and
/// `feature` names the feature the request asked for.
pub fn feature_unavailable_in_tier_message(tier: &str, feature: &str) -> String {
    format!("The following feature is not available for the {tier} pricing tier configured: {feature}")
}

/// The message a `FeatureUnavailableInTierException` carries for a pool whose
/// OWN configuration asks for a feature its feature plan excludes: the request
/// sets the plan and the feature TOGETHER, so the refusal reports the features
/// that have to be off for the plan it configured rather than a feature that is
/// unavailable to configure.
///
/// `tier` is the plan the pool is left on and `feature` the feature's display
/// name, as [`feature_unavailable_in_tier_message`] renders it for the
/// operations that configure a feature on a pool whose plan is already set.
pub fn features_need_disabling_message(tier: &str, feature: &str) -> String {
    format!(
        "The following features need to be disabled for the {tier} pricing tier configured: \
         {feature}"
    )
}

/// The feature a tier refusal names for user pool email message MFA: the
/// `EmailMfaConfiguration` member of `SetUserPoolMfaConfig` configures "user
/// pool email message MFA and sign-in with one-time passwords (OTPs)"
/// (`EmailMfaConfigType`), reported through
/// [`feature_unavailable_in_tier_message`] the way every other tier refusal is.
pub const EMAIL_MFA_FEATURE: &str = "Email MFA";

/// The feature name a tier refusal gives for a `ManagedLoginVersion` of
/// `version`: the branding version is part of the feature asked for, since only
/// managed login (version 2) needs a feature plan other than
/// [`LITE_USER_POOL_TIER`].
pub fn managed_login_feature_name(version: i32) -> String {
    format!("Managed Login Version {version}")
}

/// The feature a tier refusal names for choice-based (selection-based) sign-in:
/// the pool's `SignInPolicy` and the app client's `ALLOW_USER_AUTH` flow are the
/// two halves of ONE feature, and each documents the same floor — "To activate
/// this setting, your user pool must be in the Essentials tier or higher"
/// (`SignInPolicyType`, and `ALLOW_USER_AUTH` under
/// `CreateUserPoolClient.ExplicitAuthFlows`) — so both halves are refused under
/// this one name.
pub const CHOICE_BASED_SIGN_IN_FEATURE: &str = "Choice Based Authentication";

/// The feature a tier refusal names for threat protection: `UserPoolAddOnsType`
/// carries "settings for activation of threat protection" and "to activate this
/// setting, your user pool must be on the Plus tier" (this service's model).
pub const THREAT_PROTECTION_FEATURE: &str = "Threat Protection";

/// Whether `policies` configures choice-based sign-in — a first authentication
/// factor BEYOND username and password.
///
/// `SignInPolicyType.AllowedFirstAuthFactors` is "the sign-in methods that a
/// user pool supports as the first factor. You can permit users to start
/// authentication with a standard username and password, or with other one-time
/// password and hardware factors" (this service's model), and a pool that
/// configures no policy admits [`PASSWORD_AUTH_FACTOR`] alone
/// ([`effective_policies`]) — so a policy naming only that factor asks for
/// nothing beyond what every plan serves, while any other factor is the gated
/// feature. Both the create and the update path ask this one function, so the
/// two cannot disagree about which configuration needs which plan.
#[must_use]
pub fn configures_choice_based_sign_in(policies: Option<&UserPoolPolicyType>) -> bool {
    policies
        .and_then(|policies| policies.sign_in_policy.as_ref())
        .and_then(|policy| policy.allowed_first_auth_factors.as_ref())
        .is_some_and(|factors| factors.iter().any(|factor| factor != PASSWORD_AUTH_FACTOR))
}

/// Whether `flows` configures choice-based sign-in on an app client: the
/// `ALLOW_USER_AUTH` flow "covers username-password, secure remote password
/// (SRP), passwordless, and passkey authentication" and names the same floor
/// ([`CHOICE_BASED_SIGN_IN_FEATURE`]). Asked by the create and the update path
/// alike.
#[must_use]
pub fn configures_user_auth_flow(flows: &[String]) -> bool {
    flows
        .iter()
        .any(|flow| USER_AUTH_FLOWS.contains(&flow.as_str()))
}

/// The refusal an APP CLIENT configuration owes the feature plan of the pool it
/// belongs to, or `None` when that plan includes every feature the
/// configuration asks for.
///
/// An app-client member whose own documentation names a feature plan is gated on
/// it: `ALLOW_USER_AUTH` documents "the Essentials tier or higher"
/// ([`CHOICE_BASED_SIGN_IN_FEATURE`]), which is the only such member of an app
/// client's configuration (this service's model). The plan is the POOL's stored
/// one, which the client request does not set, so the refusal reports the
/// feature as unavailable ([`feature_unavailable_in_tier_message`]) rather than
/// as one to disable. Both the create and the update path ask this one function.
#[must_use]
pub fn client_feature_plan_refusal(tier: &str, explicit_auth_flows: Option<&[String]>) -> Option<String> {
    if explicit_auth_flows.is_some_and(configures_user_auth_flow) {
        if let Some(message) = feature_plan_refusal(
            tier,
            ESSENTIALS_USER_POOL_TIER,
            CHOICE_BASED_SIGN_IN_FEATURE,
        ) {
            return Some(message);
        }
    }
    None
}

/// The message an app client that asks to propagate additional user context data
/// without holding a client secret is refused with: "You can only activate
/// `EnablePropagateAdditionalUserContextData` in an app client that has a client
/// secret" (`CreateUserPoolClient` and `UpdateUserPoolClient` in this service's
/// model). It names no value from the request, so every such refusal carries
/// this one sentence, and both paths read it here.
pub const CLIENT_SECRET_REQUIRED_FOR_CONTEXT_DATA_MESSAGE: &str =
    "Client Secret is required to set EnablePropagateAdditionalUserContextData as true";

/// The refusal a USER POOL configuration owes the feature plan it is on, or
/// `None` when that plan includes every feature the configuration asks for.
///
/// A member whose own documentation names a feature plan is gated on it, and
/// each one states its own floor: `SignInPolicyType` ("the Essentials tier or
/// higher") and `UserPoolAddOnsType` ("the Plus tier") are the two such members
/// of a pool's configuration (this service's model). What each member asks FOR
/// is what the gate reads, not whether the member is present: `UserPoolAddOns`
/// asks for the gated feature in the mode that acts on traffic
/// ([`ADVANCED_SECURITY_MODE_ENFORCED`]) and for neither logging-only threat
/// protection ([`ADVANCED_SECURITY_MODE_OFF`] and `AUDIT`, which "take[s] no
/// action") nor a `SignInPolicy` naming only [`PASSWORD_AUTH_FACTOR`], which
/// configures no further factor.
///
/// The plan the pool is CONFIGURED on is what decides: an update is judged
/// against the plan it leaves the pool on, and a create against the plan its own
/// request names — a create that names none configures no plan and is put on one
/// that includes its features instead
/// ([`effective_user_pool_tier_for_configuration`]). Both paths ask this one
/// function, so neither can gate a feature the other serves.
#[must_use]
pub fn pool_feature_plan_refusal(
    tier: &str,
    policies: Option<&UserPoolPolicyType>,
    add_ons: Option<&UserPoolAddOnsType>,
) -> Option<String> {
    pool_configuration_feature_floors(policies, add_ons)
        .into_iter()
        .find(|(floor, _)| !user_pool_tier_includes(tier, floor))
        .map(|(_, feature)| features_need_disabling_message(tier, feature))
}

/// Every plan-gated feature a pool configuration ASKS FOR, each with the plan
/// floor its own documentation names and the name a refusal reports it under.
///
/// The two gated members of a pool's configuration are `SignInPolicyType` ("the
/// Essentials tier or higher") and `UserPoolAddOnsType` ("the Plus tier") (this
/// service's model), and what each asks FOR is what counts — not whether the
/// member is present. Both the gate ([`pool_feature_plan_refusal`]) and the plan
/// a create that names none is put on
/// ([`effective_user_pool_tier_for_configuration`]) read this one table, so the
/// plan a pool is given can never exclude a feature the same configuration is
/// refused for.
#[must_use]
fn pool_configuration_feature_floors(
    policies: Option<&UserPoolPolicyType>,
    add_ons: Option<&UserPoolAddOnsType>,
) -> Vec<(&'static str, &'static str)> {
    let mut floors = Vec::new();
    if configures_choice_based_sign_in(policies) {
        floors.push((ESSENTIALS_USER_POOL_TIER, CHOICE_BASED_SIGN_IN_FEATURE));
    }
    if add_ons.is_some_and(|add_ons| add_ons.advanced_security_mode == ADVANCED_SECURITY_MODE_ENFORCED)
    {
        floors.push((PLUS_USER_POOL_TIER, THREAT_PROTECTION_FEATURE));
    }
    floors
}

/// The feature plan a pool whose configuration is `policies` and `add_ons` is
/// created on when the request names `requested`.
///
/// A request that names a plan CONFIGURES it, so that plan is the pool's and
/// [`pool_feature_plan_refusal`] judges the configuration against it. A request
/// that names none configures no plan: "`UserPoolTier` ... determines the
/// eligibility of the user pool for features like managed login, access-token
/// customization, and threat protection" (`CreateUserPool.UserPoolTier` in this
/// service's model), so the pool is created on the cheapest plan that includes
/// every feature its configuration asks for — the documented
/// [`DEFAULT_USER_POOL_TIER`] when it asks for none, and the highest floor among
/// the features it does ask for otherwise.
#[must_use]
pub fn effective_user_pool_tier_for_configuration(
    requested: Option<String>,
    policies: Option<&UserPoolPolicyType>,
    add_ons: Option<&UserPoolAddOnsType>,
) -> String {
    if let Some(requested) = requested {
        return requested;
    }
    pool_configuration_feature_floors(policies, add_ons)
        .into_iter()
        .fold(DEFAULT_USER_POOL_TIER, |plan, (floor, _)| {
            if user_pool_tier_includes(plan, floor) {
                plan
            } else {
                floor
            }
        })
        .to_string()
}

// ---------------------------------------------------------------------------
// App client OAuth configuration
// ---------------------------------------------------------------------------

/// The `OAuthFlowType` value of the machine-to-machine grant: "`client_credentials`
/// Issue the access token from the /oauth2/token endpoint directly to a non-person
/// user" (`CreateUserPoolClient.AllowedOAuthFlows` in this service's model).
pub const CLIENT_CREDENTIALS_OAUTH_FLOW: &str = "client_credentials";

/// The message an app client whose `AllowedOAuthFlows` combine the
/// machine-to-machine grant with a user-facing one is refused with
/// (`InvalidOAuthFlowException`). It names no value from the request, so every
/// such refusal carries this one sentence.
pub const CLIENT_CREDENTIALS_FLOW_EXCLUSIVE_MESSAGE: &str =
    "client_credentials flow can not be selected along with code flow or implicit flow.";

/// The refusal an `AllowedOAuthFlows` list owes when it asks for a combination
/// no app client can hold, and `None` when it describes one the service accepts.
///
/// "To create an app client that generates client credentials grants, you must
/// add `client_credentials` as the only allowed OAuth flow"
/// (`CreateUserPoolClient.AllowedOAuthFlows` in this service's model): the
/// machine-to-machine grant is issued to a non-person user, while `code` and
/// `implicit` are grants a signed-in human is redirected through, so a list
/// naming `client_credentials` beside either of them describes an app client
/// that cannot exist. A list naming it alone — or not at all, or repeating it —
/// is accepted: the rule is about which grants are configured, not how many
/// times each is sent. Both the create and the update path ask this one
/// function, so the two cannot disagree about what an app client may hold.
#[must_use]
pub fn invalid_oauth_flow_violation(flows: &[String]) -> Option<&'static str> {
    let names_client_credentials = flows
        .iter()
        .any(|flow| flow == CLIENT_CREDENTIALS_OAUTH_FLOW);
    let names_a_user_grant = flows
        .iter()
        .any(|flow| flow != CLIENT_CREDENTIALS_OAUTH_FLOW);
    (names_client_credentials && names_a_user_grant).then_some(CLIENT_CREDENTIALS_FLOW_EXCLUSIVE_MESSAGE)
}

/// The scopes every user pool authorizes without a resource server of its own:
/// "Scope values include `phone`, `email`, `openid`, and `profile`. The
/// `aws.cognito.signin.user.admin` scope authorizes user self-service
/// operations" (`CreateUserPoolClient.AllowedOAuthScopes` in this service's
/// model). Every other scope value is a custom one, which only a resource server
/// of the pool can define ([`UserPool::defines_scope`]).
pub const STANDARD_OAUTH_SCOPES: [&str; 5] = [
    "phone",
    "email",
    "openid",
    "profile",
    USER_ADMIN_SCOPE,
];

/// The message a `ScopeDoesNotExistException` carries for `scope`: the scope the
/// request asked for is the one value such a refusal reports, echoed only as far
/// as this service echoes any submitted value.
pub fn scope_does_not_exist_message(scope: &str) -> String {
    format!("Invalid scope requested: {}", echoed_value(scope))
}

/// The `DomainStatusType` a freshly created domain is in, until the clock makes
/// its activation due: one of the modelled
/// [`crate::ops::types::DOMAIN_STATUS_TYPE_VALUES`].
pub const DOMAIN_STATUS_CREATING: &str = "CREATING";

/// The `DomainStatusType` a domain reaches once [`DOMAIN_ACTIVATION_SPAN`] has
/// elapsed: one of the modelled
/// [`crate::ops::types::DOMAIN_STATUS_TYPE_VALUES`].
pub const DOMAIN_STATUS_ACTIVE: &str = "ACTIVE";

/// The `DomainStatusType` a domain whose configuration has just changed is in,
/// until the clock makes it [`DOMAIN_STATUS_ACTIVE`] again: one of the modelled
/// [`crate::ops::types::DOMAIN_STATUS_TYPE_VALUES`].
pub const DOMAIN_STATUS_UPDATING: &str = "UPDATING";

/// How long a created domain stays `CREATING` before the clock owes it `ACTIVE`.
///
/// A chosen span, not a documented duration: `CreateUserPoolDomain` only says
/// "Your prefix domain might take up to one minute to take effect" and "Your
/// custom domain is online within five minutes", so a domain is NOT ready when
/// the call returns, and this emulator makes the transition due shortly after
/// instead of waiting minutes. Never zero, so a caller reading the domain right
/// after creating it still sees `CREATING`.
pub const DOMAIN_ACTIVATION_SPAN: Duration = Duration::from_millis(500);

/// The message an operation that resolves a domain by the PAIR it is addressed
/// by — a `UserPoolId` and a `Domain` — carries when that pair holds no domain.
///
/// The pair is resolved as a unit, so the refusal reports the pair and names
/// neither half: one fixed sentence for a pool that holds no domain of that
/// name, whichever of the two the caller got wrong.
pub const NO_SUCH_DOMAIN_MESSAGE: &str = "No such domain or user pool exists.";

/// The message an operation that resolves a domain by the PAIR `UserPoolId` +
/// `Domain` carries when the `Domain` half resolves to a domain of ANOTHER pool.
///
/// A domain name is unique across pools, so this pair does not name a resource
/// that is missing: it names one the pool it was asked for is not configured
/// with. The sentence is fixed text in full and names neither half of the pair,
/// so it discloses nothing about the pool that does hold the domain.
pub const DOMAIN_OF_OTHER_POOL_MESSAGE: &str = "The domain is not configured for this user pool.";

/// The message `CreateUserPoolDomain` carries when the `Domain` the request asks
/// for is one some pool already holds.
///
/// Fixed text in full, with no variable part: the refusal reports that the name
/// is taken and names neither the domain nor the pool holding it, so it
/// discloses nothing about a pool the caller may not own.
pub const DOMAIN_EXISTS_MESSAGE: &str = "Domain already exists.";

// ---------------------------------------------------------------------------
// User pool replicas
//
// A replica's `Status` is COMPUTED from what is stored plus the clock, so a
// read renders the status the replica is in at that instant and writes
// nothing. Each status below is one of the modelled
// [`crate::ops::types::REPLICA_STATUS_TYPE_VALUES`] and each role one of the
// modelled [`crate::ops::types::REPLICA_ROLE_TYPE_VALUES`].
// ---------------------------------------------------------------------------

/// "`CREATING`: The replica is being created" (`UserPoolReplicaType.Status`):
/// what a created secondary replica reports until the clock makes its
/// activation due.
pub const REPLICA_STATUS_CREATING: &str = "CREATING";

/// "`ACTIVE`: The replica is available for both end-user and administrator
/// operations" (`UserPoolReplicaType.Status`).
pub const REPLICA_STATUS_ACTIVE: &str = "ACTIVE";

/// "`INACTIVE`: The replica has been created, but is not accepting requests for
/// end-users. Administrator configuration operations are supported"
/// (`UserPoolReplicaType.Status`).
pub const REPLICA_STATUS_INACTIVE: &str = "INACTIVE";

/// "`DELETING`: The replica is being deleted" (`UserPoolReplicaType.Status`).
pub const REPLICA_STATUS_DELETING: &str = "DELETING";

/// "`PRIMARY`: The primary replica supports all end user and administrator
/// operations" (`UserPoolReplicaType.Role`): the role the pool holds in the
/// Region it was created in.
pub const REPLICA_ROLE_PRIMARY: &str = "PRIMARY";

/// "`SECONDARY`: The secondary replica supports a limited set of end user and
/// administrator operations" (`UserPoolReplicaType.Role`): the role of every
/// replica a `CreateUserPoolReplica` call adds.
pub const REPLICA_ROLE_SECONDARY: &str = "SECONDARY";

/// How long a created secondary replica stays `CREATING` before the clock owes
/// it the status it settles into.
///
/// A chosen span, not a documented duration: the model gives a replica that
/// "is being created" its own status without saying how long creating takes, so
/// a replica is NOT ready when the call returns and this emulator makes the
/// transition due shortly after instead of waiting minutes. Never zero, so a
/// caller listing the replicas straight after creating one still sees
/// `CREATING`.
pub const REPLICA_ACTIVATION_SPAN: Duration = Duration::from_millis(500);

/// How long a deleted replica stays [`REPLICA_STATUS_DELETING`] before the
/// clock owes its removal. A chosen span, on the same grounds as
/// [`REPLICA_ACTIVATION_SPAN`]: `DELETING` means "The replica is being
/// deleted", so the replica is still reported, as `DELETING`, right after the
/// call.
pub const REPLICA_DELETION_SPAN: Duration = Duration::from_millis(500);

/// The feature a tier refusal names for a replica: replicating a user pool to
/// another Region needs a feature plan other than [`LITE_USER_POOL_TIER`], and
/// a refusal reports it through [`feature_unavailable_in_tier_message`] the way
/// every other tier refusal does.
pub const REPLICATION_FEATURE: &str = "Multi-Region Replication";

/// The Regions a `RegionName` may name: the emulator's SHARED Region catalog,
/// not a copy of it.
///
/// A `RegionNameType` is "the Region in which the replica user pool will be
/// created" (`CreateUserPoolReplica.RegionName`), and the model bounds only its
/// length (5..32), so the set of values that name a Region at all is not on the
/// wire. That set is `emulator_telemetry::policy::KNOWN_REGIONS` ("every AWS
/// region the wire may carry, sorted for binary search", maintained centrally
/// and already a dependency of this crate), so the Regions this service accepts
/// cannot drift from the ones the rest of the emulator knows.
pub const AWS_REGIONS: &[&str] = ::emulator_telemetry::policy::KNOWN_REGIONS;

/// Whether `region` names an AWS Region ([`AWS_REGIONS`]). Every operation that
/// takes a `RegionName` asks this one question, so none of them can accept a
/// Region the others refuse. The lookup is the binary search the shared
/// catalog's ordering is maintained for.
#[must_use]
pub fn is_aws_region(region: &str) -> bool {
    AWS_REGIONS.binary_search(&region).is_ok()
}

/// The message an operation refusing a `RegionName` that names no AWS Region
/// carries. It reports that the submitted Region name is not one and quotes
/// nothing from the request, so every such refusal carries this one sentence.
pub const INVALID_REGION_MESSAGE: &str = "Invalid region name";

/// How much of a caller-supplied value a refusal message echoes before it
/// elides: the bound the framework's own constraint messages use, so a long
/// submitted value cannot amplify a small request into a large response.
const MAX_ECHOED_VALUE: usize = 256;

/// `value` as a refusal message echoes it: itself when short, else a
/// character-boundary-safe prefix of [`MAX_ECHOED_VALUE`] bytes with the
/// elision the framework's constraint messages render — `<prefix>... [<n>
/// bytes]` — so a refusal this service states by hand reads exactly like one
/// the framework stated for the same value. Rendering only: the caller checks
/// the ORIGINAL value, because eliding before a check would compare text the
/// caller never sent.
#[must_use]
pub fn echoed_value(value: &str) -> String {
    if value.len() <= MAX_ECHOED_VALUE {
        return value.to_string();
    }
    let mut end = MAX_ECHOED_VALUE;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}... [{} bytes]",
        value.get(..end).unwrap_or_default(),
        value.len()
    )
}

/// `submitted` as this service STORES an optional text member: a value the
/// request left out and one it sent empty are stored the same way — as no value
/// at all.
///
/// Amazon Cognito materialises no empty string: a member whose modelled length
/// starts at 0 accepts `""` on input (`DescriptionType` is length 0..2048,
/// `smithy_query(query="shape",shape="DescriptionType")`), and the stored
/// resource then carries no such member, so a later read omits it rather than
/// rendering an empty value. The ONE place that normalization is decided, so
/// every writer of an optional text member agrees.
#[must_use]
pub fn stored_text(submitted: Option<String>) -> Option<String> {
    submitted.filter(|value| !value.is_empty())
}

/// The attribute values `requested` writes onto a user, as this service stores
/// them: one entry per named attribute, with every empty value dropped.
///
/// "To delete an attribute from your user, submit the attribute in your API
/// request with a blank value" (`AdminUpdateUserAttributes.UserAttributes` in
/// this service's model) — an empty value is not a value of the attribute, so an
/// attribute submitted with one is stored for no user and a read lists it for
/// none. The ONE place a submitted attribute list becomes stored attribute
/// values, so a create normalizes exactly as an update does
/// ([`plan_attribute_update`]).
#[must_use]
pub fn stored_attribute_values(requested: &[AttributeType]) -> BTreeMap<String, String> {
    requested
        .iter()
        .filter_map(|attribute| {
            stored_text(attribute.value.clone()).map(|value| (attribute.name.clone(), value))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// User import jobs
//
// A job's `Status` is COMPUTED from what is stored plus the clock, so a read
// renders the status the job is in at that instant and writes nothing.
// Each value below is one of the modelled
// [`crate::ops::types::USER_IMPORT_JOB_STATUS_TYPE_VALUES`].
// ---------------------------------------------------------------------------

/// "`Created` - The job was created but not started"
/// (`UserImportJobType.Status`).
pub const JOB_STATUS_CREATED: &str = "Created";

/// "`Pending` - A transition state. You have started the job, but it has not
/// begun importing users yet" (`UserImportJobType.Status`).
pub const JOB_STATUS_PENDING: &str = "Pending";

/// "`InProgress` - The job has started, and users are being imported"
/// (`UserImportJobType.Status`).
pub const JOB_STATUS_IN_PROGRESS: &str = "InProgress";

/// "`Stopping` - You have stopped the job, but the job has not stopped
/// importing users yet" (`UserImportJobType.Status`).
pub const JOB_STATUS_STOPPING: &str = "Stopping";

/// "`Stopped` - You have stopped the job, and the job has stopped importing
/// users" (`UserImportJobType.Status`).
pub const JOB_STATUS_STOPPED: &str = "Stopped";

/// "`Expired` - You created a job, but did not start the job within 24-48
/// hours" (`UserImportJobType.Status`).
pub const JOB_STATUS_EXPIRED: &str = "Expired";

/// How long a started job stays `Pending` before the clock owes it
/// `InProgress`.
///
/// A chosen span, not a documented duration: `UserImportJobType.Status`
/// documents `Pending` as "A transition state. You have started the job, but it
/// has not begun importing users yet", so a job is NOT importing when
/// `StartUserImportJob` returns. Never zero, so a caller reading the job right
/// after starting it still sees the transient status.
pub const JOB_START_SPAN: Duration = Duration::from_millis(500);

/// The message a start refused because the job's CSV file was never uploaded
/// carries: a job "imports users from a CSV file" that the client writes to the
/// job's `PreSignedUrl` (`StartUserImportJob` and
/// `UserImportJobType.PreSignedUrl` in this service's model), so the refusal
/// reports that the file is missing and names no value from the request.
pub const NO_CSV_UPLOADED_MESSAGE: &str =
    "No users are available for import. Upload the CSV file to the job's pre-signed URL before \
     starting it.";

/// How long a stopped job stays `Stopping` before the clock owes it `Stopped`.
///
/// A chosen span, not a documented duration: `Stopping` is "You have stopped the
/// job, but the job has not stopped importing users yet"
/// (`UserImportJobType.Status`), so the stop is not complete when
/// `StopUserImportJob` returns.
pub const JOB_STOP_SPAN: Duration = Duration::from_millis(500);

/// How long a created job can go unstarted before the clock owes it `Expired`:
/// "You created a job, but did not start the job within 24-48 hours ... the job
/// can't be started" (`UserImportJobType.Status`), whose earliest documented
/// bound is 24 hours.
pub const JOB_EXPIRY_SPAN: Duration = Duration::from_secs(24 * 60 * 60);

/// How long a passkey registration challenge stays answerable, which is also the
/// `timeout` its credential creation options advertise, so the window the caller
/// was told about is the window the emulator holds: W3C Web Authentication Level
/// 2 §5.4 recommends a `PublicKeyCredentialCreationOptions.timeout` "between
/// 300000 milliseconds (5 minutes) and 600000 milliseconds (10 minutes)" for a
/// registration ceremony.
pub const WEB_AUTHN_REGISTRATION_TIMEOUT: Duration = Duration::from_millis(300_000);

/// `UserVerificationType`'s value for a pool that encourages but does not
/// require user verification, which is what a pool configuring none asks for.
pub const USER_VERIFICATION_PREFERRED: &str = "preferred";

// ---------------------------------------------------------------------------
// Store bounds
//
// This state lives in the emulator's memory, so every store a caller can grow
// is bounded: a repeated request must not grow the process without limit.
// These caps are the EMULATOR's, not an AWS quota, and are deliberately far
// above anything a test fixture builds. The stores whose entries expire are
// swept of what the clock has already expired before the cap is applied, so the
// cap is reached only by live state.
// ---------------------------------------------------------------------------

/// The most user pools this emulator holds.
pub const MAX_USER_POOLS: usize = 1_000;

/// The most app clients one user pool holds.
pub const MAX_CLIENTS_PER_POOL: usize = 1_000;

/// The most users one user pool holds.
pub const MAX_USERS_PER_POOL: usize = 10_000;

/// The most groups one user pool holds.
pub const MAX_GROUPS_PER_POOL: usize = 1_000;

/// The most devices one user holds. A sign-in from a device the user pool does
/// not recognise stores one, so the device authenticated longest ago is evicted
/// past this cap: repeated sign-ins cannot grow one user without limit.
pub const MAX_DEVICES_PER_USER: usize = 200;

/// The most superseded passwords one user holds, beside the one it holds now:
/// `PasswordHistorySizeType` is modelled with a maximum of 24
/// (`smithy_query(query="shape",shape="PasswordHistorySizeType")`), so no pool
/// policy can restrict reuse of more passwords than this and the password set
/// longest ago is dropped past the cap.
pub const MAX_PASSWORD_HISTORY_PER_USER: usize = 24;

/// The most resource servers one user pool holds.
pub const MAX_RESOURCE_SERVERS_PER_POOL: usize = 1_000;

/// The most identity providers one user pool holds.
pub const MAX_IDENTITY_PROVIDERS_PER_POOL: usize = 1_000;

/// The most secondary replicas one user pool holds.
pub const MAX_REPLICAS_PER_POOL: usize = 100;

/// The most user import jobs one user pool holds: Amazon Cognito's quota for
/// user import jobs per user pool, which its refusal states in full
/// ([`USER_IMPORT_JOB_LIMIT_MESSAGE`]). `ListUserImportJobs` reports them a page
/// at a time, so the store is not bounded by what one listing can carry.
pub const MAX_USER_IMPORT_JOBS_PER_POOL: usize = 1_000;

/// The message `CreateUserImportJob`'s `LimitExceededException` carries once the
/// pool holds [`MAX_USER_IMPORT_JOBS_PER_POOL`] jobs.
///
/// The fixed text is `The limit for user pool import jobs is: ` and the trailing
/// `.`; the one variable part is the limit itself, so the sentence and the
/// ceiling the handler enforces can never drift apart.
pub fn user_import_job_limit_message() -> String {
    format!("The limit for user pool import jobs is: {MAX_USER_IMPORT_JOBS_PER_POOL}.")
}

/// The most custom (and developer-only) attributes one user pool's schema
/// holds: Amazon Cognito's quota for custom attributes per user pool is 50 and
/// cannot be raised (Amazon Cognito Developer Guide, "Quotas in Amazon
/// Cognito"). The standard attributes every pool carries are not counted
/// against it.
pub const MAX_CUSTOM_ATTRIBUTES_PER_POOL: usize = 50;

/// The most passkey credentials one user holds: `ListWebAuthnCredentials`'s
/// `MaxResults` is modelled with a maximum of 20
/// (`smithy_query(query="operation",operation="ListWebAuthnCredentials")`), so a
/// user holds no more passkeys than ONE listing can report.
pub const MAX_WEB_AUTHN_CREDENTIALS_PER_USER: usize = 20;

/// The most unexpired passkey registration challenges this emulator holds; the
/// oldest are evicted past it.
const MAX_WEB_AUTHN_REGISTRATIONS: usize = 10_000;

/// The most unexpired challenge sessions this emulator holds; the oldest are
/// evicted past it.
const MAX_AUTH_SESSIONS: usize = 10_000;

/// The most unexpired issued tokens this emulator holds; the oldest are evicted
/// past it. Three are recorded per sign-in, two per refresh.
const MAX_ISSUED_TOKENS: usize = 30_000;

/// Per-service state. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState {
    /// Every created user pool, keyed by its minted `UserPoolId`.
    pub user_pools: HashMap<String, UserPool>,
    /// Challenge sessions, keyed by the opaque `Session` token handed to the caller.
    pub auth_sessions: HashMap<String, AuthSession>,
    /// Every token this emulator issued, keyed by the token string, so a later
    /// operation can resolve a token back to the stored user.
    pub issued_tokens: HashMap<String, TokenRecord>,
    /// The passkey registrations in flight, keyed by the challenge value the
    /// credential creation options carried to the caller.
    pub web_authn_registrations: HashMap<String, WebAuthnRegistration>,
    /// The provisioned request rate this account has set per API category, in
    /// requests per second, keyed by the category name. A category no
    /// adjustment has moved holds no entry and reports the rate it includes, so
    /// the map records the adjustments rather than the whole catalogue.
    pub provisioned_api_rates: BTreeMap<String, i32>,
    /// The managed login branding styles a modification is IN FLIGHT for right
    /// now, keyed by the (`UserPoolId`, `ManagedLoginBrandingId`) pair every
    /// handler reaches a style by, which is what makes concurrent modification
    /// of one style observable: `ConcurrentModificationException` "is thrown if
    /// two or more modifications are happening concurrently", so a modification
    /// is pending for the span it takes and a second modification of the SAME
    /// style inside that span is refused instead of silently overwriting the
    /// first one's read-modify-write. Modifications of DIFFERENT styles never
    /// collide.
    ///
    /// Entered and left by [`Self::begin_branding_modification`] /
    /// [`Self::end_branding_modification`]: a span, never a stored status, so
    /// nothing is left for a later caller to clean up.
    pub modifying_branding: BTreeSet<(String, String)>,
}

impl ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState {
    /// The stored pool, or `None` when no pool carries that id.
    pub fn pool(&self, user_pool_id: &str) -> Option<&UserPool> {
        self.user_pools.get(user_pool_id)
    }

    /// The stored pool, mutably.
    pub fn pool_mut(&mut self, user_pool_id: &str) -> Option<&mut UserPool> {
        self.user_pools.get_mut(user_pool_id)
    }

    /// The tag set of the user pool `user_pool_id` AS IT STANDS IN `region` as of
    /// `now`, or `None` when this service holds no such pool there.
    ///
    /// A pool keeps ONE id across the Regions it is replicated to, and "You can
    /// maintain tags independently on replica user pools"
    /// (`CreateUserPoolReplica.UserPoolTags`), so the Region selects WHICH tag set
    /// is addressed: the pool's own in [`UserPool::region`], and a secondary
    /// replica's own in the Region it was created in. A replica whose deletion the
    /// clock has already finished is gone, which is computed here rather than
    /// written.
    pub fn pool_tags(
        &self,
        region: &str,
        user_pool_id: &str,
        now: SystemTime,
    ) -> Option<&HashMap<String, String>> {
        let pool = self.pool(user_pool_id)?;
        if region == pool.region {
            return Some(&pool.tags);
        }
        pool.live_replica(region, now).map(|replica| &replica.tags)
    }

    /// The same tag set as [`Self::pool_tags`], mutably: what a call that assigns
    /// or removes tags writes.
    pub fn pool_tags_mut(
        &mut self,
        region: &str,
        user_pool_id: &str,
        now: SystemTime,
    ) -> Option<&mut HashMap<String, String>> {
        let pool = self.pool_mut(user_pool_id)?;
        if region == pool.region {
            return Some(&mut pool.tags);
        }
        pool.replicas
            .get_mut(region)
            .filter(|replica| !replica.removed(now))
            .map(|replica| &mut replica.tags)
    }

    /// Mark a modification of the branding style `branding_id` of the pool
    /// `user_pool_id` as in flight, answering `false` when another modification
    /// of the SAME style already is — the caller then refuses its request with
    /// the modelled `ConcurrentModificationException`.
    pub fn begin_branding_modification(&mut self, user_pool_id: &str, branding_id: &str) -> bool {
        self.modifying_branding
            .insert((user_pool_id.to_string(), branding_id.to_string()))
    }

    /// Release the in-flight marker [`Self::begin_branding_modification`] took.
    pub fn end_branding_modification(&mut self, user_pool_id: &str, branding_id: &str) {
        self.modifying_branding
            .remove(&(user_pool_id.to_string(), branding_id.to_string()));
    }

    /// The stored pool that holds the app client `client_id`, or `None` when no pool
    /// does.
    ///
    /// The unauthenticated half of this API — `InitiateAuth`, `SignUp`,
    /// `ForgotPassword`, `RespondToAuthChallenge` — identifies its user pool by
    /// `ClientId` ALONE and models no `UserPoolId` member, so the stored app client is
    /// what names the pool. An id is minted per client and lives in exactly one pool,
    /// so at most one pool can answer.
    pub fn pool_of_client(&self, client_id: &str) -> Option<&UserPool> {
        self.user_pools
            .values()
            .find(|pool| pool.clients.contains_key(client_id))
    }

    /// How an operation reports an app-client lookup it made inside the pool
    /// `user_pool_id` that resolved nothing.
    ///
    /// A `ClientId` is minted per app client and lives in exactly one pool
    /// ([`Self::pool_of_client`]), so such a lookup fails in two different ways
    /// and Amazon Cognito words them differently: an id that names an app client
    /// of ANOTHER pool is reported with both halves of what was asked for
    /// ([`app_client_not_found_in_pool_message`]), while an id that names no app
    /// client at all carries `unresolved` — the sentence the ASKING operation
    /// answers for that case, which is
    /// [`APP_CLIENT_NOT_FOUND_MESSAGE`] for the operations that address the app
    /// client itself and [`app_client_not_found_message`] for the ones that
    /// quote the id.
    ///
    /// Call it only where the lookup in `user_pool_id` has already failed.
    pub fn app_client_lookup_refusal(
        &self,
        user_pool_id: &str,
        client_id: &str,
        unresolved: &str,
    ) -> String {
        if self
            .pool_of_client(client_id)
            .is_some_and(|pool| pool.id != user_pool_id)
        {
            return app_client_not_found_in_pool_message(client_id, user_pool_id);
        }
        unresolved.to_string()
    }

    /// Whether the pool `user_pool_id` is stored and the domain `domain` is a
    /// stored domain of a DIFFERENT pool.
    ///
    /// A domain string names at most one pool ([`Self::domain`]), so an
    /// operation addressed by the PAIR `UserPoolId` + `Domain` can tell a pair
    /// that resolves nothing from a stored pool that is simply not the one
    /// configured with a domain some other pool holds, and the two are refused
    /// differently. A `UserPoolId` nothing answers to is the first case,
    /// whichever domain it is paired with.
    pub fn domain_of_other_pool(&self, user_pool_id: &str, domain: &str) -> bool {
        self.pool(user_pool_id).is_some()
            && self
                .domain(domain)
                .is_some_and(|stored| stored.user_pool_id != user_pool_id)
    }

    /// The stored user pool domain `domain` names, prefix or custom, or `None`
    /// when no pool in this emulator holds it.
    ///
    /// `DescribeUserPoolDomain` identifies a domain by `Domain` ALONE and models
    /// no `UserPoolId` member, so one domain string names at most one pool: the
    /// lookup spans every pool rather than one.
    pub fn domain(&self, domain: &str) -> Option<&UserPoolDomain> {
        self.user_pools
            .values()
            .find_map(|pool| pool.domain_named(domain))
    }

    /// Store a challenge session and return its token.
    ///
    /// Sessions the clock has already expired are swept first — the expiry was
    /// owed at this instant whether or not anybody called — and the oldest are
    /// evicted if the store is still at [`MAX_AUTH_SESSIONS`], so repeating the
    /// challenge cannot grow this map without limit.
    pub fn start_session(&mut self, now: SystemTime, session: AuthSession) -> String {
        self.auth_sessions.retain(|_, stored| stored.expires_at > now);
        evict_oldest_until(
            &mut self.auth_sessions,
            MAX_AUTH_SESSIONS.saturating_sub(1),
            |stored| stored.expires_at,
        );
        let token = new_session_token();
        self.auth_sessions.insert(token.clone(), session);
        token
    }

    /// Resolve the `Session` token a challenge response carries.
    ///
    /// Sessions the clock has already expired are swept first — the expiry was
    /// owed at this instant whether or not anybody called — which is what tells
    /// an expired session apart from one this emulator never handed out. A
    /// session a response already answered stays stored as SPENT until the
    /// clock expires it ([`Self::end_session`]), which is what tells a replay
    /// apart from a value this emulator never handed out. A session that is
    /// neither is still outstanding: a caller whose response was refused can
    /// send a corrected one while the session lives.
    pub fn session(&mut self, token: &str, now: SystemTime) -> SessionLookup {
        let expired = self
            .auth_sessions
            .get(token)
            .filter(|stored| stored.expires_at <= now)
            .cloned();
        self.auth_sessions.retain(|_, stored| stored.expires_at > now);
        match self.auth_sessions.get(token) {
            Some(stored) if stored.spent => SessionLookup::Spent(stored.clone()),
            Some(stored) => SessionLookup::Found(stored.clone()),
            None => match expired {
                Some(stored) => SessionLookup::Expired(stored),
                None => SessionLookup::Unknown,
            },
        }
    }

    /// Spend the challenge session `token` names: a session answers its
    /// challenge once.
    ///
    /// The record STAYS stored, marked spent, until the clock expires it — the
    /// sweeps in [`Self::session`] and [`Self::start_session`] are what drop it
    /// — so a caller replaying the session is answered as the replay it is
    /// rather than as a session this emulator never issued.
    pub fn end_session(&mut self, token: &str) {
        if let Some(stored) = self.auth_sessions.get_mut(token) {
            stored.spent = true;
        }
    }

    /// Store a passkey registration and return the CHALLENGE value its creation
    /// options carry, which is also the key the completing call resolves it by.
    ///
    /// Registrations the clock has already expired are swept first — the expiry
    /// was owed at this instant whether or not anybody called — and the oldest
    /// are evicted if the store is still at [`MAX_WEB_AUTHN_REGISTRATIONS`], so
    /// repeating the registration cannot grow this map without limit. A started
    /// registration leaves every registration started before it answerable: each
    /// one carries its own challenge.
    pub fn start_web_authn_registration(
        &mut self,
        now: SystemTime,
        registration: WebAuthnRegistration,
    ) -> String {
        self.web_authn_registrations
            .retain(|_, stored| stored.expires_at > now);
        evict_oldest_until(
            &mut self.web_authn_registrations,
            MAX_WEB_AUTHN_REGISTRATIONS.saturating_sub(1),
            |stored| stored.expires_at,
        );
        let challenge = new_web_authn_challenge();
        self.web_authn_registrations
            .insert(challenge.clone(), registration);
        challenge
    }

    /// The passkey registration `challenge` resolves as of `now`, or `None` when
    /// this emulator handed out no such challenge or the clock has already taken
    /// it past [`WebAuthnRegistration::expires_at`].
    ///
    /// A PURE read: an expired registration is refused where it stands rather
    /// than swept, so asking changes nothing a later caller sees.
    pub fn web_authn_registration(
        &self,
        challenge: &str,
        now: SystemTime,
    ) -> Option<&WebAuthnRegistration> {
        self.web_authn_registrations
            .get(challenge)
            .filter(|registration| registration.expires_at > now)
    }

    /// Forget the passkey registration `challenge` names: a challenge is answered
    /// once, so the call that completes a registration consumes it and a second
    /// response to the same challenge resolves nothing.
    pub fn end_web_authn_registration(&mut self, challenge: &str) {
        self.web_authn_registrations.remove(challenge);
    }

    /// The token record for `token`, or `None` when this emulator never issued it.
    pub fn token(&self, token: &str) -> Option<&TokenRecord> {
        self.issued_tokens.get(token)
    }

    /// Record that the refresh token `token` stops authorizing a token refresh at
    /// `invalid_from`, because a refresh with rotation enabled replaced it.
    ///
    /// `RefreshTokenRotationType.RetryGracePeriodSeconds`: "the original refresh
    /// token that you're rotating out can remain valid for a period of time of up
    /// to 60 seconds. This allows for client-side retries. When
    /// `RetryGracePeriodSeconds` is `0`, the grace period is disabled and a
    /// successful request immediately invalidates the submitted refresh token"
    /// (this service's model). A token already rotated out keeps the EARLIEST
    /// instant it was invalidated from, so retrying inside the grace period
    /// cannot push that window further out.
    pub fn rotate_out_refresh_token(&mut self, token: &str, invalid_from: SystemTime) {
        if let Some(record) = self.issued_tokens.get_mut(token) {
            record.rotated_out_at = Some(match record.rotated_out_at {
                Some(already) => already.min(invalid_from),
                None => invalid_from,
            });
        }
    }

    /// The stored pool and user that an ACCESS token identifies as the currently
    /// signed-in user, as of `now`.
    ///
    /// The self-service half of this API identifies its user by the `AccessToken`
    /// the sign-in returned and models no `UserPoolId` or `Username` member at
    /// all, so the stored token record is what names the pool and the user:
    /// `ListDevices.AccessToken` is "a valid access token that Amazon Cognito
    /// issued to the currently signed-in user" (this service's model). Only an
    /// access token answers — an id or refresh token names its user too, but it
    /// is not the token the member asks for.
    ///
    /// An access token the pool issued is then checked against what it
    /// AUTHORIZES: each of these operations "must include the scope
    /// `aws.cognito.signin.user.admin`" (this service's model), so a live access
    /// token whose scopes do not carry it — the machine-to-machine token of an
    /// app client, whose scopes authorize resource servers — is told apart from a
    /// value that resolves to no access token at all
    /// ([`TokenRecord::authorizes_user_self_service`]).
    ///
    /// A PURE read: a token the clock has already expired is refused where it
    /// stands rather than swept, so asking changes nothing a later caller sees.
    pub fn signed_in_user(
        &self,
        access_token: &str,
        now: SystemTime,
    ) -> Result<(&UserPool, &User), TokenRejection> {
        let record = self
            .issued_tokens
            .get(access_token)
            .ok_or(TokenRejection::NotAnAccessToken)?;
        if !record.is_access_token() {
            return Err(TokenRejection::NotAnAccessToken);
        }
        // A record this emulator issued and has since REVOKED — a sign-out of its
        // user, or a revocation of its sign-in lineage — is refused AS revoked,
        // which is a different answer from the one a value this service cannot
        // resolve to an access token at all gets.
        if record.revoked {
            return Err(TokenRejection::Revoked);
        }
        if record.expires_at <= now {
            return Err(TokenRejection::Expired);
        }
        // A live access token still has to AUTHORIZE this half of the API: one
        // whose scopes do not include the self-service scope identifies no
        // signed-in user here, whoever it was issued to.
        if !record.authorizes_user_self_service() {
            return Err(TokenRejection::MissingRequiredScopes);
        }
        let pool = self
            .pool(&record.user_pool_id)
            .ok_or(TokenRejection::PoolGone)?;
        let user = pool
            .user(&record.username)
            .ok_or(TokenRejection::UserGone)?;
        // The record points at the user by name, and a name can be taken again:
        // the stored user it finds is the one this token was issued to only when
        // it still carries the same id, so a token outliving its user resolves to
        // no user rather than to a later namesake.
        if user.sub != record.sub {
            return Err(TokenRejection::UserGone);
        }
        // A user Amazon Cognito is waiting on a new password from authorizes no
        // self-service call until that password is set: a live access token
        // issued before the reset was asked for still resolves to this user, and
        // the operations that resolve one all declare
        // `PasswordResetRequiredException` — "thrown when a password reset is
        // required" (this service's model) — so the stored status is read here,
        // once, rather than by each of them.
        if user.status == UserStatus::ResetRequired {
            return Err(TokenRejection::PasswordResetRequired);
        }
        Ok((pool, user))
    }

    /// Issue the access / id / refresh token triple for `username` on
    /// `client_id`, record all three, and return the modelled result.
    ///
    /// Every claim comes from stored state: the `sub` and `username` from the
    /// stored user, its `cognito:groups` from the groups the stored user belongs
    /// to, `client_id` from the stored app client, the issuer from the stored
    /// pool, and each lifetime from the app client's validity settings. The
    /// access and id tokens are signed with the POOL's key pair, so a verifier
    /// can check them against the key set that pool publishes; the refresh token
    /// is an OPAQUE credential ([`mint_refresh_token`]) that carries no claims at
    /// all. `None` when the pool, the client or the user is gone, when the
    /// signature could not be computed, or when no refresh credential was minted
    /// — never a token nothing signed.
    ///
    /// Tokens the clock has already expired are swept first, and the oldest are
    /// evicted if the store is still at [`MAX_ISSUED_TOKENS`], so repeated
    /// sign-ins cannot grow this map without limit.
    pub fn issue_tokens(
        &mut self,
        user_pool_id: &str,
        client_id: &str,
        username: &str,
        now: SystemTime,
        credentials: &::emulator_server::CredentialStore,
    ) -> Option<AuthenticationResultType> {
        let refresh_token = mint_refresh_token(credentials)?;
        self.mint_tokens(
            user_pool_id,
            client_id,
            username,
            now,
            Some(refresh_token),
            None,
        )
    }

    /// Issue a fresh triple that CONTINUES the sign-in lineage of
    /// `presented_refresh_token`, for a refresh that rotates the submitted
    /// refresh token out: the new tokens carry the submitted token's
    /// `origin_jti`, so they were "generated by" it (`RevokeToken` in this
    /// service's model) and a revocation naming either one reaches them all.
    /// Otherwise identical to [`issue_tokens`].
    pub fn issue_rotated_tokens(
        &mut self,
        user_pool_id: &str,
        client_id: &str,
        username: &str,
        now: SystemTime,
        presented_refresh_token: &str,
        credentials: &::emulator_server::CredentialStore,
    ) -> Option<AuthenticationResultType> {
        let lineage = self.token_lineage(presented_refresh_token);
        let refresh_token = mint_refresh_token(credentials)?;
        self.mint_tokens(
            user_pool_id,
            client_id,
            username,
            now,
            Some(refresh_token),
            lineage,
        )
    }

    /// Issue the access / id token pair for a sign-in that REUSES the refresh token the
    /// caller presented, record both, and return the modelled result.
    ///
    /// `InitiateAuth`'s `AuthFlow`: "REFRESH_TOKEN_AUTH and REFRESH_TOKEN — Receive new
    /// ID and access tokens when you pass a `REFRESH_TOKEN` parameter with a valid
    /// refresh token as the value" (this service's model). The refresh token in force
    /// stays the one the caller already holds, so none is minted and the result carries
    /// no `RefreshToken` member. Otherwise identical to [`issue_tokens`], whose claim
    /// building it shares.
    pub fn refresh_tokens(
        &mut self,
        user_pool_id: &str,
        client_id: &str,
        username: &str,
        now: SystemTime,
        presented_refresh_token: &str,
    ) -> Option<AuthenticationResultType> {
        let lineage = self.token_lineage(presented_refresh_token);
        self.mint_tokens(user_pool_id, client_id, username, now, None, lineage)
    }

    /// The sign-in lineage ([`TokenRecord::origin`]) of the token `token`, or
    /// `None` when this emulator issued no such token — in which case the tokens
    /// minted next start a lineage of their own.
    fn token_lineage(&self, token: &str) -> Option<String> {
        self.issued_tokens
            .get(token)
            .map(|record| record.origin.clone())
    }

    /// Issue the machine-to-machine access token of the app client `client_id`
    /// in `user_pool_id`, carrying `scopes`, record it, and return the modelled
    /// result.
    ///
    /// "Issues an access token for machine-to-machine (M2M) authorization. Your
    /// app client provides its client ID and secret, and receives an access
    /// token that authorizes requests to your resource servers"
    /// (`GetClientToken` in this service's model): the token is issued to the
    /// APP CLIENT, so its subject is that client's id, it carries no user claim
    /// and no id or refresh token is minted beside it. Its lifetime is the app
    /// client's own configured access-token validity — the same setting that
    /// bounds every other access token this client issues — and it is signed
    /// with the POOL's key pair and names that key by `kid`, so a resource
    /// server verifies it against the key set that pool publishes.
    ///
    /// `None` when the pool or the app client is gone, or when the signature
    /// could not be computed — never a token nothing signed.
    ///
    /// Tokens the clock has already expired are swept first, and the oldest are
    /// evicted if the store is still at [`MAX_ISSUED_TOKENS`], exactly as a
    /// sign-in's are.
    pub fn issue_client_token(
        &mut self,
        user_pool_id: &str,
        client_id: &str,
        scopes: &[String],
        now: SystemTime,
    ) -> Option<ClientAuthenticationResultType> {
        let (issuer, signing_key, access_ttl) = {
            let pool = self.user_pools.get(user_pool_id)?;
            let client = pool.clients.get(client_id)?;
            (
                pool.issuer(),
                pool.signing_key.clone(),
                client.access_token_ttl(),
            )
        };

        let issued_at = unix_seconds(now);
        let jti = new_uuid();
        // The expiry is derived from the app client's configured validity, which
        // a caller chose, so both sums are CHECKED and computed before anything
        // is stored: a validity whose expiry cannot be represented issues no
        // token at all rather than storing half a record.
        let expires_at = now.checked_add(access_ttl)?;
        let exp = expiry_claim(issued_at, access_ttl)?;
        let claims = serde_json::json!({
            "sub": client_id,
            "iss": issuer,
            "client_id": client_id,
            "token_use": "access",
            // An access token's granted scopes are carried as "a list of
            // space-delimited, case-sensitive strings" (RFC 6749 §3.3), in the
            // order the grant authorized them.
            "scope": scopes.join(" "),
            "iat": issued_at,
            "exp": exp,
            "jti": jti.clone(),
        });
        let access_token = signed_jws(&claims, &signing_key)?;

        self.issued_tokens
            .retain(|_, record| record.expires_at > now);
        evict_oldest_until(
            &mut self.issued_tokens,
            MAX_ISSUED_TOKENS.saturating_sub(1),
            |record| record.expires_at,
        );
        self.issued_tokens.insert(
            access_token.clone(),
            TokenRecord {
                user_pool_id: user_pool_id.to_string(),
                client_id: client_id.to_string(),
                // Issued to the app client, not to a user: the record names the
                // client as its subject and no user at all.
                username: String::new(),
                sub: client_id.to_string(),
                kind: TokenKind::ClientCredentials,
                // The scopes this grant authorized, which are what the token's
                // `scope` claim above carries.
                scopes: scopes.to_vec(),
                // Issued alone, with no `origin_jti` claim to share: its own
                // `jti` is its lineage, so no revocation of a sign-in's tokens
                // reaches it.
                origin: jti,
                issued_at: now,
                expires_at,
                revoked: false,
                rotated_out_at: None,
            },
        );

        Some(ClientAuthenticationResultType {
            access_token: Some(access_token),
            // "The number of seconds until the access token expires"
            // (`ClientAuthenticationResultType.ExpiresIn` in this service's
            // model).
            expires_in: i32::try_from(access_ttl.as_secs()).unwrap_or(i32::MAX),
            // "The type of the token. For example, `Bearer`"
            // (`ClientAuthenticationResultType.TokenType`), which is the type a
            // bearer access token of this API is presented as.
            token_type: Some("Bearer".to_string()),
        })
    }

    /// The one token-minting path behind [`issue_tokens`] and [`refresh_tokens`]:
    /// `refresh_token` is the opaque refresh credential to record and report beside the
    /// access and id tokens, and `None` for a refresh that reuses the one the caller
    /// already holds, so the two entry points cannot disagree about a claim.
    ///
    /// Every token it SIGNS is signed with the POOL's key pair and names that key by
    /// `kid`, so a verifier can check it against the key set the pool publishes; a token
    /// whose signature could not be computed is not issued at all ([`signed_jws`]). The
    /// refresh credential is signed by nothing and carries no claims: it is opaque to
    /// its holder, and the record this writes for it is the only place its sign-in is
    /// stated.
    ///
    /// `lineage` is the `origin_jti` the minted tokens join — the sign-in of the
    /// refresh token a refresh presented — and `None` starts a new one, which is
    /// what a first sign-in does.
    fn mint_tokens(
        &mut self,
        user_pool_id: &str,
        client_id: &str,
        username: &str,
        now: SystemTime,
        refresh_token: Option<String>,
        lineage: Option<String>,
    ) -> Option<AuthenticationResultType> {
        let TokenInputs {
            issuer,
            sub,
            stored_username,
            email,
            groups,
            role_claims,
            signing_key,
            access_ttl,
            id_ttl,
            refresh_ttl,
        } = {
            let pool = self.user_pools.get(user_pool_id)?;
            let client = pool.clients.get(client_id)?;
            let user = pool.user(username)?;
            TokenInputs {
                issuer: pool.issuer(),
                sub: user.sub.clone(),
                stored_username: user.username.clone(),
                email: user.attributes.get("email").cloned(),
                groups: pool.user_group_names(user),
                role_claims: pool.user_group_role_claims(user),
                signing_key: pool.signing_key.clone(),
                access_ttl: client.access_token_ttl(),
                id_ttl: client.id_token_ttl(),
                refresh_ttl: client.refresh_token_ttl(),
            }
        };

        let issued_at = unix_seconds(now);
        let origin_jti = lineage.unwrap_or_else(new_uuid);

        // Every expiry is derived from the app client's configured validity, which
        // the request chose, so each sum is computed with CHECKED arithmetic and
        // before anything is stored: a validity whose expiry cannot be represented
        // mints no token at all, the same answer this function already gives for a
        // token it could not sign, rather than panicking part-way through. The
        // refresh token's own expiry is computed only where that token is minted.
        let access_expires_at = now.checked_add(access_ttl)?;
        let id_expires_at = now.checked_add(id_ttl)?;
        let access_exp = expiry_claim(issued_at, access_ttl)?;
        let id_exp = expiry_claim(issued_at, id_ttl)?;

        // An access token of a sign-in authorizes the user self-service
        // operations, so it carries the one scope those operations require, and
        // the claim and the record read it from the same place.
        let access_scopes = vec![USER_ADMIN_SCOPE.to_string()];
        let mut access_claims = serde_json::json!({
            "sub": sub,
            "iss": issuer,
            "client_id": client_id,
            "username": stored_username,
            "token_use": "access",
            "scope": access_scopes.join(" "),
            "auth_time": issued_at,
            "iat": issued_at,
            "exp": access_exp,
            "jti": new_uuid(),
            "origin_jti": origin_jti,
        });
        let mut id_claims = serde_json::json!({
            "sub": sub,
            "iss": issuer,
            "aud": client_id,
            "cognito:username": stored_username,
            "token_use": "id",
            "auth_time": issued_at,
            "iat": issued_at,
            "exp": id_exp,
            "jti": new_uuid(),
            "origin_jti": origin_jti,
        });
        if let (Some(email), Some(members)) = (email, id_claims.as_object_mut()) {
            members.insert("email".to_string(), serde_json::Value::String(email));
        }

        // `AdminAddUserToGroup`: "A user who is in a group ... populates a
        // `cognito:groups` claim to their access and identity tokens" (this
        // service's model), so the claim is carried by both of those two tokens
        // and by neither the refresh token. A user in no group carries no claim
        // rather than an empty list. The model documents no order for the list,
        // so it is rendered in the stored order, which is stable across reads.
        if !groups.is_empty() {
            let claim = serde_json::Value::Array(
                groups
                    .iter()
                    .map(|name| serde_json::Value::String(name.clone()))
                    .collect(),
            );
            for claims in [&mut access_claims, &mut id_claims] {
                if let Some(members) = claims.as_object_mut() {
                    members.insert("cognito:groups".to_string(), claim.clone());
                }
            }
        }

        // The ROLE a group grants is the identity token's alone: "Amazon Cognito ID tokens
        // have a `cognito:preferred_role` claim that presents the highest-precedence group
        // that a user belongs to. Both ID and access tokens also contain a `cognito:groups`
        // claim" (`CreateGroup`'s `RoleArn`, this service's model). Which roles those are,
        // and whether the groups name one of them unambiguously, is the stored precedence's
        // decision (`UserPool::user_group_role_claims`); a user whose groups grant no role
        // carries neither claim rather than an empty list, and one whose best precedence is
        // tied between different roles carries the list without a preference.
        if let Some(members) = id_claims.as_object_mut() {
            if !role_claims.roles.is_empty() {
                members.insert(
                    "cognito:roles".to_string(),
                    serde_json::Value::Array(
                        role_claims
                            .roles
                            .iter()
                            .map(|role_arn| serde_json::Value::String(role_arn.clone()))
                            .collect(),
                    ),
                );
            }
            if let Some(preferred_role) = role_claims.preferred_role {
                members.insert(
                    "cognito:preferred_role".to_string(),
                    serde_json::Value::String(preferred_role),
                );
            }
        }

        let access_token = signed_jws(&access_claims, &signing_key)?;
        let id_token = signed_jws(&id_claims, &signing_key)?;

        let minted = 2 + usize::from(refresh_token.is_some());
        self.issued_tokens
            .retain(|_, record| record.expires_at > now);
        evict_oldest_until(
            &mut self.issued_tokens,
            MAX_ISSUED_TOKENS.saturating_sub(minted),
            |record| record.expires_at,
        );

        // A token's lifetime comes from the app client's stored validity
        // settings, which a caller sets, so every instant a token expires at is
        // the CHECKED sum computed above, before anything is stored: an
        // unrepresentable one answers no token at all rather than unwinding out
        // of this state mutation or leaving half a triple behind.
        let refresh_expires_at = match refresh_token.as_ref() {
            Some(_) => Some(now.checked_add(refresh_ttl)?),
            None => None,
        };
        let mut records = Vec::with_capacity(minted);
        for (token, kind, expires_at) in [
            Some((&access_token, TokenKind::Access, access_expires_at)),
            Some((&id_token, TokenKind::Id, id_expires_at)),
            refresh_token
                .as_ref()
                .zip(refresh_expires_at)
                .map(|(token, expires_at)| (token, TokenKind::Refresh, expires_at)),
        ]
        .into_iter()
        .flatten()
        {
            records.push((
                token.clone(),
                TokenRecord {
                    user_pool_id: user_pool_id.to_string(),
                    client_id: client_id.to_string(),
                    username: stored_username.clone(),
                    sub: sub.clone(),
                    kind,
                    // Only the access token of the triple carries a `scope`
                    // claim, so only its record holds scopes.
                    scopes: match kind {
                        TokenKind::Access => access_scopes.clone(),
                        _ => Vec::new(),
                    },
                    origin: origin_jti.clone(),
                    issued_at: now,
                    expires_at,
                    revoked: false,
                    rotated_out_at: None,
                },
            ));
        }
        for (token, record) in records {
            self.issued_tokens.insert(token, record);
        }

        Some(AuthenticationResultType {
            access_token: Some(access_token),
            // `ExpiresIn` is the access token's lifetime, in seconds.
            expires_in: i32::try_from(access_ttl.as_secs()).unwrap_or(i32::MAX),
            token_type: Some("Bearer".to_string()),
            refresh_token,
            id_token: Some(id_token),
            new_device_metadata: None,
        })
    }

    /// Record the device a completed sign-in presented and answer the
    /// `AuthenticationResult.NewDeviceMetadata` that sign-in owes.
    ///
    /// `DeviceConfigurationType`: "When you provide a value for any property of
    /// `DeviceConfiguration`, you activate the device remembering for the user
    /// pool", and `NewDeviceMetadataType` is "Information that your user pool
    /// responds with in `AuthenticationResult` when you configure it to remember
    /// devices and a user signs in with an unrecognized device" (this service's
    /// model). So a pool that stores NO `DeviceConfiguration` answers `None`,
    /// whatever the sign-in presented, and so does a sign-in that presented a
    /// `DEVICE_KEY` this user already holds — that device is recognized, and only
    /// its last-authenticated instant moves. Anything else is an unrecognized
    /// device: a device is stored for the user and its key, beside the pool's
    /// device group key, is what the caller gets back. A key the user does not
    /// hold — another user's, or one nothing minted — is unrecognized by this
    /// user pool for this user, so it is answered the same way as none at all
    /// rather than refusing the sign-in the first factor already passed.
    ///
    /// The stored device is NOT confirmed yet: `ConfirmDevice` is what confirms
    /// it and decides whether it is remembered.
    pub fn track_sign_in_device(
        &mut self,
        user_pool_id: &str,
        username: &str,
        presented_device_key: Option<&str>,
        now: SystemTime,
    ) -> Option<NewDeviceMetadataType> {
        let pool = self.user_pools.get_mut(user_pool_id)?;
        // A pool that provided no `DeviceConfiguration` activated no device
        // remembering, so it answers nothing whatever the sign-in presented.
        pool.device_configuration.as_ref()?;
        let region = pool.region.clone();
        let device_group_key = pool.device_group_key.clone();
        let user = pool.user_mut(username)?;
        if let Some(device) = presented_device_key.and_then(|key| user.devices.get_mut(key)) {
            device.last_authenticated_date = Some(now);
            return None;
        }
        let device_key = new_device_key(&region);
        user.store_device(Device::signed_in(device_key.clone(), now));
        Some(NewDeviceMetadataType {
            device_key: Some(device_key),
            device_group_key: Some(device_group_key),
        })
    }

    /// Forget the outstanding sign-in state of `username` in `user_pool_id`:
    /// the challenge sessions and the passkey registrations that could still
    /// carry a sign-in to completion. Called when the user they belong to is
    /// deleted, which is why a deleted user profile "can no longer be used to
    /// sign in" (`DeleteUser` in this service's model).
    ///
    /// The tokens already ISSUED to that user stay stored and keep naming it:
    /// they are self-contained bearer tokens their holder still has, so one
    /// presented after the profile is gone resolves the user it names and finds
    /// nothing. That is what makes a call made with such a token a refusal for
    /// the user that is gone ([`TokenRejection::UserGone`]) rather than a
    /// rejection of the token itself — and the user it names being gone, the
    /// token authorizes nothing. What bounds the record map is the same thing
    /// that bounds it for a live user: [`Self::mint_tokens`] sweeps every record
    /// the clock has expired and evicts the oldest at [`MAX_ISSUED_TOKENS`].
    pub fn forget_user_sign_in_state(&mut self, user_pool_id: &str, username: &str) {
        self.auth_sessions
            .retain(|_, s| !(s.user_pool_id == user_pool_id && s.username == username));
        self.web_authn_registrations
            .retain(|_, r| !(r.user_pool_id == user_pool_id && r.username == username));
    }

    /// Revoke every token this emulator issued to `username` in `user_pool_id`,
    /// and forget its outstanding challenge sessions.
    ///
    /// A revoked record stays stored and stays refused: the token checks reject a
    /// revoked token wherever one is presented, so a token a caller still holds
    /// authorizes nothing after the user it names is deactivated.
    pub fn revoke_user_tokens(&mut self, user_pool_id: &str, username: &str) {
        self.auth_sessions
            .retain(|_, s| !(s.user_pool_id == user_pool_id && s.username == username));
        for record in self.issued_tokens.values_mut() {
            if record.user_pool_id == user_pool_id && record.username == username {
                record.revoked = true;
            }
        }
    }

    /// Revoke every token of the sign-in lineage `origin` in `user_pool_id`:
    /// the refresh token that lineage was minted with, the access and id tokens
    /// minted beside it, and every token a refresh with it issued.
    ///
    /// "Revokes all of the access tokens generated by, and at the same time as,
    /// the specified refresh token. After a token is revoked, you can't use the
    /// revoked token to access Amazon Cognito user APIs, or to authorize access
    /// to your resource server" (`RevokeToken` in this service's model), so a
    /// revoked record stays stored and stays refused wherever the token it
    /// records is later presented, exactly as a signed-out user's does.
    pub fn revoke_token_lineage(&mut self, user_pool_id: &str, origin: &str) {
        for record in self.issued_tokens.values_mut() {
            if record.user_pool_id == user_pool_id && record.origin == origin {
                record.revoked = true;
            }
        }
    }

    /// Forget every session, token and passkey registration issued in
    /// `user_pool_id`. Called when the pool they belong to is deleted.
    pub fn forget_pool_credentials(&mut self, user_pool_id: &str) {
        self.auth_sessions
            .retain(|_, s| s.user_pool_id != user_pool_id);
        self.issued_tokens
            .retain(|_, t| t.user_pool_id != user_pool_id);
        self.web_authn_registrations
            .retain(|_, r| r.user_pool_id != user_pool_id);
    }

    /// Forget every outstanding challenge session started for the app client
    /// `client_id` of `user_pool_id`. Called when that app client is deleted: a
    /// response to such a challenge carries the `ClientId` the pool no longer
    /// holds, so the session can never be answered.
    ///
    /// The tokens the client issued stay stored and keep resolving the user they
    /// name until the clock expires them, exactly as an already-issued token
    /// does; a refresh is refused where it stands, because the app client the
    /// request has to name is gone.
    pub fn forget_client_sessions(&mut self, user_pool_id: &str, client_id: &str) {
        self.auth_sessions
            .retain(|_, s| !(s.user_pool_id == user_pool_id && s.client_id == client_id));
    }
}

/// One stored user pool, with the app clients and users it owns.
#[derive(Debug, Clone)]
pub struct UserPool {
    pub id: String,
    pub name: String,
    pub arn: String,
    pub region: String,
    pub creation_date: SystemTime,
    pub last_modified_date: SystemTime,
    pub policies: UserPoolPolicyType,
    /// This pool's Lambda triggers, EMPTY for a pool that configures none: the
    /// service materialises the configuration every pool applies, so this is
    /// stored rather than left absent ([`effective_lambda_config`]).
    pub lambda_config: LambdaConfigType,
    pub mfa_configuration: String,
    pub deletion_protection: String,
    pub user_pool_tier: String,
    pub username_attributes: Vec<String>,
    pub alias_attributes: Vec<String>,
    pub auto_verified_attributes: Vec<String>,
    pub schema: Vec<SchemaAttributeType>,
    pub admin_create_user_config: AdminCreateUserConfigType,
    pub username_configuration: Option<UsernameConfigurationType>,
    pub device_configuration: Option<DeviceConfigurationType>,
    /// How this pool sends its email messages ([`effective_email_configuration`]).
    pub email_configuration: EmailConfigurationType,
    pub sms_configuration: Option<SmsConfigurationType>,
    /// The templates this pool's verification messages are rendered from
    /// ([`effective_verification_message_template`]).
    pub verification_message_template: VerificationMessageTemplateType,
    pub account_recovery_setting: Option<AccountRecoverySettingType>,
    /// Which attribute changes wait on verification
    /// ([`effective_user_attribute_update_settings`]).
    pub user_attribute_update_settings: UserAttributeUpdateSettingsType,
    pub user_pool_add_ons: Option<UserPoolAddOnsType>,
    /// The key this pool's data is encrypted under
    /// ([`effective_key_configuration`]).
    pub key_configuration: KeyConfigurationType,
    /// The issuer this pool's tokens carry ([`effective_issuer_configuration`]).
    pub issuer_configuration: IssuerConfigurationType,
    pub acr_configuration: Option<HashMap<String, AcrLevelConfigType>>,
    pub sms_authentication_message: Option<String>,
    /// This pool's TOTP MFA configuration: "Settings for time-based one-time
    /// password (TOTP) multi-factor authentication (MFA) in a user pool"
    /// (`SoftwareTokenMfaConfigType`). `None` until a call configures TOTP, so
    /// a pool that never did reports no TOTP configuration.
    pub software_token_mfa_configuration: Option<SoftwareTokenMfaConfigType>,
    /// This pool's email-message MFA configuration: the subject and body of the
    /// message template sign-in and MFA codes are delivered with
    /// (`EmailMfaConfigType`). `None` until a call configures it.
    pub email_mfa_configuration: Option<EmailMfaConfigType>,
    /// This pool's passkey configuration: relying-party id, user-verification
    /// requirement and whether a passkey satisfies MFA
    /// (`WebAuthnConfigurationType`). `None` until a call configures it.
    pub web_authn_configuration: Option<WebAuthnConfigurationType>,
    /// This pool's threat-protection settings, once a call has set them:
    /// `RiskConfigurationType.ClientId` is "the app client where this
    /// configuration is applied. When this parameter isn't present, the risk
    /// configuration applies to all user pool app clients that don't have
    /// client-level settings" — so this is the pool-level configuration, the one
    /// every app client of the pool that holds none of its own uses. `None`
    /// until a call sets it, so a pool nobody configured reports no settings and
    /// no modification instant.
    pub risk_configuration: Option<RiskConfiguration>,
    /// This pool's log delivery configuration: the `LogConfigurations` a
    /// `SetLogDeliveryConfiguration` call last stored, which is what
    /// `GetLogDeliveryConfiguration` reports. Empty for a pool nothing has
    /// configured logging for, and empty again once a call sets an empty
    /// collection — `LogDeliveryConfigurationType.LogConfigurations` is a
    /// modelled required member, so the collection is always reported, empty or
    /// not.
    pub log_configurations: Vec<LogConfigurationType>,
    pub tags: HashMap<String, String>,
    /// This pool's Amazon Cognito prefix domain, once one is created:
    /// `UserPoolType.Domain` is "The domain prefix, if the user pool has a
    /// domain associated with it" — a single value, so a pool holds at most one.
    pub domain: Option<UserPoolDomain>,
    /// This pool's custom domain, once one is created: `UserPoolType.CustomDomain`
    /// is a single value too, so a pool holds at most one of those as well.
    pub custom_domain: Option<UserPoolDomain>,
    /// This pool's SECONDARY replicas, keyed by the Region each was created in
    /// — one Region holds at most one replica of a pool, and the key is what
    /// `UpdateUserPoolReplica` and `DeleteUserPoolReplica` name. Ascending by
    /// Region, which is the order a listing of them serves. The PRIMARY replica
    /// is the pool itself in [`UserPool::region`], so it is not stored a second
    /// time here.
    pub replicas: BTreeMap<String, UserPoolReplica>,
    pub clients: HashMap<String, UserPoolClient>,
    /// Users keyed by [`UserPool::user_key`], so a pool configured
    /// case-insensitive treats `Alice` and `alice` as one user.
    pub users: HashMap<String, User>,
    /// This pool's groups keyed by `GroupName`, ordered so a read of several is
    /// stable. `GroupNameType` carries no case-insensitivity trait, so a name is
    /// matched exactly as it was created.
    pub groups: BTreeMap<String, Group>,
    /// This pool's OAuth 2.0 resource servers keyed by their `Identifier`,
    /// ordered so a read of several is stable. `ResourceServerIdentifierType`
    /// carries no case-insensitivity trait, so an identifier is matched exactly
    /// as it was created.
    pub resource_servers: BTreeMap<String, ResourceServer>,
    /// This pool's third-party identity providers keyed by their `ProviderName`,
    /// ordered so a read of several is stable. `ProviderNameType` carries no
    /// case-insensitivity trait, so a name is matched exactly as it was created.
    pub identity_providers: BTreeMap<String, IdentityProvider>,
    /// This pool's terms documents keyed by their minted `TermsId`, ordered so a
    /// read of several is stable. A terms document belongs to one app client of
    /// this pool, and "terms document names must be unique to the app client"
    /// (`TermsExistsException` in this service's model), so the (`ClientId`,
    /// `TermsName`) pair is unique across this map too. No separate bound is
    /// needed: `TermsNameType` admits two names and a document names a stored
    /// app client, so this map holds at most twice [`MAX_CLIENTS_PER_POOL`].
    pub terms: BTreeMap<String, Terms>,
    /// This pool's managed login branding styles keyed by their minted
    /// `ManagedLoginBrandingId`, ordered so a read of several is stable. A style
    /// "is linked to an app client until you delete it"
    /// (`CreateManagedLoginBranding.ClientId`) and an app client "that already
    /// has an assigned style" refuses a second one
    /// (`ManagedLoginBrandingExistsException`), so `ClientId` identifies at most
    /// one entry of this map and it holds at most [`MAX_CLIENTS_PER_POOL`]
    /// styles.
    pub managed_login_branding: BTreeMap<String, ManagedLoginBranding>,
    /// This pool's classic hosted UI branding keyed by the app client it
    /// customizes, with [`UI_CUSTOMIZATION_ALL_CLIENTS`] holding the pool-level
    /// default: "Set the default configuration for all clients with a ClientId
    /// of ALL. When the ClientId value is an app client ID, the settings you
    /// pass in this request apply to that app client and override the default
    /// ALL configuration" (`SetUICustomization`), so one entry per level and at
    /// most one more than [`MAX_CLIENTS_PER_POOL`]. Ordered, so a read of
    /// several is stable.
    pub ui_customizations: BTreeMap<String, UiCustomization>,
    /// This pool's user import jobs keyed by their minted `JobId`, ordered so a
    /// read of several is stable: "Import jobs are retained in user pool
    /// configuration so that you can stage, stop, start, review, and delete
    /// them" (`ListUserImportJobs`).
    pub user_import_jobs: BTreeMap<String, UserImportJob>,
    /// The key pair this pool's tokens are signed with and whose public half
    /// its key set publishes, minted with the pool and kept for its lifetime,
    /// so every token the pool ever issued stays verifiable against the key set
    /// it publishes now.
    pub signing_key: PoolSigningKey,
    /// The `DeviceGroupKey` every device of this pool is reported under:
    /// "an identifier used in generating the `DEVICE_PASSWORD_VERIFIER` for
    /// device SRP authentication" (`NewDeviceMetadataType.DeviceGroupKey`).
    /// Minted with the pool and kept for its lifetime, so two sign-ins of the
    /// same pool report the same group key.
    pub device_group_key: String,
}

/// A user pool's MFA and passkey configuration, as the two operations that
/// report it render it: `MfaConfiguration` beside the configuration of each
/// factor the pool offers.
///
/// Built by [`UserPool::to_mfa_config`] from what the pool holds. It exists
/// because `GetUserPoolMfaConfig` and `SetUserPoolMfaConfig` model these same
/// five members in two distinct generated output structs, which would otherwise
/// assemble the stored configuration twice.
#[derive(Debug, Clone)]
pub struct MfaConfig {
    pub sms_mfa_configuration: Option<SmsMfaConfigType>,
    pub software_token_mfa_configuration: Option<SoftwareTokenMfaConfigType>,
    pub email_mfa_configuration: Option<EmailMfaConfigType>,
    pub mfa_configuration: String,
    pub web_authn_configuration: Option<WebAuthnConfigurationType>,
}

/// The threat-protection settings stored at one level — a user pool, or one app
/// client of it — beside the instant the last call that set them ran.
///
/// The three settings are exactly the ones `SetRiskConfiguration` carries, and
/// `RiskConfigurationType` reports them back with the level they were stored at
/// (the pool id alone, or the pool id and the app client id) and
/// `LastModifiedDate`, "the date and time when the item was modified".
///
/// A setting this struct holds as `None` is one no stored call configured: the
/// operation SETS the whole configuration of the level it names, so a request
/// that carries none of the three leaves a configuration with nothing in it —
/// which is what "to set the risk configuration for the user pool to defaults,
/// send this request with only the `UserPoolId` parameter" asks for.
#[derive(Debug, Clone)]
pub struct RiskConfiguration {
    pub compromised_credentials_risk_configuration:
        Option<CompromisedCredentialsRiskConfigurationType>,
    pub account_takeover_risk_configuration: Option<AccountTakeoverRiskConfigurationType>,
    pub risk_exception_configuration: Option<RiskExceptionConfigurationType>,
    pub last_modified_date: SystemTime,
}

impl RiskConfiguration {
    /// The stored settings as the two operations that report them render them:
    /// `user_pool_id` and `client_id` name the level they are stored at, so a
    /// client-level configuration carries the app client it applies to and a
    /// pool-level one carries none.
    pub fn to_risk_configuration_type(
        &self,
        user_pool_id: &str,
        client_id: Option<&str>,
    ) -> RiskConfigurationType {
        RiskConfigurationType {
            user_pool_id: Some(user_pool_id.to_string()),
            client_id: client_id.map(str::to_string),
            compromised_credentials_risk_configuration: self
                .compromised_credentials_risk_configuration
                .clone(),
            account_takeover_risk_configuration: self.account_takeover_risk_configuration.clone(),
            risk_exception_configuration: self.risk_exception_configuration.clone(),
            last_modified_date: Some(to_date_time(self.last_modified_date)),
        }
    }
}

impl UserPool {
    /// The token issuer for this pool: the regional user-pool endpoint.
    pub fn issuer(&self) -> String {
        format!(
            "https://{SIGNING_NAME}.{}.amazonaws.com/{}",
            self.region, self.id
        )
    }

    /// Where this pool publishes the key set its tokens are verified against:
    /// `<issuer>/.well-known/jwks.json`, the location the token's `iss` claim
    /// leads a verifier to (Amazon Cognito Developer Guide, "Verifying a JSON
    /// web token").
    pub fn jwks_uri(&self) -> String {
        format!("{}/.well-known/jwks.json", self.issuer())
    }

    /// This pool's token-signing key pair, minted when the pool was created.
    pub fn signing_key(&self) -> &PoolSigningKey {
        &self.signing_key
    }

    /// This pool's JSON Web Key Set (RFC 7517 §5): the public half of every key
    /// its tokens are signed with, each entry found by the `kid` the token
    /// header carries. This is the document a verifier reads from
    /// [`UserPool::jwks_uri`], and it holds only PUBLIC key parameters.
    ///
    /// A pure read: it renders the stored key and changes nothing.
    pub fn jwks(&self) -> serde_json::Value {
        serde_json::json!({ "keys": [self.signing_key.to_jwk()] })
    }

    /// This pool with a freshly minted id, and the ARN that id implies. Used
    /// when a minted id turns out to name a pool that already exists.
    #[must_use]
    pub fn with_new_id(mut self, region: &str) -> Self {
        self.id = new_pool_id(region);
        self.arn = ::engine::emulator_arn(SIGNING_NAME, region, &format!("userpool/{}", self.id));
        self
    }

    /// Whether usernames in this pool are compared case-sensitively.
    ///
    /// `UsernameConfiguration.CaseSensitive` is the pool's setting; a pool that
    /// sets none compares case-sensitively.
    pub fn case_sensitive(&self) -> bool {
        self.username_configuration
            .as_ref()
            .is_none_or(|configuration| configuration.case_sensitive)
    }

    /// The key `username` is stored under in [`UserPool::users`].
    pub fn user_key(&self, username: &str) -> String {
        if self.case_sensitive() {
            username.to_string()
        } else {
            username.to_lowercase()
        }
    }

    /// The stored user `username` names, by username, by `sub` or by a sign-in
    /// name of this pool ([`UserPool::sign_in_name_holder_key`]).
    pub fn user(&self, username: &str) -> Option<&User> {
        let key = self.user_storage_key(username)?;
        self.users.get(&key)
    }

    /// The key the stored user `username` names is held under, or `None` when no
    /// such user exists.
    ///
    /// Every way a request may name one user of this pool, in the order a
    /// stored name beats a derived one: the username itself, the user's own
    /// `sub`, then any sign-in name of the pool the value is. A `Username` "is
    /// typically your user's username, but it can be any of their alias
    /// attributes. If username isn't an alias attribute in your user pool, this
    /// value must be the sub of a local user" (`AdminGetUser.Username` in this
    /// service's model), so all three resolve the same stored user.
    pub fn user_storage_key(&self, username: &str) -> Option<String> {
        let key = self.user_key(username);
        if self.users.contains_key(&key) {
            return Some(key);
        }
        if let Some(stored_key) = self
            .users
            .iter()
            .find(|(_, user)| user.sub == username)
            .map(|(stored_key, _)| stored_key.clone())
        {
            return Some(stored_key);
        }
        if let Some(stored_key) = self.federated_user_key(username) {
            return Some(stored_key);
        }
        self.sign_in_name_holder_key(username)
    }

    /// The key of the user this pool holds the THIRD-PARTY IdP username
    /// `username`'s link on, or `None` when no user of this pool holds one.
    ///
    /// A `Username` "must be the sub of a local user or the username of a user
    /// from a third-party IdP" (`AdminGetUser.Username` in this service's
    /// model), and a third-party user's username is that of its provider and
    /// its id with that provider ([`federated_username`]). An external identity
    /// `AdminLinkProviderForUser` linked belongs to the local profile it was
    /// linked to — "Amazon Cognito returns this user when the new user (with the
    /// linked IdP attribute) signs in" (`AdminLinkProviderForUser.DestinationUser`)
    /// — so that profile is the user the federated username names. The name is
    /// compared the way this pool compares usernames
    /// ([`UserPool::user_key`]), so a case-insensitive pool resolves it in any
    /// case and a case-sensitive one only as the link stored it. The
    /// lowest-ordered key answers, so the lookup is stable whatever order the
    /// users are held in.
    pub fn federated_user_key(&self, username: &str) -> Option<String> {
        let asked_for = self.user_key(username);
        self.users
            .iter()
            .filter(|(_, user)| {
                user.provider_links.keys().any(|identity| {
                    self.user_key(&federated_username(
                        &identity.provider_name,
                        &identity.provider_attribute_value,
                    )) == asked_for
                })
            })
            .map(|(key, _)| key.clone())
            .min()
    }

    /// The key of the user whose SIGN-IN NAME `value` is, or `None` when no user
    /// of this pool answers to it.
    ///
    /// `UsernameAttributes` "specifies whether a user can use an email address
    /// or phone number as a username when they sign up", while `AliasAttributes`
    /// are "attributes supported as an alias for this user pool" (this service's
    /// model). A username attribute's value IS the name the user signs in with,
    /// so it resolves whatever its verified flag says; an alias attribute's
    /// value resolves only while it is the user's ACTIVE alias
    /// ([`User::alias_is_active`]), since an unconfirmed alias names no user
    /// yet. At most one user holds a given sign-in value, and the lowest-ordered
    /// key answers, so the lookup is stable whatever order the users are held
    /// in. The work is bounded by the attributes the POOL configures, not by
    /// anything the caller sends.
    pub fn sign_in_name_holder_key(&self, value: &str) -> Option<String> {
        let username_names = self
            .username_attributes
            .iter()
            .map(|name| (name.as_str(), false));
        let alias_names = self
            .alias_attributes
            .iter()
            .map(|name| (name.as_str(), true));
        for (name, only_while_active) in username_names.chain(alias_names) {
            let holder = self
                .users
                .iter()
                .filter(|(_, user)| {
                    user.attributes
                        .get(name)
                        .is_some_and(|stored| stored == value)
                        && (!only_while_active || user.alias_is_active(name))
                })
                .map(|(key, _)| key.clone())
                .min();
            if holder.is_some() {
                return holder;
            }
        }
        None
    }

    /// The attribute a pool whose sign-in name IS an attribute value stores
    /// `submitted` under, or `None` for a pool whose users sign in with a
    /// username of their own.
    ///
    /// "If your user pool only supports phone numbers or email addresses as
    /// sign-in attributes, Amazon Cognito automatically generates a username
    /// value" (`AdminCreateUser.Username` in this service's model), so in such a
    /// pool the submitted value is not a username at all: it is the value of the
    /// sign-in attribute it has the shape of — an address for `email`, a number
    /// for `phone_number` ([`attribute_shape_of`] judges which). A value of no
    /// configured sign-in attribute's shape is no sign-in value of this pool, so
    /// none is reported and nothing is stored under an attribute the value does
    /// not belong to.
    pub fn sign_in_attribute(&self, submitted: &str) -> Option<&str> {
        let shape = attribute_shape_of(submitted)?;
        self.username_attributes
            .iter()
            .map(String::as_str)
            .find(|name| *name == shape)
    }

    /// Whether this pool remembers devices at all: `DeviceConfigurationType` is
    /// "the device-remembering configuration for a user pool", and "when you
    /// provide a value for any property of `DeviceConfiguration`, you activate
    /// the device remembering for the user pool" (this service's model), so a
    /// pool that stores none has no device surface to serve.
    pub fn remembers_devices(&self) -> bool {
        self.device_configuration.is_some()
    }

    /// The names of this pool's groups `user` belongs to, in stored (name) order.
    ///
    /// Only groups the pool still HOLDS are reported: `DeleteGroup` documents that
    /// a deleted group "no longer contributes to users' `cognito:preferred_group`
    /// or `cognito:groups` claims", so a membership of a group that is gone is not
    /// rendered.
    pub fn user_group_names(&self, user: &User) -> Vec<String> {
        user.groups
            .iter()
            .filter(|name| self.groups.contains_key(*name))
            .cloned()
            .collect()
    }

    /// The role claims this pool's groups put in `user`'s identity token: every role its
    /// groups grant, best precedence first, and the ONE role the best-precedence group
    /// prefers where the groups name it unambiguously.
    ///
    /// `CreateGroup`'s own `Precedence` documentation is the rule (this service's model):
    /// "Zero is the highest precedence value. Groups with lower `Precedence` values take
    /// precedence over groups with higher or null `Precedence` values. If a user belongs to
    /// two or more groups, it is the group with the lowest precedence value whose role ARN is
    /// given in the user's tokens for the `cognito:roles` and `cognito:preferred_role`
    /// claims. Two groups can have the same `Precedence` value. If this happens, neither
    /// group takes precedence over the other. If two groups with the same `Precedence` have
    /// the same role ARN, that role is used in the `cognito:preferred_role` claim in tokens
    /// for users in each group. If the two groups have different role ARNs, the
    /// `cognito:preferred_role` claim isn't set."
    ///
    /// So a group that grants NO role contributes to neither claim, a stated precedence
    /// ranks ahead of none at all, and the preference is withheld exactly when the groups
    /// tied at the best precedence disagree about the role. Only groups the pool still
    /// HOLDS count, for the reason [`UserPool::user_group_names`] states.
    pub fn user_group_role_claims(&self, user: &User) -> GroupRoleClaims {
        // Stable-sorted by precedence alone, so groups tied at one precedence keep their
        // stored (name) order and a read renders the same claim twice.
        let mut granting: Vec<(PrecedenceRank, &str)> = user
            .groups
            .iter()
            .filter_map(|name| self.groups.get(name))
            .filter_map(|group| {
                group
                    .role_arn
                    .as_deref()
                    .map(|role_arn| (precedence_rank(group.precedence), role_arn))
            })
            .collect();
        granting.sort_by_key(|(rank, _)| *rank);

        let mut roles: Vec<String> = Vec::new();
        for (_, role_arn) in &granting {
            if !roles.iter().any(|held| held == role_arn) {
                roles.push((*role_arn).to_string());
            }
        }
        let preferred_role = granting.first().and_then(|(best, role_arn)| {
            granting
                .iter()
                .take_while(|(rank, _)| rank == best)
                .all(|(_, tied)| tied == role_arn)
                .then(|| (*role_arn).to_string())
        });
        GroupRoleClaims {
            roles,
            preferred_role,
        }
    }

    /// The keys in [`UserPool::users`] of the users that belong to
    /// `group_name`, in ascending key order — the stable order a paginated
    /// member listing indexes into.
    ///
    /// `GroupNameType` carries no case-insensitivity trait, so the name is
    /// matched exactly as it was created, and a user is reported only while its
    /// stored membership names that group.
    pub fn group_member_keys(&self, group_name: &str) -> Vec<String> {
        let mut keys: Vec<String> = self
            .users
            .iter()
            .filter(|(_, user)| user.groups.contains(group_name))
            .map(|(key, _)| key.clone())
            .collect();
        keys.sort();
        keys
    }

    /// The storage key of the user `identity` is linked to, or `None` when no
    /// user of this pool holds a link for it.
    ///
    /// A provider identity links to ONE local profile — `AdminLinkProviderForUser`
    /// links a `SourceUser` to "an existing user account in a user pool, or
    /// `DestinationUser`" (this service's model) — so at most one key answers;
    /// the lowest-ordered one is taken, so the lookup is stable whatever order
    /// the users are held in.
    pub fn linked_user_key(&self, identity: &ProviderIdentity) -> Option<String> {
        self.users
            .iter()
            .filter(|(_, user)| user.provider_links.contains_key(identity))
            .map(|(key, _)| key.clone())
            .min()
    }

    /// The stored user `username` names, mutably.
    pub fn user_mut(&mut self, username: &str) -> Option<&mut User> {
        let key = self.user_storage_key(username)?;
        self.users.get_mut(&key)
    }

    /// Write `plan` ([`plan_attribute_update`]) onto this pool as of `now` and
    /// return the delivery of each verification message it sent, or `None` when
    /// the user it applies to is no longer stored.
    ///
    /// The plan's own user takes its attributes
    /// ([`User::apply_attribute_update`]), and every OTHER holder of an alias
    /// value the plan moves loses the verified flag that made that value
    /// theirs: an alias names ONE user of the pool, so the write that gives the
    /// value to this user is the write that takes it from them.
    pub fn apply_attribute_update(
        &mut self,
        plan: &AttributeUpdatePlan,
        now: SystemTime,
    ) -> Option<Vec<CodeDeliveryDetailsType>> {
        for (key, flag) in &plan.demotions {
            if let Some(holder) = self.users.get_mut(key) {
                holder
                    .attributes
                    .insert((*flag).to_string(), FLAG_UNCONFIRMED.to_string());
                holder.user_last_modified_date = now;
            }
        }
        let user = self.user_mut(&plan.username)?;
        Some(user.apply_attribute_update(plan, now))
    }

    /// The effective password policy: the pool's own, with the documented
    /// defaults filled in for what it leaves unset.
    pub fn password_policy(&self) -> PasswordPolicyType {
        let mut policy = self
            .policies
            .password_policy
            .clone()
            .unwrap_or_else(default_password_policy);
        if policy.minimum_length.is_none() {
            policy.minimum_length = Some(DEFAULT_PASSWORD_MIN_LENGTH);
        }
        if policy.temporary_password_validity_days == 0 {
            policy.temporary_password_validity_days = DEFAULT_TEMPORARY_PASSWORD_VALIDITY_DAYS;
        }
        policy
    }

    /// `None` when `password` satisfies this pool's password policy, else the
    /// requirement it misses — the message a client has to fix the request
    /// from, worded as the asking operation's own API words it
    /// ([`PasswordPolicyWording`]).
    ///
    /// The message is the fixed clause for the requirement missed, behind that
    /// wording's prefix: it reports WHICH requirement the password failed and
    /// never the value this pool configured for it.
    pub fn check_password(
        &self,
        password: &str,
        wording: PasswordPolicyWording,
    ) -> Option<String> {
        let policy = self.password_policy();
        let minimum = policy.minimum_length.unwrap_or(DEFAULT_PASSWORD_MIN_LENGTH);
        if i64::try_from(password.chars().count()).unwrap_or(i64::MAX) < i64::from(minimum) {
            return Some(password_policy_message(wording, PASSWORD_TOO_SHORT));
        }
        if policy.require_uppercase && !password.chars().any(char::is_uppercase) {
            return Some(password_policy_message(wording, PASSWORD_NEEDS_UPPERCASE));
        }
        if policy.require_lowercase && !password.chars().any(char::is_lowercase) {
            return Some(password_policy_message(wording, PASSWORD_NEEDS_LOWERCASE));
        }
        if policy.require_numbers && !password.chars().any(|c| c.is_ascii_digit()) {
            return Some(password_policy_message(wording, PASSWORD_NEEDS_NUMBERS));
        }
        if policy.require_symbols && !password.chars().any(is_symbol) {
            return Some(password_policy_message(wording, PASSWORD_NEEDS_SYMBOLS));
        }
        None
    }

    /// Whether this pool's password policy restricts `user` from reusing
    /// `password`.
    ///
    /// "The number of previous passwords that you want Amazon Cognito to
    /// restrict each user from reusing. Users can't set a password that matches
    /// any of `n` previous passwords, where `n` is the value of
    /// `PasswordHistorySize`" (`PasswordPolicyType.PasswordHistorySize` in this
    /// service's model): the restricted set is the `n` passwords this user set
    /// most recently — the one it holds now among them, being the one set last
    /// ([`User::recent_passwords`]) — so a password set further back than the
    /// window reaches can be set again. A pool whose policy configures no size
    /// restricts none, which is what a pool created with the default policy
    /// does.
    pub fn restricts_password_reuse(&self, user: &User, password: &str) -> bool {
        let restricted = self
            .password_policy()
            .password_history_size
            .and_then(|size| usize::try_from(size).ok())
            .unwrap_or(0);
        user.recent_passwords()
            .take(restricted)
            .any(|previous| previous == password)
    }

    /// How long a temporary password this pool issues stays usable.
    pub fn temporary_password_validity(&self) -> Duration {
        let days = self.password_policy().temporary_password_validity_days;
        // Saturating, so no stored policy value can ask for a number of seconds
        // the arithmetic cannot represent.
        Duration::from_secs(u64::try_from(days).unwrap_or(0).saturating_mul(24 * 60 * 60))
    }

    /// Whether this pool's schema declares the attribute `name` — a standard
    /// attribute every pool carries, or an entry its own schema holds. An
    /// attribute a pool does not declare cannot hold a value.
    pub fn declares_attribute(&self, name: &str) -> bool {
        is_standard_attribute(name)
            || self
                .schema
                .iter()
                .any(|entry| entry.name.as_deref() == Some(name))
    }

    /// `None` when every name in `names` is an attribute this pool's schema
    /// declares, else the first one that is not: an attribute a pool does not
    /// declare cannot hold a value.
    pub fn unknown_attribute(&self, names: &[String]) -> Option<String> {
        names
            .iter()
            .find(|name| !self.declares_attribute(name))
            .cloned()
    }

    /// This pool's schema entry for the attribute STORED under `name` — the
    /// prefixed name for a custom or developer-only attribute — or `None` when
    /// the schema declares no such attribute.
    pub fn schema_attribute(&self, name: &str) -> Option<&SchemaAttributeType> {
        self.schema
            .iter()
            .find(|entry| entry.name.as_deref() == Some(name))
    }

    /// How many of this pool's schema entries are custom or developer-only
    /// attributes: the ones added beyond the standard attributes every pool
    /// carries.
    pub fn custom_attribute_count(&self) -> usize {
        self.schema
            .iter()
            .filter(|entry| {
                entry
                    .name
                    .as_deref()
                    .is_some_and(|name| !is_standard_attribute(name))
            })
            .count()
    }

    /// Whether this pool's schema lets `name`'s stored value be CHANGED:
    /// `SchemaAttributeType.Mutable` — "Specifies whether the value of the
    /// attribute can be changed" (this service's model). An attribute the
    /// schema does not declare has no entry to read, so it is reported
    /// unchangeable.
    pub fn attribute_mutable(&self, name: &str) -> bool {
        self.schema_attribute(name)
            .and_then(|entry| entry.mutable)
            .unwrap_or(false)
    }

    /// Whether this pool's schema marks `name` a required attribute.
    pub fn attribute_required(&self, name: &str) -> bool {
        self.schema_attribute(name)
            .and_then(|entry| entry.required)
            .unwrap_or(false)
    }

    /// Whether a sign-in name of this pool resolves to `name`'s value:
    /// `AliasAttributes` are "attributes supported as an alias for this user
    /// pool" and `UsernameAttributes` "specifies whether a user can use an email
    /// address or phone number as a username when they sign up" (this service's
    /// model). Either way the value names at most one user of the pool.
    pub fn is_alias_attribute(&self, name: &str) -> bool {
        self.alias_attributes
            .iter()
            .chain(self.username_attributes.iter())
            .any(|attribute| attribute == name)
    }

    /// Whether this pool verifies `name` itself: `AutoVerifiedAttributes` are
    /// "the attributes that are auto-verified in a user pool" (this service's
    /// model), so a new value of one of them is what a verification message is
    /// sent for.
    pub fn auto_verifies(&self, name: &str) -> bool {
        self.auto_verified_attributes
            .iter()
            .any(|attribute| attribute == name)
    }

    /// The attribute a sign-up confirmation message for `user` is addressed to,
    /// and the stored value it goes to, or `None` when this pool has no such
    /// destination for that user.
    ///
    /// "Amazon Cognito sends confirmation codes to the user attribute in the
    /// `AutoVerifiedAttributes` property of your user pool"
    /// (`ResendConfirmationCode` in this service's model), so the pool's own
    /// auto-verified attributes decide where the code goes, in the order the
    /// pool declares them; the user has to hold a value for one of them for a
    /// message to have an address at all.
    pub fn confirmation_destination(&self, user: &User) -> Option<(String, String)> {
        self.auto_verified_attributes
            .iter()
            .filter(|attribute| delivery_medium(attribute).is_some())
            .find_map(|attribute| {
                user.destination_of(attribute)
                    .map(|destination| (attribute.clone(), destination))
            })
    }

    /// Where a password-reset message for `user` goes, as this pool's
    /// `AccountRecoverySetting` decides.
    ///
    /// "If `verified_email` has a priority of 1 and `verified_phone_number` has
    /// a priority of 2, your user pool sends account-recovery messages to a
    /// verified email address but falls back to an SMS message if the user has a
    /// verified phone number. The `admin_only` option prevents self-service
    /// account recovery" (`RecoveryOptionType` in this service's model) \u2014 so the
    /// mechanisms are tried in ascending priority and only a VERIFIED value is a
    /// destination.
    pub fn recovery_destination(&self, user: &User) -> RecoveryDestination {
        let mut mechanisms: Vec<(i32, String)> = self
            .account_recovery_setting
            .as_ref()
            .and_then(|setting| setting.recovery_mechanisms.clone())
            .map(|mechanisms| {
                mechanisms
                    .into_iter()
                    .map(|mechanism| (mechanism.priority, mechanism.name))
                    .collect()
            })
            .unwrap_or_else(|| {
                DEFAULT_RECOVERY_MECHANISMS
                    .iter()
                    .enumerate()
                    .map(|(index, name)| {
                        (
                            i32::try_from(index).unwrap_or_default() + 1,
                            (*name).to_string(),
                        )
                    })
                    .collect()
            });
        if mechanisms
            .iter()
            .any(|(_, name)| name == RECOVERY_ADMIN_ONLY)
        {
            return RecoveryDestination::AdminOnly;
        }
        mechanisms.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
        mechanisms
            .iter()
            .filter_map(|(_, name)| recovered_attribute(name))
            .find_map(|attribute| {
                user.verified_destination_of(attribute)
                    .map(|destination| RecoveryDestination::Delivered {
                        attribute: attribute.to_string(),
                        destination,
                    })
            })
            .unwrap_or(RecoveryDestination::None)
    }

    /// Whether `factor` is a first authentication factor this pool admits:
    /// `SignInPolicyType.AllowedFirstAuthFactors` is "the sign-in methods that a
    /// user pool supports as the first factor" (this service's model), and a pool
    /// that configures none admits `PASSWORD` alone
    /// ([`effective_policies`]).
    pub fn allows_first_auth_factor(&self, factor: &str) -> bool {
        self.policies
            .sign_in_policy
            .as_ref()
            .and_then(|policy| policy.allowed_first_auth_factors.as_ref())
            .is_some_and(|factors| factors.iter().any(|allowed| allowed == factor))
    }

    /// The `AuthFactorType` values `user` can authenticate with in a choice-based
    /// (`USER_AUTH`) sign-in of this pool, in [`AUTH_FACTOR_ORDER`].
    ///
    /// A factor is available to the user when this pool offers it as a first
    /// authentication factor and the user holds what that factor needs
    /// ([`UserPool::offers_first_auth_factor`]), or when the user has it available
    /// as an MFA factor ([`UserPool::offers_mfa_auth_factor`]) — "EMAIL_OTP,
    /// SMS_OTP, and WEB_AUTHN can be used as either a first authentication factor
    /// or an MFA factor" (`ConfiguredUserAuthFactors` in this service's model). A
    /// factor neither half offers is not one the user can sign in with, so it is
    /// not reported.
    ///
    /// A PURE read, computed from the stored pool and the stored user.
    pub fn configured_user_auth_factors(&self, user: &User) -> Vec<String> {
        AUTH_FACTOR_ORDER
            .iter()
            .copied()
            .filter(|factor| {
                self.offers_first_auth_factor(user, factor)
                    || self.offers_mfa_auth_factor(user, factor)
            })
            .map(str::to_string)
            .collect()
    }

    /// Whether `user` can present `factor` as the FIRST authentication factor of a
    /// choice-based sign-in of this pool.
    ///
    /// The pool has to admit the factor ([`UserPool::allows_first_auth_factor`])
    /// AND the user has to hold what presenting it needs: a stored password, a
    /// verified address a one-time password can be delivered to, or a registered
    /// passkey. `SOFTWARE_TOKEN` "can only be used as an MFA factor"
    /// (`ConfiguredUserAuthFactors` in this service's model), so no sign-in policy
    /// entry makes it a first factor.
    pub fn offers_first_auth_factor(&self, user: &User, factor: &str) -> bool {
        if !self.allows_first_auth_factor(factor) {
            return false;
        }
        match factor {
            PASSWORD_AUTH_FACTOR => user.password.is_some(),
            EMAIL_OTP_AUTH_FACTOR => user.verified_destination_of("email").is_some(),
            SMS_OTP_AUTH_FACTOR => user.verified_destination_of("phone_number").is_some(),
            WEB_AUTHN_AUTH_FACTOR => !user.web_authn_credentials.is_empty(),
            _ => false,
        }
    }

    /// Whether `user` can present `factor` as an MFA factor of a choice-based
    /// sign-in of this pool.
    ///
    /// A factor the user has ACTIVATED is available as one
    /// ([`User::activates_mfa_auth_factor`]). `WEB_AUTHN` is the one factor the
    /// POOL has to enable as well: it "is available as an MFA factor only
    /// when passkey MFA is enabled at the user pool level"
    /// (`ConfiguredUserAuthFactors`), which
    /// `WebAuthnConfigurationType.FactorConfiguration` is "when set to
    /// `MULTI_FACTOR_WITH_USER_VERIFICATION`, passkey authentication with user
    /// verification satisfies MFA requirements. When set to `SINGLE_FACTOR` or not
    /// set, passkeys are a single authentication factor" (both in this service's
    /// model) — and presenting it still needs a registered passkey the user has
    /// not deactivated passkey MFA for ([`User::activates_passkey_mfa`]).
    pub fn offers_mfa_auth_factor(&self, user: &User, factor: &str) -> bool {
        if factor == WEB_AUTHN_AUTH_FACTOR {
            return self.passkeys_satisfy_mfa()
                && !user.web_authn_credentials.is_empty()
                && user.activates_passkey_mfa();
        }
        user.activates_mfa_auth_factor(factor)
    }

    /// Whether a passkey of this pool satisfies an MFA requirement:
    /// `WebAuthnConfigurationType.FactorConfiguration` set to
    /// [`WEB_AUTHN_MULTI_FACTOR`], the one of that enum's values that makes passkey
    /// authentication "satisfy MFA requirements" (this service's model). A pool
    /// that stores no passkey configuration at all has not enabled it.
    pub fn passkeys_satisfy_mfa(&self) -> bool {
        self.web_authn_configuration
            .as_ref()
            .and_then(|config| config.factor_configuration.as_deref())
            .is_some_and(|configuration| configuration == WEB_AUTHN_MULTI_FACTOR)
    }

    /// Whether this pool holds a new value of `name` PENDING until the user
    /// confirms it:
    /// `UserAttributeUpdateSettings.AttributesRequireVerificationBeforeUpdate`
    /// "requires that your user verifies their email address, phone number, or
    /// both before Amazon Cognito updates the value of that attribute" (this
    /// service's model).
    pub fn requires_verification_before_update(&self, name: &str) -> bool {
        self.user_attribute_update_settings
            .attributes_require_verification_before_update
            .as_ref()
            .is_some_and(|names| names.iter().any(|attribute| attribute == name))
    }

    /// Whether a user of this pool OTHER than `except_username` already holds
    /// `value` as its active alias value of `name`.
    ///
    /// An alias value identifies ONE user of the pool, so no two users may hold
    /// the same one at once: for `email` and `phone_number` the value is an
    /// alias only while the user's matching verified flag says the value was
    /// confirmed, and an alias attribute with no such flag counts whatever its
    /// value (see [`User::alias_is_active`]).
    pub fn alias_taken(&self, name: &str, value: &str, except_username: &str) -> bool {
        !self.alias_holder_keys(name, value, except_username).is_empty()
    }

    /// The keys of the users of this pool OTHER than `except_username` that hold
    /// `value` as their active alias value of `name`, in ascending key order.
    ///
    /// These are the users an update that gives `value` to another user takes
    /// the alias away from ([`AttributeUpdatePlan::demotions`]).
    pub fn alias_holder_keys(
        &self,
        name: &str,
        value: &str,
        except_username: &str,
    ) -> Vec<String> {
        let except = self.user_key(except_username);
        self.users
            .iter()
            .filter(|(key, user)| {
                *key != &except
                    && user
                        .attributes
                        .get(name)
                        .is_some_and(|stored| stored == value)
                    && user.alias_is_active(name)
            })
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// The names of the attributes this pool's schema marks required that `user`
    /// has no value for: what a `NEW_PASSWORD_REQUIRED` challenge asks the user
    /// to supply. `sub` is never among them — the pool mints it.
    pub fn missing_required_attributes(&self, user: &User) -> Vec<String> {
        self.unset_required_attributes(&user.attributes)
    }

    /// The names of the attributes this pool's schema marks required that
    /// `attributes` holds no value for — the same rule as
    /// [`UserPool::missing_required_attributes`], asked of an attribute set that
    /// is not stored yet.
    pub fn unset_required_attributes(&self, attributes: &BTreeMap<String, String>) -> Vec<String> {
        self.schema
            .iter()
            .filter(|entry| entry.required == Some(true))
            .filter_map(|entry| entry.name.clone())
            .filter(|name| name != SUB_ATTRIBUTE && !attributes.contains_key(name))
            .collect()
    }

    /// This pool's domain under the name `domain` — its prefix domain or its
    /// custom one — or `None` when this pool holds no domain of that name.
    pub fn domain_named(&self, domain: &str) -> Option<&UserPoolDomain> {
        [self.domain.as_ref(), self.custom_domain.as_ref()]
            .into_iter()
            .flatten()
            .find(|stored| stored.domain == domain)
    }

    /// This pool's domain under the name `domain`, mutably — the same lookup as
    /// [`UserPool::domain_named`], for an operation that writes the stored
    /// domain back.
    pub fn domain_named_mut(&mut self, domain: &str) -> Option<&mut UserPoolDomain> {
        [self.domain.as_mut(), self.custom_domain.as_mut()]
            .into_iter()
            .flatten()
            .find(|stored| stored.domain == domain)
    }

    /// Take this pool's domain under the name `domain` out of the pool — its
    /// prefix domain or its custom one — and hand back the record that was
    /// stored, or `None` when this pool holds no domain of that name.
    ///
    /// The two slots are the same resource kind under `UserPoolType.Domain` and
    /// `UserPoolType.CustomDomain`, so a delete clears whichever one holds the
    /// name and leaves the other in place.
    pub fn remove_domain_named(&mut self, domain: &str) -> Option<UserPoolDomain> {
        [&mut self.domain, &mut self.custom_domain]
            .into_iter()
            .find(|slot| slot.as_ref().is_some_and(|stored| stored.domain == domain))
            .and_then(Option::take)
    }

    /// Whether `scope` is a custom scope of a resource server of this pool.
    ///
    /// "Amazon Cognito represents scopes in the access token in the format
    /// `$resource-server-identifier/$scope`" (`CreateResourceServer.Identifier`
    /// in this service's model), so a scope value names the stored resource
    /// server and one of ITS scope names. The value is matched against the
    /// STORED identifier and then against that server's own scope names, rather
    /// than split on the separator, because a resource server identifier may
    /// itself carry one.
    pub fn defines_scope(&self, scope: &str) -> bool {
        self.resource_servers.values().any(|server| {
            let Some(name) = scope
                .strip_prefix(server.identifier.as_str())
                .and_then(|rest| rest.strip_prefix('/'))
            else {
                return false;
            };
            server
                .scopes
                .iter()
                .any(|defined| defined.scope_name == name)
        })
    }

    /// The CSV header of this pool: "a comma-separated value (CSV) list
    /// populated with available user attributes in the user pool ... the header
    /// for the CSV file that determines the users in a user import job"
    /// (`GetCSVHeader`), as the list of column names it is modelled as.
    ///
    /// COMPUTED from the stored schema IN ITS STORED ORDER, so a pool whose
    /// schema carries a custom attribute reports a column for it where the
    /// schema holds it: one column per attribute of this pool a user import can
    /// carry, then the columns the import format adds
    /// ([`CSV_HEADER_TRAILING_COLUMNS`]). [`SUB_ATTRIBUTE`] is not among them —
    /// the pool mints it for every user and no import supplies it — and neither
    /// are the attributes the service maintains itself
    /// ([`SERVICE_MAINTAINED_SCHEMA`]), which no import supplies either. The
    /// stored order is the pool's own, as created ([`effective_schema`]), so the
    /// header a client builds its CSV from and the schema `DescribeUserPool`
    /// reports list the pool's attributes in ONE order.
    pub fn csv_header(&self) -> Vec<String> {
        self.schema
            .iter()
            .filter_map(|attribute| attribute.name.clone())
            .filter(|name| name != SUB_ATTRIBUTE)
            .chain(
                CSV_HEADER_TRAILING_COLUMNS
                    .iter()
                    .map(|column| (*column).to_string()),
            )
            .collect()
    }

    /// Whether `scope` is one an app client of this pool may be permitted to
    /// authorize access with: a standard scope every pool serves
    /// ([`STANDARD_OAUTH_SCOPES`]), or a custom scope a resource server of THIS
    /// pool defines ([`UserPool::defines_scope`]).
    ///
    /// "Scope values include `phone`, `email`, `openid`, and `profile`. The
    /// `aws.cognito.signin.user.admin` scope authorizes user self-service
    /// operations. Custom scopes with resource servers authorize access to
    /// external APIs" (`CreateUserPoolClient.AllowedOAuthScopes` in this
    /// service's model), so a value that is neither names a scope this pool has
    /// no definition of. Every writer of `AllowedOAuthScopes` asks this one
    /// question, so the create and the update path cannot disagree about which
    /// scopes exist.
    pub fn authorizes_scope(&self, scope: &str) -> bool {
        STANDARD_OAUTH_SCOPES.contains(&scope) || self.defines_scope(scope)
    }

    /// This pool's terms documents that app client `client_id` holds under
    /// `terms_name`, or `None` when the client holds none of that name.
    ///
    /// A terms document is assigned to one app client and its name is unique to
    /// that client (`TermsExistsException` in this service's model), so at most
    /// one stored document can answer.
    pub fn terms_for_client(&self, client_id: &str, terms_name: &str) -> Option<&Terms> {
        self.terms
            .values()
            .find(|stored| stored.client_id == client_id && stored.terms_name == terms_name)
    }

    /// This pool's managed login branding style assigned to app client
    /// `client_id`, or `None` when that app client has none.
    ///
    /// "Each style is linked to an app client until you delete it"
    /// (`CreateManagedLoginBranding.ClientId`) and a second style for an app
    /// client "that already has an assigned style" is refused
    /// (`ManagedLoginBrandingExistsException`), so at most one stored style can
    /// answer.
    pub fn branding_for_client(&self, client_id: &str) -> Option<&ManagedLoginBranding> {
        self.managed_login_branding
            .values()
            .find(|stored| stored.client_id == client_id)
    }

    /// The classic hosted UI branding in force for `client_id`, or `None` when
    /// neither that app client nor the pool has any.
    ///
    /// "Returns user-pool level branding information if no app client branding
    /// is applied, or if you don't specify an app client ID"
    /// (`GetUICustomization`), so a client-level entry answers when the named
    /// app client has one and the pool-level default answers otherwise. A caller
    /// that names [`UI_CUSTOMIZATION_ALL_CLIENTS`] names that default itself.
    pub fn ui_customization(&self, client_id: Option<&str>) -> Option<&UiCustomization> {
        client_id
            .filter(|id| *id != UI_CUSTOMIZATION_ALL_CLIENTS)
            .and_then(|id| self.ui_customizations.get(id))
            .or_else(|| self.ui_customizations.get(UI_CUSTOMIZATION_ALL_CLIENTS))
    }

    /// Store the classic hosted UI branding of `client_id`, keeping every value
    /// the request leaves out.
    ///
    /// `SetUICustomization` models `CSS` and `ImageFile` as optional members of
    /// a call that "configures UI branding settings", so a member the request
    /// omits names no new value for it and the stored one stays in force; an
    /// `ImageFile` of no bytes (`ImageFileType`'s length admits the empty blob)
    /// carries no image, so it stores none.
    pub fn apply_ui_customization(
        &mut self,
        client_id: &str,
        css: Option<String>,
        image_file: Option<Vec<u8>>,
        now: SystemTime,
    ) {
        let version = css_version_stamp(now);
        let stored = self
            .ui_customizations
            .entry(client_id.to_string())
            .or_insert_with(|| UiCustomization {
                client_id: client_id.to_string(),
                css: None,
                image_file: None,
                css_version: version.clone(),
                creation_date: now,
                last_modified_date: now,
            });
        if let Some(css) = css {
            stored.css = Some(css);
        }
        if let Some(image_file) = image_file {
            stored.image_file = (!image_file.is_empty()).then_some(image_file);
        }
        stored.css_version = version;
        stored.last_modified_date = now;
    }

    /// The modelled view of the stored classic hosted UI branding
    /// `customization` of this pool, rendered from what is stored, with its
    /// stored logo image located on the endpoint `host` the reading request
    /// arrived at.
    pub fn to_ui_customization_type(
        &self,
        customization: &UiCustomization,
        host: Option<&str>,
    ) -> crate::ops::types::UICustomizationType {
        crate::ops::types::UICustomizationType {
            user_pool_id: Some(self.id.clone()),
            client_id: Some(customization.client_id.clone()),
            image_url: customization.image_file.as_ref().and_then(|_| {
                host.and_then(|host| {
                    hosted_logo_url(host, &self.id, &customization.client_id)
                })
            }),
            css: customization.css.clone(),
            css_version: Some(customization.css_version.clone()),
            last_modified_date: Some(to_date_time(customization.last_modified_date)),
            creation_date: Some(to_date_time(customization.creation_date)),
        }
    }

    /// The logo image this pool serves for `client_id`, which is the image
    /// stored for THAT level: the locator a read handed out names one level, so
    /// it serves that level's image and never another's.
    pub fn hosted_logo(&self, client_id: &str) -> Option<&[u8]> {
        self.ui_customizations
            .get(client_id)
            .and_then(|stored| stored.image_file.as_deref())
    }

    /// The domain this pool's passkeys name as their relying party, or `None`
    /// when the pool has neither a configured relying party id nor a domain to
    /// serve as one.
    ///
    /// `WebAuthnConfigurationType.RelyingPartyId` "Sets or displays the
    /// authentication domain, typically your user pool domain, that passkey
    /// providers must use as a relying party (RP) in their configuration" (this
    /// service's model), so the configured value is what is in force when the
    /// pool sets one. A pool that sets none falls back to its own domain, the
    /// CUSTOM one first: "when you create a custom domain, the passkey RP ID
    /// defaults to the custom domain. If you had a prefix domain active, this
    /// will cause passkey integration for your prefix domain to stop working due
    /// to a mismatch in RP ID. To keep the prefix domain passkey integration
    /// working, you can explicitly set RP ID to the prefix domain"
    /// (`CreateUserPoolDomain.CustomDomainConfig`). A custom domain's stored
    /// value IS the domain, while a prefix domain's is the prefix alone: "for
    /// custom domains, this is the fully-qualified domain name, such as
    /// auth.example.com. For prefix domains, this is the prefix alone, such as
    /// myprefix. A prefix value of myprefix for a user pool in the us-east-1
    /// Region results in a domain of myprefix.auth.us-east-1.amazoncognito.com"
    /// (`CreateUserPoolDomain.Domain`), so only the prefix is expanded. A pool
    /// with neither is the pool that "doesn't have a configured relying party id
    /// or a user pool domain" (`WebAuthnConfigurationMissingException`).
    pub fn relying_party_id(&self) -> Option<String> {
        let configured = self
            .web_authn_configuration
            .as_ref()
            .and_then(|config| config.relying_party_id.as_deref())
            .filter(|id| !id.is_empty());
        if let Some(configured) = configured {
            return Some(configured.to_string());
        }
        if let Some(custom) = &self.custom_domain {
            return Some(custom.domain.clone());
        }
        self.domain
            .as_ref()
            .map(|prefix| format!("{}.auth.{}.amazoncognito.com", prefix.domain, self.region))
    }

    /// The user-verification requirement this pool's passkey registrations ask
    /// for.
    ///
    /// `WebAuthnConfigurationType.UserVerification`: "When `required`, users can
    /// only register and sign in users with passkeys that are capable of user
    /// verification. When `preferred`, your user pool doesn't require the use of
    /// authenticators with user verification but encourages it" (this service's
    /// model), so a pool that configures none does not require it, which is the
    /// other of `UserVerificationType`'s two values.
    pub fn web_authn_user_verification(&self) -> String {
        self.web_authn_configuration
            .as_ref()
            .and_then(|config| config.user_verification.as_deref())
            .filter(|requirement| !requirement.is_empty())
            .unwrap_or(USER_VERIFICATION_PREFERRED)
            .to_string()
    }

    /// Whether this pool's user pool add-ons are enabled, which is what the
    /// threat-protection surface rests on.
    ///
    /// `UserPoolAddOns` "contains settings for activation of threat protection"
    /// (`CreateUserPool`) and threat protection is changed "to audit-only or
    /// off" by updating that member (`SetRiskConfiguration`), so add-ons are on
    /// exactly when the pool stores a mode other than
    /// [`ADVANCED_SECURITY_MODE_OFF`] — and a pool that stores no
    /// `UserPoolAddOns` at all has them off.
    pub fn add_ons_enabled(&self) -> bool {
        self.user_pool_add_ons
            .as_ref()
            .is_some_and(|add_ons| add_ons.advanced_security_mode != ADVANCED_SECURITY_MODE_OFF)
    }

    /// Whether this pool requires a second authentication factor from every
    /// sign-in: `MfaConfiguration` "sets multi-factor authentication (MFA) to be
    /// on, off, or optional" and [`MFA_REQUIRED`] is the mode in which "MFA
    /// tokens are required for all user sign-ins" (this service's model). Under
    /// the optional mode a user that has activated no factor signs in without
    /// one, so only the required mode makes a factor a precondition.
    pub fn requires_mfa(&self) -> bool {
        self.mfa_configuration == MFA_REQUIRED
    }

    /// The `UserMFASettingList` settings `user` can SET UP as an MFA factor of
    /// this pool, in the order that member documents its values in.
    ///
    /// A method is available to set up when this POOL offers it — `SetUserPoolMfaConfig`
    /// is what "sets the user pool multi-factor authentication (MFA) and passkey
    /// configuration" and each of `SmsMfaConfiguration`,
    /// `EmailMfaConfiguration` and `SoftwareTokenMfaConfiguration` is the
    /// configuration of one method (this service's model) — and the user has not
    /// activated it already ([`User::mfa_setting_list`]): a factor the user holds
    /// is one to present, not one to set up. `SoftwareTokenMfaConfigType` carries
    /// its own activation state ("the activation state of TOTP MFA"), while the
    /// SMS and email methods are offered exactly while the pool stores the
    /// configuration each is delivered through. An email or SMS method is set up
    /// by verifying the address or number it is delivered to, and the software
    /// token by registering an authenticator, so neither is filtered on what the
    /// user holds today.
    pub fn mfa_settings_available_for_setup(&self, user: &User) -> Vec<String> {
        [
            (SMS_MFA_SETTING, self.to_sms_mfa_config().is_some()),
            (
                EMAIL_OTP_MFA_SETTING,
                self.email_mfa_configuration.is_some(),
            ),
            (
                SOFTWARE_TOKEN_MFA_SETTING,
                self.software_token_mfa_configuration
                    .as_ref()
                    .is_some_and(|config| config.enabled),
            ),
        ]
        .into_iter()
        .filter(|(setting, offered)| {
            *offered && !user.mfa_setting_list.iter().any(|held| held == setting)
        })
        .map(|(setting, _)| setting.to_string())
        .collect()
    }

    /// The modelled view of this pool's MFA and passkey configuration, rendered
    /// from what is stored.
    ///
    /// `GetUserPoolMfaConfig` and `SetUserPoolMfaConfig` model the same five
    /// members in two generated output structs, so the stored configuration is
    /// assembled once here and each handler maps it into its own output.
    pub fn to_mfa_config(&self) -> MfaConfig {
        MfaConfig {
            sms_mfa_configuration: self.to_sms_mfa_config(),
            software_token_mfa_configuration: self.software_token_mfa_configuration.clone(),
            email_mfa_configuration: self.email_mfa_configuration.clone(),
            mfa_configuration: self.mfa_configuration.clone(),
            web_authn_configuration: self.web_authn_configuration.clone(),
        }
    }

    /// The modelled view of this pool's SMS MFA configuration, rendered from the
    /// pool's own SMS settings.
    ///
    /// `SmsMfaConfigType` is a view over them rather than a second copy: "You
    /// can set `SmsConfiguration` in `CreateUserPool` and `UpdateUserPool`, or
    /// in `SetUserPoolMfaConfig`" (`SmsMfaConfigType.SmsConfiguration` in this
    /// service's model), and `SmsAuthenticationMessage` is a member of both this
    /// structure and the pool itself. `None` when the pool holds neither, so a
    /// pool with no SMS settings reports no SMS MFA configuration.
    pub fn to_sms_mfa_config(&self) -> Option<SmsMfaConfigType> {
        if self.sms_configuration.is_none() && self.sms_authentication_message.is_none() {
            return None;
        }
        Some(SmsMfaConfigType {
            sms_authentication_message: self.sms_authentication_message.clone(),
            sms_configuration: self.sms_configuration.clone(),
        })
    }

    /// The modelled view of this pool's log delivery configuration, rendered
    /// from what is stored.
    ///
    /// `GetLogDeliveryConfiguration` and `SetLogDeliveryConfiguration` model the
    /// same `LogDeliveryConfigurationType`, so both answer the pool this is
    /// read from beside the collection it holds. A pool with no logging
    /// configured reports an EMPTY collection rather than none: the member is
    /// modelled required.
    pub fn to_log_delivery_configuration(&self) -> LogDeliveryConfigurationType {
        LogDeliveryConfigurationType {
            user_pool_id: self.id.clone(),
            log_configurations: self.log_configurations.clone(),
        }
    }

    /// The ARN of this pool in `region`: a replicated user pool keeps ONE id
    /// across the Regions it is replicated to (`CreateUserPoolReplica` takes
    /// the pool's id and a Region and mints no second id), so a replica's ARN
    /// is this pool's id under that Region. For [`UserPool::region`] this is
    /// the pool's own stored ARN.
    pub fn arn_in_region(&self, region: &str) -> String {
        if region == self.region {
            return self.arn.clone();
        }
        ::engine::emulator_arn(SIGNING_NAME, region, &format!("userpool/{}", self.id))
    }

    /// The modelled view of this pool's PRIMARY replica: the pool itself, in
    /// the Region it was created in. `ListUserPoolReplicas` lists "both primary
    /// and secondary replicas", and the pool serves every operation in its own
    /// Region, which is what [`REPLICA_STATUS_ACTIVE`] reports.
    pub fn to_primary_replica(&self) -> UserPoolReplicaType {
        UserPoolReplicaType {
            region_name: Some(self.region.clone()),
            status: Some(REPLICA_STATUS_ACTIVE.to_string()),
            role: Some(REPLICA_ROLE_PRIMARY.to_string()),
            user_pool_arn: Some(self.arn.clone()),
        }
    }

    /// Every Region this pool holds a replica in as of `now`, ASCENDING: its
    /// own Region, where the primary lives, beside every secondary a create
    /// added and a delete has not finished removing. The stable order a page
    /// token indexes into.
    pub fn replica_regions(&self, now: SystemTime) -> Vec<String> {
        let mut regions: Vec<String> = self
            .replicas
            .values()
            .filter(|replica| !replica.removed(now))
            .map(|replica| replica.region_name.clone())
            .collect();
        regions.push(self.region.clone());
        regions.sort();
        regions.dedup();
        regions
    }

    /// The modelled view of this pool's replica in `region` as of `now`, or
    /// `None` when the pool holds none there. A secondary replica whose
    /// deletion the clock has finished is gone, whether or not anything has
    /// pruned it from the store yet.
    pub fn to_replica_in(&self, region: &str, now: SystemTime) -> Option<UserPoolReplicaType> {
        if let Some(replica) = self.live_replica(region, now) {
            return Some(replica.to_user_pool_replica_type(now));
        }
        (region == self.region).then(|| self.to_primary_replica())
    }

    /// The stored SECONDARY replica in `region`, unless the clock has already
    /// made its removal due.
    pub fn live_replica(&self, region: &str, now: SystemTime) -> Option<&UserPoolReplica> {
        self.replicas
            .get(region)
            .filter(|replica| !replica.removed(now))
    }

    /// Drop the replicas whose removal the clock has already made due, so a
    /// later create can use the Region again. A write only an operation that
    /// mutates this pool's replicas performs; a read computes the same
    /// disappearance with [`UserPool::live_replica`] instead.
    pub fn prune_removed_replicas(&mut self, now: SystemTime) {
        self.replicas.retain(|_, replica| !replica.removed(now));
    }

    /// The schema a READ of this pool reports: the attributes the pool is
    /// CONFIGURED with ([`UserPool::schema`]) and, behind the standard ones, the
    /// attributes the service maintains for every pool
    /// ([`SERVICE_MAINTAINED_SCHEMA`]).
    ///
    /// A service-maintained attribute belongs to the pool rather than to the
    /// request that created it, so it sits with the standard attributes a pool
    /// starts with, ahead of the custom attributes added behind them; an
    /// attribute the pool's own schema already declares under that name is left
    /// as the pool declares it.
    pub fn reported_schema(&self) -> Vec<SchemaAttributeType> {
        let mut reported = self.schema.clone();
        // Behind the last standard attribute the pool carries, so the custom
        // attributes appended after them stay behind them.
        let behind_standard = reported
            .iter()
            .rposition(|entry| {
                entry
                    .name
                    .as_deref()
                    .is_some_and(is_standard_attribute)
            })
            .map_or(0, |at| at + 1);
        for (offset, standard) in SERVICE_MAINTAINED_SCHEMA
            .iter()
            .filter(|standard| self.schema_attribute(standard.name).is_none())
            .enumerate()
        {
            reported.insert(behind_standard + offset, standard_schema_attribute(standard));
        }
        reported
    }

    /// The modelled view of this pool, rendered from what is stored.
    pub fn to_user_pool_type(&self) -> UserPoolType {
        self.to_user_pool_type_with(self.reported_schema())
    }

    /// The modelled view of this pool as the CREATE that configured it answers:
    /// [`UserPool::to_user_pool_type`] carrying the schema the request
    /// CONFIGURED, without the attributes the service maintains itself
    /// ([`SERVICE_MAINTAINED_SCHEMA`]).
    ///
    /// A create answers with the pool it stored from the request; the service's
    /// own attributes are the pool's from then on, and every later read of the
    /// pool reports them beside the configured ones.
    pub fn to_configured_user_pool_type(&self) -> UserPoolType {
        self.to_user_pool_type_with(self.schema.clone())
    }

    /// The modelled view of this pool carrying `schema_attributes`, which is
    /// what the two views above differ in.
    fn to_user_pool_type_with(&self, schema_attributes: Vec<SchemaAttributeType>) -> UserPoolType {
        UserPoolType {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            policies: Some(self.policies.clone()),
            deletion_protection: Some(self.deletion_protection.clone()),
            lambda_config: Some(self.lambda_config.clone()),
            // `UserPoolType.Status` is `@deprecated` ("This property is no longer
            // available."), so it is left out rather than invented.
            status: None,
            last_modified_date: Some(to_date_time(self.last_modified_date)),
            creation_date: Some(to_date_time(self.creation_date)),
            schema_attributes: Some(schema_attributes),
            auto_verified_attributes: non_empty(&self.auto_verified_attributes),
            alias_attributes: non_empty(&self.alias_attributes),
            username_attributes: non_empty(&self.username_attributes),
            sms_verification_message: None,
            email_verification_message: None,
            email_verification_subject: None,
            verification_message_template: Some(self.verification_message_template.clone()),
            sms_authentication_message: self.sms_authentication_message.clone(),
            user_attribute_update_settings: Some(self.user_attribute_update_settings.clone()),
            mfa_configuration: Some(self.mfa_configuration.clone()),
            device_configuration: self.device_configuration.clone(),
            // Computed from what the pool holds, never from a request.
            estimated_number_of_users: i32::try_from(self.users.len()).unwrap_or(i32::MAX),
            email_configuration: Some(self.email_configuration.clone()),
            sms_configuration: self.sms_configuration.clone(),
            user_pool_tags: Some(self.tags.clone()),
            sms_configuration_failure: None,
            email_configuration_failure: None,
            domain: self.domain.as_ref().map(|domain| domain.domain.clone()),
            custom_domain: self.custom_domain.as_ref().map(|domain| domain.domain.clone()),
            admin_create_user_config: Some(self.admin_create_user_config.clone()),
            user_pool_add_ons: self.user_pool_add_ons.clone(),
            username_configuration: self.username_configuration.clone(),
            arn: Some(self.arn.clone()),
            account_recovery_setting: self.account_recovery_setting.clone(),
            user_pool_tier: Some(self.user_pool_tier.clone()),
            key_configuration: Some(self.key_configuration.clone()),
            issuer_configuration: Some(self.issuer_configuration.clone()),
            acr_configuration: self.acr_configuration.clone(),
        }
    }

    /// The SHORT modelled view of this pool, rendered from what is stored: the
    /// entry a listing of several pools serves ("A short description of a user
    /// pool", `UserPoolDescriptionType`). Every member it shares with
    /// [`UserPool::to_user_pool_type`] renders the same stored value, so a
    /// listing entry and a read of the one pool never disagree.
    pub fn to_user_pool_description_type(&self, now: SystemTime) -> UserPoolDescriptionType {
        UserPoolDescriptionType {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            lambda_config: Some(self.lambda_config.clone()),
            // `UserPoolDescriptionType.Status` is `@deprecated` ("This property
            // is no longer available."), so it is left out rather than
            // invented — as `to_user_pool_type` leaves it out too.
            status: None,
            last_modified_date: Some(to_date_time(self.last_modified_date)),
            creation_date: Some(to_date_time(self.creation_date)),
            // "A list of Amazon Web Services Regions where replicas of this
            // user pool exist": exactly the Regions `ListUserPoolReplicas`
            // reports a replica in, so the two readers never disagree. Every
            // pool holds its PRIMARY replica in the Region it was created in, so
            // a pool nothing was replicated to reports that one Region, and so
            // does one whose only secondary the clock has already removed.
            replica_regions: Some(self.replica_regions(now)),
        }
    }
}

/// One stored user pool domain: what `CreateUserPoolDomain` persisted and every
/// later read of that domain renders.
///
/// A domain is a PREFIX domain when it carries no `CustomDomainConfig` and a
/// CUSTOM domain when it does (`CreateUserPoolDomain.CustomDomainConfig`:
/// "Provide this parameter only if you want to use a custom domain for your user
/// pool. Otherwise, you can omit this parameter and use a prefix domain
/// instead.").
#[derive(Debug, Clone)]
pub struct UserPoolDomain {
    /// The domain string the request named, which is this resource's key.
    pub domain: String,
    /// The pool the domain is attached to.
    pub user_pool_id: String,
    /// The custom-domain configuration as submitted; `None` for a prefix domain.
    pub custom_domain_config: Option<CustomDomainConfigType>,
    /// The managed login branding version in force for this domain.
    pub managed_login_version: i32,
    /// The routing configuration as submitted, which only a custom domain may
    /// carry.
    pub routing: Option<RoutingType>,
    /// The CloudFront endpoint that hosts a CUSTOM domain, minted with it;
    /// `None` for a prefix domain, for which `CreateUserPoolDomain` documents
    /// "If you set an Amazon Cognito prefix domain, this parameter returns
    /// null."
    pub cloud_front_distribution: Option<String>,
    /// When the domain was created.
    pub creation_date: SystemTime,
    /// The instant the clock owes this domain's move to `ACTIVE`. Stored, never
    /// slept on: a read computes the status from it.
    pub active_at: SystemTime,
    /// The `DomainStatusType` this domain reports until [`Self::active_at`] is
    /// due: `CREATING` for a domain that was just created, `UPDATING` for one
    /// whose configuration was just changed.
    pub status_until_active: &'static str,
}

impl UserPoolDomain {
    /// A new domain for `user_pool_id`, `CREATING` until the clock makes its
    /// activation due one [`DOMAIN_ACTIVATION_SPAN`] from `now`.
    pub fn new(
        domain: String,
        user_pool_id: String,
        custom_domain_config: Option<CustomDomainConfigType>,
        managed_login_version: i32,
        routing: Option<RoutingType>,
        now: SystemTime,
    ) -> Self {
        // A custom domain is served through CloudFront, and the caller has to
        // point an alias record at that endpoint, so one is minted with the
        // domain; a prefix domain has none.
        let cloud_front_distribution = custom_domain_config
            .is_some()
            .then(new_cloud_front_domain);
        Self {
            domain,
            user_pool_id,
            custom_domain_config,
            managed_login_version,
            routing,
            cloud_front_distribution,
            creation_date: now,
            active_at: now + DOMAIN_ACTIVATION_SPAN,
            status_until_active: DOMAIN_STATUS_CREATING,
        }
    }

    /// Whether this is a custom domain rather than an Amazon Cognito prefix one.
    pub fn is_custom(&self) -> bool {
        self.custom_domain_config.is_some()
    }

    /// Record that this domain's configuration changed at `now`, which the
    /// service does not serve instantly: "Changes to the domain branding version
    /// take up to one minute to take effect for a prefix domain and up to five
    /// minutes for a custom domain" (`UpdateUserPoolDomain`), so the domain is
    /// `UPDATING` until the clock makes it `ACTIVE` again one
    /// [`DOMAIN_ACTIVATION_SPAN`] from `now`.
    pub fn begin_update(&mut self, now: SystemTime) {
        self.active_at = now + DOMAIN_ACTIVATION_SPAN;
        self.status_until_active = DOMAIN_STATUS_UPDATING;
    }

    /// This domain's `DomainStatusType` as of `now`: the status its last write
    /// left it in until the clock has made its activation due, `ACTIVE` from
    /// then on.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now >= self.active_at {
            DOMAIN_STATUS_ACTIVE
        } else {
            self.status_until_active
        }
    }

    /// The modelled view of this domain, rendered from what is stored.
    pub fn to_domain_description(&self, now: SystemTime) -> DomainDescriptionType {
        DomainDescriptionType {
            user_pool_id: Some(self.user_pool_id.clone()),
            aws_account_id: Some(::engine::emulator_account_id().to_string()),
            domain: Some(self.domain.clone()),
            // `S3Bucket` is "The Amazon S3 bucket where the static files for
            // this domain are stored" and `Version` "The app version": both
            // name AWS-internal hosted-UI infrastructure this emulator does not
            // deploy, so neither is invented.
            s3_bucket: None,
            cloud_front_distribution: self.cloud_front_distribution.clone(),
            version: None,
            status: Some(self.status(now).to_string()),
            custom_domain_config: self.custom_domain_config.clone(),
            managed_login_version: Some(self.managed_login_version),
            routing: self.routing.clone(),
        }
    }
}

/// One stored SECONDARY replica of a user pool: what `CreateUserPoolReplica`
/// persisted and every later read of that replica renders.
///
/// The PRIMARY replica is the pool itself in [`UserPool::region`], rendered by
/// [`UserPool::to_primary_replica`], so it is not stored here a second time.
#[derive(Debug, Clone)]
pub struct UserPoolReplica {
    /// The Region the replica was created in, which is this resource's key.
    pub region_name: String,
    /// The ARN of the replica user pool: this pool's id under the replica's
    /// Region ([`UserPool::arn_in_region`]).
    pub user_pool_arn: String,
    /// The tags the create submitted. "You can maintain tags independently on
    /// replica user pools" (`CreateUserPoolReplica.UserPoolTags`), so these are
    /// the replica's own rather than the pool's.
    pub tags: HashMap<String, String>,
    /// When the replica was created.
    pub creation_date: SystemTime,
    /// The instant the clock owes this replica's `CREATING` -> `INACTIVE` move.
    /// Stored, never slept on: a read computes the status from it.
    pub active_at: SystemTime,
    /// The status an `UpdateUserPoolReplica` call last wrote, which SUPERSEDES
    /// the clock-owed one: that operation is how a caller "modif[ies] the
    /// status to activate or deactivate the replica", so the status it set is
    /// the status every later read reports. `None` until a call sets one.
    pub requested_status: Option<String>,
    /// The instant the clock owes this replica's removal, once a delete has
    /// started; until then the replica reports [`REPLICA_STATUS_DELETING`], and
    /// from then on it is gone.
    pub removal_due: Option<SystemTime>,
}

impl UserPoolReplica {
    /// A new secondary replica in `region_name`, `CREATING` until the clock
    /// makes its activation due one [`REPLICA_ACTIVATION_SPAN`] from `now`.
    pub fn new(
        region_name: String,
        user_pool_arn: String,
        tags: HashMap<String, String>,
        now: SystemTime,
    ) -> Self {
        Self {
            region_name,
            user_pool_arn,
            tags,
            creation_date: now,
            active_at: now + REPLICA_ACTIVATION_SPAN,
            requested_status: None,
            removal_due: None,
        }
    }

    /// This replica's `ReplicaStatusType` as of `now`: `DELETING` once a delete
    /// has started, else the status a call last set, else `CREATING` until the
    /// clock has made its activation due and `INACTIVE` from then on — "The
    /// replica has been created, but is not accepting requests for end-users"
    /// (`UserPoolReplicaType.Status`), which `UpdateUserPoolReplica` is what
    /// activates.
    pub fn status(&self, now: SystemTime) -> &str {
        if self.removal_due.is_some() {
            return REPLICA_STATUS_DELETING;
        }
        if let Some(status) = &self.requested_status {
            return status;
        }
        if now >= self.active_at {
            REPLICA_STATUS_INACTIVE
        } else {
            REPLICA_STATUS_CREATING
        }
    }

    /// Whether the clock has made this replica's removal due, which is when a
    /// started deletion has finished.
    pub fn removed(&self, now: SystemTime) -> bool {
        self.removal_due.is_some_and(|due| now >= due)
    }

    /// Start this replica's deletion: it reports [`REPLICA_STATUS_DELETING`]
    /// until the clock makes [`REPLICA_DELETION_SPAN`] elapse, and is gone from
    /// then on.
    pub fn start_deletion(&mut self, now: SystemTime) {
        self.removal_due = Some(now + REPLICA_DELETION_SPAN);
    }

    /// The modelled view of this replica, rendered from what is stored.
    pub fn to_user_pool_replica_type(&self, now: SystemTime) -> UserPoolReplicaType {
        UserPoolReplicaType {
            region_name: Some(self.region_name.clone()),
            status: Some(self.status(now).to_string()),
            role: Some(REPLICA_ROLE_SECONDARY.to_string()),
            user_pool_arn: Some(self.user_pool_arn.clone()),
        }
    }
}

/// How many client secrets one app client holds at a time:
/// `AddUserPoolClientSecret` "supports up to 2 active secrets per app client for
/// zero-downtime credential rotation workflows" (that operation in this
/// service's model), so the cap is the API's rather than this emulator's.
pub const MAX_CLIENT_SECRETS_PER_CLIENT: usize = 2;

/// The message a `ResourceNotFoundException` carries when the `ClientSecretId` a
/// request names is not one the named app client currently holds a secret for.
///
/// Amazon Cognito answers the same sentence for every secret id — it reports
/// that the secret lookup resolved nothing and quotes neither the id, the app
/// client nor the pool, exactly as [`USER_NOT_FOUND_MESSAGE`] and
/// [`TERMS_NOT_FOUND_MESSAGE`] report their own unresolved lookups, and unlike
/// those two it carries no trailing period. So an id no add ever minted, an id
/// another app client holds and an id an earlier delete already took away are
/// all reported identically, and the refusal never echoes the caller's secret
/// identifier back to it.
pub const CLIENT_SECRET_NOT_FOUND_MESSAGE: &str = "Client secret does not exist";

/// The message `DeleteUserPoolClientSecret`'s `InvalidParameterException`
/// carries when the secret the request names is the only one the app client
/// still holds: "you cannot delete the last remaining secret for an app client"
/// (that operation in this service's model), so an app client never loses the
/// credential that authenticates it.
///
/// Like [`CLIENT_SECRET_NOT_FOUND_MESSAGE`] it is one fixed sentence: it reports
/// the precondition that failed and quotes neither the `ClientSecretId` asked
/// for, the app client nor the pool, so rotation code that branches on this
/// refusal sees the same text for every client, and no `@sensitive` `ClientId`
/// reaches the error string.
pub const LAST_CLIENT_SECRET_MESSAGE: &str = "Cannot delete the only client secret.";

/// One active client secret of an app client: the credential a confidential app
/// client authenticates itself with, beside the id and creation time a listing
/// reports it by.
#[derive(Debug, Clone)]
pub struct ClientSecret {
    /// The secret's unique id. "This identifier follows the format
    /// <client-id>--<epoch-create-time>"
    /// (`ClientSecretDescriptorType.ClientSecretId`), minted by
    /// [`ClientSecret::new`].
    pub id: String,
    /// The secret value itself: either the one the request supplied or one this
    /// emulator generated.
    pub value: String,
    /// When this secret was created.
    pub create_date: SystemTime,
}

impl ClientSecret {
    /// A secret of the app client `client_id` holding `value`, created at `now`.
    ///
    /// The id is "`<client-id>--<epoch-create-time>`"
    /// (`ClientSecretDescriptorType.ClientSecretId`), with the creation time
    /// counted in milliseconds since the epoch so two secrets rotated within one
    /// second still carry different ids.
    pub fn new(client_id: &str, value: String, now: SystemTime) -> Self {
        let created_millis = now
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        Self {
            id: format!("{client_id}--{created_millis}"),
            value,
            create_date: now,
        }
    }

    /// The modelled view of this secret, rendered from what is stored.
    ///
    /// `ClientSecretValue` is left out: "this is only returned when creating a new
    /// secret and only if Amazon Cognito generated the secret"
    /// (`ClientSecretDescriptorType.ClientSecretValue`), so the call that
    /// generates a value is the only one that carries it and every later read
    /// renders the descriptor without it.
    pub fn to_client_secret_descriptor_type(&self) -> ClientSecretDescriptorType {
        ClientSecretDescriptorType {
            client_secret_id: Some(self.id.clone()),
            client_secret_value: None,
            client_secret_create_date: Some(to_date_time(self.create_date)),
        }
    }
}

/// The message a `ResourceNotFoundException` carries when the `ClientId`
/// `DescribeUserPoolClient` reads or `UpdateUserPoolClient` writes names no app
/// client of this emulator at all — an id no create minted, or one an earlier
/// `DeleteUserPoolClient` took away.
///
/// These two operations report that lookup without quoting the id: the sentence
/// is fixed text in full, exactly as [`USER_NOT_FOUND_MESSAGE`] and
/// [`GROUP_NOT_FOUND_MESSAGE`] report their own unresolved lookups, so a
/// `@sensitive` `ClientId` (`ClientIdType` in this service's model) reaches no
/// error string.
///
/// An id that DOES name an app client, of another pool, is a different failure
/// and carries a different sentence — see
/// [`ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState::app_client_lookup_refusal`].
pub const APP_CLIENT_NOT_FOUND_MESSAGE: &str = "User pool client does not exist.";

/// The sentence the operations that report an unresolved app-client lookup BY
/// ITS `ClientId` carry — the ones that resolve a client on the way to another
/// resource (its terms documents, its branding, its UI customization, its
/// secrets), the authentication operations that take a `UserPoolId` beside the
/// `ClientId`, and `DeleteUserPoolClient`, which names the id it was asked to
/// remove.
///
/// The fixed text is `User pool client ` and ` does not exist.`; the one
/// variable part is the `ClientId` the REQUEST carried.
pub fn app_client_not_found_message(client_id: &str) -> String {
    format!("User pool client {client_id} does not exist.")
}

/// The sentence every operation carries when the `ClientId` it was given names
/// an app client that exists but is NOT in the pool the request named.
///
/// The fixed text is `User pool client `, ` does not exist in user pool ` and
/// the trailing `.`; the two variable parts are the `ClientId` and the
/// `UserPoolId` the REQUEST carried, never a value read out of the store.
pub fn app_client_not_found_in_pool_message(client_id: &str, user_pool_id: &str) -> String {
    format!("User pool client {client_id} does not exist in user pool {user_pool_id}.")
}

/// One stored app client of a user pool.
#[derive(Debug, Clone)]
pub struct UserPoolClient {
    pub client_id: String,
    /// This app client's active client secrets, oldest first. A confidential app
    /// client is created holding one, and `AddUserPoolClientSecret` "supports up
    /// to 2 active secrets per app client for zero-downtime credential rotation
    /// workflows" (that operation in this service's model), so a client holds a
    /// LIST of them and each one it holds authenticates a request. A public app
    /// client — one created with neither `GenerateSecret` nor a `ClientSecret` —
    /// holds none.
    pub client_secrets: Vec<ClientSecret>,
    pub user_pool_id: String,
    pub client_name: String,
    pub creation_date: SystemTime,
    pub last_modified_date: SystemTime,
    pub refresh_token_validity: i32,
    pub access_token_validity: Option<i32>,
    pub id_token_validity: Option<i32>,
    pub token_validity_units: Option<TokenValidityUnitsType>,
    pub read_attributes: Option<Vec<String>>,
    pub write_attributes: Option<Vec<String>>,
    pub explicit_auth_flows: Vec<String>,
    pub supported_identity_providers: Option<Vec<String>>,
    pub callback_urls: Option<Vec<String>>,
    pub logout_urls: Option<Vec<String>>,
    pub default_redirect_uri: Option<String>,
    pub allowed_oauth_flows: Option<Vec<String>>,
    pub allowed_oauth_scopes: Option<Vec<String>>,
    pub allowed_oauth_flows_user_pool_client: bool,
    pub analytics_configuration: Option<AnalyticsConfigurationType>,
    pub prevent_user_existence_errors: String,
    pub enable_token_revocation: bool,
    pub enable_propagate_additional_user_context_data: bool,
    pub auth_session_validity: Option<i32>,
    pub refresh_token_rotation: Option<RefreshTokenRotationType>,
    /// This app client's own threat-protection settings, once a call has set
    /// them: `RiskConfigurationType.ClientId` is "the app client where this
    /// configuration is applied", and a client that holds none uses the pool's
    /// — "to reset the threat protection settings of an app client to be
    /// inherited from the user pool, send `UserPoolId` and `ClientId`
    /// parameters only" (`SetRiskConfiguration`), which stores a configuration
    /// with nothing in it.
    pub risk_configuration: Option<RiskConfiguration>,
}

impl UserPoolClient {
    /// Whether `AuthFlow=ADMIN_USER_PASSWORD_AUTH` is enabled for this client.
    pub fn allows_admin_password_auth(&self) -> bool {
        self.allows_any_of(&ADMIN_PASSWORD_AUTH_FLOWS)
    }

    /// Whether `AuthFlow=USER_PASSWORD_AUTH` is enabled for this client.
    pub fn allows_user_password_auth(&self) -> bool {
        self.allows_any_of(&USER_PASSWORD_AUTH_FLOWS)
    }

    /// Whether `AuthFlow=REFRESH_TOKEN_AUTH` / `REFRESH_TOKEN` is enabled for this
    /// client.
    pub fn allows_refresh_token_auth(&self) -> bool {
        self.allows_any_of(&REFRESH_TOKEN_AUTH_FLOWS)
    }

    /// Whether `AuthFlow=USER_AUTH` is enabled for this client.
    pub fn allows_user_auth(&self) -> bool {
        self.allows_any_of(&USER_AUTH_FLOWS)
    }

    /// Whether this app client's stored `ExplicitAuthFlows` enable
    /// machine-to-machine authorization: "to use this operation, you must
    /// configure the app client with a client secret and enable the
    /// `ALLOW_CLIENT_TOKEN_AUTH` authentication flow" (`GetClientToken` in this
    /// service's model). The client secret is the other half of that
    /// configuration, and it is what authenticates the request.
    pub fn allows_client_token_auth(&self) -> bool {
        self.allows_any_of(&[CLIENT_TOKEN_AUTH_FLOW])
    }

    /// Whether this client's stored `ExplicitAuthFlows` carry any of `flows`.
    fn allows_any_of(&self, flows: &[&str]) -> bool {
        self.explicit_auth_flows
            .iter()
            .any(|flow| flows.contains(&flow.as_str()))
    }

    /// Whether this client suppresses "no such user": `PreventUserExistenceErrors
    /// = ENABLED` answers a generic authentication error instead.
    pub fn hides_user_existence(&self) -> bool {
        self.prevent_user_existence_errors == "ENABLED"
    }

    /// Whether `presented` is the `SecretHash` a request for `username` has to
    /// carry to be accepted by this app client.
    ///
    /// `SecretHash` is "a keyed-hash message authentication code (HMAC)
    /// calculated using the secret key of a user pool client and username plus
    /// the client ID in the message" (`SignUp.SecretHash` in this service's
    /// model), carried base64-encoded as the modelled `SecretHashType` pattern
    /// `^[\w+=/]+$` admits.
    ///
    /// A client with NO stored secret has no such secret key, so there is
    /// nothing to verify and every request to it is accepted. A client that
    /// HOLDS a secret accepts only the tag THAT secret produces over
    /// [`secret_hash_message`]: a request that carries none, or one computed
    /// from another secret, another username or another client, proves nothing
    /// about holding the secret.
    ///
    /// The check is `ring`'s own tag verification, so a rejected hash tells a
    /// caller nothing about how much of the expected one it got right.
    /// A client that holds SEVERAL secrets accepts the tag ANY of them produces:
    /// each one is an active credential of the app client, which is what lets a
    /// caller move from one to the next without an outage.
    pub fn accepts_secret_hash(&self, username: &str, presented: Option<&str>) -> bool {
        if self.client_secrets.is_empty() {
            return true;
        }
        let Some(presented) = presented else {
            return false;
        };
        let Ok(tag) = ::aws_smithy_types::base64::decode(presented) else {
            return false;
        };
        let message = secret_hash_message(username, &self.client_id);
        self.client_secrets.iter().any(|secret| {
            let key = ::ring::hmac::Key::new(::ring::hmac::HMAC_SHA256, secret.value.as_bytes());
            ::ring::hmac::verify(&key, message.as_bytes(), &tag).is_ok()
        })
    }

    /// Whether `presented` is the `ClientSecret` this app client requires a request
    /// to carry.
    ///
    /// `ClientSecret` is "the client secret of the requested app client, if the
    /// client has a secret" (this service's model), so a client that holds NO
    /// secret has nothing for a request to present and every request to it is
    /// accepted, while one that holds a secret accepts only that value — a request
    /// that carries none, or another client's secret, proves nothing about holding
    /// it. The comparison is a constant-time one, so a rejected secret tells a
    /// caller nothing about how much of it was right. A client that holds SEVERAL
    /// secrets accepts any one of them: each is an active credential of the app
    /// client.
    pub fn accepts_client_secret(&self, presented: Option<&str>) -> bool {
        if self.client_secrets.is_empty() {
            return true;
        }
        presented.is_some_and(|presented| {
            self.client_secrets.iter().any(|secret| {
                bool::from(::subtle::ConstantTimeEq::ct_eq(
                    presented.as_bytes(),
                    secret.value.as_bytes(),
                ))
            })
        })
    }

    /// The app client secret a read of this client renders: its oldest active
    /// secret, which is the one the create minted or accepted, or `None` for a
    /// public app client that holds none.
    ///
    /// `UserPoolClientType.ClientSecret` is "the app client secret" (this
    /// service's model) — one value, while an app client under rotation holds up
    /// to [`MAX_CLIENT_SECRETS_PER_CLIENT`], so the member renders the secret the
    /// client already had and a later read of the SAME client still renders it.
    pub fn client_secret(&self) -> Option<&str> {
        self.client_secrets
            .first()
            .map(|secret| secret.value.as_str())
    }

    /// Whether this app client already holds every secret it may hold at once,
    /// which is what makes one more [`MAX_CLIENT_SECRETS_PER_CLIENT`] exceeded.
    pub fn client_secrets_at_limit(&self) -> bool {
        self.client_secrets.len() >= MAX_CLIENT_SECRETS_PER_CLIENT
    }

    /// Store `value` as one more active secret of this app client, created at
    /// `now`, and answer the record as stored.
    ///
    /// Secrets are appended, so the order they were created in is the order a
    /// read reports them in and [`Self::client_secret`] keeps naming the same
    /// one. The minted id names this app client and this instant, and the instant
    /// is carried forward until no secret of this client already holds that id,
    /// so one id never names two secrets.
    pub fn add_client_secret(&mut self, value: String, now: SystemTime) -> ClientSecret {
        let mut secret = ClientSecret::new(&self.client_id, value, now);
        while self.client_secrets.iter().any(|held| held.id == secret.id) {
            let created = secret.create_date + Duration::from_millis(1);
            secret = ClientSecret::new(&self.client_id, secret.value, created);
        }
        self.client_secrets.push(secret.clone());
        secret
    }

    /// How long a refresh token this client rotates out stays valid for retries, or
    /// `None` when this client does not rotate refresh tokens at all.
    ///
    /// `RefreshTokenRotationType` is "the configuration of your app client for
    /// refresh token rotation. When enabled, your app client issues new ID, access,
    /// and refresh tokens when users renew their sessions with refresh tokens. When
    /// disabled, token refresh issues only ID and access tokens", and
    /// `RetryGracePeriodSeconds` "is `0`" means "the grace period is disabled and a
    /// successful request immediately invalidates the submitted refresh token"
    /// (this service's model). A client that stores no rotation configuration, and
    /// one whose `Feature` is not switched on, rotates nothing; one that rotates
    /// and asked for no grace period gets none. The modelled range of
    /// `RetryGracePeriodSeconds` starts at 0, so the boundary validator has already
    /// refused a negative value before any handler reads one.
    pub fn refresh_token_rotation_grace(&self) -> Option<Duration> {
        let rotation = self.refresh_token_rotation.as_ref()?;
        if rotation.feature != FEATURE_ENABLED {
            return None;
        }
        let seconds = rotation
            .retry_grace_period_seconds
            .and_then(|seconds| u64::try_from(seconds).ok())
            .unwrap_or_default();
        Some(Duration::from_secs(seconds))
    }

    /// The access token lifetime this client issues.
    pub fn access_token_ttl(&self) -> Duration {
        token_ttl(
            self.access_token_validity,
            self.token_validity_units
                .as_ref()
                .and_then(|units| units.access_token.as_deref()),
            // "The default time unit for AccessTokenValidity in an API request is hours."
            TokenValidityUnit::Hours,
            DEFAULT_ACCESS_TOKEN_TTL,
        )
    }

    /// The id token lifetime this client issues.
    pub fn id_token_ttl(&self) -> Duration {
        token_ttl(
            self.id_token_validity,
            self.token_validity_units
                .as_ref()
                .and_then(|units| units.id_token.as_deref()),
            TokenValidityUnit::Hours,
            DEFAULT_ACCESS_TOKEN_TTL,
        )
    }

    /// The refresh token lifetime this client issues. A stored `0` is the
    /// model's "Amazon Cognito overrides the value with the default value of 30
    /// days".
    pub fn refresh_token_ttl(&self) -> Duration {
        let validity = (self.refresh_token_validity != 0).then_some(self.refresh_token_validity);
        token_ttl(
            validity,
            self.token_validity_units
                .as_ref()
                .and_then(|units| units.refresh_token.as_deref()),
            // "The default time unit for RefreshTokenValidity in an API request is days."
            TokenValidityUnit::Days,
            DEFAULT_REFRESH_TOKEN_TTL,
        )
    }

    /// How long a challenge session this client starts stays valid.
    pub fn auth_session_validity(&self) -> Duration {
        let minutes = self
            .auth_session_validity
            .and_then(|minutes| u64::try_from(minutes).ok())
            .unwrap_or(DEFAULT_AUTH_SESSION_VALIDITY_MINUTES);
        Duration::from_secs(minutes * 60)
    }

    /// The modelled view of this client, rendered from what is stored.
    pub fn to_user_pool_client_type(&self) -> UserPoolClientType {
        UserPoolClientType {
            user_pool_id: Some(self.user_pool_id.clone()),
            client_name: Some(self.client_name.clone()),
            client_id: Some(self.client_id.clone()),
            client_secret: self.client_secret().map(ToString::to_string),
            last_modified_date: Some(to_date_time(self.last_modified_date)),
            creation_date: Some(to_date_time(self.creation_date)),
            refresh_token_validity: self.refresh_token_validity,
            access_token_validity: self.access_token_validity,
            id_token_validity: self.id_token_validity,
            token_validity_units: self.token_validity_units.clone(),
            read_attributes: self.read_attributes.clone(),
            write_attributes: self.write_attributes.clone(),
            explicit_auth_flows: Some(self.explicit_auth_flows.clone()),
            supported_identity_providers: self.supported_identity_providers.clone(),
            callback_ur_ls: self.callback_urls.clone(),
            logout_ur_ls: self.logout_urls.clone(),
            default_redirect_uri: self.default_redirect_uri.clone(),
            allowed_o_auth_flows: self.allowed_oauth_flows.clone(),
            allowed_o_auth_scopes: self.allowed_oauth_scopes.clone(),
            allowed_o_auth_flows_user_pool_client: Some(self.allowed_oauth_flows_user_pool_client),
            analytics_configuration: self.analytics_configuration.clone(),
            prevent_user_existence_errors: Some(self.prevent_user_existence_errors.clone()),
            enable_token_revocation: Some(self.enable_token_revocation),
            enable_propagate_additional_user_context_data: Some(
                self.enable_propagate_additional_user_context_data,
            ),
            auth_session_validity: self.auth_session_validity,
            refresh_token_rotation: self.refresh_token_rotation.clone(),
        }
    }
}

/// A user's confirmation state. The value set is `UserStatusType` from this
/// service's model (`smithy_query(query="shape",shape="UserStatusType")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserStatus {
    Unconfirmed,
    Confirmed,
    Archived,
    Compromised,
    Unknown,
    ResetRequired,
    ForceChangePassword,
    ExternalProvider,
}

impl UserStatus {
    /// Whether a user in this status holds a password of its OWN, which is what
    /// a password reset applies to.
    ///
    /// A user that has not confirmed its sign-up (`UNCONFIRMED`) has not
    /// finished registering, and one an administrator created holds a temporary
    /// password it must replace at its first sign-in
    /// (`FORCE_CHANGE_PASSWORD`) — "the user must change their password to
    /// complete sign-in" — so neither owns a password yet. Every other status
    /// names a user whose registration is complete.
    pub fn holds_own_password(self) -> bool {
        !matches!(self, Self::Unconfirmed | Self::ForceChangePassword)
    }

    /// The wire value of this status.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unconfirmed => "UNCONFIRMED",
            Self::Confirmed => "CONFIRMED",
            Self::Archived => "ARCHIVED",
            Self::Compromised => "COMPROMISED",
            Self::Unknown => "UNKNOWN",
            Self::ResetRequired => "RESET_REQUIRED",
            Self::ForceChangePassword => "FORCE_CHANGE_PASSWORD",
            Self::ExternalProvider => "EXTERNAL_PROVIDER",
        }
    }
}

/// One stored user of a user pool.
#[derive(Debug, Clone)]
pub struct User {
    pub username: String,
    /// The user's immutable id, also its `sub` attribute.
    pub sub: String,
    /// Attributes keyed by name, `sub` included, ordered so a read is stable.
    pub attributes: BTreeMap<String, String>,
    /// The values this user has asked for but not confirmed yet, keyed by
    /// attribute name: a pool that requires verification before an update
    /// "doesn't change the value of the attribute until your user responds to
    /// the verification message and confirms the new value"
    /// (`UserAttributeUpdateSettings.AttributesRequireVerificationBeforeUpdate`),
    /// and `VerifyUserAttribute` "updates the affected attribute to its pending
    /// value" (this service's model). Never rendered: a pending value is not
    /// the user's value yet.
    pub pending_attributes: BTreeMap<String, String>,
    pub password: Option<String>,
    /// The passwords this user held BEFORE the one it holds now, most recently
    /// superseded first: "users can't set a password that matches any of `n`
    /// previous passwords, where `n` is the value of `PasswordHistorySize`"
    /// (`PasswordPolicyType.PasswordHistorySize` in this service's model), so
    /// every path that replaces a password records the one it replaced here
    /// ([`User::set_password`]). Never rendered — no modelled member reports a
    /// password — and bounded by [`MAX_PASSWORD_HISTORY_PER_USER`].
    pub password_history: Vec<String>,
    /// Whether [`User::password`] is permanent; a temporary one must be changed
    /// at the next sign-in and expires with the pool's policy.
    pub password_permanent: bool,
    pub password_set_at: Option<SystemTime>,
    pub status: UserStatus,
    pub enabled: bool,
    pub mfa_options: Vec<MFAOptionType>,
    pub preferred_mfa_setting: Option<String>,
    pub mfa_setting_list: Vec<String>,
    pub user_create_date: SystemTime,
    pub user_last_modified_date: SystemTime,
    pub groups: BTreeSet<String>,
    /// The devices remembered for this user, keyed by `DeviceKey` and ordered so
    /// a listing of several is stable. `DeviceKeyType` carries no
    /// case-insensitivity trait, so a key is matched exactly as it was stored.
    pub devices: BTreeMap<String, Device>,
    /// The verification codes outstanding for this user, keyed by what each was
    /// issued for and ordered so a read is stable.
    ///
    /// A fresh code for one purpose REPLACES the one issued for it before, so a
    /// user holds at most one code per purpose and this map is bounded by
    /// [`CodePurpose`]'s variants and the attributes a message is sent for at all
    /// ([`delivery_medium`]).
    pub outstanding_codes: BTreeMap<CodePurpose, VerificationCode>,
    /// The passkey credentials registered to this user, in REGISTRATION order:
    /// `WebAuthnCredentialDescription.CreatedAt` is "the date and time when the
    /// item was created" (this service's model), and a completed registration
    /// appends, so the order the credentials are held in is the order they were
    /// created in. Bounded by [`MAX_WEB_AUTHN_CREDENTIALS_PER_USER`].
    pub web_authn_credentials: Vec<WebAuthnCredential>,
    /// The external identity-provider identities linked to this local profile,
    /// keyed by the identity: `AdminLinkProviderForUser` "links an existing
    /// user account in a user pool, or `DestinationUser`, to an identity from
    /// an external IdP, or `SourceUser`" (this service's model), so a link is
    /// held on the local profile it resolves to and is gone once that profile
    /// is. Ordered, so a read of several is stable, and bounded by
    /// [`MAX_LINKED_PROVIDER_IDENTITIES`]. Rendered as the profile's
    /// [`IDENTITIES_ATTRIBUTE`].
    pub provider_links: BTreeMap<ProviderIdentity, LinkedProvider>,
    /// Whether this user has REGISTERED a time-based one-time password (TOTP)
    /// authenticator: "users must register a TOTP authenticator before they set
    /// this as their preferred MFA method"
    /// (`SetUserMFAPreference.SoftwareTokenMfaSettings` in this service's model),
    /// so this is the delivery configuration TOTP MFA is delivered with
    /// ([`User::holds_mfa_delivery_config`]). A user registers one by associating
    /// a software token and verifying a code from it, so `false` until that
    /// registration succeeds.
    pub software_token_registered: bool,
    /// Whether this user has activated or deactivated passkey MFA for itself
    /// (`SetUserMFAPreference.WebAuthnMfaSettings.Enabled`), or `None` while the
    /// user has expressed no preference and the pool's passkey configuration
    /// alone decides ([`User::activates_passkey_mfa`]).
    pub web_authn_mfa_enabled: Option<bool>,
}

impl User {
    /// The stored attributes as the modelled list, in name order, with the
    /// [`IDENTITIES_ATTRIBUTE`] this profile's provider links render among
    /// them.
    pub fn attribute_list(&self) -> Vec<AttributeType> {
        let mut attributes: Vec<AttributeType> = self
            .attributes
            .iter()
            .map(|(name, value)| AttributeType {
                name: name.clone(),
                value: Some(value.clone()),
            })
            .collect();
        if let Some(identities) = self.identities_attribute() {
            attributes.push(AttributeType {
                name: IDENTITIES_ATTRIBUTE.to_string(),
                value: Some(identities),
            });
            attributes.sort_by(|left, right| left.name.cmp(&right.name));
        }
        attributes
    }

    /// The value this profile's [`IDENTITIES_ATTRIBUTE`] renders: the links it
    /// holds as the JSON array Amazon Cognito reports them in, oldest link
    /// first. `None` for a profile that holds no link, which renders no such
    /// attribute at all.
    pub fn identities_attribute(&self) -> Option<String> {
        if self.provider_links.is_empty() {
            return None;
        }
        let mut links: Vec<(&ProviderIdentity, &LinkedProvider)> =
            self.provider_links.iter().collect();
        links.sort_by(|(left_identity, left), (right_identity, right)| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left_identity.cmp(right_identity))
        });
        let rendered: Vec<serde_json::Value> = links
            .into_iter()
            .map(|(identity, link)| {
                serde_json::json!({
                    // The identity's own value is the id the external IdP knows
                    // this user by, and the provider is the pool IdP it
                    // federates through, whose stored type is reported beside
                    // it.
                    "userId": identity.provider_attribute_value,
                    "providerName": identity.provider_name,
                    "providerType": link.provider_type,
                    // The issuer of a linked identity is known only once the
                    // external IdP has asserted one, so none is invented.
                    "issuer": serde_json::Value::Null,
                    // The local profile the link resolves to is the primary
                    // identity: a linked external identity is an additional way
                    // into that profile, not the identity that created it.
                    "primary": false,
                    "dateCreated": unix_millis(link.created_at),
                })
            })
            .collect();
        Some(serde_json::Value::Array(rendered).to_string())
    }

    /// The modelled view of this user, rendered from what is stored.
    pub fn to_user_type(&self) -> UserType {
        UserType {
            username: Some(self.username.clone()),
            attributes: Some(self.attribute_list()),
            user_create_date: Some(to_date_time(self.user_create_date)),
            user_last_modified_date: Some(to_date_time(self.user_last_modified_date)),
            enabled: self.enabled,
            user_status: Some(self.status.as_str().to_string()),
            mfa_options: non_empty(&self.mfa_options),
        }
    }

    /// Whether this user's stored value of `name` is an ACTIVE alias value: an
    /// `email` or a `phone_number` counts only while its verified flag says the
    /// value was confirmed ([`verified_flag`]), since an unconfirmed value is
    /// not one a sign-in resolves; an alias attribute that carries no such flag
    /// counts whatever its value.
    pub fn alias_is_active(&self, name: &str) -> bool {
        match verified_flag(name) {
            Some(flag) => self
                .attributes
                .get(flag)
                .is_some_and(|value| value == FLAG_CONFIRMED),
            None => true,
        }
    }

    /// The keys of the devices this user's OWN device surface reports, in
    /// ascending order — the stable order a paginated self-service listing
    /// indexes into.
    ///
    /// `ListDevices` lists "the devices that Amazon Cognito has registered to
    /// the currently signed-in user", `ConfirmDevice` "confirms a device that a
    /// user wants to remember" and `UpdateDeviceStatus` marks a device "as
    /// remembered or not remembered" (this service's model), so the
    /// self-service surface answers for the devices the user still REMEMBERS:
    /// a device a sign-in only minted a key for is not reported until it is
    /// confirmed, and one whose remembered status was cleared leaves the
    /// listing. The admin surface is wider — `AdminGetDevice` "returns details
    /// for a user's device" and reports a registered device whatever its
    /// remembered status ([`User::confirmed_device_keys`]) — exactly as
    /// [`User::remembered_device`] is narrower than [`User::confirmed_device`].
    pub fn device_keys(&self) -> Vec<String> {
        self.devices
            .iter()
            .filter(|(_, device)| device.confirmed && device.remembered)
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// The keys of every device REGISTERED to this user, in ascending order:
    /// what the admin device surface reports, whether or not the user still
    /// remembers each ([`User::device_keys`]).
    pub fn confirmed_device_keys(&self) -> Vec<String> {
        self.devices
            .iter()
            .filter(|(_, device)| device.confirmed)
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// Store `options` as this user's SMS MFA configuration, as of `now`.
    ///
    /// The submitted list REPLACES what the user held, so an empty list leaves
    /// the user with no MFA option at all. SMS MFA is activated exactly while an
    /// option is stored for it, so `UserMFASettingList` gains
    /// [`SMS_MFA_SETTING`] with the first option and loses it with the last, and
    /// a factor that is not activated is not one the user prefers, so a
    /// preference for SMS MFA goes with it. Every other factor's setting is
    /// untouched: an `MFAOptionType` configures SMS and nothing else.
    pub fn set_sms_mfa_options(&mut self, options: Vec<MFAOptionType>, now: SystemTime) {
        let activated = !options.is_empty();
        self.mfa_options = options;
        self.mfa_setting_list
            .retain(|setting| setting != SMS_MFA_SETTING);
        if activated {
            self.mfa_setting_list.push(SMS_MFA_SETTING.to_string());
        } else if self.preferred_mfa_setting.as_deref() == Some(SMS_MFA_SETTING) {
            self.preferred_mfa_setting = None;
        }
        // A successful call stores the submitted configuration over the one the
        // user held, which is a change to the user record itself.
        self.user_last_modified_date = now;
    }

    /// Whether this user has ACTIVATED `factor` as an MFA factor.
    ///
    /// `UserMFASettingList` reports "the MFA options that are activated for the
    /// user. The possible values in this list are `SMS_MFA`, `EMAIL_OTP`, and
    /// `SOFTWARE_TOKEN_MFA`", and `ConfiguredUserAuthFactors` reports the same
    /// factors under `AuthFactorType`'s names (both in this service's model), so
    /// each activated setting activates the factor it maps to
    /// ([`mfa_auth_factor_of`]) and a setting that maps to none activates nothing.
    pub fn activates_mfa_auth_factor(&self, factor: &str) -> bool {
        self.mfa_setting_list
            .iter()
            .any(|setting| mfa_auth_factor_of(setting.as_str()) == Some(factor))
    }

    /// Whether this user holds the delivery configuration the
    /// `UserMFASettingList` setting `setting` is delivered with.
    ///
    /// Each of the three factors reaches the user through something the user has
    /// to hold first: SMS message MFA through the verified phone number the text
    /// message is sent to, email message MFA through the verified email address
    /// the message is sent to ([`User::verified_destination_of`]), and TOTP MFA
    /// through the registered authenticator the one-time password is read from
    /// ([`User::software_token_registered`]). A value that names none of the three
    /// settings is delivered by nothing.
    pub fn holds_mfa_delivery_config(&self, setting: &str) -> bool {
        match setting {
            SMS_MFA_SETTING => self.verified_destination_of("phone_number").is_some(),
            EMAIL_OTP_MFA_SETTING => self.verified_destination_of("email").is_some(),
            SOFTWARE_TOKEN_MFA_SETTING => self.software_token_registered,
            _ => false,
        }
    }

    /// Apply `requested` as this user's MFA preference as of `now`, or report why
    /// it is refused.
    ///
    /// Each submitted setting REPLACES what this user held for the factor it
    /// configures, and a factor the request names none of is left exactly as it
    /// was. Activating a factor needs the delivery configuration it reaches the
    /// user through ([`User::holds_mfa_delivery_config`]), so a request that asks
    /// for one the user cannot receive is refused; and a factor the request does
    /// not activate is not one it can also make PREFERRED, so a setting that asks
    /// for the preference without the activation is refused for the preference
    /// itself. The WHOLE request is checked before anything is written, so a
    /// refused one leaves this user untouched.
    ///
    /// "Only one factor can be set as preferred" (`SetUserMFAPreference` in this
    /// service's model), so the preference a request expresses replaces the one
    /// the user held; and a factor that is not activated is not one the user
    /// prefers, so deactivating the preferred factor, or submitting it as no
    /// longer preferred, leaves the user with no preference at all.
    pub fn set_mfa_preference(
        &mut self,
        requested: &[MfaSettingRequest<'_>],
        now: SystemTime,
    ) -> Result<(), MfaPreferenceRejection> {
        // A factor the request does not ACTIVATE is not one it can also make
        // preferred, whatever the user holds for it, so that is judged first and
        // reported as the preference it refuses.
        if requested
            .iter()
            .any(|setting| setting.preferred && !setting.activates())
        {
            return Err(MfaPreferenceRejection::PreferredWhileDisabled);
        }
        if let Some(refused) = requested
            .iter()
            .find(|setting| setting.activates() && !self.holds_mfa_delivery_config(setting.setting))
        {
            return Err(MfaPreferenceRejection::NoDeliveryConfig(
                refused.setting.to_string(),
            ));
        }
        for setting in requested {
            if setting.setting == SMS_MFA_SETTING {
                // SMS MFA is activated exactly while an MFA option is stored for
                // it ([`User::set_sms_mfa_options`]), so activating the factor
                // stores the one option that configures it and deactivating the
                // factor stores none.
                let options = if setting.enabled {
                    vec![sms_mfa_option()]
                } else {
                    Vec::new()
                };
                self.set_sms_mfa_options(options, now);
            } else {
                self.mfa_setting_list.retain(|held| held != setting.setting);
                if setting.enabled {
                    self.mfa_setting_list.push(setting.setting.to_string());
                }
            }
            if setting.enabled && setting.preferred {
                self.preferred_mfa_setting = Some(setting.setting.to_string());
            } else if self.preferred_mfa_setting.as_deref() == Some(setting.setting) {
                self.preferred_mfa_setting = None;
            }
        }
        if !requested.is_empty() {
            self.user_last_modified_date = now;
        }
        Ok(())
    }

    /// Register `credential` to this user, APPENDING it: the credentials are
    /// held in registration order ([`User::web_authn_credentials`]), so the
    /// passkey registered last is read last.
    pub fn register_web_authn_credential(&mut self, credential: WebAuthnCredential) {
        self.web_authn_credentials.push(credential);
    }

    /// Whether this user already holds the passkey credential `credential_id`.
    pub fn holds_web_authn_credential(&self, credential_id: &str) -> bool {
        self.web_authn_credentials
            .iter()
            .any(|credential| credential.credential_id == credential_id)
    }

    /// Delete the passkey credential `credential_id` from this user, reporting
    /// whether one was held under that id.
    ///
    /// A credential id identifies ONE passkey, so the FIRST credential held
    /// under that id is the one removed and the credentials that remain keep
    /// their registration order ([`User::web_authn_credentials`]).
    pub fn delete_web_authn_credential(&mut self, credential_id: &str) -> bool {
        let Some(held) = self
            .web_authn_credentials
            .iter()
            .position(|credential| credential.credential_id == credential_id)
        else {
            return false;
        };
        self.web_authn_credentials.remove(held);
        true
    }

    /// Store whether passkey MFA is activated for this user, as of `now`.
    pub fn set_web_authn_mfa_enabled(&mut self, enabled: bool, now: SystemTime) {
        self.web_authn_mfa_enabled = Some(enabled);
        self.user_last_modified_date = now;
    }

    /// Whether passkey MFA is activated for this user.
    ///
    /// `WebAuthnMfaSettings.Enabled` "specifies whether passkey MFA is activated
    /// for a user" (this service's model), so a user that deactivated it presents
    /// no passkey as an MFA factor. A user that expressed no preference leaves the
    /// pool's passkey configuration to decide ([`UserPool::passkeys_satisfy_mfa`]).
    pub fn activates_passkey_mfa(&self) -> bool {
        self.web_authn_mfa_enabled.is_none_or(|enabled| enabled)
    }

    /// Replace this user's password with `password`, as of `now`, recording the
    /// password it replaces in this user's history.
    ///
    /// Every path that sets a password for an EXISTING user goes through here,
    /// so the history a pool's `PasswordHistorySize` is read against
    /// ([`UserPool::restricts_password_reuse`]) holds every password the user
    /// has had, whichever operation set it. `permanent` is whether the user may
    /// sign in with it straight away or owes a change at its next sign-in
    /// ([`User::password_permanent`]).
    pub fn set_password(&mut self, password: String, permanent: bool, now: SystemTime) {
        if let Some(replaced) = self.password.replace(password) {
            self.password_history.insert(0, replaced);
            self.password_history.truncate(MAX_PASSWORD_HISTORY_PER_USER);
        }
        self.password_permanent = permanent;
        self.password_set_at = Some(now);
        self.user_last_modified_date = now;
    }

    /// This user's passwords, most recently set FIRST: the one it holds now,
    /// then the ones it held before it, as far back as
    /// [`MAX_PASSWORD_HISTORY_PER_USER`] keeps them.
    pub fn recent_passwords(&self) -> impl Iterator<Item = &str> {
        self.password
            .as_deref()
            .into_iter()
            .chain(self.password_history.iter().map(String::as_str))
    }

    /// Whether the stored password is a temporary one that has outlived
    /// `validity` as of `now`.
    pub fn temporary_password_expired(&self, validity: Duration, now: SystemTime) -> bool {
        if self.password_permanent {
            return false;
        }
        // The deadline is computed with the CHECKED addition: a window whose end
        // is not a representable instant is a deadline that never arrives, so
        // such a password is not expired rather than a failure of this read.
        self.password_set_at
            .and_then(|set_at| set_at.checked_add(validity))
            .is_some_and(|deadline| now > deadline)
    }

    /// Store `device` for this user, evicting the device authenticated longest
    /// ago while the user is already at [`MAX_DEVICES_PER_USER`].
    pub fn store_device(&mut self, device: Device) {
        while self.devices.len() >= MAX_DEVICES_PER_USER {
            let Some(oldest) = self
                .devices
                .iter()
                .min_by_key(|(_, stored)| stored.last_authenticated_date)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.devices.remove(&oldest);
        }
        self.devices.insert(device.device_key.clone(), device);
    }

    /// The stored device `device_key` names, once `ConfirmDevice` has confirmed
    /// it. `GetDevice` "returns information about a remembered device" and
    /// `ConfirmDevice` "confirms a device that a user wants to remember" (this
    /// service's model), so a device a sign-in minted and nothing confirmed is
    /// not yet a device of this user pool.
    pub fn confirmed_device(&self, device_key: &str) -> Option<&Device> {
        self.devices
            .get(device_key)
            .filter(|device| device.confirmed)
    }

    /// The stored device `device_key` names, mutably, once it is confirmed.
    pub fn confirmed_device_mut(&mut self, device_key: &str) -> Option<&mut Device> {
        self.devices
            .get_mut(device_key)
            .filter(|device| device.confirmed)
    }

    /// The stored device `device_key` names, once it is confirmed AND remembered.
    ///
    /// The self-service reader is narrower than the admin one: `GetDevice`
    /// "returns information about a remembered device for the current user",
    /// while `AdminGetDevice` "returns details for a user's device" and
    /// `ListDevices` lists "the devices that Amazon Cognito has registered to the
    /// currently signed-in user" (this service's model). So a device the user has
    /// not (yet) chosen to remember is registered and listed, but is not a
    /// remembered device.
    pub fn remembered_device(&self, device_key: &str) -> Option<&Device> {
        self.confirmed_device(device_key)
            .filter(|device| device.remembered)
    }

    /// The stored value of `attribute` a message can be addressed to, or `None`
    /// when this user holds no value for it: a code is a message, so it needs an
    /// address to be sent to.
    pub fn destination_of(&self, attribute: &str) -> Option<String> {
        self.attributes
            .get(attribute)
            .filter(|value| !value.is_empty())
            .cloned()
    }

    /// The stored value of `attribute` whose verified flag says this user
    /// confirmed it ([`verified_flag`]) \u2014 the only kind of destination a
    /// password-reset message is sent to.
    pub fn verified_destination_of(&self, attribute: &str) -> Option<String> {
        self.destination_of(attribute)
            .filter(|_| self.alias_is_active(attribute))
    }

    /// Record `code` as the code outstanding for `purpose`, REPLACING whatever
    /// was outstanding for it before \u2014 which is what makes a re-sent code
    /// supersede the one before it \u2014 and return what was stored.
    pub fn issue_code(&mut self, purpose: CodePurpose, code: VerificationCode) -> VerificationCode {
        self.outstanding_codes.insert(purpose, code.clone());
        code
    }

    /// The code outstanding for `purpose`, or `None` when nothing is.
    pub fn outstanding_code(&self, purpose: &CodePurpose) -> Option<&VerificationCode> {
        self.outstanding_codes.get(purpose)
    }

    /// Check `presented` against the code outstanding for `purpose` as of `now`.
    ///
    /// A PURE read: accepting a code is what spends it ([`User::consume_code`]),
    /// so a refused attempt leaves the outstanding code exactly as it was and a
    /// caller can present the right one afterwards.
    ///
    /// A presented value is compared only against a LIVE code. Where none is
    /// live for `purpose` — none was ever issued, the one that was has been
    /// spent, or its validity window has closed — there is nothing to compare
    /// against at all, so the answer is [`CodeRejection::Expired`]: the
    /// remediation is to ask for a code. Only a value that differs from a code
    /// that IS live, a code a later one superseded among them, is the
    /// [`CodeRejection::Mismatch`] whose remediation is to present the right
    /// code.
    pub fn check_code(
        &self,
        purpose: &CodePurpose,
        presented: &str,
        now: SystemTime,
    ) -> Result<&VerificationCode, CodeRejection> {
        let Some(outstanding) = self.outstanding_code(purpose) else {
            return Err(CodeRejection::Expired);
        };
        if outstanding.expired(now) {
            return Err(CodeRejection::Expired);
        }
        if outstanding.code != presented {
            return Err(CodeRejection::Mismatch);
        }
        Ok(outstanding)
    }

    /// Spend the code outstanding for `purpose`: a delivered code is accepted
    /// ONCE, so presenting it again finds nothing outstanding.
    pub fn consume_code(&mut self, purpose: &CodePurpose) -> Option<VerificationCode> {
        self.outstanding_codes.remove(purpose)
    }

    /// Write `plan` ([`plan_attribute_update`]) onto this user and return the
    /// delivery of each verification message it sent, in the order the plan
    /// lists them.
    ///
    /// The plan was decided against this user as it stood, so applying it never
    /// refuses anything: a request is judged in full BEFORE any of it is
    /// written, and only a plan reaches here.
    pub fn apply_attribute_update(
        &mut self,
        plan: &AttributeUpdatePlan,
        now: SystemTime,
    ) -> Vec<CodeDeliveryDetailsType> {
        for update in &plan.updates {
            match &update.value {
                // A blank value deletes the attribute, so neither a stored nor
                // a pending value of it survives.
                None => {
                    self.attributes.remove(&update.name);
                    self.pending_attributes.remove(&update.name);
                }
                // A pool that requires verification first keeps the stored
                // value and holds the new one pending.
                Some(value) if update.deferred => {
                    self.pending_attributes
                        .insert(update.name.clone(), value.clone());
                }
                Some(value) => {
                    self.attributes.insert(update.name.clone(), value.clone());
                    self.pending_attributes.remove(&update.name);
                }
            }
            if let Some(flag) = update.stale_verified_flag {
                if update.value.is_none() {
                    // The attribute it vouches for is gone, so the flag has
                    // nothing left to describe.
                    self.attributes.remove(flag);
                } else {
                    self.attributes
                        .insert(flag.to_string(), FLAG_UNCONFIRMED.to_string());
                }
            }
        }
        // A call that names no attribute to change modifies nothing, so the
        // user's last-modified instant stands.
        if !plan.updates.is_empty() {
            self.user_last_modified_date = now;
        }

        // A reported delivery is a code that was SENT, so each one is minted
        // and recorded against the attribute it was sent for — which is the
        // code `VerifyUserAttribute` accepts for it, and which supersedes any
        // code outstanding for that attribute before. The destination is the
        // value the message went to: the pending one where the pool holds the
        // update until the user confirms it.
        let mut deliveries = Vec::new();
        for (attribute, value) in &plan.verifications {
            let code = VerificationCode::issued(attribute, value, now);
            let stored = self.issue_code(CodePurpose::AttributeVerification(attribute.clone()), code);
            deliveries.extend(stored.delivery_details());
        }
        deliveries
    }

    /// Write `plan` ([`plan_attribute_deletion`]) onto this user.
    ///
    /// The plan was decided against this user as it stood, so applying it never
    /// refuses anything: a request is judged in full BEFORE any of it is
    /// written, and only a plan reaches here.
    pub fn apply_attribute_deletion(&mut self, plan: &AttributeDeletionPlan, now: SystemTime) {
        // Naming an attribute the user holds no value of removes nothing, so it
        // is the values that WERE held that make this call a modification.
        let mut removed = false;
        for name in &plan.deletions {
            removed |= self.attributes.remove(name).is_some();
            // A value asked for but not confirmed yet is a value of the same
            // attribute, so it goes with the attribute rather than outliving it.
            removed |= self.pending_attributes.remove(name).is_some();
        }
        // A call that removed nothing changed nothing, so the user's
        // last-modified instant stands.
        if removed {
            self.user_last_modified_date = now;
        }
    }
}

/// One stored link from an external IdP identity to a local profile: what the
/// identity's own triple does not carry, so a read can render the link.
#[derive(Debug, Clone)]
pub struct LinkedProvider {
    /// The `ProviderType` of the pool IdP the identity federates through, as
    /// `CreateIdentityProvider` stored it.
    pub provider_type: String,
    /// When the link was made, which is what the rendered identity reports as
    /// its creation instant.
    pub created_at: SystemTime,
}

/// The attribute a local profile reports its provider links under: Amazon
/// Cognito renders a linked user's identities as a JSON array under the
/// `identities` attribute name, so a profile nothing is linked to reports no
/// such attribute.
pub const IDENTITIES_ATTRIBUTE: &str = "identities";

/// The username a user of a third-party IdP signs in under: the provider's name
/// and the user's id with that provider, joined by `_` — the form a `Username`
/// naming "the username of a user from a third-party IdP" (`AdminGetUser.Username`
/// in this service's model) carries. The ONE place the two parts are joined, so
/// every lookup of a federated username agrees
/// ([`UserPool::federated_user_key`]).
#[must_use]
pub fn federated_username(provider_name: &str, provider_user_id: &str) -> String {
    format!("{provider_name}_{provider_user_id}")
}

/// One provider identity: the (`ProviderName`, `ProviderAttributeName`,
/// `ProviderAttributeValue`) triple a `ProviderUserIdentifierType` names.
///
/// It is the key a link is stored and looked up under, because the attribute
/// identifying the external user is part of the identity: de-linking requires
/// that "the `ProviderAttributeName` and `ProviderAttributeValue` must be the
/// same values that were used for the `SourceUser` when the identities were
/// originally linked" (`AdminDisableProviderForUser` in this service's model).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProviderIdentity {
    pub provider_name: String,
    pub provider_attribute_name: String,
    pub provider_attribute_value: String,
}

/// The member of a `ProviderUserIdentifierType` a submitted identifier left
/// out, as [`ProviderIdentity::from_identifier`] reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingProviderMember {
    ProviderName,
    ProviderAttributeName,
    ProviderAttributeValue,
}

impl MissingProviderMember {
    /// The modelled member name, as a refusal names it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProviderName => "ProviderName",
            Self::ProviderAttributeName => "ProviderAttributeName",
            Self::ProviderAttributeValue => "ProviderAttributeValue",
        }
    }
}

impl ProviderIdentity {
    /// The identity `identifier` names, or the first member it left out.
    ///
    /// Every member of a `ProviderUserIdentifierType` is optional in the model,
    /// so an identifier naming no provider, no attribute or no value identifies
    /// nothing and the refusal names the member that is missing. An empty value
    /// is no value either: `ProviderNameType` starts at length 1, and an
    /// identity is what a link is keyed by.
    pub fn from_identifier(
        identifier: &ProviderUserIdentifierType,
    ) -> Result<Self, MissingProviderMember> {
        Ok(Self {
            provider_name: provider_member(&identifier.provider_name)
                .ok_or(MissingProviderMember::ProviderName)?,
            provider_attribute_name: provider_member(&identifier.provider_attribute_name)
                .ok_or(MissingProviderMember::ProviderAttributeName)?,
            provider_attribute_value: provider_member(&identifier.provider_attribute_value)
                .ok_or(MissingProviderMember::ProviderAttributeValue)?,
        })
    }

    /// The value that names the local profile a DESTINATION identifier resolves
    /// to, or the member it left out.
    ///
    /// "The `ProviderAttributeName` of the `DestinationUser` is ignored"
    /// (`AdminLinkProviderForUser` in this service's model), so a destination is
    /// read as the provider it names and the value that names the profile —
    /// each under the same rule [`ProviderIdentity::from_identifier`] applies to
    /// every member, so the two identifiers cannot disagree about what an
    /// absent member is.
    pub fn destination_value(
        identifier: &ProviderUserIdentifierType,
    ) -> Result<String, MissingProviderMember> {
        provider_member(&identifier.provider_name).ok_or(MissingProviderMember::ProviderName)?;
        provider_member(&identifier.provider_attribute_value)
            .ok_or(MissingProviderMember::ProviderAttributeValue)
    }

    /// Whether this identity names a user pool's own users rather than a
    /// third-party IdP ([`NATIVE_PROVIDER_NAME`]).
    pub fn is_native(&self) -> bool {
        self.provider_name == NATIVE_PROVIDER_NAME
    }
}

/// One member of a submitted `ProviderUserIdentifierType`, or `None` when the
/// request left it out: a member sent as whitespace or as an empty string names
/// nothing either, and `ProviderNameType` starts at length 1.
fn provider_member(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// What one stored verification code was issued for.
///
/// Amazon Cognito sends a code for several distinct purposes, and each is
/// answered by its own operation: the sign-up confirmation code by
/// `ConfirmSignUp`, the password-reset code by `ConfirmForgotPassword` and an
/// attribute's code by `VerifyUserAttribute`. Keying the outstanding codes by
/// purpose is what keeps them independent — a resent sign-up code does not
/// disturb an outstanding reset code — while a second code for the SAME purpose
/// supersedes the first.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CodePurpose {
    /// The code that confirms a new user's sign-up, sent by `SignUp` and
    /// re-sent by `ResendConfirmationCode`.
    SignUpConfirmation,
    /// The code that authorises a password reset, sent by `ForgotPassword`.
    PasswordReset,
    /// The code that confirms the value of ONE attribute, sent by
    /// `GetUserAttributeVerificationCode` and by an update of an attribute the
    /// pool verifies, and carried for the attribute it names.
    AttributeVerification(String),
}

/// One verification code issued to a user: what the operation that sent the
/// code recorded, and the delivery it reported.
#[derive(Debug, Clone)]
pub struct VerificationCode {
    /// The code itself ([`new_verification_code`]). The real service delivers
    /// it out of band — by mail or text message — so no response carries it;
    /// it is stored because it is what the operation that consumes this code
    /// checks the presented one against.
    pub code: String,
    /// The user attribute the message was addressed to (`VerifiedAttributeType`),
    /// which is what `CodeDeliveryDetailsType.AttributeName` reports and what
    /// decides the medium it travelled over ([`delivery_medium`]).
    pub attribute: String,
    /// The attribute value the code was sent to: the address or number the
    /// message was addressed to, which is what
    /// `CodeDeliveryDetailsType.Destination` reports, masked.
    pub destination: String,
    /// When this code was issued, which is what its validity window is counted
    /// from.
    pub issued_at: SystemTime,
}

impl VerificationCode {
    /// A fresh verification code for `attribute`, addressed to its stored
    /// `destination`.
    pub fn issued(attribute: &str, destination: &str, now: SystemTime) -> Self {
        Self::minted(new_verification_code(), attribute, destination, now)
    }

    /// A fresh email one-time password for `attribute`, addressed to its stored
    /// `destination`.
    pub fn issued_email_otp(attribute: &str, destination: &str, now: SystemTime) -> Self {
        Self::minted(new_email_otp_code(), attribute, destination, now)
    }

    /// `code`, recorded as issued for `attribute` at `now`.
    fn minted(code: String, attribute: &str, destination: &str, now: SystemTime) -> Self {
        Self {
            code,
            attribute: attribute.to_string(),
            destination: destination.to_string(),
            issued_at: now,
        }
    }

    /// The delivery this code's message reported, rendered from what is stored.
    pub fn delivery_details(&self) -> Option<CodeDeliveryDetailsType> {
        code_delivery_details(&self.attribute, &self.destination)
    }

    /// Whether this code's validity window has closed by `now`.
    pub fn expired(&self, now: SystemTime) -> bool {
        now >= self.issued_at + VERIFICATION_CODE_VALIDITY
    }
}

/// Why a presented code was not accepted, which decides the modelled error the
/// operation that was given it answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeRejection {
    /// A code IS live for the purpose and the presented value is not it —
    /// including the case where a later code for the same purpose superseded
    /// the presented one.
    Mismatch,
    /// No live code is there to compare against: none was ever issued for the
    /// purpose, the one that was has been spent, or its validity window has
    /// closed.
    Expired,
}

/// How long an issued code stays acceptable: Amazon Cognito's verification and
/// confirmation codes expire 24 hours after they are sent (Amazon Cognito
/// Developer Guide, "Signing up and confirming user accounts").
pub const VERIFICATION_CODE_VALIDITY: Duration = Duration::from_secs(24 * 60 * 60);

/// The message a value that is not the live code is refused with
/// ([`CodeRejection::Mismatch`]): it asks for the code to be presented again,
/// because one is there to present.
pub const CODE_MISMATCH_MESSAGE: &str = "Invalid verification code provided, please try again.";

/// The message a check against no live code is refused with
/// ([`CodeRejection::Expired`]): it asks for a code to be REQUESTED, because
/// there is none to present.
pub const EXPIRED_CODE_MESSAGE: &str = "Invalid code provided, please request a code again.";

/// One device of a user: the device a sign-in minted a `DeviceKey` for, as the
/// device operations stored it, and what every later device read of this user
/// renders.
///
/// A sign-in on a pool that remembers devices stores one of these for a device
/// it does not recognise; `ConfirmDevice` is what confirms it — which is what
/// makes the user pool report it — and what decides whether it is remembered.
#[derive(Debug, Clone)]
pub struct Device {
    /// The key this device is known by (`DeviceKeyType`, len 1..55, pattern
    /// `^[\w-]+_[0-9a-f-]+$`), which every device operation names it with.
    pub device_key: String,
    /// The device's attributes keyed by name, ordered so a read is stable.
    pub attributes: BTreeMap<String, String>,
    /// The SRP verifier `ConfirmDevice` registered for this device, when it
    /// carried one. Stored because it is what the device's own authentication
    /// would check; no operation implemented here reads it yet.
    pub secret_verifier: Option<DeviceSecretVerifierConfigType>,
    /// Whether `ConfirmDevice` has confirmed this device, which is what makes it
    /// a device the user pool reports.
    pub confirmed: bool,
    /// Whether this device is remembered, i.e. eligible for device
    /// authentication: `ConfirmDevice` sets it unless the pool asks for the
    /// user's own prompt first, and `UpdateDeviceStatus` is what the user's
    /// answer reaches.
    pub remembered: bool,
    pub create_date: SystemTime,
    pub last_modified_date: SystemTime,
    /// When this device last authenticated a sign-in, absent until one has:
    /// `DeviceType.DeviceLastAuthenticatedDate` is "the date when the device was
    /// last authenticated" (this service's model), which is nothing yet for a
    /// device that has not authenticated.
    pub last_authenticated_date: Option<SystemTime>,
}

impl Device {
    /// The device a sign-in just stored: known to this user pool, not confirmed
    /// and so not remembered, and last authenticated at that sign-in.
    pub fn signed_in(device_key: String, now: SystemTime) -> Self {
        Self {
            device_key,
            attributes: BTreeMap::new(),
            secret_verifier: None,
            confirmed: false,
            remembered: false,
            create_date: now,
            last_modified_date: now,
            last_authenticated_date: Some(now),
        }
    }

    /// The stored attributes as the modelled list, in name order.
    pub fn attribute_list(&self) -> Vec<AttributeType> {
        self.attributes
            .iter()
            .map(|(name, value)| AttributeType {
                name: name.clone(),
                value: Some(value.clone()),
            })
            .collect()
    }

    /// The modelled view of this device, rendered from what is stored.
    pub fn to_device_type(&self) -> DeviceType {
        DeviceType {
            device_key: Some(self.device_key.clone()),
            device_attributes: non_empty(&self.attribute_list()),
            device_create_date: Some(to_date_time(self.create_date)),
            device_last_modified_date: Some(to_date_time(self.last_modified_date)),
            device_last_authenticated_date: self.last_authenticated_date.map(to_date_time),
        }
    }
}

/// The `DeviceAttributes` name a device's `ConfirmDevice.DeviceName` — "A
/// friendly name for the device" — is reported under.
pub const DEVICE_NAME_ATTRIBUTE: &str = "device_name";

/// `DeviceRememberedStatusType`'s value for a device that is remembered, i.e.
/// eligible for device authentication; its only other value is the one for a
/// device that is not (`crate::ops::types::DEVICE_REMEMBERED_STATUS_TYPE_VALUES`).
pub const DEVICE_REMEMBERED_STATUS: &str = "remembered";

/// Why an access token does not identify a signed-in user
/// ([`ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState::signed_in_user`]).
/// The operation that asked maps each one to a modelled error IT declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenRejection {
    /// Not a token this emulator issued, or not an ACCESS token.
    NotAnAccessToken,
    /// An access token this emulator issued and has since revoked, which
    /// authorizes nothing ([`ACCESS_TOKEN_REVOKED_MESSAGE`]).
    Revoked,
    /// An access token whose lifetime the clock is past.
    Expired,
    /// A live access token whose scopes do not authorize the user self-service
    /// operations ([`TokenRecord::authorizes_user_self_service`]).
    MissingRequiredScopes,
    /// The token names a user pool this emulator no longer holds.
    PoolGone,
    /// The token names a user its pool no longer holds.
    UserGone,
    /// The token names a user Amazon Cognito is waiting on a new password from
    /// ([`UserStatus::ResetRequired`]), which authorizes no self-service call
    /// until that password is set.
    PasswordResetRequired,
}

/// How ONE group's `Precedence` ranks against another's, lowest first.
///
/// `CreateGroup`'s `Precedence` (this service's model): "Groups with lower `Precedence`
/// values take precedence over groups with higher or null `Precedence` values", so a group
/// that states none ranks behind every group that states one — which the leading flag
/// orders — while two groups stating the same value rank equally.
type PrecedenceRank = (bool, i32);

/// [`PrecedenceRank`] for a group whose stored `Precedence` is `precedence`.
fn precedence_rank(precedence: Option<i32>) -> PrecedenceRank {
    (precedence.is_none(), precedence.unwrap_or_default())
}

/// What a user's group memberships put in its identity token
/// ([`UserPool::user_group_role_claims`]).
#[derive(Debug, Clone, Default)]
pub struct GroupRoleClaims {
    /// The `cognito:roles` claim: each distinct role the user's groups grant, once, best
    /// precedence first. Empty when no group of the user's grants a role, which is a token
    /// that carries no such claim at all.
    pub roles: Vec<String>,
    /// The `cognito:preferred_role` claim: the role of the best-precedence group, and `None`
    /// where the groups tied at that precedence name different roles — the case
    /// `CreateGroup`'s `Precedence` documents the claim "isn't set" for.
    pub preferred_role: Option<String>,
}

/// One stored group of a user pool: what `CreateGroup` stored and every later
/// read of that group renders.
#[derive(Debug, Clone)]
pub struct Group {
    pub group_name: String,
    pub user_pool_id: String,
    pub description: Option<String>,
    pub role_arn: Option<String>,
    pub precedence: Option<i32>,
    pub creation_date: SystemTime,
    pub last_modified_date: SystemTime,
}

impl Group {
    /// The modelled view of this group, rendered from what is stored. An
    /// optional member the creating request left out stays absent rather than
    /// being invented.
    pub fn to_group_type(&self) -> GroupType {
        GroupType {
            group_name: Some(self.group_name.clone()),
            user_pool_id: Some(self.user_pool_id.clone()),
            description: self.description.clone(),
            role_arn: self.role_arn.clone(),
            precedence: self.precedence,
            creation_date: Some(to_date_time(self.creation_date)),
            last_modified_date: Some(to_date_time(self.last_modified_date)),
        }
    }
}

/// One stored OAuth 2.0 resource server of a user pool: what
/// `CreateResourceServer` stored and every later read of it renders.
#[derive(Debug, Clone)]
pub struct ResourceServer {
    pub user_pool_id: String,
    pub identifier: String,
    pub name: String,
    /// The custom scopes of this resource server, in the order they were
    /// submitted. A resource server created with no `Scopes` holds none.
    pub scopes: Vec<ResourceServerScopeType>,
}

impl ResourceServer {
    /// The modelled view of this resource server, rendered from what is stored.
    pub fn to_resource_server_type(&self) -> ResourceServerType {
        ResourceServerType {
            user_pool_id: Some(self.user_pool_id.clone()),
            identifier: Some(self.identifier.clone()),
            name: Some(self.name.clone()),
            scopes: Some(self.scopes.clone()),
        }
    }
}

/// One stored user import job of a user pool: what `CreateUserImportJob` stored
/// and every later read of it renders.
///
/// The job's `Status` is not stored: it is COMPUTED from the instants below and
/// the clock ([`UserImportJob::status`]), so a read never has to move it.
#[derive(Debug, Clone)]
pub struct UserImportJob {
    pub job_id: String,
    pub job_name: String,
    pub user_pool_id: String,
    /// The role "that has permission to log import-job results to Amazon
    /// CloudWatch Logs" (`CreateUserImportJob.CloudWatchLogsRoleArn`), kept
    /// exactly as it was submitted.
    pub cloud_watch_logs_role_arn: String,
    /// The hashing algorithm the CSV's password hashes were made with, absent on
    /// a job created without one: the model documents no default.
    pub password_hashing_algorithm: Option<String>,
    /// Where a client uploads this job's CSV file, as it was minted when the job
    /// was created, so every read of the job reports the same locator. Absent
    /// when the create named no endpoint to build one from.
    pub pre_signed_url: Option<String>,
    /// The instant a CSV file was uploaded to this job's
    /// [`UserImportJob::pre_signed_url`], absent until one is: a job "imports
    /// users from a CSV file" (`StartUserImportJob`), so a job whose upload
    /// target was never written to has nothing to import.
    pub csv_uploaded_at: Option<SystemTime>,
    /// How many bytes the uploaded CSV carried. The bytes themselves are not
    /// held: this emulator imports no users from them, so the record is the
    /// upload's size and instant rather than a copy of the file.
    pub csv_bytes: usize,
    pub creation_date: SystemTime,
    /// When `StartUserImportJob` accepted this job, absent until it does.
    pub start_date: Option<SystemTime>,
    /// The instant the clock owes this job's `Pending` -> `InProgress` move.
    /// Stored, never slept on: a read computes the status from it.
    pub in_progress_at: Option<SystemTime>,
    /// The instant the clock owes this job's `Stopping` -> `Stopped` move,
    /// absent until `StopUserImportJob` accepted it.
    pub stopped_at: Option<SystemTime>,
}

impl UserImportJob {
    /// A new job of `user_pool_id`, `Created` until it is started or the clock
    /// makes its expiry due.
    pub fn new(
        job_id: String,
        job_name: String,
        user_pool_id: String,
        cloud_watch_logs_role_arn: String,
        password_hashing_algorithm: Option<String>,
        pre_signed_url: Option<String>,
        now: SystemTime,
    ) -> Self {
        Self {
            job_id,
            job_name,
            user_pool_id,
            cloud_watch_logs_role_arn,
            password_hashing_algorithm,
            pre_signed_url,
            csv_uploaded_at: None,
            csv_bytes: 0,
            creation_date: now,
            start_date: None,
            in_progress_at: None,
            stopped_at: None,
        }
    }

    /// This job's `UserImportJobStatusType` as of `now`, computed from the
    /// stored instants: a stopped job is `Stopping` until the clock has made its
    /// stop due and `Stopped` from then on, a started one `Pending` until the
    /// clock has made the import due and `InProgress` from then on, and one that
    /// was never started `Created` until its expiry is due.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if let Some(stopped_at) = self.stopped_at {
            return if now >= stopped_at {
                JOB_STATUS_STOPPED
            } else {
                JOB_STATUS_STOPPING
            };
        }
        if let Some(in_progress_at) = self.in_progress_at {
            return if now >= in_progress_at {
                JOB_STATUS_IN_PROGRESS
            } else {
                JOB_STATUS_PENDING
            };
        }
        if now >= self.creation_date + JOB_EXPIRY_SPAN {
            JOB_STATUS_EXPIRED
        } else {
            JOB_STATUS_CREATED
        }
    }

    /// Whether this job is importing users as of `now` — `Pending` or
    /// `InProgress` — which is the state a stop acts on.
    pub fn is_running(&self, now: SystemTime) -> bool {
        matches!(
            self.status(now),
            JOB_STATUS_PENDING | JOB_STATUS_IN_PROGRESS
        )
    }

    /// Record that a CSV of `bytes` bytes arrived at this job's upload target at
    /// `now`. The latest upload is what the job holds: a client that writes the
    /// target twice has replaced the file the job is going to import.
    pub fn record_csv_upload(&mut self, bytes: usize, now: SystemTime) {
        self.csv_uploaded_at = Some(now);
        self.csv_bytes = bytes;
    }

    /// Whether a CSV file has been uploaded to this job's target, which is what
    /// a start has to import. An EMPTY body uploads no CSV: a file of no bytes
    /// carries neither the header row nor a user.
    #[must_use]
    pub fn has_uploaded_csv(&self) -> bool {
        self.csv_uploaded_at.is_some() && self.csv_bytes > 0
    }

    /// Record that the import was started at `now`, which makes the job
    /// `Pending` and the `InProgress` move due one [`JOB_START_SPAN`] later.
    pub fn start(&mut self, now: SystemTime) {
        self.start_date = Some(now);
        self.in_progress_at = Some(now + JOB_START_SPAN);
    }

    /// Record that the stop was asked for at `now`, which makes the job
    /// `Stopping` and the `Stopped` move due one [`JOB_STOP_SPAN`] later.
    pub fn stop(&mut self, now: SystemTime) {
        self.stopped_at = Some(now + JOB_STOP_SPAN);
    }

    /// The modelled view of this job as of `now`, rendered from what is stored.
    pub fn to_user_import_job_type(&self, now: SystemTime) -> UserImportJobType {
        let status = self.status(now);
        UserImportJobType {
            job_name: Some(self.job_name.clone()),
            job_id: Some(self.job_id.clone()),
            user_pool_id: Some(self.user_pool_id.clone()),
            // The locator the job was minted with, reported exactly as it is
            // stored, so a later read of the job names the same target.
            pre_signed_url: self.pre_signed_url.clone(),
            creation_date: Some(to_date_time(self.creation_date)),
            start_date: self.start_date.map(to_date_time),
            // "The date when the user import job was completed"
            // (`UserImportJobType.CompletionDate`): the instant the job stopped
            // importing users, reported once the clock has made that move due.
            completion_date: self
                .stopped_at
                .filter(|_| status == JOB_STATUS_STOPPED)
                .map(to_date_time),
            status: Some(status.to_string()),
            cloud_watch_logs_role_arn: Some(self.cloud_watch_logs_role_arn.clone()),
            // No CSV was imported, so the three counters stay at the zero the
            // model defaults them to, and `CompletionMessage` — "The message
            // returned when the user import job is completed" — is not invented.
            imported_users: 0,
            skipped_users: 0,
            failed_users: 0,
            completion_message: None,
            password_hashing_algorithm: self.password_hashing_algorithm.clone(),
        }
    }
}

/// One stored third-party identity provider of a user pool: what
/// `CreateIdentityProvider` stored and every later read of it renders.
#[derive(Debug, Clone)]
pub struct IdentityProvider {
    pub user_pool_id: String,
    pub provider_name: String,
    pub provider_type: String,
    /// "The scopes, URLs, and identifiers for your external identity provider"
    /// (`IdentityProviderType.ProviderDetails`): an opaque key/value map, kept
    /// exactly as it was submitted.
    pub provider_details: HashMap<String, String>,
    /// "A mapping of IdP attributes to standard and custom user pool attributes"
    /// (`IdentityProviderType.AttributeMapping`). A provider created with no
    /// mapping holds an empty one.
    pub attribute_mapping: HashMap<String, String>,
    /// The IdP identifiers of this provider, in the order they were submitted. A
    /// provider created with no identifiers holds none.
    pub idp_identifiers: Vec<String>,
    /// The ACR level mapping, which only an OIDC provider carries, so it stays
    /// absent on a provider that was created without one.
    pub acr_mapping: Option<HashMap<String, String>>,
    pub creation_date: SystemTime,
    pub last_modified_date: SystemTime,
}

impl IdentityProvider {
    /// The modelled view of this provider, rendered from what is stored.
    pub fn to_identity_provider_type(&self) -> IdentityProviderType {
        IdentityProviderType {
            user_pool_id: Some(self.user_pool_id.clone()),
            provider_name: Some(self.provider_name.clone()),
            provider_type: Some(self.provider_type.clone()),
            provider_details: Some(self.provider_details.clone()),
            attribute_mapping: Some(self.attribute_mapping.clone()),
            idp_identifiers: Some(self.idp_identifiers.clone()),
            acr_mapping: self.acr_mapping.clone(),
            creation_date: Some(to_date_time(self.creation_date)),
            last_modified_date: Some(to_date_time(self.last_modified_date)),
        }
    }
}

/// Every language `LinksType`'s key admits, in the order its target
/// `LanguageIdType` lists them
/// (`^cognito:(default|dutch|english|french|spanish|german|bahasa-indonesia|italian|japanese|korean|portuguese-brazil|chinese-(simplified|traditional))$`
/// in this service's model). `cognito:default` is the one a language with no
/// entry of its own falls back to.
pub const LINK_LANGUAGES: [&str; 13] = [
    "cognito:default",
    "cognito:dutch",
    "cognito:english",
    "cognito:french",
    "cognito:spanish",
    "cognito:german",
    "cognito:bahasa-indonesia",
    "cognito:italian",
    "cognito:japanese",
    "cognito:korean",
    "cognito:portuguese-brazil",
    "cognito:chinese-simplified",
    "cognito:chinese-traditional",
];

/// The lowest-ordered key of a submitted `Links` map that is not one of
/// [`LINK_LANGUAGES`], or `None` when every key names a language managed login
/// localizes. Lowest-ordered so one request is refused over the same key every
/// time, whatever order the map arrived in.
pub fn unsupported_link_language<'links>(
    links: impl IntoIterator<Item = &'links String>,
) -> Option<&'links String> {
    links
        .into_iter()
        .filter(|language| !LINK_LANGUAGES.contains(&language.as_str()))
        .min()
}

/// One stored set of terms documents of a user pool: what `CreateTerms` stored
/// and every later read of it renders.
#[derive(Debug, Clone)]
pub struct Terms {
    pub terms_id: String,
    pub user_pool_id: String,
    pub client_id: String,
    pub terms_name: String,
    pub terms_source: String,
    pub enforcement: String,
    /// The language-to-URL map, ordered so a read of several languages is
    /// stable. `LinksType` admits 1..13 entries, so this map is never empty.
    pub links: BTreeMap<String, String>,
    pub creation_date: SystemTime,
    pub last_modified_date: SystemTime,
}

impl Terms {
    /// The modelled view of these terms documents, rendered from what is stored.
    pub fn to_terms_type(&self) -> TermsType {
        TermsType {
            terms_id: self.terms_id.clone(),
            user_pool_id: self.user_pool_id.clone(),
            client_id: self.client_id.clone(),
            terms_name: self.terms_name.clone(),
            terms_source: self.terms_source.clone(),
            enforcement: self.enforcement.clone(),
            links: self
                .links
                .iter()
                .map(|(language, url)| (language.clone(), url.clone()))
                .collect(),
            creation_date: to_date_time(self.creation_date),
            last_modified_date: to_date_time(self.last_modified_date),
        }
    }

    /// The summary view a listing renders of these terms documents: the subset
    /// of the stored record `TermsDescriptionType` declares, so a list and a
    /// describe of one document agree member for member on every member both
    /// carry.
    pub fn to_terms_description_type(&self) -> TermsDescriptionType {
        TermsDescriptionType {
            terms_id: self.terms_id.clone(),
            terms_name: self.terms_name.clone(),
            enforcement: self.enforcement.clone(),
            creation_date: to_date_time(self.creation_date),
            last_modified_date: to_date_time(self.last_modified_date),
        }
    }
}

/// One stored managed login branding style of a user pool: what
/// `CreateManagedLoginBranding` persisted and every later read of that style
/// renders.
///
/// A style belongs to exactly one app client of the pool — "Creates a new set of
/// branding settings for a user pool style and associates it with an app
/// client" — and keeps the customizations the request supplied: "Amazon Cognito
/// doesn't require that you pass all parameters in one request and preserves
/// existing style settings that you don't specify"
/// (`CreateManagedLoginBranding` in this service's model).
#[derive(Debug, Clone)]
pub struct ManagedLoginBranding {
    pub branding_id: String,
    pub user_pool_id: String,
    /// The app client the style is assigned to. `ManagedLoginBrandingType`
    /// models no `ClientId`, so this is not rendered; it is what makes the
    /// style reachable by app client and what the uniqueness rule keys on.
    pub client_id: String,
    /// Whether the style takes "the default branding style options" Amazon
    /// Cognito manages (`CreateManagedLoginBranding.UseCognitoProvidedValues`).
    pub use_cognito_provided_values: bool,
    /// The customized settings document, or `None` for a style that carries no
    /// customization of its own.
    pub settings: Option<::aws_smithy_types::Document>,
    /// The customized image files, in the order they were submitted.
    pub assets: Vec<crate::ops::types::AssetType>,
    pub creation_date: SystemTime,
    pub last_modified_date: SystemTime,
}

impl ManagedLoginBranding {
    /// The modelled view of this style, rendered from what is stored.
    ///
    /// `Assets` is reported only when the style stores image files, because the
    /// member is optional and an empty list is not the same answer as no list.
    pub fn to_managed_login_branding_type(&self) -> crate::ops::types::ManagedLoginBrandingType {
        crate::ops::types::ManagedLoginBrandingType {
            managed_login_branding_id: Some(self.branding_id.clone()),
            user_pool_id: Some(self.user_pool_id.clone()),
            use_cognito_provided_values: self.use_cognito_provided_values,
            settings: self.settings.clone(),
            assets: (!self.assets.is_empty()).then(|| self.assets.clone()),
            creation_date: Some(to_date_time(self.creation_date)),
            last_modified_date: Some(to_date_time(self.last_modified_date)),
        }
    }

    /// The MERGED view of this style: the customized view with every branding
    /// option this style leaves at the service's defaults filled in from
    /// [`default_branding_settings`].
    ///
    /// "When true, returns values for branding options that are unchanged from
    /// Amazon Cognito defaults. When false or when you omit this parameter,
    /// returns only values that you customized in your branding style"
    /// (`ReturnMergedResources` on the two readers of a style), so the merged
    /// answer differs from the customized one in `Settings` ALONE: the default
    /// style carries no image file, so a style's `Assets` are its own in both
    /// views, and every other member identifies the stored style.
    pub fn to_merged_managed_login_branding_type(
        &self,
    ) -> crate::ops::types::ManagedLoginBrandingType {
        crate::ops::types::ManagedLoginBrandingType {
            settings: Some(merged_branding_settings(self.settings.as_ref())),
            ..self.to_managed_login_branding_type()
        }
    }
}

/// The branding options this service manages for a style that customizes none
/// of them: the baseline the MERGED view of a style reports, and therefore the
/// only `Settings` a style that takes the Cognito-provided values has.
///
/// The option names are the ones this API documents — `Settings` names
/// `signUp`, `instructions`, `sessionTimerDisplay` and `languageSelector` among
/// the components a style carries (this service's model), and the branding
/// editor groups them under `categories`, `componentClasses` and `components` —
/// while the VALUES are this emulator's defaults: the managed defaults are not
/// in the model, so a style that overrides none of them reads back this
/// document. Rendered fresh per read, so no caller can change what another
/// caller's merged read reports.
pub fn default_branding_settings() -> ::aws_smithy_types::Document {
    document_from_json(&::serde_json::json!({
        "categories": {
            "auth": {
                "authMethodOrder": [
                    [
                        { "display": "BUTTON", "type": "FEDERATED" },
                        { "display": "INPUT", "type": "USERNAME_PASSWORD" }
                    ]
                ],
                "federation": { "interfaceStyle": "BUTTON_LIST", "order": [] }
            },
            "form": {
                "displayGraphics": true,
                "instructions": { "enabled": false },
                "languageSelector": { "enabled": false },
                "location": { "horizontal": "CENTER", "vertical": "CENTER" },
                "sessionTimerDisplay": "NONE"
            },
            "global": {
                "colorSchemeMode": "LIGHT",
                "pageFooter": { "enabled": false },
                "pageHeader": { "enabled": false },
                "spacingDensity": "REGULAR"
            },
            "signUp": { "acceptanceElements": [] }
        },
        "componentClasses": {
            "buttons": { "borderRadius": 8 },
            "divider": { "darkMode": { "borderColor": "232b37ff" }, "lightMode": { "borderColor": "e9ebedff" } },
            "focusState": { "darkMode": { "borderColor": "539fe5ff" }, "lightMode": { "borderColor": "0972d3ff" } },
            "idpButtons": { "icons": { "enabled": true } },
            "input": { "borderRadius": 8 },
            "inputLabel": { "darkMode": {}, "lightMode": {} },
            "link": { "darkMode": {}, "lightMode": {} },
            "optionControls": { "darkMode": {}, "lightMode": {} },
            "statusIndicator": { "darkMode": {}, "lightMode": {} }
        },
        "components": {
            "alert": { "borderRadius": 12 },
            "favicon": { "enabledTypes": ["ICO", "SVG"] },
            "form": {
                "borderRadius": 8,
                "borderWidth": 1,
                "logo": {
                    "enabled": false,
                    "formInclusion": "IN",
                    "location": "CENTER",
                    "position": "TOP"
                }
            },
            "idpButton": { "custom": {}, "standard": { "darkMode": {}, "lightMode": {} } },
            "pageBackground": { "image": { "enabled": false } },
            "pageFooter": { "logo": { "enabled": false } },
            "pageHeader": { "logo": { "enabled": false } },
            "pageText": { "darkMode": {}, "lightMode": {} },
            "phoneNumberSelector": { "displayType": "TEXT" },
            "primaryButton": { "darkMode": {}, "lightMode": {} },
            "secondaryButton": { "darkMode": {}, "lightMode": {} }
        }
    }))
}

/// [`default_branding_settings`] with `customized` laid over it: every option a
/// style customizes is the style's, and every option it leaves alone is the
/// default. A style that customizes nothing reads back the defaults unchanged.
///
/// An option the style supplies REPLACES the default at that name, whatever
/// either side holds — a merged read reports the style's own value, not a value
/// nothing configured. Only where BOTH sides hold a JSON object are the two
/// walked into.
pub fn merged_branding_settings(
    customized: Option<&::aws_smithy_types::Document>,
) -> ::aws_smithy_types::Document {
    let defaults = default_branding_settings();
    match customized {
        None => defaults,
        Some(customized) => overlay_documents(defaults, customized),
    }
}

/// `customized` laid over `base`, member by member: a member only `base` holds
/// stays, a member only `customized` holds is added, and a member both hold is
/// the customized one — except where both hold a JSON object, which is where the
/// two are walked into.
///
/// Walked ITERATIVELY over an explicit worklist of the objects still being
/// merged: BOTH sides can be a document a caller supplied (a stored style's
/// settings under an update's), so the nesting depth is the caller's to choose
/// and a recursive walk of it would put that depth on the stack.
fn overlay_documents(
    base: ::aws_smithy_types::Document,
    customized: &::aws_smithy_types::Document,
) -> ::aws_smithy_types::Document {
    use ::aws_smithy_types::Document;
    // The two objects to merge, or nothing to walk at all: anything else the
    // customization names replaces what `base` held.
    let (base, customized) = match (base, customized) {
        (Document::Object(base), Document::Object(customized)) => (base, customized),
        (_, customized) => return customized.clone(),
    };

    /// One object being merged: the members merged so far (starting as the
    /// base's own), the customized members still to lay over them, and the name
    /// this object is held under in the object one level up.
    struct Pending<'a> {
        name: Option<&'a str>,
        merged: HashMap<String, ::aws_smithy_types::Document>,
        remaining: Vec<(&'a String, &'a ::aws_smithy_types::Document)>,
    }

    let mut worklist = vec![Pending {
        name: None,
        merged: base,
        remaining: customized.iter().collect(),
    }];
    loop {
        let Some(current) = worklist.last_mut() else {
            // Unreachable: the worklist starts non-empty and the last frame
            // finished is returned rather than popped into nothing.
            return Document::Object(HashMap::new());
        };
        match current.remaining.pop() {
            // A member both sides hold as an object: merged one level down.
            Some((name, Document::Object(customized))) => {
                match current.merged.remove(name.as_str()) {
                    Some(Document::Object(base)) => worklist.push(Pending {
                        name: Some(name),
                        merged: base,
                        remaining: customized.iter().collect(),
                    }),
                    _ => {
                        current
                            .merged
                            .insert(name.clone(), Document::Object(customized.clone()));
                    }
                }
            }
            // Every other member the customization names replaces what the base
            // held under that name.
            Some((name, value)) => {
                current.merged.insert(name.clone(), value.clone());
            }
            // This object is merged: it belongs to the object above it, and the
            // outermost one is the answer.
            None => {
                let finished = worklist.pop().expect("the frame just inspected");
                let merged = Document::Object(finished.merged);
                match (worklist.last_mut(), finished.name) {
                    (Some(parent), Some(name)) => {
                        parent.merged.insert(name.to_string(), merged);
                    }
                    _ => return merged,
                }
            }
        }
    }
}

/// The `aws_smithy_types::Document` a JSON value is, so a document this service
/// builds from a JSON literal travels as the modelled `Document` type.
///
/// Called only on a literal this crate wrote, never on caller input: the walk is
/// one level of recursion per nesting level, and the documents it is given nest
/// as deeply as the source they are written in.
pub fn document_from_json(value: &::serde_json::Value) -> ::aws_smithy_types::Document {
    use ::aws_smithy_types::{Document, Number};
    match value {
        ::serde_json::Value::Null => Document::Null,
        ::serde_json::Value::Bool(value) => Document::Bool(*value),
        ::serde_json::Value::Number(value) => {
            if let Some(value) = value.as_u64() {
                Document::Number(Number::PosInt(value))
            } else if let Some(value) = value.as_i64() {
                Document::Number(Number::NegInt(value))
            } else {
                Document::Number(Number::Float(value.as_f64().unwrap_or_default()))
            }
        }
        ::serde_json::Value::String(value) => Document::String(value.clone()),
        ::serde_json::Value::Array(items) => {
            Document::Array(items.iter().map(document_from_json).collect())
        }
        ::serde_json::Value::Object(members) => Document::Object(
            members
                .iter()
                .map(|(name, member)| (name.clone(), document_from_json(member)))
                .collect(),
        ),
    }
}

/// `customized` laid over a style's STORED settings: "Amazon Cognito doesn't
/// require that you pass all parameters in one request and preserves existing
/// style settings that you don't specify", and a style larger than one request
/// may carry is submitted as "multiple requests, each with a size smaller than
/// the limit" (`UpdateManagedLoginBranding` in this service's model) — which
/// only builds up one style if each request's options are laid over the stored
/// ones instead of replacing them.
///
/// `None` for `customized` names no setting at all, so the stored document
/// stands.
pub fn updated_branding_settings(
    stored: Option<&::aws_smithy_types::Document>,
    customized: Option<&::aws_smithy_types::Document>,
) -> Option<::aws_smithy_types::Document> {
    match (stored, customized) {
        (stored, None) => stored.cloned(),
        (None, Some(customized)) => Some(customized.clone()),
        (Some(stored), Some(customized)) => Some(overlay_documents(stored.clone(), customized)),
    }
}

/// `customized` laid over a style's STORED image files, under the same
/// preserve-what-you-did-not-specify rule as [`updated_branding_settings`].
///
/// An image file is the one the style shows for its category in its display
/// mode — "The category that the image corresponds to in your managed login
/// configuration" and "The display-mode target of the asset: light, dark, or
/// browser-adaptive" (`AssetType` in this service's model) — so a submitted
/// file REPLACES the stored file of that category and display mode and leaves
/// every other one alone. The stored order is kept and a file for a pair the
/// style did not hold is appended, so a read of several is stable.
pub fn updated_branding_assets(
    stored: &[crate::ops::types::AssetType],
    customized: &[crate::ops::types::AssetType],
) -> Vec<crate::ops::types::AssetType> {
    let mut merged: Vec<crate::ops::types::AssetType> = stored.to_vec();
    // Where each slot already sits, INDEXED rather than searched for: both lists
    // are as long as the requests that built them, so a scan per submitted file
    // would do work in the product of the two lengths. One lookup per submitted
    // file keeps the merge linear in them, and the key is built once per file
    // rather than once per comparison. The first file of a slot is the one a
    // later request replaces, so a style that somehow holds two of one slot
    // keeps its later one untouched.
    let mut slot_positions: ::std::collections::HashMap<(String, String), usize> =
        ::std::collections::HashMap::with_capacity(merged.len());
    for (position, asset) in merged.iter().enumerate() {
        slot_positions
            .entry((asset.category.clone(), asset.color_mode.clone()))
            .or_insert(position);
    }
    for asset in customized {
        let slot = (asset.category.clone(), asset.color_mode.clone());
        match slot_positions.get(&slot) {
            Some(&position) => merged[position] = asset.clone(),
            None => {
                slot_positions.insert(slot, merged.len());
                merged.push(asset.clone());
            }
        }
    }
    merged
}

/// AWS's refusal message for a managed login branding style this service holds
/// none of: fixed text that names no style, because the request already carries
/// the id that found nothing. Shared by every operation that reaches a style by
/// its `ManagedLoginBrandingId`, so the readers and the writers cannot disagree
/// about what a missing style is.
pub const BRANDING_NOT_FOUND_MESSAGE: &str = "ManagedLoginBranding does not exist.";

/// AWS's refusal message for an app client that has no managed login branding
/// style assigned, naming the app client from the request.
pub fn branding_for_client_not_found_message(client_id: &str) -> String {
    format!("ManagedLoginBranding for client {client_id} does not exist.")
}

/// The message a modification of a branding style that another modification is
/// already applying is refused with: "This exception is thrown if two or more
/// modifications are happening concurrently"
/// (`ConcurrentModificationException` in this service's model). It names no
/// style, so every loser of a race carries this one sentence.
pub const CONCURRENT_BRANDING_MODIFICATION_MESSAGE: &str =
    "Only one request to modify a ManagedLoginBranding can be processed at a time";

/// The `ClientId` that names a user pool's POOL-LEVEL classic hosted UI
/// branding: "Set the default configuration for all clients with a ClientId of
/// ALL" (`SetUICustomization`), which is also the level an app client without
/// branding of its own is served from.
pub const UI_CUSTOMIZATION_ALL_CLIENTS: &str = "ALL";

/// One stored classic hosted UI branding configuration of a user pool: what
/// `SetUICustomization` persisted for one level, and what every later read of
/// that level renders.
///
/// The level is the app client the branding applies to, or
/// [`UI_CUSTOMIZATION_ALL_CLIENTS`] for the pool-level default: "When the
/// ClientId value is an app client ID, the settings you pass in this request
/// apply to that app client and override the default ALL configuration"
/// (`SetUICustomization`).
#[derive(Debug, Clone)]
pub struct UiCustomization {
    /// The level this branding applies to, which is the key it is stored under.
    pub client_id: String,
    /// The stored style sheet: "A plaintext CSS file that contains the custom
    /// fields that you want to apply to your user pool or app client"
    /// (`SetUICustomization.CSS`), kept exactly as submitted.
    pub css: Option<String>,
    /// The stored logo image: "The image that you want to set as your login in
    /// the classic hosted UI" (`SetUICustomization.ImageFile`), kept byte for
    /// byte as submitted.
    pub image_file: Option<Vec<u8>>,
    /// The version this branding is at, reported as
    /// `UICustomizationType.CSSVersion`, "The CSS version number": the stamp of
    /// the write that produced what is stored, so a read reports the version of
    /// the configuration it renders.
    pub css_version: String,
    pub creation_date: SystemTime,
    pub last_modified_date: SystemTime,
}

/// The version a write of a classic hosted UI branding configuration at `at`
/// is at: that instant in UTC as the digits of the modelled timestamp rendering
/// (`yyyymmddhhmmss`), so a later write is at a later version and the version
/// is a number as `UICustomizationType.CSSVersion` reports one.
fn css_version_stamp(at: SystemTime) -> String {
    let seconds = at
        .duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs(seconds)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
        .chars()
        .filter(char::is_ascii_digit)
        .collect()
}

/// The most a managed login branding style's settings and image files may
/// occupy together: "This operation has a 2-megabyte request-size limit and
/// include the CSS settings and image assets for your app client"
/// (`CreateManagedLoginBranding` in this service's model), so a style past it is
/// a style Amazon Cognito would not have accepted in one request.
pub const MAX_BRANDING_PAYLOAD_BYTES: usize = 2 * 1024 * 1024;

/// The bytes a branding style's payload takes on the wire: its settings
/// document plus the image files it carries, which is what
/// [`MAX_BRANDING_PAYLOAD_BYTES`] bounds. An approximation of the request's own
/// size — it counts the members that carry the style and not the surrounding
/// protocol framing — so a style measured inside the limit is one a single
/// request can carry.
pub fn branding_payload_bytes(
    settings: Option<&::aws_smithy_types::Document>,
    assets: &[crate::ops::types::AssetType],
) -> usize {
    let settings = settings.map(document_bytes).unwrap_or_default();
    assets.iter().fold(settings, |total, asset| {
        total
            .saturating_add(asset.category.len())
            .saturating_add(asset.color_mode.len())
            .saturating_add(asset.extension.len())
            .saturating_add(asset.resource_id.as_ref().map_or(0, String::len))
            // A blob travels base64-encoded (`AssetType.Bytes` is "the image
            // file, in Base64-encoded binary"), which is four characters per
            // three bytes, rounded up to the next quantum.
            .saturating_add(base64_len(asset.bytes.as_ref().map_or(0, Vec::len)))
    })
}

/// The length base64 encodes `bytes` bytes to: four characters per three-byte
/// group, the last group padded.
fn base64_len(bytes: usize) -> usize {
    bytes.div_ceil(3).saturating_mul(4)
}

/// The bytes a settings document takes on the wire: every string and member name
/// counts its own UTF-8 length, a number counts eight bytes and every other
/// scalar one byte, so a document cannot be grown without the count growing with
/// it.
///
/// Walked ITERATIVELY over an explicit worklist: the document's nesting depth is
/// the caller's to choose, and a recursive walk of it would put that depth on
/// the stack.
fn document_bytes(document: &::aws_smithy_types::Document) -> usize {
    let mut total: usize = 0;
    let mut pending: Vec<&::aws_smithy_types::Document> = vec![document];
    while let Some(node) = pending.pop() {
        let node_bytes = match node {
            ::aws_smithy_types::Document::Null | ::aws_smithy_types::Document::Bool(_) => 1,
            ::aws_smithy_types::Document::Number(_) => ::std::mem::size_of::<f64>(),
            ::aws_smithy_types::Document::String(text) => text.len(),
            ::aws_smithy_types::Document::Array(items) => {
                pending.extend(items.iter());
                1
            }
            ::aws_smithy_types::Document::Object(members) => {
                members.iter().fold(1_usize, |names, (name, member)| {
                    pending.push(member);
                    names.saturating_add(name.len())
                })
            }
        };
        total = total.saturating_add(node_bytes);
    }
    total
}

/// A challenge session: what a `RespondToAuthChallenge` for this session resumes.
#[derive(Debug, Clone)]
pub struct AuthSession {
    pub user_pool_id: String,
    pub client_id: String,
    pub username: String,
    pub challenge_name: String,
    pub expires_at: SystemTime,
    /// The one-time code this challenge is answered with, for a challenge whose
    /// start DELIVERED one (the email one-time password), and `None` for a
    /// challenge that is answered with something else.
    ///
    /// The code lives on the session because that is what bounds it: the
    /// challenge is answerable only while its session lasts
    /// (`CreateUserPoolClient.AuthSessionValidity` is "the duration, in minutes,
    /// of that session token"), so the code's window is the session's window.
    pub code: Option<VerificationCode>,
    /// Whether a response has already ANSWERED this session's challenge
    /// ([`ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState::end_session`]).
    ///
    /// A spent session stays stored until the clock expires it, which is what
    /// tells a session a caller is replaying apart from a value this emulator
    /// never handed out: the two owe different refusals
    /// ([`SESSION_ALREADY_USED_MESSAGE`] and [`INVALID_AUTH_STATE_MESSAGE`]).
    pub spent: bool,
}

/// What resolving a `Session` token found
/// ([`ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState::session`]).
/// An expired session is told apart from an unknown one because a challenge
/// response owes a different refusal for each.
#[derive(Debug, Clone)]
pub enum SessionLookup {
    /// The stored session, still inside its validity window and not yet
    /// answered.
    Found(AuthSession),
    /// A stored session whose validity window had already closed, as it was
    /// stored: which challenge it was waiting on decides the refusal it owes.
    Expired(AuthSession),
    /// A stored session an earlier response already ANSWERED: the challenge it
    /// maintained the state of is over, so this one is a replay.
    Spent(AuthSession),
    /// No session of that token: this emulator never handed it out.
    Unknown,
}

/// Which token a [`TokenRecord`] records: one of a sign-in's triple, or the
/// machine-to-machine token an app client gets for itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Access,
    Id,
    Refresh,
    /// An access token issued to an APP CLIENT rather than to a user, "for
    /// machine-to-machine (M2M) authorization" (`GetClientToken` in this
    /// service's model). It "authorizes requests to your resource servers", so
    /// it is not the access token "that Amazon Cognito issued to the currently
    /// signed-in user" the self-service operations ask for, and the record of
    /// it names no user.
    ClientCredentials,
}

/// One issued token, so a later operation can resolve it back to its user.
#[derive(Debug, Clone)]
pub struct TokenRecord {
    pub user_pool_id: String,
    pub client_id: String,
    /// The user this token was issued to, and EMPTY for a token that was issued
    /// to the app client itself ([`TokenKind::ClientCredentials`]), which names
    /// no user: a username is at least one character long, so no user-scoped
    /// lookup or revocation matches such a record.
    pub username: String,
    /// The `sub` claim this token carries: the immutable id of the user it was
    /// issued to. A user created later under the same username is a different
    /// user with a different id, so recording the id beside the name is what
    /// keeps a token of a deleted user from resolving to its namesake. A
    /// [`TokenKind::ClientCredentials`] token is issued to the app client
    /// itself, so its subject is that client's id.
    pub sub: String,
    pub kind: TokenKind,
    /// The scopes this token's `scope` claim carries, in the order it carries
    /// them: the self-service scope for an access token of a sign-in, and the
    /// scopes the grant authorized for a client-credentials one. An id or
    /// refresh token carries no `scope` claim, so it holds none.
    pub scopes: Vec<String>,
    /// The `origin_jti` claim this token carries: the id of the SIGN-IN it came
    /// from, shared by the triple minted together and by every access or id
    /// token a refresh with that sign-in's refresh token minted. It is what
    /// makes the tokens "generated by, and at the same time as, the specified
    /// refresh token" (`RevokeToken` in this service's model) one unit a
    /// revocation can name. A [`TokenKind::ClientCredentials`] token is issued
    /// alone and carries no `origin_jti` claim, so its record holds its own
    /// `jti` and shares a lineage with nothing.
    pub origin: String,
    pub issued_at: SystemTime,
    pub expires_at: SystemTime,
    pub revoked: bool,
    /// When refresh token rotation stops this REFRESH token authorizing a token
    /// refresh, once a rotation has replaced it; `None` for a token no rotation
    /// has replaced. Recorded rather than deleted, so a refresh with the
    /// replaced token is answered as the reuse it is
    /// ([`ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState::rotate_out_refresh_token`]).
    pub rotated_out_at: Option<SystemTime>,
}

impl TokenRecord {
    /// Whether this record is of an ACCESS token: the one a sign-in issued to a
    /// user, or the one a client-credentials grant issued to an app client. Both
    /// state `token_use` `access`; an id or refresh token does not.
    pub fn is_access_token(&self) -> bool {
        matches!(self.kind, TokenKind::Access | TokenKind::ClientCredentials)
    }

    /// Whether this token authorizes the user self-service operations: it has to
    /// be an access token a USER's sign-in issued, carrying the
    /// [`USER_ADMIN_SCOPE`] those operations require.
    ///
    /// A client-credentials token authorizes "requests to your resource servers"
    /// (`GetClientToken` in this service's model) and identifies an app client
    /// rather than a signed-in user, so it authorizes none of them whatever its
    /// granted scopes are.
    pub fn authorizes_user_self_service(&self) -> bool {
        self.kind == TokenKind::Access
            && self.scopes.iter().any(|scope| scope == USER_ADMIN_SCOPE)
    }

    /// Whether refresh token rotation has already invalidated this token as of
    /// `now`: a rotation recorded the instant it stops being accepted, and up to
    /// that instant it still authorizes a retry.
    pub fn rotated_out_by(&self, now: SystemTime) -> bool {
        self.rotated_out_at.is_some_and(|invalid_from| now >= invalid_from)
    }
}

/// A passkey registration in flight: what `StartWebAuthnRegistration` handed out
/// credential creation options for, and what the call that completes the
/// registration resolves by the challenge it answers.
#[derive(Debug, Clone)]
pub struct WebAuthnRegistration {
    pub user_pool_id: String,
    /// The app client the access token that started this registration was issued
    /// to: a credential presented with an access token "for a different client
    /// than the one in the original StartWebAuthnRegistration request" is
    /// refused (`WebAuthnClientMismatchException` in this service's model), so
    /// the client is recorded with the challenge.
    pub client_id: String,
    pub username: String,
    /// The relying party the credential is registered under, as it was in force
    /// when the registration started.
    pub relying_party_id: String,
    /// The user-verification requirement the creation options asked for.
    pub user_verification: String,
    pub created_at: SystemTime,
    /// When the challenge stops being answerable, one
    /// [`WEB_AUTHN_REGISTRATION_TIMEOUT`] after it was handed out.
    pub expires_at: SystemTime,
}

/// One passkey credential registered to a user, as the completed registration
/// stored it and every later read of it renders.
#[derive(Debug, Clone)]
pub struct WebAuthnCredential {
    /// "The unique identifier of the passkey credential"
    /// (`WebAuthnCredentialDescription.CredentialId`), which the authenticator
    /// minted and the registration presented.
    pub credential_id: String,
    /// "An automatically-generated friendly name for the passkey credential"
    /// (`WebAuthnCredentialDescription.FriendlyCredentialName`).
    pub friendly_credential_name: String,
    /// The relying party the credential was registered under.
    pub relying_party_id: String,
    /// "The general category of the passkey authenticator"
    /// (`WebAuthnCredentialDescription.AuthenticatorAttachment`), when the
    /// registration reported one.
    pub authenticator_attachment: Option<String>,
    /// "The transport methods of the passkey credential, for example USB or
    /// Bluetooth Low Energy"
    /// (`WebAuthnCredentialDescription.AuthenticatorTransports`).
    pub authenticator_transports: Vec<String>,
    pub created_at: SystemTime,
}

impl WebAuthnCredential {
    /// The modelled view of this passkey credential, rendered from what the
    /// completed registration STORED, so a later read reports the registered
    /// credential and not the request that registered it.
    pub fn to_credential_description(&self) -> WebAuthnCredentialDescription {
        WebAuthnCredentialDescription {
            credential_id: self.credential_id.clone(),
            friendly_credential_name: self.friendly_credential_name.clone(),
            relying_party_id: self.relying_party_id.clone(),
            authenticator_attachment: self.authenticator_attachment.clone(),
            authenticator_transports: self.authenticator_transports.clone(),
            created_at: to_date_time(self.created_at),
        }
    }
}

/// The time unit a token-validity number is counted in
/// (`TokenValidityUnitsType`: `seconds`, `minutes`, `hours`, `days`).
#[derive(Debug, Clone, Copy)]
enum TokenValidityUnit {
    Seconds,
    Minutes,
    Hours,
    Days,
}

impl TokenValidityUnit {
    fn parse(unit: &str) -> Option<Self> {
        match unit {
            "seconds" => Some(Self::Seconds),
            "minutes" => Some(Self::Minutes),
            "hours" => Some(Self::Hours),
            "days" => Some(Self::Days),
            _ => None,
        }
    }

    fn seconds(self) -> u64 {
        match self {
            Self::Seconds => 1,
            Self::Minutes => 60,
            Self::Hours => 60 * 60,
            Self::Days => 24 * 60 * 60,
        }
    }
}

/// The `exp` claim of a token issued `issued_at` seconds after the epoch and
/// living for `ttl`, and `None` when that sum is not representable as the whole
/// seconds a claim states: a token whose expiry cannot be stated is not issued.
fn expiry_claim(issued_at: i64, ttl: Duration) -> Option<i64> {
    issued_at.checked_add(i64::try_from(ttl.as_secs()).ok()?)
}

/// `validity` counted in `unit` (or `default_unit` when the client set none), or
/// `default_ttl` when the client set no validity at all.
fn token_ttl(
    validity: Option<i32>,
    unit: Option<&str>,
    default_unit: TokenValidityUnit,
    default_ttl: Duration,
) -> Duration {
    let Some(validity) = validity.and_then(|validity| u64::try_from(validity).ok()) else {
        return default_ttl;
    };
    let unit = unit
        .and_then(TokenValidityUnit::parse)
        .unwrap_or(default_unit);
    Duration::from_secs(validity.saturating_mul(unit.seconds()))
}

/// Drop entries from `map`, the one `expiry` puts first soonest, until `map`
/// holds at most `keep`.
fn evict_oldest_until<V, F: Fn(&V) -> SystemTime>(
    map: &mut HashMap<String, V>,
    keep: usize,
    expiry: F,
) {
    while map.len() > keep {
        let Some(oldest) = map
            .iter()
            .min_by_key(|(_, value)| expiry(value))
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        map.remove(&oldest);
    }
}

/// Amazon Cognito's default password policy for a new pool.
pub fn default_password_policy() -> PasswordPolicyType {
    PasswordPolicyType {
        minimum_length: Some(DEFAULT_PASSWORD_MIN_LENGTH),
        require_uppercase: true,
        require_lowercase: true,
        require_numbers: true,
        require_symbols: true,
        password_history_size: None,
        temporary_password_validity_days: DEFAULT_TEMPORARY_PASSWORD_VALIDITY_DAYS,
    }
}

/// The pool policies a request leaves unset, filled in with the documented
/// defaults. A policy the request DOES carry is stored as sent.
pub fn effective_policies(requested: Option<UserPoolPolicyType>) -> UserPoolPolicyType {
    let mut policies = requested.unwrap_or_default();
    let mut password_policy = policies
        .password_policy
        .unwrap_or_else(default_password_policy);
    if password_policy.minimum_length.is_none() {
        password_policy.minimum_length = Some(DEFAULT_PASSWORD_MIN_LENGTH);
    }
    if password_policy.temporary_password_validity_days == 0 {
        password_policy.temporary_password_validity_days =
            DEFAULT_TEMPORARY_PASSWORD_VALIDITY_DAYS;
    }
    policies.password_policy = Some(password_policy);
    if policies.sign_in_policy.is_none() {
        policies.sign_in_policy = Some(SignInPolicyType {
            // Password is the first auth factor of a pool that configures none.
            allowed_first_auth_factors: Some(vec!["PASSWORD".to_string()]),
        });
    }
    policies
}

/// The `AdminCreateUserConfig` a request leaves unset, with the model's
/// documented defaults filled in.
pub fn effective_admin_create_user_config(
    requested: Option<AdminCreateUserConfigType>,
) -> AdminCreateUserConfigType {
    let mut config = requested.unwrap_or_default();
    if config.unused_account_validity_days == 0 {
        config.unused_account_validity_days = DEFAULT_UNUSED_ACCOUNT_VALIDITY_DAYS;
    }
    config
}

/// The `EmailSendingAccount` a pool that asks for no email sending method gets:
/// "When Amazon Cognito emails your users, it uses its built-in email
/// functionality. When you use the default option ..."
/// (`EmailConfigurationType.EmailSendingAccount` in this service's model), as
/// against `DEVELOPER`, which sends through the caller's own Amazon SES
/// configuration. One of
/// [`crate::ops::types::EMAIL_SENDING_ACCOUNT_TYPE_VALUES`].
pub const DEFAULT_EMAIL_SENDING_ACCOUNT: &str = "COGNITO_DEFAULT";

/// The `IssuerConfiguration.Type` a pool that asks for none gets: "the ORIGINAL
/// issuer configuration for user pools. The issuer URL is hosted in the user
/// pool's region" (`IssuerConfigurationType.Type` in this service's model),
/// which is the issuer every pool's tokens carry until one is configured
/// otherwise. One of [`crate::ops::types::ISSUER_TYPE_VALUES`].
pub const DEFAULT_ISSUER_TYPE: &str = "ORIGINAL";

/// The `KeyConfiguration.KeyType` a pool that names no key gets: "a key owned by
/// Amazon Web Services in Key Management Service"
/// (`KeyConfigurationType.KeyType` in this service's model), the key a pool that
/// carries no `KmsKeyArn` is encrypted under. One of
/// [`crate::ops::types::ENCRYPTION_KEY_TYPE_VALUES`].
pub const DEFAULT_ENCRYPTION_KEY_TYPE: &str = "AWS_OWNED_KEY";

/// The `DefaultEmailOption` a pool that configures no verification message gets:
/// a verification email carries a code rather than a clickable link, which is
/// the option the code-bearing `{####}` template of
/// `VerificationMessageTemplateType.EmailMessage` is written for. One of
/// [`crate::ops::types::DEFAULT_EMAIL_OPTION_TYPE_VALUES`].
pub const DEFAULT_EMAIL_OPTION: &str = "CONFIRM_WITH_CODE";

/// The `LambdaConfig` a pool is STORED with, which is the one the request
/// carried or no trigger at all.
///
/// A pool always HAS a trigger configuration — every stage runs the function it
/// names, or none ([`crate::state::lambda_triggers`]) — so a request that
/// configures none is stored with an empty collection rather than with nothing,
/// and every read reports the configuration the pool actually applies.
pub fn effective_lambda_config(requested: Option<LambdaConfigType>) -> LambdaConfigType {
    requested.unwrap_or_default()
}

/// The `EmailConfiguration` a pool is STORED with: the request's, with the
/// sending method the pool applies filled in.
///
/// A pool always sends its messages one of the two ways
/// `EmailConfigurationType.EmailSendingAccount` names, so a configuration that
/// names neither is stored with [`DEFAULT_EMAIL_SENDING_ACCOUNT`], the built-in
/// one; a request that carries no configuration at all gets that same method
/// with nothing else set.
pub fn effective_email_configuration(
    requested: Option<EmailConfigurationType>,
) -> EmailConfigurationType {
    let mut configuration = requested.unwrap_or_default();
    if configuration.email_sending_account.is_none() {
        configuration.email_sending_account = Some(DEFAULT_EMAIL_SENDING_ACCOUNT.to_string());
    }
    configuration
}

/// The `IssuerConfiguration` a pool is STORED with: the request's, with
/// [`DEFAULT_ISSUER_TYPE`] filled in where it names no issuer type, since a
/// pool's tokens always carry one issuer.
pub fn effective_issuer_configuration(
    requested: Option<IssuerConfigurationType>,
) -> IssuerConfigurationType {
    let mut configuration = requested.unwrap_or_default();
    if configuration.r#type.is_none() {
        configuration.r#type = Some(DEFAULT_ISSUER_TYPE.to_string());
    }
    configuration
}

/// The `KeyConfiguration` a pool is STORED with: the request's, with
/// [`DEFAULT_ENCRYPTION_KEY_TYPE`] filled in where it names no key type, since a
/// pool's data is always encrypted under one key.
pub fn effective_key_configuration(
    requested: Option<KeyConfigurationType>,
) -> KeyConfigurationType {
    let mut configuration = requested.unwrap_or_default();
    if configuration.key_type.is_none() {
        configuration.key_type = Some(DEFAULT_ENCRYPTION_KEY_TYPE.to_string());
    }
    configuration
}

/// The `UserAttributeUpdateSettings` a pool is STORED with: the request's, with
/// an EMPTY list of attributes filled in where it names none.
///
/// The setting decides, for every attribute, whether a change waits on
/// verification ([`UserPool::requires_verification_before_update`]), so a pool
/// that configures none holds the empty collection — no attribute requires it —
/// rather than nothing.
pub fn effective_user_attribute_update_settings(
    requested: Option<UserAttributeUpdateSettingsType>,
) -> UserAttributeUpdateSettingsType {
    let mut settings = requested.unwrap_or_default();
    if settings
        .attributes_require_verification_before_update
        .is_none()
    {
        settings.attributes_require_verification_before_update = Some(Vec::new());
    }
    settings
}

/// The `VerificationMessageTemplate` a pool is STORED with: the request's, with
/// [`DEFAULT_EMAIL_OPTION`] filled in where it chooses no email option, since a
/// pool's verification emails always carry either a code or a link.
pub fn effective_verification_message_template(
    requested: Option<VerificationMessageTemplateType>,
) -> VerificationMessageTemplateType {
    let mut template = requested.unwrap_or_default();
    if template.default_email_option.is_none() {
        template.default_email_option = Some(DEFAULT_EMAIL_OPTION.to_string());
    }
    template
}

/// The `UserPoolAddOns` a pool is STORED with: `None` for a pool whose request
/// configures no threat protection at all, and otherwise the requested add-ons
/// with the mode for the additional authentication types filled in.
///
/// `AdvancedSecurityAdditionalFlows` holds the mode threat protection runs
/// custom authentication in, and that mode's enum admits no "off" value, so a
/// pool whose add-ons are configured runs them in
/// [`ADVANCED_SECURITY_MODE_AUDIT`] unless the request chose otherwise.
pub fn effective_user_pool_add_ons(
    requested: Option<UserPoolAddOnsType>,
) -> Option<UserPoolAddOnsType> {
    let mut add_ons = requested?;
    let mut flows = add_ons.advanced_security_additional_flows.unwrap_or_default();
    if flows.custom_auth_mode.is_none() {
        flows.custom_auth_mode = Some(ADVANCED_SECURITY_MODE_AUDIT.to_string());
    }
    add_ons.advanced_security_additional_flows = Some(flows);
    Some(add_ons)
}

/// The feature plan a request leaves unset: "Defaults to `ESSENTIALS`".
pub fn effective_user_pool_tier(requested: Option<String>) -> String {
    requested.unwrap_or_else(|| DEFAULT_USER_POOL_TIER.to_string())
}

/// The MFA mode a request leaves unset: MFA is `OFF`.
pub fn effective_mfa_configuration(requested: Option<String>) -> String {
    requested.unwrap_or_else(|| MFA_OFF.to_string())
}

/// The deletion-protection mode a request leaves unset: `INACTIVE`.
pub fn effective_deletion_protection(requested: Option<String>) -> String {
    requested.unwrap_or_else(|| DEFAULT_DELETION_PROTECTION.to_string())
}

/// `PreventUserExistenceErrors` a request leaves unset: "Defaults to `LEGACY`".
pub fn effective_prevent_user_existence_errors(requested: Option<String>) -> String {
    requested.unwrap_or_else(|| DEFAULT_PREVENT_USER_EXISTENCE_ERRORS.to_string())
}

/// One of Amazon Cognito's standard (OIDC) user-pool attributes, as the default
/// schema declares it. `min`/`max` are the bounds the service stamps on that
/// attribute's values: `MinLength`/`MaxLength` for a `String` attribute and
/// `MinValue`/`MaxValue` for a `Number` one
/// (`smithy_query(query="shape",shape="SchemaAttributeType")`); `None` leaves
/// that bound out of the attribute's constraints.
#[derive(Debug, Clone, Copy)]
struct StandardAttribute {
    name: &'static str,
    data_type: &'static str,
    mutable: bool,
    required: bool,
    min: Option<&'static str>,
    max: Option<&'static str>,
}

/// Amazon Cognito's standard (OIDC) user-pool attributes, the schema every new
/// pool starts with. The names are the standard attributes of the Amazon Cognito
/// Developer Guide's "User pool attributes"; the fields are
/// `SchemaAttributeType` (`smithy_query(query="shape",shape="SchemaAttributeType")`).
///
/// Each attribute carries the service's OWN bounds rather than one generic
/// template: the minted identifier is at least one character long, the
/// date-of-birth attribute holds exactly a ten-character date, the number
/// attribute starts at zero, and every other string attribute takes up to the
/// 2048 characters an attribute value may be.
const STANDARD_SCHEMA: [StandardAttribute; 20] = [
    StandardAttribute { name: "sub", data_type: "String", mutable: false, required: true, min: Some("1"), max: Some("2048") },
    StandardAttribute { name: "name", data_type: "String", mutable: true, required: false, min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "given_name", data_type: "String", mutable: true, required: false, min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "family_name", data_type: "String", mutable: true, required: false, min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "middle_name", data_type: "String", mutable: true, required: false, min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "nickname", data_type: "String", mutable: true, required: false, min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "preferred_username", data_type: "String", mutable: true, required: false, min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "profile", data_type: "String", mutable: true, required: false, min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "picture", data_type: "String", mutable: true, required: false, min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "website", data_type: "String", mutable: true, required: false, min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "email", data_type: "String", mutable: true, required: false, min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "email_verified", data_type: "Boolean", mutable: true, required: false, min: None, max: None },
    StandardAttribute { name: "gender", data_type: "String", mutable: true, required: false, min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "birthdate", data_type: "String", mutable: true, required: false, min: Some("10"), max: Some("10") },
    StandardAttribute { name: "zoneinfo", data_type: "String", mutable: true, required: false, min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "locale", data_type: "String", mutable: true, required: false, min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "phone_number", data_type: "String", mutable: true, required: false, min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "phone_number_verified", data_type: "Boolean", mutable: true, required: false, min: None, max: None },
    StandardAttribute { name: "address", data_type: "String", mutable: true, required: false, min: Some("0"), max: Some("2048") },
    StandardAttribute { name: "updated_at", data_type: "Number", mutable: true, required: false, min: Some("0"), max: None },
];

/// The schema attributes the SERVICE itself maintains on every pool, beyond the
/// ones a create configures: the attribute that renders the external identities
/// a profile is linked to ([`IDENTITIES_ATTRIBUTE`], whose value this service
/// writes when a link is made, never a caller). Unbounded, so it reports the
/// empty string constraints of a string attribute with no bounds.
///
/// They belong to the pool from the moment it exists, so every READ of a pool
/// reports them ([`UserPool::reported_schema`]); the create that configured the
/// pool answers with the configured attributes alone
/// ([`UserPool::to_configured_user_pool_type`]).
const SERVICE_MAINTAINED_SCHEMA: [StandardAttribute; 1] = [StandardAttribute {
    name: IDENTITIES_ATTRIBUTE,
    data_type: "String",
    mutable: true,
    required: false,
    min: None,
    max: None,
}];

/// The schema entry `standard` declares.
fn standard_schema_attribute(standard: &StandardAttribute) -> SchemaAttributeType {
    let (number_attribute_constraints, string_attribute_constraints) =
        schema_attribute_constraints(standard.data_type, standard.min, standard.max);
    SchemaAttributeType {
        name: Some(standard.name.to_string()),
        attribute_data_type: Some(standard.data_type.to_string()),
        developer_only_attribute: Some(false),
        mutable: Some(standard.mutable),
        required: Some(standard.required),
        number_attribute_constraints,
        string_attribute_constraints,
    }
}

/// Whether `name` is one of the standard attributes a pool's schema starts
/// from, which a request names rather than adds.
pub fn is_standard_attribute(name: &str) -> bool {
    STANDARD_SCHEMA
        .iter()
        .any(|standard| standard.name == name)
}

/// The `AttributeDataType` values whose values a `String` bound describes, and
/// the one whose values a `Number` bound describes
/// (`smithy_query(query="shape",shape="AttributeDataType")`): an attribute of
/// any OTHER type carries neither constraints structure.
const STRING_ATTRIBUTE_DATA_TYPE: &str = "String";
const NUMBER_ATTRIBUTE_DATA_TYPE: &str = "Number";

/// The constraints structures a schema attribute of `data_type` reports, as
/// `(NumberAttributeConstraints, StringAttributeConstraints)`.
///
/// An attribute reports the structure that describes ITS data type and nothing
/// else: "specifies the constraints for an attribute of the number type" /
/// "... of the string type" (`SchemaAttributeType` in this service's model). The
/// structure is reported even where the attribute is unbounded — then as the
/// EMPTY structure — because it is the attribute's data type, not the presence
/// of a bound, that decides which constraints an attribute has; an attribute of
/// any other type (a boolean, a date) reports neither.
fn schema_attribute_constraints(
    data_type: &str,
    min: Option<&str>,
    max: Option<&str>,
) -> (
    Option<NumberAttributeConstraintsType>,
    Option<StringAttributeConstraintsType>,
) {
    match data_type {
        NUMBER_ATTRIBUTE_DATA_TYPE => (
            Some(NumberAttributeConstraintsType {
                min_value: min.map(str::to_string),
                max_value: max.map(str::to_string),
            }),
            None,
        ),
        STRING_ATTRIBUTE_DATA_TYPE => (
            None,
            Some(StringAttributeConstraintsType {
                min_length: min.map(str::to_string),
                max_length: max.map(str::to_string),
            }),
        ),
        _ => (None, None),
    }
}

/// The columns Amazon Cognito's user-import CSV carries BEYOND the pool's own
/// attributes, in the order the import format puts them: the MFA state of the
/// imported user, the username to create them under (Amazon Cognito Developer
/// Guide, "Importing users from a CSV file") and that user's password as a hash
/// — "the password hashing algorithm used to generate the hashes in the CSV file
/// for this import job" (`CreateUserImportJob.PasswordHashingAlgorithm` in this
/// service's model) is what an import job reads that last column with. They are
/// not attributes of the pool's schema, so they are not reported by any other
/// read of it.
const CSV_HEADER_TRAILING_COLUMNS: [&str; 3] = [
    "cognito:mfa_enabled",
    "cognito:username",
    "password_hash",
];

/// How many digits a minted verification code carries: Amazon Cognito's
/// verification messages carry a six-digit code.
const VERIFICATION_CODE_LENGTH: usize = 6;

/// How many digits a minted email one-time password carries: Amazon Cognito's
/// `EMAIL_OTP` sign-in challenge delivers an eight-digit one-time password.
const EMAIL_OTP_CODE_LENGTH: usize = 8;

/// A fresh verification code: [`VERIFICATION_CODE_LENGTH`] decimal digits.
pub fn new_verification_code() -> String {
    new_numeric_code(VERIFICATION_CODE_LENGTH)
}

/// A fresh email one-time password: [`EMAIL_OTP_CODE_LENGTH`] decimal digits.
pub fn new_email_otp_code() -> String {
    new_numeric_code(EMAIL_OTP_CODE_LENGTH)
}

/// A fresh code of `digits` decimal digits, which satisfies
/// `ConfirmationCodeType`'s modelled length 1..2048 and pattern `^[\S]+$` — the
/// shape the member that carries it back accepts.
///
/// The randomness is the same v4-UUID source every other minted value in this
/// file draws on ([`new_uuid`]), so nothing here is a hand-rolled generator: a
/// v4 UUID renders as 32 hex characters, each reduced to one decimal digit, so
/// one draw always yields the whole code and this makes no loop.
fn new_numeric_code(digits: usize) -> String {
    new_uuid()
        .chars()
        .filter_map(|character| character.to_digit(16))
        .filter_map(|value| char::from_digit(value % 10, 10))
        .take(digits)
        .collect()
}

/// The attribute that records whether `attribute`'s stored value was confirmed,
/// for the two attributes this API verifies (`VerifiedAttributeType`: `email`
/// and `phone_number`, whose flags are the standard `email_verified` and
/// `phone_number_verified`), or `None` for an attribute that carries no such
/// flag.
pub fn verified_flag(attribute: &str) -> Option<&'static str> {
    match attribute {
        "email" => Some("email_verified"),
        "phone_number" => Some("phone_number_verified"),
        _ => None,
    }
}

/// The attribute `value` has the SHAPE of a value of: an `email` holds an
/// address, a `phone_number` holds a number in E.164 form ("A phone number must
/// be in E.164 format", `AdminCreateUser.UserAttributes` in this service's
/// model, which is a `+` and digits), and a value that is neither belongs to no
/// addressing attribute.
///
/// Used to tell which sign-in attribute a submitted sign-in name is a value of
/// ([`UserPool::sign_in_attribute`]); it judges the shape only, never whether a
/// value is deliverable.
pub fn attribute_shape_of(value: &str) -> Option<&'static str> {
    if value.starts_with('+') {
        return Some("phone_number");
    }
    if value.contains('@') {
        return Some("email");
    }
    None
}

/// The value a verified flag carries once the attribute's value is confirmed.
/// `email_verified` and `phone_number_verified` are `Boolean` schema
/// attributes, whose values are carried as the strings every attribute value
/// is.
pub const FLAG_CONFIRMED: &str = "true";

/// The value a verified flag carries while the attribute's value is not
/// confirmed.
pub const FLAG_UNCONFIRMED: &str = "false";

/// Each `DeliveryMediumType` value beside the user attribute whose stored value
/// addresses a message sent over it: an email address is mailed and a phone
/// number is texted, so `EMAIL` and `SMS` — the values
/// [`crate::ops::types::DELIVERY_MEDIUM_TYPE_VALUES`] carries — name the two
/// attributes a message is ever addressed to.
///
/// The ONE place the two value sets are mapped to each other, so the medium a
/// delivery REPORTS and the attribute a delivery NEEDS cannot disagree.
pub const DELIVERY_MEDIUM_ATTRIBUTES: [(&str, &str); 2] =
    [("EMAIL", "email"), ("SMS", "phone_number")];

/// The `DeliveryMediumType` a verification message for `attribute` travels by —
/// an email address is mailed and a phone number is texted — or `None` for an
/// attribute no message is sent for.
pub fn delivery_medium(attribute: &str) -> Option<&'static str> {
    DELIVERY_MEDIUM_ATTRIBUTES
        .iter()
        .find(|(_, addressed_by)| *addressed_by == attribute)
        .map(|(medium, _)| *medium)
}

/// The message the refusal of a request for a verification code of an attribute
/// outside the verifiable set carries.
///
/// Only an attribute whose stored value ADDRESSES a message can have a code sent
/// to it ([`DELIVERY_MEDIUM_ATTRIBUTES`]), and the refusal states that permitted
/// set rather than the name the request asked for, so it is one fixed sentence
/// for every other attribute name.
pub const UNVERIFIABLE_ATTRIBUTE_MESSAGE: &str =
    "Invalid attribute name. Only phone_number and email can be verified.";

/// The user attribute whose stored value addresses a message delivered over
/// `medium` — the inverse of [`delivery_medium`] — or `None` for a value no
/// `DeliveryMediumType` names.
pub fn medium_destination_attribute(medium: &str) -> Option<&'static str> {
    DELIVERY_MEDIUM_ATTRIBUTES
        .iter()
        .find(|(named, _)| *named == medium)
        .map(|(_, addressed_by)| *addressed_by)
}

/// The first medium among `requested` that no address is held for, beside the
/// attribute that would have addressed it, or `None` when every requested
/// medium can be delivered over.
///
/// A message needs an address to go to, and the medium it travels over is what
/// names the attribute holding one ([`DELIVERY_MEDIUM_ATTRIBUTES`]): a request
/// that asks for delivery over a medium the recipient holds no value for asks
/// for a message that can never be addressed, which is a bad parameter rather
/// than a delivery that was attempted. `holds_destination` answers whether a
/// value for the attribute it is given is held.
///
/// A PURE function of the request and the lookup it is given — it reads and
/// writes no stored state itself — so a request refused by it leaves the store
/// exactly as it was. Each requested medium costs ONE call of
/// `holds_destination` and nothing is stored per entry, so a caller that
/// decides the addressing attributes once, ahead of this walk, keeps the whole
/// check linear in the request's size: the submitted medium list bounds nothing
/// that is stored and nothing that is re-scanned. A medium no
/// `DeliveryMediumType` names addresses no attribute and is passed over here:
/// the request walk has already refused a value outside the modelled set.
pub fn undeliverable_medium<F>(
    requested: &[String],
    holds_destination: F,
) -> Option<(&str, &'static str)>
where
    F: Fn(&str) -> bool,
{
    requested.iter().find_map(|medium| {
        let attribute = medium_destination_attribute(medium)?;
        (!holds_destination(attribute)).then_some((medium.as_str(), attribute))
    })
}

/// The message an `InvalidParameterException` carries for a request that asks
/// for delivery over `medium` where no `attribute` addresses it
/// ([`undeliverable_medium`]).
///
/// The refusal reports the medium the request asked for and the attribute that
/// medium needs — both of them values the caller submitted or the model names —
/// so it says what to send instead and discloses nothing stored.
///
/// Amazon Cognito states this condition as two clauses, the message it could not
/// send and the registration it has no record of, and closes neither with a
/// period: a resend with nothing to send to answers "Cannot resend codes. No
/// registered/verified email or phone_number" (the sentence
/// `ResendConfirmationCode` already carries in this crate). This is that
/// sentence for ONE medium and the ONE attribute addressing it, so a caller
/// matching the family matches both.
pub fn undeliverable_medium_message(medium: &str, attribute: &str) -> String {
    format!("Cannot send the message by {medium}. No registered {attribute}")
}

/// What one submitted attribute does to a stored user
/// ([`plan_attribute_update`]).
#[derive(Debug)]
pub struct AttributeUpdate {
    /// The attribute the request named.
    pub name: String,
    /// The value to store, or `None` to delete the attribute (a blank value).
    pub value: Option<String>,
    /// Whether the value is held pending the user's confirmation rather than
    /// stored over the confirmed one.
    pub deferred: bool,
    /// The verified flag this change leaves vouching for a value the user no
    /// longer holds, when the request does not set that flag itself: it goes
    /// unconfirmed beside a value that changed, and away with a value that was
    /// deleted, since a deleted attribute has nothing to vouch for.
    pub stale_verified_flag: Option<&'static str>,
}

/// The whole update: which stored user it applies to, what it does to that
/// user's attributes and the verification messages it sends.
#[derive(Debug)]
pub struct AttributeUpdatePlan {
    /// The pool holding the user this plan applies to.
    pub user_pool_id: String,
    /// The STORED username of that user, however the request named it.
    pub username: String,
    /// What the update does to that user's attributes, one entry per attribute
    /// it changes.
    pub updates: Vec<AttributeUpdate>,
    /// One `(attribute, value)` per verification message this update sends: the
    /// attribute the message is addressed to and the value it goes to, which is
    /// what the code issued for it is recorded against.
    pub verifications: Vec<(String, String)>,
    /// The OTHER users of the pool this update takes an alias value away from,
    /// as `(storage key, verified flag)`: the alias each held is now this
    /// user's, so the flag that made it theirs no longer vouches for anything
    /// ([`UserPool::apply_attribute_update`]).
    pub demotions: Vec<(String, &'static str)>,
}

/// Why a submitted attribute list is no update to make
/// ([`plan_attribute_update`]). The operation that asked maps each one to a
/// modelled error IT declares.
#[derive(Debug)]
pub enum AttributeUpdateRefusal {
    /// The list names no single storable update: the message says which rule it
    /// breaks, for a client to fix the request from, and names no stored value.
    Invalid(String),
    /// The submitted value of the named attribute is one another user of the
    /// pool already holds as its alias ([`UserPool::alias_taken`]) and which no
    /// verified flag can take away from that user, so the two would both answer
    /// to it.
    AliasTaken(String),
}

/// What a verified flag a request submits ITSELF says about the value beside
/// it.
///
/// `AdminUpdateUserAttributes.UserAttributes` documents that "to skip the
/// verification message and update the value of an attribute that requires
/// verification in the same API request, include the `email_verified` or
/// `phone_number_verified` attribute, with a value of `true`" and that Amazon
/// Cognito then "doesn't send a verification message to your user" (this
/// service's model), while the self-service `UpdateUserAttributes` documents no
/// such clause: the caller of that one is the user whose value would be
/// verified, so the flag it submits is not an assertion that it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifiedFlagAuthority {
    /// A submitted flag of [`FLAG_CONFIRMED`] asserts the value beside it is
    /// verified: that value is stored at once and no message is sent for it.
    Asserts,
    /// A submitted flag is stored as the value of a `Boolean` attribute and
    /// asserts nothing: the pool's own settings alone decide whether a new
    /// value waits for a confirmation.
    Records,
}

/// The message a request refused because it would change an attribute the
/// pool's schema does not let change carries.
///
/// `SchemaAttributeType.Mutable` is what "specifies whether the value of the
/// attribute can be changed" (this service's model), so an attribute it freezes
/// is one no request may write — storing a new value or taking the stored one
/// away alike, since both change it. Amazon Cognito reports that in one
/// sentence whose only variable part is the attribute the request named, and
/// names neither the pool nor the user: the schema, not this user's state, is
/// what refused it.
///
/// This is the sentence a DELETION of such an attribute carries. A write that
/// submits a VALUE for it is validated against the schema value by value, so it
/// is refused with that report's own line for the finding
/// ([`schema_validation_report`]) rather than with this sentence.
pub fn non_mutable_attribute_message(attribute_name: &str) -> String {
    format!("Cannot modify the non-mutable attribute {attribute_name}")
}

/// What Amazon Cognito's schema validation of a user's submitted attributes reports
/// about ONE attribute it will not write.
///
/// A write that submits name/value pairs is validated against the pool's schema
/// attribute by attribute, and each finding is reported as its own line: the
/// attribute, under the `user.` prefix the schema gives a user's own attributes,
/// then what is wrong with it, then the newline every line of that report ends
/// with. [`schema_validation_report`] is the report those lines make up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaValidationFinding {
    /// The pool's schema declares no such attribute, so there is nothing to write.
    NotInSchema,
    /// The pool's schema declares the attribute immutable
    /// (`SchemaAttributeType.Mutable`), so its stored value cannot change.
    NotUpdatable,
}

impl SchemaValidationFinding {
    /// The sentence this finding is reported with.
    fn sentence(self) -> &'static str {
        match self {
            Self::NotInSchema => "Attribute does not exist in the schema.",
            Self::NotUpdatable => "Attribute cannot be updated.",
        }
    }
}

/// The report Amazon Cognito's schema validation states for `attributes`, each with the
/// finding it earned: one line per attribute, in the order they are handed over, each
/// ending in its own newline.
///
/// Empty when `attributes` is empty, which is a request that broke nothing.
pub fn schema_validation_report<'a>(
    attributes: impl IntoIterator<Item = (&'a str, SchemaValidationFinding)>,
) -> String {
    attributes
        .into_iter()
        .map(|(name, finding)| format!("user.{name}: {}\n", finding.sentence()))
        .collect()
}

/// The message a request refused for naming an attribute whose type the pool's schema
/// does not declare carries.
///
/// An attribute DELETION names attributes without values
/// (`AttributeNameListType`), so each name's declared type is what says what is being
/// taken away, and a name the schema declares nothing for has no type to resolve at all.
/// Amazon Cognito reports that rather than a schema-validation line, since no value was
/// validated: the attribute is named in braces, as the report names a name it could not
/// resolve.
pub fn undetermined_attribute_type_message(attribute_name: &str) -> String {
    format!(
        "Invalid user attributes: Type for attribute {{{attribute_name}}} could not be determined"
    )
}

/// Decide what `requested` does to `user` of `pool`, or the refusal it earns.
///
/// A pure read of the stored pool and user: it returns the plan rather than
/// applying it ([`User::apply_attribute_update`]), so a request refused halfway
/// through leaves nothing written.
///
/// The rules, each from this service's model:
///
/// * An attribute the pool's schema does not declare cannot hold a value, and
///   one it declares immutable cannot change (`SchemaAttributeType.Mutable`),
///   so neither is an update to make — nor is blanking an attribute the schema
///   marks required, which has to keep a value.
/// * One value per attribute: `AttributeType` is "the name and value of a user
///   attribute", so a list naming the same attribute twice asks for two values
///   of one attribute, and a list naming more attributes than the schema
///   declares asks for more than any user of the pool can hold.
/// * "To delete an attribute from your user, submit the attribute in your API
///   request with a blank value", so a blank value REMOVES the stored
///   attribute rather than storing an empty one.
/// * A new `email` or `phone_number` is no longer the value its verified flag
///   vouched for ([`verified_flag`]), so the flag goes unconfirmed unless the
///   request sets it itself, and a deleted one takes its flag with it.
/// * A pool that verifies the attribute itself (`AutoVerifiedAttributes`) or
///   requires verification before the update
///   (`AttributesRequireVerificationBeforeUpdate`) sends a verification
///   message, and the second holds the new value pending until the user
///   confirms it — unless the caller's own flag asserts it is already verified
///   ([`VerifiedFlagAuthority`]).
/// * An alias value names ONE user of the pool, so a value another user holds
///   as an active alias moves to this user and the other holder's verified flag
///   is demoted with it; an alias attribute that carries no such flag cannot be
///   moved that way, so taking its value is refused.
///
/// Every step below is bounded by the POOL's schema rather than by the length
/// of the submitted list: the count is capped and repeated names are refused
/// before any per-attribute work, so one request cannot drive work the caller
/// chose the size of.
pub fn plan_attribute_update(
    pool: &UserPool,
    user: &User,
    requested: &[AttributeType],
    authority: VerifiedFlagAuthority,
) -> Result<AttributeUpdatePlan, AttributeUpdateRefusal> {
    // A request cannot update more attributes than the pool declares: past
    // that it either names one the schema does not declare or names one twice,
    // both refused below. Checked FIRST, so nothing further is proportional to
    // a list length the caller chose.
    if requested.len() > pool.schema.len() {
        return Err(AttributeUpdateRefusal::Invalid(format!(
            "Invalid attributes given, at most {} attributes of this user pool can be updated in one request",
            pool.schema.len()
        )));
    }
    // Every attribute written has to be one the pool's schema declares. Asked
    // of the submitted list as it arrived, so an undeclared attribute is
    // reported as undeclared whatever else the list carries, and EVERY one of
    // them is reported: the schema validation states a line per attribute it
    // will not write.
    let names: Vec<String> = requested
        .iter()
        .map(|attribute| attribute.name.clone())
        .collect();
    let undeclared: Vec<&str> = names
        .iter()
        .filter(|name| !pool.declares_attribute(name))
        .map(String::as_str)
        .collect();
    if !undeclared.is_empty() {
        return Err(AttributeUpdateRefusal::Invalid(schema_validation_report(
            undeclared
                .into_iter()
                .map(|name| (name, SchemaValidationFinding::NotInSchema)),
        )));
    }
    // A user holds ONE value per attribute, so a list naming the same
    // attribute twice names no single update to make. Keyed by name, so every
    // lookup below is a lookup rather than a scan of the request.
    let mut submitted: BTreeMap<&str, &str> = BTreeMap::new();
    for attribute in requested {
        let value = attribute.value.as_deref().unwrap_or_default();
        if submitted.insert(attribute.name.as_str(), value).is_some() {
            return Err(AttributeUpdateRefusal::Invalid(format!(
                "Invalid attributes given, {} was submitted more than once",
                attribute.name
            )));
        }
    }

    let mut updates = Vec::new();
    let mut verifications = Vec::new();
    let mut demotions: Vec<(String, &'static str)> = Vec::new();
    // `SchemaAttributeType.Mutable` is what says a stored value may change, so an
    // attribute the schema freezes cannot be updated at all — and EVERY one the
    // request names is reported, a line each, exactly as the undeclared ones above
    // are: the schema validation states a line per attribute it will not write.
    // Judged in request order, over the attributes this request actually writes.
    let frozen: Vec<&str> = requested
        .iter()
        .map(|attribute| attribute.name.as_str())
        .filter(|name| !settled_by_a_deletion(name, &submitted))
        .filter(|name| !pool.attribute_mutable(name))
        .collect();
    if !frozen.is_empty() {
        return Err(AttributeUpdateRefusal::Invalid(schema_validation_report(
            frozen
                .into_iter()
                .map(|name| (name, SchemaValidationFinding::NotUpdatable)),
        )));
    }
    for (name, value) in &submitted {
        let name = (*name).to_string();
        // A verified flag whose attribute this same request DELETES has
        // nothing left to record, so the deletion settles the flag (below)
        // rather than the value submitted for it.
        if settled_by_a_deletion(&name, &submitted) {
            continue;
        }
        let value = (*value).to_string();
        if value.is_empty() {
            // An attribute the schema marks required must hold a value, so it
            // is not one a blank value may delete.
            if pool.attribute_required(&name) {
                return Err(AttributeUpdateRefusal::Invalid(format!(
                    "Attribute {name} cannot be deleted: this user pool's schema declares it required"
                )));
            }
            // A deleted email address or phone number leaves its verified flag
            // vouching for a value the user no longer holds, so the flag goes
            // with it.
            let stale_verified_flag = verified_flag(&name);
            updates.push(AttributeUpdate {
                name,
                value: None,
                deferred: false,
                stale_verified_flag,
            });
            continue;
        }

        let unchanged = user
            .attributes
            .get(&name)
            .is_some_and(|stored| stored == &value);
        let verified_flag = verified_flag(&name);
        // A caller who may assert the value verified, and does, has nothing
        // left to verify: the value applies at once and no message goes out.
        let asserted_verified = authority == VerifiedFlagAuthority::Asserts
            && verified_flag.is_some_and(|flag| {
                submitted
                    .get(flag)
                    .is_some_and(|value| *value == FLAG_CONFIRMED)
            });
        // A pool that requires verification before the update holds the new
        // value pending instead of storing it over the confirmed one.
        let deferred = !asserted_verified
            && verified_flag.is_some()
            && pool.requires_verification_before_update(&name);
        if deferred && unchanged {
            // The user already holds this value, confirmed: there is nothing
            // to hold pending and nothing to confirm.
            continue;
        }
        // A value that changed is no longer the one the verified flag vouched
        // for — unless the request sets that flag itself, which is then the
        // value stored for it.
        let stale_verified_flag = verified_flag
            .filter(|flag| !deferred && !unchanged && !submitted.contains_key(*flag));

        // An alias value names ONE user of the pool, so this user taking
        // `value` is the other holders losing it. An `email` or a
        // `phone_number` is an alias only while its verified flag vouches for
        // it ([`verified_flag`]), so the holder's flag is what the write takes
        // away: the update is accepted and the previous holder's copy is
        // demoted out of verified status. An alias attribute that carries no
        // such flag cannot be demoted, so there the value stays the holder's
        // and this update is refused. Only an alias attribute of the pool is
        // looked up, and each is looked up once, so the pool's users are walked
        // at most once per alias attribute the pool configures.
        let becomes_active_alias = !deferred
            && verified_flag
                .is_none_or(|flag| flag_after_update(&submitted, user, flag, &stale_verified_flag));
        if becomes_active_alias && pool.is_alias_attribute(&name) {
            match verified_flag {
                Some(flag) => demotions.extend(
                    pool.alias_holder_keys(&name, &value, &user.username)
                        .into_iter()
                        .map(|key| (key, flag)),
                ),
                None if pool.alias_taken(&name, &value, &user.username) => {
                    return Err(AttributeUpdateRefusal::AliasTaken(name));
                }
                None => {}
            }
        }

        // A verification message goes to a NEW email address or phone number
        // the pool verifies itself, and to one it holds pending until the user
        // confirms it. One entry per attribute at most, since the attributes
        // are the distinct ones submitted.
        if !unchanged
            && !asserted_verified
            && (deferred || pool.auto_verifies(&name))
            && delivery_medium(&name).is_some()
        {
            verifications.push((name.clone(), value.clone()));
        }

        updates.push(AttributeUpdate {
            name,
            value: Some(value),
            deferred,
            stale_verified_flag,
        });
    }

    Ok(AttributeUpdatePlan {
        user_pool_id: pool.id.clone(),
        username: user.username.clone(),
        updates,
        verifications,
        demotions,
    })
}

/// The whole deletion: which stored user it applies to and which of that
/// user's attributes it removes ([`plan_attribute_deletion`]).
#[derive(Debug)]
pub struct AttributeDeletionPlan {
    /// The pool holding the user this plan applies to.
    pub user_pool_id: String,
    /// The STORED username of that user, however the request named it.
    pub username: String,
    /// Every attribute name to remove, each once: the names the request asked
    /// for, plus the verified flag of each, which has nothing left to vouch for
    /// once the value it describes is gone.
    pub deletions: BTreeSet<String>,
}

/// Decide what deleting `requested` does to `user` of `pool`, or the message of
/// the `InvalidParameterException` the request earns — which the asking
/// operation carries in the modelled error IT declares.
///
/// A pure read of the stored pool and user: it returns the plan rather than
/// applying it ([`User::apply_attribute_deletion`]), so a request refused
/// halfway through leaves nothing written.
///
/// The rules, each from this service's model:
///
/// * `AttributeNameListType` is a list of attribute NAMES and a user holds one
///   value per attribute, so naming an attribute twice asks for the same single
///   removal, and a list of more distinct names than the schema declares names
///   more attributes than any user of the pool can hold.
/// * An attribute the pool's schema does not declare cannot hold a value, so it
///   is not one a user can be asked to give up.
/// * An attribute the schema declares immutable (`SchemaAttributeType.Mutable`)
///   cannot change, and taking its value away changes it; one the schema marks
///   required has to keep a value, so neither is one this operation may remove.
/// * A deleted `email` or `phone_number` leaves its verified flag
///   ([`verified_flag`]) vouching for a value the user no longer holds, so the
///   flag goes with it.
///
/// Every per-attribute step below is bounded by the POOL's schema rather than
/// by the length of the submitted list: the names are reduced to the distinct
/// ones and their count is capped before any per-attribute work, so one request
/// cannot drive work the caller chose the size of.
pub fn plan_attribute_deletion(
    pool: &UserPool,
    user: &User,
    requested: &[String],
) -> Result<AttributeDeletionPlan, String> {
    let names: Vec<String> = requested
        .iter()
        .collect::<BTreeSet<&String>>()
        .into_iter()
        .cloned()
        .collect();
    // Checked FIRST, so nothing further is proportional to a list length the
    // caller chose.
    if names.len() > pool.schema.len() {
        return Err(format!(
            "Invalid attributes given, at most {} attributes of this user pool can be deleted in one request",
            pool.schema.len()
        ));
    }
    if let Some(unknown) = pool.unknown_attribute(&names) {
        return Err(undetermined_attribute_type_message(&unknown));
    }

    let mut deletions = BTreeSet::new();
    for name in names {
        if !pool.attribute_mutable(&name) {
            return Err(non_mutable_attribute_message(&name));
        }
        if pool.attribute_required(&name) {
            return Err(format!(
                "Attribute {name} cannot be deleted: this user pool's schema declares it required"
            ));
        }
        // Exactly as a blank value submitted to an attribute update takes it.
        if let Some(flag) = verified_flag(&name) {
            deletions.insert(flag.to_string());
        }
        deletions.insert(name);
    }

    Ok(AttributeDeletionPlan {
        user_pool_id: pool.id.clone(),
        username: user.username.clone(),
        deletions,
    })
}

/// The attribute whose confirmation `flag` records, or `None` when `flag` is
/// not one of the verified flags ([`verified_flag`]).
fn verified_attribute_of(flag: &str) -> Option<&'static str> {
    ["email", "phone_number"]
        .into_iter()
        .find(|attribute| verified_flag(attribute) == Some(flag))
}

/// Whether `name` is a verified flag whose attribute the same request DELETES, so
/// the deletion settles the flag and the value submitted for it writes nothing
/// ([`plan_attribute_update`]).
fn settled_by_a_deletion(name: &str, submitted: &BTreeMap<&str, &str>) -> bool {
    verified_attribute_of(name)
        .is_some_and(|described| submitted.get(described).is_some_and(|value| value.is_empty()))
}

/// Whether `flag` says "confirmed" once this request has been applied: the
/// value the request sets for it if it names it, else `false` where the change
/// makes the flag stale, else the value the user already holds.
fn flag_after_update(
    submitted: &BTreeMap<&str, &str>,
    user: &User,
    flag: &str,
    stale_verified_flag: &Option<&'static str>,
) -> bool {
    if let Some(value) = submitted.get(flag) {
        return *value == FLAG_CONFIRMED;
    }
    if stale_verified_flag.is_some() {
        return false;
    }
    user.attributes
        .get(flag)
        .is_some_and(|value| value == FLAG_CONFIRMED)
}

/// The `UserMFASettingList` value for SMS message MFA: that member is "the MFA
/// options that are activated for the user. The possible values in this list are
/// `SMS_MFA`, `EMAIL_OTP`, and `SOFTWARE_TOKEN_MFA`" (this service's model), and
/// SMS is the one factor an `MFAOptionType` configures.
pub const SMS_MFA_SETTING: &str = "SMS_MFA";

/// The `UserMFASettingList` value for email message MFA, the second of the three
/// values that member documents (see [`SMS_MFA_SETTING`]).
pub const EMAIL_OTP_MFA_SETTING: &str = "EMAIL_OTP";

/// The `UserMFASettingList` value for time-based one-time password MFA, the third
/// of the three values that member documents (see [`SMS_MFA_SETTING`]).
pub const SOFTWARE_TOKEN_MFA_SETTING: &str = "SOFTWARE_TOKEN_MFA";

/// `AuthFactorType`'s password factor, which "can only be used as a first
/// authentication factor" (`ConfiguredUserAuthFactors` in this service's model).
pub const PASSWORD_AUTH_FACTOR: &str = "PASSWORD";

/// `AuthFactorType`'s email one-time-password factor.
pub const EMAIL_OTP_AUTH_FACTOR: &str = "EMAIL_OTP";

/// `AuthFactorType`'s SMS one-time-password factor.
pub const SMS_OTP_AUTH_FACTOR: &str = "SMS_OTP";

/// `AuthFactorType`'s passkey factor.
pub const WEB_AUTHN_AUTH_FACTOR: &str = "WEB_AUTHN";

/// `AuthFactorType`'s time-based one-time-password factor, which "can only be
/// used as an MFA factor" (`ConfiguredUserAuthFactors` in this service's model).
pub const SOFTWARE_TOKEN_AUTH_FACTOR: &str = "SOFTWARE_TOKEN";

/// Every `AuthFactorType` value, in the order the model DECLARES them in
/// (`smithy_query(query="shape",shape="AuthFactorType")`) rather than the
/// alphabetical order the generated `crate::ops::types::AUTH_FACTOR_TYPE_VALUES`
/// catalog carries them in. `ConfiguredUserAuthFactors` reports a user's factors
/// in that declared order, as its own example `["PASSWORD", "WEB_AUTHN"]` shows
/// (this service's model).
pub const AUTH_FACTOR_ORDER: [&str; 5] = [
    PASSWORD_AUTH_FACTOR,
    EMAIL_OTP_AUTH_FACTOR,
    SMS_OTP_AUTH_FACTOR,
    WEB_AUTHN_AUTH_FACTOR,
    SOFTWARE_TOKEN_AUTH_FACTOR,
];

/// The `WebAuthnFactorConfigurationType` value under which "passkey
/// authentication with user verification satisfies MFA requirements"
/// (`WebAuthnConfigurationType.FactorConfiguration` in this service's model); its
/// only other value makes passkeys a single authentication factor.
pub const WEB_AUTHN_MULTI_FACTOR: &str = "MULTI_FACTOR_WITH_USER_VERIFICATION";

/// The `AuthFactorType` value a `UserMFASettingList` setting activates as an MFA
/// factor, or `None` for a setting that names no `AuthFactorType` value. The one
/// place the two value sets are mapped to each other.
pub fn mfa_auth_factor_of(setting: &str) -> Option<&'static str> {
    match setting {
        SMS_MFA_SETTING => Some(SMS_OTP_AUTH_FACTOR),
        EMAIL_OTP_MFA_SETTING => Some(EMAIL_OTP_AUTH_FACTOR),
        SOFTWARE_TOKEN_MFA_SETTING => Some(SOFTWARE_TOKEN_AUTH_FACTOR),
        _ => None,
    }
}

/// The only `DeliveryMediumType` an `MFAOptionType` may set:
/// `MFAOptionType.DeliveryMedium` is "the delivery medium to send the MFA code.
/// You can use this parameter to set only the `SMS` delivery medium value" (this
/// service's model).
pub const MFA_OPTION_DELIVERY_MEDIUM: &str = "SMS";

/// The only attribute an `MFAOptionType` may name:
/// `MFAOptionType.AttributeName` is "the attribute name of the MFA option type.
/// The only valid value is `phone_number`" (this service's model).
pub const MFA_OPTION_ATTRIBUTE: &str = "phone_number";

/// The one SMS MFA option a user holds while SMS message MFA is activated for
/// it: an option sets the only delivery medium ([`MFA_OPTION_DELIVERY_MEDIUM`])
/// and names the only attribute ([`MFA_OPTION_ATTRIBUTE`]) an `MFAOptionType`
/// may carry, so a user holds either this option or none.
pub fn sms_mfa_option() -> MFAOptionType {
    MFAOptionType {
        delivery_medium: Some(MFA_OPTION_DELIVERY_MEDIUM.to_string()),
        attribute_name: Some(MFA_OPTION_ATTRIBUTE.to_string()),
    }
}

/// What one submitted MFA settings structure asks for one factor
/// ([`User::set_mfa_preference`]): the `UserMFASettingList` setting the factor is
/// reported under, whether the request activates it, and whether the request
/// makes it the user's preferred factor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MfaSettingRequest<'a> {
    pub setting: &'a str,
    pub enabled: bool,
    pub preferred: bool,
}

impl MfaSettingRequest<'_> {
    /// Whether this setting asks to ACTIVATE the factor, which is what needs the
    /// delivery configuration the factor reaches the user through. A setting that
    /// does not turns the factor off, which needs nothing.
    pub fn activates(&self) -> bool {
        self.enabled
    }
}

/// Why a submitted MFA preference is refused ([`User::set_mfa_preference`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MfaPreferenceRejection {
    /// The `UserMFASettingList` setting the request asks to ACTIVATE while the
    /// user holds no delivery configuration for it.
    NoDeliveryConfig(String),
    /// The request asks for a factor to be the user's PREFERRED one while not
    /// activating it: a factor that is off is not one a user prefers.
    PreferredWhileDisabled,
}

impl MfaPreferenceRejection {
    /// What the refusal reports.
    ///
    /// An activation the user cannot receive reports the factor it asked for and
    /// that the user holds no configuration to receive it with. A preference over a
    /// factor the request leaves off reports that alone and names NO factor: "only
    /// one factor can be set as preferred" (`SetUserMFAPreference` in this
    /// service's model) and a factor that is off is not one a user prefers, so the
    /// refusal is about the preference rather than about any one factor. Nothing
    /// stored is disclosed either way.
    pub fn message(&self) -> String {
        match self {
            Self::NoDeliveryConfig(setting) => {
                format!("User does not have delivery config set to turn on {setting}")
            }
            Self::PreferredWhileDisabled => "Can't mark a disabled mfa as preferred".to_string(),
        }
    }
}

/// Why a submitted `MFAOptionListType` configures no SMS MFA option
/// ([`sms_mfa_options`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MfaOptionRejection {
    /// The submitted delivery medium, which is not the one SMS MFA travels by,
    /// or `None` where the option set no medium at all.
    DeliveryMedium(Option<String>),
    /// The submitted attribute name, which is not the one an MFA option may
    /// name, or `None` where the option named no attribute at all.
    AttributeName(Option<String>),
}

impl MfaOptionRejection {
    /// What the refusal reports: the member of the submitted option that an SMS
    /// MFA configuration cannot carry, and the value submitted for it. Only
    /// submitted values are named, so nothing stored is disclosed.
    pub fn message(&self) -> String {
        match self {
            Self::DeliveryMedium(Some(medium)) => format!(
                "Invalid DeliveryMedium: {medium}, the only delivery medium an MFA option can \
                 set is {MFA_OPTION_DELIVERY_MEDIUM}"
            ),
            Self::DeliveryMedium(None) => format!(
                "Invalid MFAOptions: an MFA option must set {MFA_OPTION_DELIVERY_MEDIUM} as its \
                 DeliveryMedium"
            ),
            Self::AttributeName(Some(name)) => format!(
                "Invalid AttributeName: {name}, the only attribute name an MFA option can name \
                 is {MFA_OPTION_ATTRIBUTE}"
            ),
            Self::AttributeName(None) => format!(
                "Invalid MFAOptions: an MFA option must name {MFA_OPTION_ATTRIBUTE} as its \
                 AttributeName"
            ),
        }
    }
}

/// The SMS MFA configuration a submitted `MFAOptionListType` asks one user to
/// hold, or the refusal it earns.
///
/// `MFAOptions` is a parameter "you can use only to set an SMS configuration
/// that uses SMS for delivery" (`SetUserSettings` and `AdminSetUserSettings` in
/// this service's model), so every submitted option has to set SMS delivery of
/// the one attribute an option may name. A user holds ONE value per attribute,
/// so the same option submitted twice is still one configuration and the
/// DISTINCT options are what is stored.
///
/// A PURE function of the request: it decides the configuration without reading
/// or writing stored state, so a request refused here writes nothing. Each
/// submitted option costs one constant-time check and the configuration it
/// yields holds at most one option per attribute an option may name, so the
/// length of the submitted list bounds nothing that is stored.
pub fn sms_mfa_options(
    requested: &[MFAOptionType],
) -> Result<Vec<MFAOptionType>, MfaOptionRejection> {
    let mut stored: Vec<MFAOptionType> = Vec::new();
    for option in requested {
        match option.delivery_medium.as_deref() {
            Some(MFA_OPTION_DELIVERY_MEDIUM) => {}
            other => {
                return Err(MfaOptionRejection::DeliveryMedium(
                    other.map(str::to_string),
                ))
            }
        }
        match option.attribute_name.as_deref() {
            Some(MFA_OPTION_ATTRIBUTE) => {}
            other => return Err(MfaOptionRejection::AttributeName(other.map(str::to_string))),
        }
        if !stored
            .iter()
            .any(|kept| kept.attribute_name == option.attribute_name)
        {
            stored.push(option.clone());
        }
    }
    Ok(stored)
}

/// Where a password-reset message for one user goes
/// ([`UserPool::recovery_destination`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryDestination {
    /// The attribute the message is addressed to, and the stored value it goes
    /// to.
    Delivered {
        attribute: String,
        destination: String,
    },
    /// This pool reserves password resets to administrators (`admin_only`), so
    /// no self-service message is sent at all.
    AdminOnly,
    /// The user holds no verified address or number for any mechanism this pool
    /// allows, so there is nowhere to send a reset code.
    None,
}

/// The `RecoveryOptionNameType` value that "prevents self-service account
/// recovery" (`RecoveryOptionType` in this service's model).
pub const RECOVERY_ADMIN_ONLY: &str = "admin_only";

/// The message a password reset is refused with where the pool reserves account
/// recovery to administrators ([`RecoveryDestination::AdminOnly`]): no
/// password-reset message is sent at all, so no reset can be begun through the
/// API that sends one.
pub const ADMIN_ONLY_RECOVERY_MESSAGE: &str = "Contact administrator to reset password.";

/// The message a password reset is refused with where the user holds no verified
/// address a reset message could go to ([`RecoveryDestination::None`]): "if
/// neither a verified phone number nor a verified email exists, Amazon Cognito
/// responds with an InvalidParameterException error" (`ForgotPassword` in this
/// service's model).
pub const NO_RECOVERY_DESTINATION_MESSAGE: &str =
    "Cannot reset password for the user as there is no registered/verified email or phone_number";

/// The recovery mechanisms a pool that configures none gets, in priority order:
/// Amazon Cognito's default account recovery is the verified email address,
/// falling back to the verified phone number (the two attribute-bearing
/// `RecoveryOptionNameType` values).
const DEFAULT_RECOVERY_MECHANISMS: [&str; 2] = ["verified_email", "verified_phone_number"];

/// The `AccountRecoverySetting` a pool is STORED with, which is the one a
/// request carried or Amazon Cognito's own default.
///
/// A pool always has an effective account-recovery configuration — it is what
/// decides where a self-service reset message goes
/// ([`UserPool::recovery_destination`]) — so a request that configures none is
/// stored with the service default rather than with nothing, and
/// `DescribeUserPool` reports the configuration the pool actually applies.
/// The default is [`DEFAULT_RECOVERY_MECHANISMS`] in that order, each
/// mechanism's `Priority` being its rank from 1, since "groups ... take
/// precedence" by ascending priority ("if `verified_email` has a priority of 1
/// and `verified_phone_number` has a priority of 2, your user pool sends
/// account-recovery messages to a verified email address but falls back to an
/// SMS message", `RecoveryOptionType` in this service's model).
pub fn effective_account_recovery_setting(
    requested: Option<AccountRecoverySettingType>,
) -> AccountRecoverySettingType {
    requested.unwrap_or_else(|| AccountRecoverySettingType {
        recovery_mechanisms: Some(
            DEFAULT_RECOVERY_MECHANISMS
                .iter()
                .enumerate()
                .map(|(index, name)| RecoveryOptionType {
                    priority: i32::try_from(index).unwrap_or_default() + 1,
                    name: (*name).to_string(),
                })
                .collect(),
        ),
    })
}

/// The user attribute a `RecoveryOptionNameType` recovers through, or `None` for
/// a mechanism that names no attribute (`admin_only`).
fn recovered_attribute(mechanism: &str) -> Option<&'static str> {
    match mechanism {
        "verified_email" => Some("email"),
        "verified_phone_number" => Some("phone_number"),
        _ => None,
    }
}

/// How many trailing characters of a masked phone number stay readable, which
/// is what identifies the number to the user who gave it.
const UNMASKED_DESTINATION_SUFFIX: usize = 4;

/// `destination` as a `CodeDeliveryDetailsType.Destination`.
///
/// Amazon Cognito reports the address it sent the code to MASKED, so the reply
/// tells the caller which of the user's addresses was used without disclosing
/// an address the caller does not already hold. An email address keeps the
/// first character of its local part, the first of its domain and the domain's
/// last label; anything else keeps a leading `+` and its last
/// [`UNMASKED_DESTINATION_SUFFIX`] characters.
pub fn masked_destination(destination: &str) -> String {
    if let Some((local, domain)) = destination.split_once('@') {
        let local_initial = local.chars().next().unwrap_or('*');
        let domain_initial = domain.chars().next().unwrap_or('*');
        let last_label = domain.rfind('.').map_or("", |dot| &domain[dot..]);
        return format!("{local_initial}***@{domain_initial}***{last_label}");
    }
    let characters: Vec<char> = destination.chars().collect();
    let readable_from = characters
        .len()
        .saturating_sub(UNMASKED_DESTINATION_SUFFIX);
    characters
        .iter()
        .enumerate()
        .map(|(index, character)| {
            if index >= readable_from || (index == 0 && *character == '+') {
                *character
            } else {
                '*'
            }
        })
        .collect()
}

/// The delivery details a message to a SIMULATED destination is reported with.
///
/// `PreventUserExistenceErrors` set to `ENABLED` "suppresses messages that might
/// indicate a valid user exists" (this service's model): the account-confirmation
/// and password-recovery operations therefore answer a request naming a user
/// nobody registered exactly as they answer one naming a user that exists —
/// reporting a delivery — rather than with a refusal that tells the caller the
/// account does not exist. Nothing was sent and nothing is stored, so the
/// destination reported is a SIMULATED one: a fixed masked address, which names
/// nothing of the request and nothing of this emulator's state, so no two
/// usernames can be told apart by the answer they get.
pub fn simulated_code_delivery_details() -> Option<CodeDeliveryDetailsType> {
    code_delivery_details(SIMULATED_DESTINATION_ATTRIBUTE, SIMULATED_DESTINATION)
}

/// The attribute a simulated delivery is reported against: an email address is
/// the destination every pool can address a message to, and the medium it
/// travels by is the one [`delivery_medium`] maps it to.
const SIMULATED_DESTINATION_ATTRIBUTE: &str = "email";

/// The address [`simulated_code_delivery_details`] reports, which identifies
/// nobody: it is reported through the same masking every real destination is
/// reported through ([`masked_destination`]), so the answer for a user nobody
/// registered has the shape of the answer for one that exists.
const SIMULATED_DESTINATION: &str = "a@b";

/// The delivery details for a verification message sent to `value` for
/// `attribute`, or `None` for an attribute no message is sent for.
///
/// Rendered from the value the message goes to, masked as
/// [`masked_destination`] reports it.
pub fn code_delivery_details(attribute: &str, value: &str) -> Option<CodeDeliveryDetailsType> {
    Some(CodeDeliveryDetailsType {
        destination: Some(masked_destination(value)),
        delivery_medium: Some(delivery_medium(attribute)?.to_string()),
        attribute_name: Some(attribute.to_string()),
    })
}

/// The schema a new pool is created with: the standard attributes, with a
/// requested attribute of the same name overriding its properties, and every
/// other requested attribute added under the `custom:` prefix Amazon Cognito
/// gives custom attributes.
///
/// The stored ORDER is the pool's own, as created: "an array of attributes for
/// the new user pool. You can add custom attributes and modify the properties of
/// default attributes" (`CreateUserPool.Schema` in this service's model), so the
/// attributes the request names are stored in the order IT names them — a custom
/// one among the standard ones where the request put it — followed by the
/// standard attributes it does not name, in Amazon Cognito's standard claim
/// order. [`SUB_ATTRIBUTE`] leads the schema whatever the request says: the pool
/// mints it for every user rather than taking it from a caller. Every read of
/// the schema — `DescribeUserPool`, `GetCSVHeader` — reports that one order, and
/// a later `AddCustomAttributes` appends behind it.
///
/// The work is bounded and linear in the request: each requested attribute is
/// resolved through a name-keyed index rather than by scanning what the schema
/// already holds, the ordering reads each attribute's rank out of a map, and no
/// more attributes are taken than a pool may hold
/// ([`MAX_CUSTOM_ATTRIBUTES_PER_POOL`] beside the standard ones) — a bound
/// `CreateUserPool` refuses past before it calls this, so nothing a request can
/// reach is dropped here.
pub fn effective_schema(requested: Option<Vec<SchemaAttributeType>>) -> Vec<SchemaAttributeType> {
    let requested = requested.unwrap_or_default();
    let mut schema: Vec<SchemaAttributeType> =
        STANDARD_SCHEMA.iter().map(standard_schema_attribute).collect();

    // Where each stored attribute sits, so a requested one is resolved by name
    // instead of by scanning the schema.
    let mut position: BTreeMap<String, usize> = schema
        .iter()
        .enumerate()
        .filter_map(|(at, entry)| entry.name.clone().map(|name| (name, at)))
        .collect();
    // The rank of each attribute the request names, in the order it names them.
    let mut requested_rank: BTreeMap<String, usize> = BTreeMap::new();

    let bound = STANDARD_SCHEMA.len().saturating_add(MAX_CUSTOM_ATTRIBUTES_PER_POOL);
    for attribute in requested.iter().take(bound) {
        let Some(name) = attribute.name.clone() else {
            continue;
        };
        if let Some(standard) = position.get(&name).and_then(|at| schema.get_mut(*at)) {
            standard.required = attribute.required.or(standard.required);
            standard.mutable = attribute.mutable.or(standard.mutable);
            if attribute.string_attribute_constraints.is_some() {
                standard.string_attribute_constraints =
                    attribute.string_attribute_constraints.clone();
            }
            if attribute.number_attribute_constraints.is_some() {
                standard.number_attribute_constraints =
                    attribute.number_attribute_constraints.clone();
            }
            let next = requested_rank.len();
            requested_rank.entry(name).or_insert(next);
            continue;
        }
        if let Some((stored_name, entry)) = custom_schema_attribute(attribute) {
            if position.contains_key(&stored_name) {
                continue;
            }
            position.insert(stored_name.clone(), schema.len());
            schema.push(entry);
            let next = requested_rank.len();
            requested_rank.insert(stored_name, next);
        }
    }

    // One attribute is stored under exactly one name, so ranking each entry and
    // sorting by that rank keeps the minted attribute first, then the requested
    // order, then the standard order a stable sort leaves the rest in.
    schema.sort_by_key(|entry| {
        let name = entry.name.as_deref().unwrap_or_default();
        if name == SUB_ATTRIBUTE {
            return (0, 0);
        }
        match requested_rank.get(name) {
            Some(rank) => (1, *rank),
            None => (2, 0),
        }
    });
    schema
}

/// The message a `CreateUserPool` whose `Schema` asks for more custom
/// attributes than a pool may hold is refused with: the per-pool quota is
/// [`MAX_CUSTOM_ATTRIBUTES_PER_POOL`], the same one `AddCustomAttributes`
/// enforces for a schema that grows after the pool exists, and the request
/// names no pool yet.
pub fn custom_attribute_quota_message() -> String {
    format!("A user pool holds at most {MAX_CUSTOM_ATTRIBUTES_PER_POOL} custom attributes.")
}

/// How many of `requested`'s attributes a pool would hold as CUSTOM ones: every
/// requested attribute that is not one of the standard attributes a pool already
/// carries, which are not counted against the quota.
pub fn requested_custom_attribute_count(requested: &[SchemaAttributeType]) -> usize {
    requested
        .iter()
        .filter(|attribute| {
            attribute
                .name
                .as_deref()
                .is_some_and(|name| !is_standard_attribute(name))
        })
        .count()
}

/// The `AttributeDataType` a requested custom attribute that names none is
/// stored with: `SchemaAttributeType.AttributeDataType` is "the data type of the
/// attribute" and `String` is the one every unqualified custom attribute reads
/// back as (this service's model). One of
/// [`crate::ops::types::ATTRIBUTE_DATA_TYPE_VALUES`].
pub const DEFAULT_ATTRIBUTE_DATA_TYPE: &str = "String";

/// The STORED NAME and the schema entry a requested CUSTOM attribute becomes,
/// with the defaults a custom attribute carries filled in.
///
/// Its stored name is the requested name under the prefix Amazon Cognito gives
/// it: "When you add an attribute with a `Name` value of `MyAttribute`, Amazon
/// Cognito creates the custom attribute `custom:MyAttribute`. When
/// `DeveloperOnlyAttribute` is `true`, Amazon Cognito creates your attribute as
/// `dev:MyAttribute`" (`SchemaAttributeType.Name`, this service's model). A name
/// that already carries one of those prefixes is stored as it is, so a schema
/// read and a re-add agree on the stored name.
///
/// `None` when the request names no attribute: an entry with no name could never
/// be found again. `CustomAttributeNameType` has a minimum length of 1, so only
/// a request the model does not admit reaches that answer.
pub fn custom_schema_attribute(
    requested: &SchemaAttributeType,
) -> Option<(String, SchemaAttributeType)> {
    let name = requested.name.clone()?;
    if name.is_empty() {
        return None;
    }
    let developer_only = requested.developer_only_attribute.unwrap_or(false);
    let prefix = if developer_only { "dev:" } else { "custom:" };
    let stored_name = if name.starts_with("custom:") || name.starts_with("dev:") {
        name
    } else {
        format!("{prefix}{name}")
    };
    let data_type = requested
        .attribute_data_type
        .clone()
        .unwrap_or_else(|| DEFAULT_ATTRIBUTE_DATA_TYPE.to_string());
    // The attribute reports the constraints its own data type has, holding the
    // bounds the request asked for and EMPTY where it asked for none — the same
    // rule the default schema's attributes report theirs by
    // ([`schema_attribute_constraints`]).
    let (min, max) = match data_type.as_str() {
        NUMBER_ATTRIBUTE_DATA_TYPE => (
            requested
                .number_attribute_constraints
                .as_ref()
                .and_then(|constraints| constraints.min_value.as_deref()),
            requested
                .number_attribute_constraints
                .as_ref()
                .and_then(|constraints| constraints.max_value.as_deref()),
        ),
        _ => (
            requested
                .string_attribute_constraints
                .as_ref()
                .and_then(|constraints| constraints.min_length.as_deref()),
            requested
                .string_attribute_constraints
                .as_ref()
                .and_then(|constraints| constraints.max_length.as_deref()),
        ),
    };
    let (number_attribute_constraints, string_attribute_constraints) =
        schema_attribute_constraints(&data_type, min, max);
    let entry = SchemaAttributeType {
        name: Some(stored_name.clone()),
        attribute_data_type: Some(data_type.clone()),
        developer_only_attribute: Some(developer_only),
        // A custom attribute is mutable unless the request says otherwise;
        // a custom attribute can never be required.
        mutable: Some(requested.mutable.unwrap_or(true)),
        required: Some(false),
        number_attribute_constraints,
        string_attribute_constraints,
    };
    Some((stored_name, entry))
}

/// A new user pool with `name` in `region`, with every configuration default
/// filled in. The caller stores it under [`UserPool::id`].
pub fn new_user_pool(name: String, region: &str, now: SystemTime) -> UserPool {
    let region = sanitize_region(region);
    let id = new_pool_id(&region);
    let arn = ::engine::emulator_arn(SIGNING_NAME, &region, &format!("userpool/{id}"));
    UserPool {
        id,
        name,
        arn,
        region,
        creation_date: now,
        last_modified_date: now,
        policies: effective_policies(None),
        lambda_config: effective_lambda_config(None),
        mfa_configuration: effective_mfa_configuration(None),
        deletion_protection: effective_deletion_protection(None),
        user_pool_tier: DEFAULT_USER_POOL_TIER.to_string(),
        username_attributes: Vec::new(),
        alias_attributes: Vec::new(),
        auto_verified_attributes: Vec::new(),
        schema: effective_schema(None),
        admin_create_user_config: effective_admin_create_user_config(None),
        username_configuration: None,
        device_configuration: None,
        email_configuration: effective_email_configuration(None),
        sms_configuration: None,
        verification_message_template: effective_verification_message_template(None),
        account_recovery_setting: None,
        user_attribute_update_settings: effective_user_attribute_update_settings(None),
        user_pool_add_ons: None,
        key_configuration: effective_key_configuration(None),
        issuer_configuration: effective_issuer_configuration(None),
        acr_configuration: None,
        sms_authentication_message: None,
        software_token_mfa_configuration: None,
        email_mfa_configuration: None,
        web_authn_configuration: None,
        log_configurations: Vec::new(),
        tags: HashMap::new(),
        domain: None,
        custom_domain: None,
        replicas: BTreeMap::new(),
        clients: HashMap::new(),
        users: HashMap::new(),
        groups: BTreeMap::new(),
        resource_servers: BTreeMap::new(),
        identity_providers: BTreeMap::new(),
        terms: BTreeMap::new(),
        managed_login_branding: BTreeMap::new(),
        ui_customizations: BTreeMap::new(),
        user_import_jobs: BTreeMap::new(),
        // A pool's token-signing key exists from the moment the pool does, as
        // on AWS, so its key set is publishable before anything signs in. One
        // scalar multiplication, so creating a pool waits on nothing.
        signing_key: PoolSigningKey::generate(),
        device_group_key: new_device_group_key(),
        risk_configuration: None,
    }
}

/// The region the request was signed for, or [`DEFAULT_REGION`] when it carries
/// no SigV4 credential scope. A pool's id and ARN name the region it was created
/// in, so they are derived from the request's scope rather than fixed.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| sanitize_region(&scope.region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `region`, reduced to the characters a `UserPoolId` may carry
/// (`^[\w-]+_[0-9a-zA-Z]+$`), falling back to [`DEFAULT_REGION`].
fn sanitize_region(region: &str) -> String {
    let cleaned: String = region
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    if cleaned.is_empty() {
        DEFAULT_REGION.to_string()
    } else {
        cleaned
    }
}

/// A fresh `UserPoolId`: `<region>_<9 random characters>`, which satisfies the
/// modelled pattern `^[\w-]+_[0-9a-zA-Z]+$` and length 1..55.
fn new_pool_id(region: &str) -> String {
    format!("{region}_{}", random_token(9))
}

/// Whether `region` can stand in the Region field of an ARN, and in the `^[\w-]+`
/// half of a `UserPoolId`.
fn is_region_like(region: &str) -> bool {
    !region.is_empty()
        && region
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Whether `user_pool_id` could be a `UserPoolId` at all: the modelled pattern
/// `^[\w-]+_[0-9a-zA-Z]+$` and length 1..[`USER_POOL_ID_MAX_LENGTH`]
/// (`smithy_query(query="shape",shape="UserPoolIdType")`). The separator is the
/// LAST underscore, because `\w` admits one in the first half too.
pub fn is_user_pool_id_shaped(user_pool_id: &str) -> bool {
    if user_pool_id.is_empty() || user_pool_id.len() > USER_POOL_ID_MAX_LENGTH {
        return false;
    }
    let Some((region, suffix)) = user_pool_id.rsplit_once('_') else {
        return false;
    };
    is_region_like(region)
        && !suffix.is_empty()
        && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The Region and the `UserPoolId` the user-pool ARN `arn` names, or `None` when
/// `arn` names no user pool of this service at all — another ARN namespace or
/// resource type, a malformed partition, Region or account, or a resource id no
/// `UserPoolId` could be.
///
/// Only the ARN's FORM is read here. Whether this service holds that pool in that
/// Region is the caller's lookup, which is what separates a `ResourceArn` that
/// names no user pool (a bad parameter) from one naming a pool that is not stored
/// (a missing resource).
pub fn user_pool_arn_parts(arn: &str) -> Option<(&str, &str)> {
    let mut fields = arn.splitn(6, ':');
    let (
        Some("arn"),
        Some(partition),
        Some(namespace),
        Some(region),
        Some(account),
        Some(resource),
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    )
    else {
        return None;
    };
    let partition_is_named = !partition.is_empty()
        && partition
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-');
    // An AWS account id is twelve digits.
    let account_is_an_id = account.len() == 12 && account.bytes().all(|b| b.is_ascii_digit());
    if namespace != SIGNING_NAME
        || !partition_is_named
        || !is_region_like(region)
        || !account_is_an_id
    {
        return None;
    }
    let user_pool_id = resource
        .strip_prefix(USER_POOL_RESOURCE_TYPE)
        .and_then(|rest| rest.strip_prefix('/'))
        .filter(|user_pool_id| is_user_pool_id_shaped(user_pool_id))?;
    Some((region, user_pool_id))
}

/// A fresh CloudFront endpoint for a custom domain: `<distribution>.cloudfront.net`,
/// the form AWS reports as "The fully-qualified domain name (FQDN) of the Amazon
/// CloudFront distribution" (`CreateUserPoolDomain.CloudFrontDomain`) and the
/// value a caller's alias record has to point at. A CloudFront distribution
/// domain is `d` followed by 13 lowercase alphanumerics.
fn new_cloud_front_domain() -> String {
    format!("d{}.cloudfront.net", random_token(13))
}

/// A fresh `JobId` for a user import job: `import-` followed by a v4 UUID,
/// which satisfies the modelled pattern `^import-[0-9a-zA-Z-]+$` and length
/// 1..55.
pub fn new_user_import_job_id() -> String {
    format!("import-{}", new_uuid())
}

/// Whether `job_id` could be a `JobId` at all: the modelled pattern
/// `^import-[0-9a-zA-Z-]+$` and length 1..55
/// (`smithy_query(query="shape",shape="UserImportJobIdType")`). Asked by the
/// non-modelled upload route, so a path of some other shape is not mistaken for
/// one of this service's CSV upload targets.
#[must_use]
pub fn is_user_import_job_id_shaped(job_id: &str) -> bool {
    let Some(suffix) = job_id.strip_prefix("import-") else {
        return false;
    };
    job_id.len() <= 55
        && !suffix.is_empty()
        && suffix
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// The first path segment of the locator a user import job's CSV is uploaded to.
/// Shared by the locator [`user_import_csv_upload_url`] hands out and by the
/// route that accepts the upload (`crate::service`), so the two cannot drift.
pub const USER_IMPORT_CSV_ROUTE: &str = "user-import-jobs";

/// Where a client uploads the CSV file of the import job `job_id`:
/// `UserImportJobType.PreSignedUrl`, "The pre-signed URL target for uploading
/// the CSV file".
///
/// AWS answers an Amazon S3 pre-signed URL here. This emulator provisions no
/// object store, so the URL names THIS job's CSV on the endpoint THIS REQUEST
/// ARRIVED AT, and this service accepts the upload on that path itself (see
/// `crate::service`) — a URL a client can really write the file to, rather than
/// one pointing at the real service. A request that names no host of its own gets
/// no locator at all rather than a baked-in one, which would point a following
/// client's upload at the real service.
///
/// `host` is taken only when it LOOKS like a `host[:port]`, so a caller cannot
/// get a string of its own into the value this service reports.
pub fn user_import_csv_upload_url(host: &str, user_pool_id: &str, job_id: &str) -> Option<String> {
    is_host_shaped(host)
        .then(|| format!("https://{host}/{USER_IMPORT_CSV_ROUTE}/{user_pool_id}/{job_id}"))
}

/// The path segment under a user pool's level that the classic hosted UI serves
/// its logo image on. Shared by the locator [`hosted_logo_url`] hands out and by
/// the route that serves it (`crate::service`), so the two cannot drift.
pub const HOSTED_LOGO_FILE: &str = "assets/login_logo.png";

/// Where a client fetches the logo image stored for `client_id` of
/// `user_pool_id`: `UICustomizationType.ImageUrl`, "A URL path to the hosted
/// logo image of your UI customization".
///
/// AWS serves it from the content delivery network that fronts the classic
/// hosted UI. This emulator provisions none, so the locator names the image on
/// the endpoint THIS REQUEST ARRIVED AT, which this service serves itself (see
/// `crate::service`) — a URL a caller can fetch the stored bytes from, rather
/// than one pointing at the real service. A request that names no host of its
/// own gets no locator at all.
///
/// `host` is taken only when it LOOKS like a `host[:port]`, so a caller cannot
/// get a string of its own into the value this service reports.
pub fn hosted_logo_url(host: &str, user_pool_id: &str, client_id: &str) -> Option<String> {
    is_host_shaped(host)
        .then(|| format!("https://{host}/{user_pool_id}/{client_id}/{HOSTED_LOGO_FILE}"))
}

/// The endpoint host this request addressed — its `Host` header, or the
/// authority of the request URI when it carries one — which is the endpoint a
/// locator an operation hands out has to name. `None` when the request names no
/// endpoint of its own.
pub fn request_host(ctx: &smithy_server_runtime::OperationContext<'_>) -> Option<String> {
    ctx.parts()
        .headers
        .get(::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| {
            ctx.parts()
                .uri
                .authority()
                .map(::http::uri::Authority::as_str)
        })
        .map(ToString::to_string)
}

/// Whether `host` is a non-empty `host[:port]` of the characters a hostname
/// carries.
fn is_host_shaped(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 255
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b':'))
}

/// A fresh `ClientId`: 26 characters, satisfying the modelled `^[\w+]+$`.
pub fn new_client_id() -> String {
    random_token(26)
}

/// A fresh `ClientSecret`: 51 characters, inside the modelled length 24..64 and
/// satisfying `^[\w+]+$`.
pub fn new_client_secret() -> String {
    random_token(51)
}

/// The message a `SecretHash` authenticates: "username plus the client ID in
/// the message" (`SignUp.SecretHash` in this service's model), in that order, so
/// the hash of one username is not the hash of another and the hash for one app
/// client is not the hash for another.
fn secret_hash_message(username: &str, client_id: &str) -> String {
    format!("{username}{client_id}")
}

/// A fresh user `sub`: a v4 UUID, as Amazon Cognito's `sub` attribute is.
///
/// The randomness is the `uuid` crate's, reached through `emulator_telemetry`'s
/// uuid-backed id newtype (`BatchId::random()` is `uuid::Uuid::new_v4()`
/// rendered canonically — `vendor/emulator-telemetry/src/types.rs`), which this
/// crate already depends on: nothing here is a hand-rolled generator and this
/// crate declares no second path to the same crate.
pub fn new_uuid() -> String {
    ::emulator_telemetry::BatchId::random().as_str().to_string()
}

/// A fresh `Session` token: 64 characters, inside the modelled length 20..4096.
fn new_session_token() -> String {
    random_token(64)
}

/// A fresh passkey registration challenge: the value the authenticator signs
/// over, carried base64url as the creation options encode one (W3C Web
/// Authentication Level 2 §5.4, `PublicKeyCredentialCreationOptions.challenge`,
/// a `BufferSource` the JSON form base64url-encodes).
///
/// 32 characters of the same v4-UUID randomness every other minted value in this
/// file draws on ([`random_token`]), so nothing here is a hand-rolled generator.
fn new_web_authn_challenge() -> String {
    base64_url(random_token(32).as_bytes())
}

/// A fresh `DeviceKey` for a device of a pool in `region`: `<region>_<v4 UUID>`,
/// which satisfies the modelled pattern `^[\w-]+_[0-9a-f-]+$` — `region` carries
/// only the characters [`sanitize_region`] admits, and a v4 UUID is lowercase
/// hex and hyphens.
///
/// The prefix is clamped to [`MAX_DEVICE_KEY_PREFIX`] so the key stays inside
/// the modelled length 1..55 whatever region the pool was created in: the
/// region comes from the caller's credential scope, so its length is not this
/// service's to assume.
fn new_device_key(region: &str) -> String {
    let prefix = region
        .char_indices()
        .take_while(|(at, _)| *at < MAX_DEVICE_KEY_PREFIX)
        .map(|(_, c)| c)
        .collect::<String>();
    format!("{prefix}_{}", new_uuid())
}

/// How many characters of the region a minted `DeviceKey` carries ahead of its
/// separator: `DeviceKeyType`'s modelled length is 1..55 and the UUID after the
/// separator is 36 characters long.
const MAX_DEVICE_KEY_PREFIX: usize = 55 - 1 - 36;

/// A fresh `DeviceGroupKey` for a user pool: a v4 UUID, the shape Amazon Cognito
/// issues this identifier in, inside the modelled length 0..131072.
fn new_device_group_key() -> String {
    new_uuid()
}

/// A fresh refresh token: the opaque credential a completed sign-in hands its
/// caller, and the value a later refresh presents back.
///
/// "Your user's refresh token" (`AuthenticationResultType.RefreshToken` in this
/// service's model) is a bearer credential and nothing else: unlike the access
/// and id tokens beside it, it states no claim its holder can read, and
/// everything this service knows about it lives in the [`TokenRecord`] keyed by
/// it. It is therefore minted by the one component that generates secrets — the
/// emulator's credential store — and then removed from that store again: the
/// caller gets the secret, the emulator keeps no credential it would accept as a
/// request signature, and nothing accumulates per sign-in.
///
/// The minted secret is rendered base64url with [`base64_url`], the codec this
/// crate already uses for every opaque value it puts on the wire, so the
/// credential satisfies the modelled `TokenModelType` pattern
/// `^[A-Za-z0-9-_=.]+$` whatever alphabet the store drew it from, and keeps every
/// bit of its randomness.
///
/// `None` when the store minted no session-token secret, or when the minted
/// credential could not be removed from the store again — a credential the
/// emulator would still accept as a request signature is not the token a sign-in
/// promises, so it is refused rather than handed out, and the failed removal is
/// logged. Either way the sign-in refuses rather than reporting a credential
/// nothing issued.
pub fn mint_refresh_token(store: &::emulator_server::CredentialStore) -> Option<String> {
    let minted = store.create(&::emulator_server::aws_credentials::AWS_SESSION);
    let token = minted
        .secret(::emulator_server::aws_credentials::SESSION_TOKEN)
        .map(|secret| base64_url(secret.as_bytes()));
    // The refresh token authenticates to this service's own token exchange, not
    // to the emulator's request signing; see above.
    if let Err(error) = store.delete(&minted.id) {
        ::tracing::warn!(
            access_key_id = %minted.id,
            error = ?error,
            "a credential minted for a refresh token could not be deleted"
        );
        return None;
    }
    token
}

/// `len` random hex characters, drawn from v4 UUIDs (122 bits of entropy each)
/// with their hyphens removed, so the result satisfies `^[\w+]+$`.
fn random_token(len: usize) -> String {
    let mut token = String::with_capacity(len + 32);
    while token.len() < len {
        token.extend(new_uuid().chars().filter(|c| *c != '-'));
    }
    token.truncate(len);
    token
}

/// What one sign-in's tokens are built from, read from the stored pool, app
/// client and user under the state lock before any token is minted.
struct TokenInputs {
    /// The pool's token issuer (its regional user-pool endpoint).
    issuer: String,
    /// The stored user's immutable id, the `sub` claim.
    sub: String,
    /// The username as STORED, which a case-insensitive pool may case
    /// differently from the request.
    stored_username: String,
    /// The stored `email` attribute, when the user has one.
    email: Option<String>,
    /// The names of the pool's groups the stored user belongs to, in stored order.
    groups: Vec<String>,
    /// What those groups' roles and precedences put in the identity token.
    role_claims: GroupRoleClaims,
    /// The pool's own token-signing key pair: every token of this sign-in is
    /// signed with it and names it by `kid`.
    signing_key: PoolSigningKey,
    access_ttl: Duration,
    id_ttl: Duration,
    refresh_ttl: Duration,
}

// ---------------------------------------------------------------------------
// Token signing
//
// Amazon Cognito signs every token a user pool issues against a key pair
// belonging to THAT pool, and publishes the verifying half as a JSON Web Key
// Set: a verifier must "decode the token ... and verify the signature" against
// the key the token's `kid` names in the pool's `jwks.json` (Amazon Cognito
// Developer Guide, "Verifying a JSON web token"). Every consumer of such a
// token — Verified Permissions, an API Gateway JWT authorizer, any JOSE
// library — reads `alg` and `kid` from the header, finds the matching JWK and
// checks the signature over `base64url(header) '.' base64url(claims)`, so a
// token whose third segment is EMPTY is unusable: it is not even a
// syntactically valid signed JWT.
//
// AWS signs with RS256. This emulator signs with ES256 (ECDSA on NIST P-256
// with SHA-256, RFC 7518 §3.4) because the only maintained pure-Rust RSA
// implementation carries two unfixable timing advisories (RUSTSEC-2023-0071 /
// CVE-2023-49092 and GHSA-4grx-2x9w-596c, both "no fixed version") that this
// repository's dependency audit blocks. ES256 comes from `ring`, the
// cryptography library this workspace already signs and encrypts with
// elsewhere, rather than from the `p256`/`elliptic-curve` stack, whose
// `crypto-bigint` 0.5 dependency carries a timing advisory (RUSTSEC-2026-0387)
// with no release in that line that fixes it. Nothing a verifier does changes:
// the algorithm is named in the token header AND in the published JWK, which is
// where a JOSE library reads it from, so the token verifies by the same
// `kid`-lookup-then-check-signature path. Both algorithms are JOSE registered
// (RFC 7518 §3.1), and the signature segment is a non-empty base64url run
// either way.
// ---------------------------------------------------------------------------

/// The `alg` of every token this service signs, and of the JWK it publishes for
/// the key that signed it: ECDSA using P-256 and SHA-256 (RFC 7518 §3.1/§3.4).
const TOKEN_SIGNING_ALGORITHM: &str = "ES256";

/// The `crv` of the curve [`TOKEN_SIGNING_ALGORITHM`] is defined over, as the
/// published JWK names it (RFC 7518 §6.2.1.1).
const TOKEN_SIGNING_CURVE: &str = "P-256";

/// The signing algorithm every pool's key is generated for and signs with:
/// ECDSA on P-256 with SHA-256, in the FIXED (`R || S`) signature encoding RFC
/// 7518 §3.4 puts in a JWS segment — not the ASN.1 DER form, which a JOSE
/// verifier would reject.
const SIGNING_ALGORITHM: &::ring::signature::EcdsaSigningAlgorithm =
    &::ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;

/// One user pool's token-signing key pair: the private key the pool's tokens
/// are signed with, named by the `kid` its published JWK carries.
///
/// The key belongs to the pool, so a token one pool issued carries one pool's
/// `kid` and verifies against only that pool's JWKS.
#[derive(Clone)]
pub struct PoolSigningKey {
    /// The `kid` the token header carries and the published JWK is found by:
    /// this key's RFC 7638 JWK thumbprint, so it is derived from the key itself
    /// rather than minted beside it.
    kid: String,
    /// The pool's private key. Behind an [`::std::sync::Arc`] because the key
    /// pair itself is not clonable and a pool's key is READ from many places —
    /// sharing the one key is also what makes every token of one pool verify
    /// against that pool's single published JWK.
    private: ::std::sync::Arc<::ring::signature::EcdsaKeyPair>,
}

/// Renders the key's PUBLIC identity only. Private key material is never
/// logged, traced or put in a response, so it is not in this rendering either.
impl ::std::fmt::Debug for PoolSigningKey {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("PoolSigningKey")
            .field("kid", &self.kid)
            .field("alg", &TOKEN_SIGNING_ALGORITHM)
            .finish_non_exhaustive()
    }
}

impl PoolSigningKey {
    /// A freshly generated key pair, drawn from the operating system's CSPRNG
    /// (`ring::rand::SystemRandom`).
    ///
    /// One scalar multiplication, so this is microseconds of work: a pool mints
    /// its key when the pool is created and no request path ever generates one.
    /// INFALLIBLE: the only failure the generator reports is the OS CSPRNG or
    /// the key encoding failing, neither of which a pool can be created without.
    #[must_use]
    pub fn generate() -> Self {
        let rng = ::ring::rand::SystemRandom::new();
        let document = ::ring::signature::EcdsaKeyPair::generate_pkcs8(SIGNING_ALGORITHM, &rng)
            .expect("the operating system CSPRNG yields a P-256 key pair");
        let private =
            ::ring::signature::EcdsaKeyPair::from_pkcs8(SIGNING_ALGORITHM, document.as_ref(), &rng)
                .expect("a freshly generated P-256 key pair parses");
        let kid = jwk_thumbprint(&public_coordinates(&private));
        Self {
            kid,
            private: ::std::sync::Arc::new(private),
        }
    }

    /// The `kid` naming this key in the pool's JWKS.
    #[must_use]
    pub fn kid(&self) -> &str {
        &self.kid
    }

    /// This key's PUBLIC half as a JWK (RFC 7517), the entry a verifier finds
    /// by `kid` in the pool's key set: `kty`/`crv` and the `x`/`y` curve
    /// coordinates RFC 7518 §6.2.1 defines, with the `use` and `alg` the
    /// signature is checked under. No private parameter (`d`) is in it.
    #[must_use]
    pub fn to_jwk(&self) -> serde_json::Value {
        let (x, y) = public_coordinates(&self.private);
        serde_json::json!({
            "alg": TOKEN_SIGNING_ALGORITHM,
            "crv": TOKEN_SIGNING_CURVE,
            "kid": self.kid,
            "kty": "EC",
            "use": "sig",
            "x": base64_url(&x),
            "y": base64_url(&y),
        })
    }

    /// The ES256 signature over `signing_input` — the 64-byte `R || S` form RFC
    /// 7518 §3.4 puts in the third segment — or `None` when signing failed, in
    /// which case the caller issues no token rather than one carrying bytes
    /// that are not a signature.
    #[must_use]
    pub fn sign(&self, signing_input: &[u8]) -> Option<Vec<u8>> {
        let rng = ::ring::rand::SystemRandom::new();
        let signature = self.private.sign(&rng, signing_input).ok()?;
        Some(signature.as_ref().to_vec())
    }
}

/// The public curve point `point` (uncompressed SEC1, `0x04 || X || Y`) as the
/// two fixed-width coordinates a JWK publishes: `x` and `y`, 32 bytes each
/// (RFC 7518 §6.2.1.2 — "the octet string representation MUST utilize all
/// octets", i.e. no leading zero is dropped).
fn public_coordinates(key: &::ring::signature::EcdsaKeyPair) -> (Vec<u8>, Vec<u8>) {
    // SEC 1 §2.3.3: the uncompressed point is `0x04 || X || Y`, each coordinate
    // the field element's full width, which on P-256 is
    // [`P256_COORDINATE_BYTES`].
    let point = key.public_key().as_ref();
    let x = point
        .get(1..=P256_COORDINATE_BYTES)
        .unwrap_or_default()
        .to_vec();
    let y = point
        .get(1 + P256_COORDINATE_BYTES..=2 * P256_COORDINATE_BYTES)
        .unwrap_or_default()
        .to_vec();
    (x, y)
}

/// The width of one P-256 coordinate: the curve's field elements are 256 bits,
/// and RFC 7518 §6.2.1.2 publishes each as the full-width octet string.
const P256_COORDINATE_BYTES: usize = 32;

/// The RFC 7638 JWK thumbprint of the public key whose coordinates are
/// `(x, y)`: the base64url SHA-256 of the key's canonical JWK — its required
/// members (`crv`, `kty`, `x`, `y` for an EC key) in lexicographic order, with
/// no whitespace. Two pools' keys therefore never share a `kid`, and a key's
/// `kid` is the same every time it is published.
fn jwk_thumbprint((x, y): &(Vec<u8>, Vec<u8>)) -> String {
    let canonical = serde_json::json!({
        "crv": TOKEN_SIGNING_CURVE,
        "kty": "EC",
        "x": base64_url(x),
        "y": base64_url(y),
    })
    .to_string();
    base64_url(&::sha2::Sha256::digest(canonical.as_bytes()))
}

/// `claims` as a JWS in compact serialization (RFC 7515 §3.1) signed with
/// `key`: `base64url(header) '.' base64url(claims) '.' base64url(signature)`,
/// where the header names the algorithm and the key (`kid`) a verifier checks
/// the signature with.
///
/// `None` when the signature could not be computed, so a token is either
/// signed or not issued.
fn signed_jws(claims: &serde_json::Value, key: &PoolSigningKey) -> Option<String> {
    let header = serde_json::json!({
        "alg": TOKEN_SIGNING_ALGORITHM,
        "kid": key.kid(),
        "typ": "JWT",
    });
    // RFC 7515 §5.1: the signature is computed over the ENCODED header and
    // payload, joined by '.', so those exact bytes are what the token carries.
    let signing_input = format!(
        "{}.{}",
        base64_url(header.to_string().as_bytes()),
        base64_url(claims.to_string().as_bytes())
    );
    let signature = key.sign(signing_input.as_bytes())?;
    Some(format!("{signing_input}.{}", base64_url(&signature)))
}

/// The bytes `text` carries, read as base64url (RFC 4648 §5) — the encoding the
/// JSON form of a WebAuthn `BufferSource` member uses — or `None` when `text` is
/// not a base64 encoding of anything.
///
/// The inverse of [`base64_url`]: the URL alphabet's `-` and `_` are mapped back
/// to the standard alphabet's `+` and `/`, and the padding [`base64_url`] trims
/// is restored, so a value encoded with padding and one encoded without both
/// decode. Decoding itself is the codec this workspace already uses for every
/// Smithy blob (`aws_smithy_types::base64`).
pub fn base64_url_decode(text: &str) -> Option<Vec<u8>> {
    let mut standard = text.replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    ::aws_smithy_types::base64::decode(standard).ok()
}

/// `bytes` in base64url without padding, as a JWS segment is encoded
/// (RFC 7515 §2): the standard-alphabet codec this workspace already uses for
/// every Smithy blob (`aws_smithy_types::base64`), with `+` and `/` replaced by
/// `-` and `_` and the padding removed.
pub fn base64_url(bytes: &[u8]) -> String {
    ::aws_smithy_types::base64::encode(bytes)
        .replace('+', "-")
        .replace('/', "_")
        .trim_end_matches('=')
        .to_string()
}

/// `time` as the modelled timestamp type.
pub fn to_date_time(time: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_secs_f64(
        time.duration_since(UNIX_EPOCH)
            .map(|since| since.as_secs_f64())
            .unwrap_or_default(),
    )
}

/// `time` as whole seconds since the epoch, for a token claim.
fn unix_seconds(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// `time` as whole milliseconds since the epoch, the unit a rendered linked
/// identity reports its creation instant in.
fn unix_millis(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// `Some(values)` when `values` is non-empty, else `None`: an absent list is
/// absent on the wire rather than an empty one.
pub fn non_empty<T: Clone>(values: &[T]) -> Option<Vec<T>> {
    (!values.is_empty()).then(|| values.to_vec())
}

// ---------------------------------------------------------------------------
// Provisioned API rate limits
//
// An account holds one request-rate limit per API category: the rate the
// category includes at no additional cost, and the provisioned rate in force,
// which the account may set anywhere from that included rate up to its
// account-level maximum. These limits belong to the ACCOUNT rather than to a
// user pool, so they live beside the pools rather than inside one.
// ---------------------------------------------------------------------------

/// The one `LimitClass` this service models, which is the class of the
/// request-rate limits: "For API rate limits, this is `API_CATEGORY`"
/// (`LimitDefinitionType.LimitClass` in this service's model, whose only
/// modelled value is [`crate::ops::types::LIMIT_CLASS_VALUES`]).
pub const API_CATEGORY_LIMIT_CLASS: &str = "API_CATEGORY";

/// The attribute that identifies WHICH API rate limit a definition means:
/// "For API rate limits, specify the `Category` key with a value like
/// `UserAuthentication` or `UserCreation`"
/// (`LimitDefinitionType.Attributes` in this service's model).
pub const API_CATEGORY_ATTRIBUTE: &str = "Category";

/// The highest rate the account may provision for ANY one API rate limit, in
/// requests per second.
///
/// `UpdateProvisionedLimit`: "The value must be between the default limit and
/// your account-level maximum limit in Service Quotas" (this service's model).
/// That maximum is the account's own Service Quotas value, which this API
/// neither reports nor raises — only a Service Quotas increase request does —
/// so this emulator's account holds ONE maximum, the same for every limit, and
/// a rate above it is refused as a quota excess rather than stored.
pub const MAX_PROVISIONED_API_RATE: i32 = 10_000;

/// One API category an account holds a request-rate limit for.
#[derive(Debug)]
pub struct ApiCategory {
    /// The `Category` attribute value that names it.
    pub name: &'static str,
    /// The rate this category includes at no additional cost, in requests per
    /// second: `LimitType.FreeLimitValue` is "the default (free) limit value,
    /// in requests per second (RPS) … the rate included at no additional cost"
    /// (this service's model), which is the category's published request-rate
    /// quota (Amazon Cognito Developer Guide, "Quotas in Amazon Cognito"). It
    /// is the rate in force until an adjustment provisions more, and the lowest
    /// rate the limit can be provisioned at.
    pub free_rate: i32,
}

/// The API categories an account holds a request-rate limit for, with the rate
/// each one includes at no additional cost. A definition naming no category
/// here identifies no limit this account holds.
///
/// The names are the ones this service's model documents for the `Category`
/// attribute (`LimitDefinitionType.Attributes` names `UserAuthentication` and
/// `UserCreation`, `UpdateProvisionedLimit` also `UserFederation`), beside the
/// remaining request-rate categories of user pool operations, whose published
/// per-category rates are the included rates here (Amazon Cognito Developer
/// Guide, "Quotas in Amazon Cognito").
///
/// Every limit is provisionable over the same range, from the rate it includes
/// up to [`MAX_PROVISIONED_API_RATE`]: the upper bound is the account's, not
/// the category's.
pub const API_CATEGORIES: &[ApiCategory] = &[
    ApiCategory {
        name: "UserAuthentication",
        free_rate: 120,
    },
    ApiCategory {
        name: "UserFederation",
        free_rate: 25,
    },
    ApiCategory {
        name: "UserCreation",
        free_rate: 50,
    },
    ApiCategory {
        name: "UserRead",
        free_rate: 120,
    },
    ApiCategory {
        name: "UserUpdate",
        free_rate: 25,
    },
    ApiCategory {
        name: "UserAccountRecovery",
        free_rate: 30,
    },
];

/// Why a limit definition identifies no limit this account holds.
///
/// Both operations resolve a definition the same way, so both render these; a
/// rate refusal is [`RateRefusal`], which only the operation that requests a
/// rate can reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LimitLookupFailure {
    /// The definition names a limit class this service holds no limits of.
    UnsupportedClass(String),
    /// The definition carries none of the attributes that identify which limit
    /// of its class is meant; the attribute it has to carry is named.
    MissingAttribute(&'static str),
    /// The definition's attributes identify no limit this account holds.
    UnknownCategory(String),
}

impl LimitLookupFailure {
    /// What a refusal tells the caller, which is the same sentence whichever
    /// operation was refused: the caller has to correct the same definition
    /// from it either way.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::UnsupportedClass(class) => {
                format!("There is no limit of class {class} in this account.")
            }
            Self::MissingAttribute(attribute) => format!(
                "The limit definition must include the attribute {attribute} to identify \
                 the {API_CATEGORY_LIMIT_CLASS} limit it means."
            ),
            Self::UnknownCategory(category) => {
                format!("There is no limit for API category {category} in this account.")
            }
        }
    }
}

/// Why a requested rate is not one this account can provision for a limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RateRefusal {
    /// The requested rate is below the rate the limit includes, which is the
    /// lowest rate it can hold.
    BelowFreeRate { requested: i32, free_rate: i32 },
    /// The requested rate is above the account's maximum, so provisioning it
    /// would need a Service Quotas increase.
    AboveAccountMaximum { requested: i32, maximum: i32 },
}

impl RateRefusal {
    /// What a refusal tells the caller.
    ///
    /// A rate outside the account's range names the requested rate and the
    /// bound it missed, because `UpdateProvisionedLimit` documents the range as
    /// "the value must be between the default limit and your account-level
    /// maximum limit in Service Quotas" (this service's model) and neither
    /// bound is in the request.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::BelowFreeRate {
                requested,
                free_rate,
            } => format!(
                "The requested limit value {requested} is below the default limit of \
                 {free_rate} requests per second for this limit."
            ),
            Self::AboveAccountMaximum { requested, maximum } => format!(
                "The requested limit value {requested} exceeds your account's maximum limit \
                 of {maximum} requests per second for this limit. To increase your limit, \
                 submit a Service Quotas increase request."
            ),
        }
    }
}

/// The API category `definition` identifies.
///
/// A definition identifies a limit by its class and the attributes of that
/// class: the request-rate limits are the `API_CATEGORY` class, and the
/// `Category` attribute is what names one of them. Nothing about the account's
/// state takes part, so both operations resolve a definition the same way.
pub fn api_category(
    definition: &LimitDefinitionType,
) -> Result<&'static ApiCategory, LimitLookupFailure> {
    if definition.limit_class != API_CATEGORY_LIMIT_CLASS {
        return Err(LimitLookupFailure::UnsupportedClass(
            definition.limit_class.clone(),
        ));
    }
    let named = definition
        .attributes
        .get(API_CATEGORY_ATTRIBUTE)
        .ok_or(LimitLookupFailure::MissingAttribute(API_CATEGORY_ATTRIBUTE))?;
    API_CATEGORIES
        .iter()
        .find(|candidate| candidate.name == named)
        .ok_or_else(|| LimitLookupFailure::UnknownCategory(named.clone()))
}

/// The definition that identifies `category`'s limit, as every reader of that
/// limit renders it: the class of the request-rate limits and the one attribute
/// that names the category. Built from the RESOLVED category rather than echoed
/// from a request, so one limit is identified the same way whatever a caller
/// sent.
#[must_use]
pub fn api_category_definition(category: &ApiCategory) -> LimitDefinitionType {
    LimitDefinitionType {
        limit_class: API_CATEGORY_LIMIT_CLASS.to_string(),
        attributes: [(
            API_CATEGORY_ATTRIBUTE.to_string(),
            category.name.to_string(),
        )]
        .into_iter()
        .collect(),
    }
}

impl ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState {
    /// The limit this account holds for `category`.
    ///
    /// A PURE read: the provisioned rate is the one a previous update stored,
    /// and a limit nothing has adjusted reports the rate the category includes
    /// at no additional cost — the rate Amazon Cognito enforces until the
    /// account provisions more.
    #[must_use]
    pub fn api_category_limit(&self, category: &ApiCategory) -> LimitType {
        LimitType {
            limit_definition: api_category_definition(category),
            provisioned_limit_value: self
                .provisioned_api_rates
                .get(category.name)
                .copied()
                .unwrap_or(category.free_rate),
            free_limit_value: category.free_rate,
        }
    }

    /// Provision `requested` requests per second for `category` and return the
    /// limit as it now stands.
    ///
    /// Every rate in the account's range is stored, the rate already in force
    /// included: the range runs from the rate the category includes up to
    /// [`MAX_PROVISIONED_API_RATE`], and a rate outside it is refused. A
    /// refused call stores nothing, so the limit a later read reports is the
    /// one that was already in force.
    pub fn set_api_category_limit(
        &mut self,
        category: &ApiCategory,
        requested: i32,
    ) -> Result<LimitType, RateRefusal> {
        if requested < category.free_rate {
            return Err(RateRefusal::BelowFreeRate {
                requested,
                free_rate: category.free_rate,
            });
        }
        if requested > MAX_PROVISIONED_API_RATE {
            return Err(RateRefusal::AboveAccountMaximum {
                requested,
                maximum: MAX_PROVISIONED_API_RATE,
            });
        }
        self.provisioned_api_rates
            .insert(category.name.to_string(), requested);
        Ok(self.api_category_limit(category))
    }
}

// ---------------------------------------------------------------------------
// Pagination
//
// Every paginated listing of this service pages the same way: over a stable
// ascending order of keys, with an OPAQUE `NextToken` naming the key the next
// page resumes strictly after.
// ---------------------------------------------------------------------------

/// The most items one page of a paginated listing holds, and what a request
/// that asks for no page size gets: `QueryLimitType`'s modelled range is 0..60
/// (this service's model, e.g. `AdminListGroupsForUser.Limit`), so 60 is the
/// most a caller can ask for.
pub const MAX_QUERY_LIMIT: i32 = 60;

/// Marks a `NextToken` as one THIS service minted, so a token it never minted
/// is refused on its prefix rather than on whether its bytes happen to decode.
const PAGE_TOKEN_PREFIX: &str = "cognito-idp-page-1:";

/// Separates a page token's listing scope from the key it resumes after. ASCII
/// unit separator: `PaginationKey`'s pattern is `^[\S]+$`, but the token is
/// base64-encoded before it reaches the wire, so the separator only has to be a
/// byte no scope carries.
const PAGE_TOKEN_SEPARATOR: char = '\u{1f}';

/// A `NextToken` that resumes `scope`'s listing strictly after `last_key`.
///
/// `scope` names the listing the token belongs to — the operation plus the
/// resource whose items it pages — so a token one listing issued names no page
/// of another. The token is base64-encoded with `aws_smithy_types::base64`, the
/// codec the wire protocols already carry, which keeps it opaque to the caller
/// and free of the whitespace `PaginationKey`'s pattern `^[\S]+$` excludes.
#[must_use]
pub fn encode_page_token(scope: &str, last_key: &str) -> String {
    ::aws_smithy_types::base64::encode(format!(
        "{PAGE_TOKEN_PREFIX}{scope}{PAGE_TOKEN_SEPARATOR}{last_key}"
    ))
}

/// The key `token` resumes `scope`'s listing after, or `None` when `token` is
/// not a token this service minted for that listing.
#[must_use]
pub fn decode_page_token(scope: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::aws_smithy_types::base64::decode(token).ok()?).ok()?;
    let body = decoded.strip_prefix(PAGE_TOKEN_PREFIX)?;
    let (token_scope, last_key) = body.split_once(PAGE_TOKEN_SEPARATOR)?;
    (token_scope == scope).then(|| last_key.to_string())
}

/// One page of a paginated listing: the keys it serves and the `NextToken` the
/// page after it resumes at, absent when this page is the last one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub keys: Vec<String>,
    pub next_token: Option<String>,
}

/// The message an `InvalidParameterException` carries for a pagination token that
/// names no page of the listing it was presented to.
///
/// Amazon Cognito reports a token it cannot resume a listing from in FIXED text
/// that names neither the token nor the resource the listing was over: a token is
/// a value the caller holds already and the pool it belonged to is not something
/// the refusal discloses. It does not word that sentence identically in every list
/// API — the group listing answers
/// [`PAGINATION_TOKEN_INVALID_RETRY_MESSAGE`] — so an operation names the sentence
/// ITS OWN API answers, and this is the one the rest of them answer.
pub const PAGINATION_TOKEN_INVALID_MESSAGE: &str = "Invalid pagination token given.";

/// The same refusal as the group listing (`ListGroups`) words it: the sentence that
/// also tells the caller what to do about it. See
/// [`PAGINATION_TOKEN_INVALID_MESSAGE`].
pub const PAGINATION_TOKEN_INVALID_RETRY_MESSAGE: &str =
    "Provided pagination token is invalid. Check the token and try your request again.";

/// How many items a listing serves for `limit`.
///
/// `QueryLimitType`'s modelled range (0..60) is refused at the boundary before
/// a handler runs, so the cap here never changes a request the framework
/// accepted. A request that asks for no limit takes the modelled maximum, and
/// so does one that asks for `0`: a page that may hold no item terminates no
/// pagination loop, and this service's model already reads a submitted `0` as
/// "no value" elsewhere (`PasswordPolicyType.TemporaryPasswordValidityDays`).
#[must_use]
pub fn page_size(limit: Option<i32>) -> usize {
    let maximum = MAX_QUERY_LIMIT as usize;
    match limit {
        Some(limit) if limit > 0 => (limit as usize).min(maximum),
        _ => maximum,
    }
}

/// The page of `keys` that `limit` and `next_token` ask for in `scope`'s
/// listing, or `None` when `next_token` is not a token this listing minted —
/// which the calling operation refuses with its own modelled error.
///
/// `keys` must be in the listing's stable ASCENDING order; the page resumes at
/// the first key strictly after the one the token names, so a key deleted
/// between two pages neither repeats nor skips the keys around it.
#[must_use]
pub fn page_of(
    scope: &str,
    keys: &[String],
    limit: Option<i32>,
    next_token: Option<&str>,
) -> Option<Page> {
    let start = match next_token {
        None => 0,
        Some(token) => {
            let after = decode_page_token(scope, token)?;
            keys.partition_point(|key| key <= &after)
        }
    };
    let end = start.saturating_add(page_size(limit)).min(keys.len());
    let served = keys.get(start..end).unwrap_or_default().to_vec();
    let next_token = match served.last() {
        Some(last) if end < keys.len() => Some(encode_page_token(scope, last)),
        _ => None,
    };
    Some(Page {
        keys: served,
        next_token,
    })
}

/// Whether `c` counts as a symbol for a password policy: anything that is
/// neither a letter nor a digit.
fn is_symbol(c: char) -> bool {
    !c.is_alphanumeric()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCognitoidentityproviderAwscognitoidentityproviderserviceState>)
}
