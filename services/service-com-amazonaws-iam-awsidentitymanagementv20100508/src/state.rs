// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iam-awsidentitymanagementv20100508 service.
//!
//! This module owns the persisted IAM entities plus the small helpers every
//! handler needs (name keying, ARN building, id minting, policy-document checks,
//! opaque pagination markers). Handlers reach it through [`service_state`]; they
//! never keep a second copy of a resource.

use std::collections::{BTreeMap, BTreeSet};

use crate::ops::types::{
    AttachedPermissionsBoundary, InstanceProfile, LoginProfile, Policy, PolicyVersion, Role,
    ServiceSpecificCredentialMetadata, Tag, User,
};

/// IAM's default entity path: every entity whose request omits `Path` is created
/// directly under the account root. The model's `pathType` documentation states
/// the parameter "defaults to a slash (/)".
pub const DEFAULT_PATH: &str = "/";

/// The number of entities `ListUsers` (and IAM's other list operations) returns
/// when the request supplies no `MaxItems`. IAM's `MaxItems` documentation states
/// that "if you do not include this parameter, the number of items defaults to
/// 100".
pub const DEFAULT_MAX_ITEMS: i32 = 100;

/// The one value the model's `PermissionsBoundaryAttachmentType` enum declares
/// (`smithy_query(query="shape", shape="PermissionsBoundaryAttachmentType")`),
/// reported on every entity that carries a permissions boundary.
pub const PERMISSIONS_BOUNDARY_TYPE: &str = "PermissionsBoundaryPolicy";

/// The `EntityType` value that names an IAM role — the value `ListEntitiesForPolicy`'s
/// `EntityFilter` documentation itself uses ("when EntityFilter is Role, only the
/// roles that are attached to the specified policy are returned"). One of the five
/// values the enum declares, which `ops::types::ENTITY_TYPE_VALUES` records.
pub const ENTITY_FILTER_ROLE: &str = "Role";

/// The `EntityType` value that names an IAM user. See [`ENTITY_FILTER_ROLE`].
pub const ENTITY_FILTER_USER: &str = "User";

/// The `PolicyUsageType` value that names a policy used as a permissions POLICY:
/// "to list only permissions policies, set PolicyUsageFilter to PermissionsPolicy"
/// (`ListEntitiesForPolicy$PolicyUsageFilter`). One of the two values the enum
/// declares, which `ops::types::POLICY_USAGE_TYPE_VALUES` records.
pub const POLICY_USAGE_PERMISSIONS_POLICY: &str = "PermissionsPolicy";

/// The `PolicyUsageType` value that names a policy used to set a permissions
/// BOUNDARY: "to list only the policies used to set permissions boundaries, set the
/// value to PermissionsBoundary". See [`POLICY_USAGE_PERMISSIONS_POLICY`].
pub const POLICY_USAGE_PERMISSIONS_BOUNDARY: &str = "PermissionsBoundary";

/// The prefix AWS gives an IAM user's unique id. `User$UserId` is modelled as
/// `idType` (len 16..128, pattern `^[\w]+$`), which this prefix plus
/// [`UNIQUE_ID_SUFFIX_LEN`] base32 characters satisfies.
const USER_ID_PREFIX: &str = "AIDA";

/// The prefix AWS gives an IAM role's unique id. `Role$RoleId` is modelled as
/// `idType` (len 16..128, pattern `^[\w]+$`), which this prefix plus
/// [`UNIQUE_ID_SUFFIX_LEN`] base32 characters satisfies.
const ROLE_ID_PREFIX: &str = "AROA";

/// The prefix AWS gives an instance profile's unique id.
/// `InstanceProfile$InstanceProfileId` is modelled as `idType` (len 16..128, pattern
/// `^[\w]+$`), which this prefix plus [`UNIQUE_ID_SUFFIX_LEN`] base32 characters
/// satisfies.
const INSTANCE_PROFILE_ID_PREFIX: &str = "AIPA";

/// The prefix AWS gives a managed policy's unique id. `Policy$PolicyId` is
/// modelled as `idType` (len 16..128, pattern `^[\w]+$`), which this prefix plus
/// [`UNIQUE_ID_SUFFIX_LEN`] base32 characters satisfies.
const POLICY_ID_PREFIX: &str = "ANPA";

/// The version identifier the first version of a managed policy carries.
/// `CreatePolicy`'s documentation: "This operation creates a policy version with a
/// version identifier of v1 and sets v1 as the policy's default version."
pub const FIRST_POLICY_VERSION_ID: &str = "v1";

/// How many characters an IAM role name may carry: `Role$RoleName` is modelled as
/// `roleNameType` with length 1..64
/// (`smithy_query(query="shape", shape="Role")`), so a name a service-linked role
/// create would derive past that bound cannot name a role at all.
pub const MAX_ROLE_NAME_CHARACTERS: usize = 64;

/// How many IAM roles one account may hold. AWS's "IAM and STS quotas" page gives
/// the quota "Roles in an account: 1,000", and a create past it is refused with
/// `LimitExceededException`, whose documentation says "the error message describes
/// the limit exceeded". It is also what stops the role map growing without end.
pub const MAX_ROLES_PER_ACCOUNT: usize = 1_000;

/// How many roles one instance profile may hold. `AddRoleToInstanceProfile`'s
/// documentation: "An instance profile can contain only one role, and this quota
/// cannot be increased."
pub const MAX_ROLES_PER_INSTANCE_PROFILE: usize = 1;

/// The maximum session duration, in seconds, a role created without one carries.
/// `CreateRole$MaxSessionDuration`'s documentation states "if you do not specify a
/// value for this setting, the default value of one hour is applied", and
/// `roleMaxSessionDurationType`'s range (3600..43200) puts one hour at its floor.
pub const DEFAULT_MAX_SESSION_DURATION_SECONDS: i32 = 3_600;

/// How many IAM instance profiles one account may hold. AWS's "IAM and STS quotas"
/// page gives the quota "Instance profiles in an account: 1,000", and a create past
/// it is refused with `LimitExceededException`, whose documentation says "the error
/// message describes the limit exceeded". It is also what stops the instance profile
/// map growing without end.
pub const MAX_INSTANCE_PROFILES_PER_ACCOUNT: usize = 1_000;

/// How many customer managed policies one account may hold. AWS's "IAM and STS
/// quotas" page gives the quota "Customer managed policies in an account: 1,500",
/// and a create past it is refused with `LimitExceededException`, whose
/// documentation says "the error message describes the limit exceeded". It is also
/// what stops the policy map growing without end.
pub const MAX_POLICIES_PER_ACCOUNT: usize = 1_500;

/// The size, in non-whitespace characters, of one managed policy document. AWS's
/// "IAM and STS character quotas" page gives "Managed policy size: 6,144
/// characters" and states that IAM "doesn't count whitespace when calculating the
/// size of a policy against this limitation", so the measure
/// ([`managed_policy_characters`]) skips whitespace too.
pub const MAX_MANAGED_POLICY_CHARACTERS: usize = 6_144;

/// How many IAM users one account may hold. AWS's "IAM and STS quotas" page gives
/// the quota "Users in an account: 5,000", and a create past it is refused with
/// `LimitExceededException`, whose documentation says "the error message describes
/// the limit exceeded".
pub const MAX_USERS_PER_ACCOUNT: usize = 5_000;

/// The status a key IAM has just minted carries. `CreateAccessKey`'s
/// documentation states "the default status for new keys is Active", and `Active`
/// is one of the three values `statusType` declares
/// (`smithy_query(query="shape", shape="statusType")`: Active, Inactive, Expired).
pub const ACCESS_KEY_ACTIVE_STATUS: &str = "Active";

/// How many access keys one IAM user may hold. AWS's "IAM and STS quotas" page
/// gives the quota "Access keys per IAM user: 2", and a create past it is refused
/// with `LimitExceededException`, whose documentation says "the error message
/// describes the limit exceeded".
pub const MAX_ACCESS_KEYS_PER_USER: usize = 2;

/// The aggregate size, in characters, of all inline policies embedded in one IAM
/// user. AWS's "IAM and STS quotas" page gives the quota "User policy size cannot
/// exceed 2,048 characters"; a put that would carry the user past it is refused
/// with `LimitExceededException`.
pub const MAX_USER_INLINE_POLICY_CHARACTERS: usize = 2_048;

/// The aggregate size, in characters, of all inline policies embedded in one IAM
/// role. AWS's "IAM and STS quotas" page gives the quota "Role policy size cannot
/// exceed 10,240 characters"; a put that would carry the role past it is refused
/// with `LimitExceededException`.
pub const MAX_ROLE_INLINE_POLICY_CHARACTERS: usize = 10_240;

/// How many sets of service-specific credentials one IAM user may hold for one
/// supported service. `CreateServiceSpecificCredential`'s documentation: "You can
/// have a maximum of two sets of service-specific credentials for each supported
/// service per user", so a create past it is refused with `LimitExceededException`,
/// whose documentation says "the error message describes the limit exceeded".
pub const MAX_SERVICE_SPECIFIC_CREDENTIALS_PER_SERVICE: usize = 2;

/// What joins the IAM user name to the account id in a generated `ServiceUserName`.
/// `ServiceSpecificCredential$ServiceUserName`: the value "is generated by combining
/// the IAM user's name combined with the ID number of the Amazon Web Services
/// account, as in `jane-at-123456789012`".
const SERVICE_USER_NAME_INFIX: &str = "-at-";

/// The version tag a long-term API key's `ServiceCredentialAlias` carries.
/// `ServiceSpecificCredential$ServiceCredentialAlias` says the alias "includes the
/// IAM user name and a suffix containing version and creation information"; this is
/// the version part of that suffix, and the creation instant is the rest.
const API_KEY_ALIAS_VERSION: &str = "v1";

/// Seconds in a day: the unit `CreateServiceSpecificCredential$CredentialAgeDays`
/// counts ("the number of days until the service specific credential expires").
const SECONDS_PER_DAY: i64 = 86_400;

/// The status a freshly created credential carries — `statusType`'s `Active`, which
/// `ServiceSpecificCredential$Status` documents as "the key is valid for API calls".
pub const CREDENTIAL_STATUS_ACTIVE: &str = "Active";

/// The status a credential whose expiration date has passed carries: `statusType`'s
/// third value (`smithy_query(query="shape", shape="statusType")` gives `Active`,
/// `Inactive`, `Expired`).
pub const CREDENTIAL_STATUS_EXPIRED: &str = "Expired";

/// The path AWS reserves for service-linked roles: every service-linked role is
/// filed under this prefix followed by the service principal it is linked to
/// (`CreateServiceLinkedRole`'s handler builds the path, and
/// [`is_service_linked_role_path`] recognises one). It is shared because a role's
/// path is what tells an operation the role is service-linked, and
/// `UnmodifiableEntityException`'s documentation says "only the service that
/// depends on the service-linked role can modify or delete the role on your
/// behalf".
pub const SERVICE_LINKED_ROLE_PATH_PREFIX: &str = "/aws-service-role/";

/// The account segment an AWS-managed policy's ARN carries
/// (`arn:aws:iam::aws:policy/...`): AWS-managed policies are owned by AWS rather
/// than by an account, which is what distinguishes them from the
/// customer-managed policies an account creates.
pub const AWS_MANAGED_POLICY_ACCOUNT: &str = "aws";

/// How many managed policies one IAM role may carry. AWS's "IAM and STS quotas"
/// page gives the quota "Managed policies attached to an IAM role: 10", and an
/// attach past it is refused with `LimitExceededException`, whose documentation
/// says "the error message describes the limit exceeded". It is also what stops a
/// role's attachment set growing without end.
pub const MAX_MANAGED_POLICIES_PER_ROLE: usize = 10;

/// The tag-key prefix AWS reserves. "Tagging IAM resources" (IAM User Guide):
/// "You cannot create tag keys that begin with `aws:` because this prefix is
/// reserved for Amazon Web Services use."
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// How many tags ONE IAM resource may carry. AWS's "IAM and STS quotas" page gives
/// the quota "Number of tags per IAM resource: 50" — the same bound `tagListType`
/// puts on a single request's list (len 0..50,
/// `smithy_query(query="shape", shape="tagListType")`), which is why the bound has
/// to be taken against the resource's tag set AFTER the merge and not just against
/// the request. A tag call that would carry a resource past it is refused with
/// `LimitExceededException`, whose documentation says "the error message describes
/// the limit exceeded", and nothing is stored: `TagRole`'s documentation states
/// "if you exceed the allowed maximum number of tags, then the entire request
/// fails".
pub const MAX_TAGS_PER_ENTITY: usize = 50;

/// How many base32 characters follow the prefix in a minted unique id.
const UNIQUE_ID_SUFFIX_LEN: usize = 17;

/// What every pagination `Marker` this service issues carries, inside the encoded
/// payload, so a token it did not issue is recognised as foreign rather than read
/// as a position in the store. The SCOPE each marker is minted with follows it
/// (see [`encode_marker`]), so a token issued for one collection is not read as a
/// position in another.
const MARKER_PREFIX: &str = "iam:";

/// The marker scope of the account's user collection, which `ListUsers` pages
/// over.
pub const MARKER_SCOPE_USERS: &str = "users";

/// The marker scope of the account's role collection, which `ListRoles` pages
/// over. It is its OWN scope, so a marker issued while paging roles is not read
/// as a position in the user collection and vice versa.
pub const MARKER_SCOPE_ROLES: &str = "roles";

/// The marker scope of ONE role's attached managed policies, which
/// `ListAttachedRolePolicies` pages over. The role's key follows it, so a marker
/// issued while paging one role's attachments does not name a position in
/// another's: see [`role_attached_policies_marker_scope`].
pub const MARKER_SCOPE_ROLE_ATTACHED_POLICIES: &str = "role-attached-policies";

/// The marker scope of ONE role's EMBEDDED inline policies, which
/// `ListRolePolicies` pages over. The role's key follows it, exactly as it does
/// for the attachment set, so a marker issued while paging one role's inline
/// policies names no position in another role's — and a marker issued for a
/// role's attachments is not read as a position among its inline policies:
/// see [`role_policies_marker_scope`].
pub const MARKER_SCOPE_ROLE_POLICIES: &str = "role-policies";

/// The marker scope of the entities ONE managed policy relates to, which
/// `ListEntitiesForPolicy` pages over. The policy's ARN follows it, so a marker
/// issued while paging the entities of one policy does not name a position in
/// another's: see [`entities_for_policy_marker_scope`]. One scope covers that
/// policy's whole entity collection because the operation pages over the same key
/// order whichever entity type, usage or path prefix it filters by — the filters
/// select within the order rather than changing it.
pub const MARKER_SCOPE_ENTITIES_FOR_POLICY: &str = "entities-for-policy";

/// The marker scope of the account's service-specific credentials, which
/// `ListServiceSpecificCredentials` pages over. One scope covers the whole
/// collection because that operation pages over the same key order whichever user
/// or service it filters by — the filters select within the order rather than
/// changing it.
pub const MARKER_SCOPE_SERVICE_SPECIFIC_CREDENTIALS: &str = "service-specific-credentials";

/// The alphabet the suffix is drawn from: RFC 4648 base32, every character of
/// which is an uppercase alphanumeric and so inside `idType`'s `^[\w]+$`.
const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// The FORM of credential a service issues to an IAM user: the two shapes the
/// `ServiceSpecificCredential` structure describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceCredentialForm {
    /// A generated user name and password — `ServiceUserName` plus `ServicePassword`
    /// — which is what `CreateServiceSpecificCredential` documents it generates:
    /// "a set of credentials consisting of a user name and password".
    UserNameAndPassword,
    /// A long-term API key: the public `ServiceCredentialAlias` plus the secret
    /// `ServiceCredentialSecret`, which both members document as being "For Bedrock
    /// API keys and CloudWatch Logs API keys". These are the services
    /// `CredentialAgeDays` and `ExpirationDate` apply to.
    LongTermApiKey,
}

/// One service that supports service-specific credentials.
#[derive(Debug)]
pub struct ServiceWithCredentials {
    /// The service principal the request names in `ServiceName`.
    pub service_name: &'static str,
    /// The form of credential this service issues.
    pub form: ServiceCredentialForm,
}

/// Every service that supports service-specific credentials, with the form each
/// issues — the catalog `ServiceNotSupportedException` ("the specified service does
/// not support service-specific credentials") is taken against.
///
/// Source: `CreateServiceSpecificCredential`'s own documentation, which names
/// CodeCommit ("IAM credentials for CodeCommit: Git credentials, SSH keys") and
/// Amazon Keyspaces ("Use IAM with Amazon Keyspaces (for Apache Cassandra)") for
/// the user-name-and-password form and then "services that support long-term API
/// keys", which `ServiceSpecificCredential$ExpirationDate` and
/// `$ServiceCredentialAlias` name as "Bedrock API keys and CloudWatch Logs API
/// keys". Each entry is that service's service principal.
pub const SERVICES_WITH_SPECIFIC_CREDENTIALS: &[ServiceWithCredentials] = &[
    ServiceWithCredentials {
        service_name: "bedrock.amazonaws.com",
        form: ServiceCredentialForm::LongTermApiKey,
    },
    ServiceWithCredentials {
        service_name: "cassandra.amazonaws.com",
        form: ServiceCredentialForm::UserNameAndPassword,
    },
    ServiceWithCredentials {
        service_name: "codecommit.amazonaws.com",
        form: ServiceCredentialForm::UserNameAndPassword,
    },
    ServiceWithCredentials {
        service_name: "logs.amazonaws.com",
        form: ServiceCredentialForm::LongTermApiKey,
    },
];

/// The catalog entry for `service_name`, or `None` when no service by that name
/// supports service-specific credentials.
///
/// A service principal is a DNS name, and DNS names are case-insensitive, so the
/// lookup folds ASCII case and the caller reports the catalog's own spelling.
pub fn service_with_credentials(service_name: &str) -> Option<&'static ServiceWithCredentials> {
    SERVICES_WITH_SPECIFIC_CREDENTIALS
        .iter()
        .find(|service| service.service_name.eq_ignore_ascii_case(service_name))
}

/// The `ServiceUserName` IAM generates for a service-specific credential of the
/// user-name-and-password form: the IAM user's name joined to the account id (see
/// [`SERVICE_USER_NAME_INFIX`]).
///
/// Inside `serviceUserName`'s constraints (len 0..200, pattern `^[\w+=,.@-]*$`) for
/// every name `userNameType` admits: that pattern is a subset of this one, the
/// infix and the account id add 16 characters, and a user name is at most 64.
pub fn service_user_name(user_name: &str) -> String {
    format!(
        "{user_name}{SERVICE_USER_NAME_INFIX}{}",
        ::engine::emulator_account_id()
    )
}

/// The `ServiceCredentialAlias` IAM generates for a long-term API key: the IAM
/// user's name followed by the suffix `ServiceCredentialAlias` documents, "version
/// and creation information" — the alias format's version and the second the key was
/// created.
///
/// The exact suffix AWS assembles is not documented, so this is a server-generated
/// value of the modelled type (`serviceCredentialAlias`, len 0..200, pattern
/// `^[\w+=,.@-]+$`): it is built once at creation, stored with the credential, and
/// every later read reports the stored value.
pub fn api_key_alias(user_name: &str, created: ::aws_smithy_types::DateTime) -> String {
    format!("{user_name}-{API_KEY_ALIAS_VERSION}-{}", created.secs())
}

/// When a credential created at `created` with `credential_age_days` expires:
/// `CredentialAgeDays` is "the number of days until the service specific credential
/// expires", counted from its creation.
pub fn credential_expiration(
    created: ::aws_smithy_types::DateTime,
    credential_age_days: i32,
) -> ::aws_smithy_types::DateTime {
    let age = i64::from(credential_age_days).saturating_mul(SECONDS_PER_DAY);
    ::aws_smithy_types::DateTime::from_secs(created.secs().saturating_add(age))
}

/// A fresh service-specific credential: the id to report it under, and its secret.
///
/// A service-specific credential authenticates its IAM user to ONE other Amazon Web
/// Services service — CodeCommit over HTTPS Git, Amazon Keyspaces over CQL — and
/// never to this emulator over SigV4, so the pair must not stay in the emulator's
/// credential store. Nothing in a service may construct an id or a secret itself,
/// though, so both are minted by the one component that generates them, the
/// credential store, and the record is then removed from it again — the shape
/// `service-com-amazonaws-mwaa-amazonmwaa`'s `mint_airflow_token` uses for the same
/// reason. The id that comes back is 20 characters from the base32 alphabet, inside
/// `serviceSpecificCredentialId`'s len 20..128 and `^[\w]+$`.
///
/// `None` when the store minted no secret, which leaves the caller to refuse rather
/// than report a credential it made up.
pub fn mint_service_specific_credential(
    store: &::emulator_server::CredentialStore,
) -> Option<(String, String)> {
    let minted = store.create(&::emulator_server::aws_credentials::AWS_ACCESS_KEY);
    let secret = minted
        .secret(::emulator_server::aws_credentials::SECRET_ACCESS_KEY)
        .map(str::to_string);
    // The pair is not a credential of this emulator; see above.
    let _ = store.delete(&minted.id);
    secret.map(|secret| (minted.id, secret))
}

/// One inline policy embedded in an IAM entity.
///
/// The name is kept beside the document because the map is keyed by the
/// case-folded name ([`name_key`]) while a read must report the policy as the
/// account holds it.
#[derive(Debug, Clone)]
pub struct InlinePolicy {
    /// The policy name in the case the last put chose.
    pub policy_name: String,
    /// The policy document exactly as it was stored.
    pub policy_document: String,
}

/// One managed policy the account holds.
///
/// The modelled [`Policy`] is kept exactly as every reader reports it, beside the
/// policy's versions — the `Policy` shape carries no document, so the document a
/// create or an update supplied lives in the version it created.
#[derive(Debug, Clone)]
pub struct ManagedPolicy {
    /// The policy as `CreatePolicy` returned it and `GetPolicy` reports it.
    pub policy: Policy,
    /// The policy's versions, keyed by `PolicyVersion$VersionId` (`v1`, `v2`, …).
    pub versions: BTreeMap<String, PolicyVersion>,
}

/// One instance profile the account holds.
///
/// The modelled [`InstanceProfile`] is kept exactly as every reader reports it, beside
/// the BINDING `AddRoleToInstanceProfile` writes: the [`name_key`]s of the roles the
/// profile holds. A bound role is NOT copied in here — it stays in
/// [`ComAmazonawsIamAwsidentitymanagementv20100508State::roles`], the one record of what
/// a role is, so a read of the profile reports that role as it stands now rather than a
/// snapshot taken when it was bound (see
/// [`instance_profile_report`](ComAmazonawsIamAwsidentitymanagementv20100508State::instance_profile_report)).
#[derive(Debug, Clone)]
pub struct InstanceProfileRecord {
    /// The profile as `CreateInstanceProfile` returned it and `GetInstanceProfile`
    /// reports it. Its `roles` member is not the binding and is never read: the report
    /// builder fills it from [`role_keys`](Self::role_keys).
    pub profile: InstanceProfile,
    /// The [`name_key`]s of the roles bound to this profile, in the order they were
    /// bound. At most [`MAX_ROLES_PER_INSTANCE_PROFILE`] of them, because "an instance
    /// profile can contain only one role, and this quota cannot be increased"
    /// (`AddRoleToInstanceProfile`).
    pub role_keys: Vec<String>,
}

/// One access key an IAM user holds, keyed in [`ComAmazonawsIamAwsidentitymanagementv20100508State::access_keys`]
/// by the access key id.
///
/// The id and its secret are minted by the emulator's credential store, which is
/// the only thing that creates credential material; the secret is never stored
/// here, because `AccessKey$SecretAccessKey` "is returned only in response to
/// CreateAccessKey" (the `AccessKey` shape's documentation) and nothing may hand
/// it out again. The owning user's name is not copied in either: it is read from
/// the user the record's [`user_key`](AccessKeyRecord::user_key) names, so the
/// name lives in exactly one place.
#[derive(Debug, Clone)]
pub struct AccessKeyRecord {
    /// [`name_key`] of the IAM user that owns the key.
    pub user_key: String,
    /// The key's `statusType` value: [`ACCESS_KEY_ACTIVE_STATUS`] when it was
    /// minted, until an update moves it.
    pub status: String,
    /// The instant the key was minted, reported unchanged by every later read.
    pub create_date: ::aws_smithy_types::DateTime,
}

/// One IAM user's login profile: the console password `CreateLoginProfile`
/// created, as the account holds it.
///
/// The PASSWORD itself is deliberately not among its members. `LoginProfile` (the
/// only shape any operation reports a login profile through) carries just
/// `UserName`, `CreateDate` and `PasswordResetRequired`
/// (`smithy_query(query="shape", shape="LoginProfile")`), the model marks
/// `CreateLoginProfile$Password` sensitive, and this emulator serves no console
/// sign-in, so nothing can ever read the password back: holding it would be
/// keeping a secret no caller may be given. The owning user's NAME is not copied
/// in either, for the same reason [`AccessKeyRecord`] does not copy it — it is
/// read from the user the record's key names, so the name lives in one place.
#[derive(Debug, Clone)]
pub struct LoginProfileRecord {
    /// The instant the password was created, reported unchanged by every later
    /// read: "the CreateDate reflects the initial password creation date"
    /// (`GetLoginProfile`).
    pub create_date: ::aws_smithy_types::DateTime,
    /// Whether "the user is required to set a new password on next sign-in"
    /// (`CreateLoginProfile$PasswordResetRequired`), as the create asked.
    pub password_reset_required: bool,
}

/// Per-service state for com-amazonaws-iam-awsidentitymanagementv20100508. A handler reaches it through [`service_state`].
///
/// Every map is a `BTreeMap` so enumeration order is total and stable, which is
/// what lets a list operation hand out a resumable marker: the next page starts
/// strictly after the key the marker names.
#[derive(Debug, Default)]
pub struct ComAmazonawsIamAwsidentitymanagementv20100508State {
    /// The persisted IAM users, keyed by [`name_key`] of the user name. IAM names
    /// "are not distinguished by case" (`CreateUser$UserName` documentation), so
    /// the key is the case-folded name while `User::user_name` keeps the case the
    /// creator chose.
    pub users: BTreeMap<String, User>,
    /// The persisted IAM roles, keyed by [`name_key`] of the role name, for the
    /// same reason users are: IAM entity names are unique within the account and
    /// "are not distinguished by case", while `Role::role_name` keeps the case the
    /// name was minted with.
    pub roles: BTreeMap<String, Role>,
    /// Inline user policies: [`name_key`] of the user name -> [`name_key`] of the
    /// policy name -> the policy. Policy names are case-folded for the same reason
    /// user names are: the model's `CreateUser$UserName` documentation says "user,
    /// group, role, and policy names must be unique within the account. Names are
    /// not distinguished by case."
    pub user_inline_policies: BTreeMap<String, BTreeMap<String, InlinePolicy>>,
    /// Inline role policies: [`name_key`] of the role name -> [`name_key`] of the
    /// policy name -> the policy. Keyed exactly as
    /// [`user_inline_policies`](Self::user_inline_policies) is, and for the same
    /// reason: "user, group, role, and policy names must be unique within the
    /// account. Names are not distinguished by case."
    pub role_inline_policies: BTreeMap<String, BTreeMap<String, InlinePolicy>>,
    /// Access key id -> the key's record, including [`name_key`] of the user that
    /// owns it. `GetUser` with no `UserName` resolves the caller through this map,
    /// and `CreateAccessKey` is what populates it.
    pub access_keys: BTreeMap<String, AccessKeyRecord>,
    /// The persisted login profiles, keyed by [`name_key`] of the user name — the
    /// same key [`Self::users`] uses, because a user has AT MOST ONE login profile
    /// ("a login profile is created when you create a password for the user",
    /// `GetLoginProfile`) and that profile is reached only through its user.
    /// `CreateLoginProfile` is what fills this map and `GetLoginProfile` reads it.
    pub login_profiles: BTreeMap<String, LoginProfileRecord>,
    /// The persisted service-specific credentials, keyed by their
    /// `ServiceSpecificCredentialId` — the handle every other operation on one takes
    /// (`ListServiceSpecificCredentials` reports it,
    /// `DeleteServiceSpecificCredential`, `ResetServiceSpecificCredential` and
    /// `UpdateServiceSpecificCredential` name it). Keying by the id keeps
    /// enumeration order total and stable, which is what lets the list operation
    /// hand out a resumable marker.
    ///
    /// The stored value is the `ServiceSpecificCredentialMetadata` shape, so one
    /// record serves both the create response and the list; the generated PASSWORD
    /// is deliberately not among its members, because the create response "is the
    /// only time that the password for this credential set is available. It cannot
    /// be recovered later"
    /// (`CreateServiceSpecificCredential$ServiceSpecificCredential`).
    pub service_specific_credentials: BTreeMap<String, ServiceSpecificCredentialMetadata>,
    /// The persisted customer managed policies, keyed by [`name_key`] of the
    /// policy name, for the same reason users and roles are: the model's
    /// `CreatePolicy$PolicyName` documentation says "IAM user, group, role, and
    /// policy names must be unique within the account. Names are not
    /// distinguished by case."
    ///
    /// This is also the ONE record of which customer-managed policies the account
    /// holds, which is what makes such a policy attachable: an ARN naming a policy
    /// this map does not carry refers to no entity (see [`Self::policy_by_arn`]).
    /// AWS-managed policies (`arn:aws:iam::aws:policy/...`) are NOT in here — they
    /// are owned by AWS, not by the account ([`AWS_MANAGED_POLICY_ACCOUNT`]).
    pub policies: BTreeMap<String, ManagedPolicy>,
    /// The persisted IAM instance profiles, keyed by [`name_key`] of the instance
    /// profile name, for the same reason users and roles are: IAM entity names are
    /// unique within the account and "are not distinguished by case", while
    /// `InstanceProfile::instance_profile_name` keeps the case the creator chose.
    ///
    /// The stored value is an [`InstanceProfileRecord`]: the modelled profile beside the
    /// names of the roles bound to it — "The role associated with the instance profile"
    /// (`InstanceProfile$Roles`) — so `AddRoleToInstanceProfile` has one place to bind a
    /// role and `GetInstanceProfile` one place to read the binding from, while the role
    /// itself stays in [`Self::roles`].
    pub instance_profiles: BTreeMap<String, InstanceProfileRecord>,
    /// The managed policies attached to each role: [`name_key`] of the role name
    /// -> the attached policies' ARNs. A `BTreeSet` because a policy is attached
    /// to a role at most once (a repeat attach is the same attachment) and
    /// because the total ARN order is what lets
    /// `ListAttachedRolePolicies` hand out a resumable marker.
    pub role_attached_policies: BTreeMap<String, BTreeSet<String>>,
    /// Monotonic counter feeding [`next_user_id`](Self::next_user_id).
    pub user_id_seq: u64,
    /// Monotonic counter feeding [`next_role_id`](Self::next_role_id).
    pub role_id_seq: u64,
    /// Monotonic counter feeding [`next_policy_id`](Self::next_policy_id).
    pub policy_id_seq: u64,
    /// Monotonic counter feeding
    /// [`next_instance_profile_id`](Self::next_instance_profile_id).
    pub instance_profile_id_seq: u64,
}

impl ComAmazonawsIamAwsidentitymanagementv20100508State {
    /// Mint the next user's `UserId`.
    ///
    /// Unique by construction: the counter is bijectively mixed and the whole
    /// 64-bit result is spelled out in the suffix, so two distinct counter values
    /// can never produce one id. Stable by construction too — the id is minted
    /// once, stored with the user, and every later read returns the stored value.
    pub fn next_user_id(&mut self) -> String {
        self.user_id_seq += 1;
        unique_id(USER_ID_PREFIX, self.user_id_seq)
    }

    /// Mint the next role's `RoleId`.
    ///
    /// Unique and stable for the same reason [`next_user_id`](Self::next_user_id)
    /// is: the counter is bijectively mixed into the suffix, and the id is minted
    /// once and then stored with the role.
    pub fn next_role_id(&mut self) -> String {
        self.role_id_seq += 1;
        unique_id(ROLE_ID_PREFIX, self.role_id_seq)
    }

    /// How many access keys the user keyed by `user_key` holds.
    ///
    /// This is the measure IAM's access-keys-per-user quota
    /// ([`MAX_ACCESS_KEYS_PER_USER`]) is taken against, and what tells a delete
    /// that credentials still depend on the user.
    pub fn access_keys_held_by(&self, user_key: &str) -> usize {
        self.access_keys
            .values()
            .filter(|record| record.user_key == user_key)
            .count()
    }

    /// Materialise the status changes the clock has already made due: a
    /// service-specific credential whose `ExpirationDate` has passed is `Expired`,
    /// whether or not anybody called.
    ///
    /// This is the only write a reporting operation on these credentials performs,
    /// and it is owed at the same instant regardless of the call that observes it.
    /// Every other status change is a caller's
    /// (`UpdateServiceSpecificCredential` moves one between `Active` and
    /// `Inactive`).
    pub fn expire_service_specific_credentials(&mut self, now: ::aws_smithy_types::DateTime) {
        for credential in self.service_specific_credentials.values_mut() {
            let due = credential
                .expiration_date
                .is_some_and(|expires_at| expires_at.secs() <= now.secs());
            if due {
                credential.status = CREDENTIAL_STATUS_EXPIRED.to_string();
            }
        }
    }

    /// How many sets of service-specific credentials the user keyed by `user_key`
    /// holds for `service_name` — the count
    /// [`MAX_SERVICE_SPECIFIC_CREDENTIALS_PER_SERVICE`] is taken against.
    ///
    /// Both the user name and the service name are compared case-insensitively, for
    /// the same reasons the stores key on them that way: IAM entity names "are not
    /// distinguished by case", and a service principal is a DNS name.
    pub fn service_specific_credential_count(&self, user_key: &str, service_name: &str) -> usize {
        self.service_specific_credentials
            .values()
            .filter(|credential| {
                name_key(&credential.user_name) == user_key
                    && credential.service_name.eq_ignore_ascii_case(service_name)
            })
            .count()
    }

    /// Mint the next managed policy's `PolicyId`.
    ///
    /// Unique and stable for the same reason [`next_user_id`](Self::next_user_id)
    /// is: the counter is bijectively mixed into the suffix, and the id is minted
    /// once and then stored with the policy.
    pub fn next_policy_id(&mut self) -> String {
        self.policy_id_seq += 1;
        unique_id(POLICY_ID_PREFIX, self.policy_id_seq)
    }

    /// Mint the next instance profile's `InstanceProfileId`.
    ///
    /// Unique and stable for the same reason [`next_user_id`](Self::next_user_id) is: the
    /// counter is bijectively mixed into the suffix, and the id is minted once and then
    /// stored with the profile.
    pub fn next_instance_profile_id(&mut self) -> String {
        self.instance_profile_id_seq += 1;
        unique_id(INSTANCE_PROFILE_ID_PREFIX, self.instance_profile_id_seq)
    }

    /// The managed policy `arn` names, if the account holds it.
    ///
    /// Every modelled read of a managed policy addresses it by ARN while the map
    /// is keyed by the name (the identity IAM enforces uniqueness on), so the ARN
    /// is parsed ([`parse_policy_arn`]) for the name to look up, and the held
    /// policy answers only when its own ARN is the one asked for — so an ARN that
    /// carries another path, another account or another casing of the name names no
    /// policy.
    pub fn policy_by_arn(&self, arn: &str) -> Option<&ManagedPolicy> {
        let parsed = parse_policy_arn(arn)?;
        let held = self.policies.get(&name_key(&parsed.policy_name))?;
        (held.policy.arn.as_deref() == Some(arn)).then_some(held)
    }

    /// The aggregate size, in characters, of the inline policies embedded in the
    /// user keyed by `user_key`, counting every policy except the one keyed by
    /// `replacing` (a put of an existing name replaces that document rather than
    /// adding to it).
    ///
    /// This is the measure IAM's user policy size quota
    /// ([`MAX_USER_INLINE_POLICY_CHARACTERS`]) is taken against.
    pub fn user_inline_policy_characters(&self, user_key: &str, replacing: &str) -> usize {
        inline_policy_characters(&self.user_inline_policies, user_key, replacing)
    }

    /// The same measure for the role keyed by `role_key`, the one IAM's role policy
    /// size quota ([`MAX_ROLE_INLINE_POLICY_CHARACTERS`]) is taken against.
    pub fn role_inline_policy_characters(&self, role_key: &str, replacing: &str) -> usize {
        inline_policy_characters(&self.role_inline_policies, role_key, replacing)
    }

    /// The instance profile keyed by `profile_key` as a reader must report it: the
    /// profile's own members, with `Roles` resolved from [`Self::roles`].
    ///
    /// `InstanceProfile$Roles` reports the ROLE, not a name, so the report has to carry
    /// the role as the account holds it at the moment of the read: the binding
    /// `AddRoleToInstanceProfile` wrote names the role, and a later change to that role
    /// (a tag, a description, a permissions boundary) is part of what the next read of
    /// the profile reports. A binding whose role the account no longer holds reports no
    /// role rather than a resurrected copy of it.
    pub fn instance_profile_report(&self, profile_key: &str) -> Option<InstanceProfile> {
        let held = self.instance_profiles.get(profile_key)?;
        Some(InstanceProfile {
            roles: held
                .role_keys
                .iter()
                .filter_map(|role_key| self.roles.get(role_key).cloned())
                .collect(),
            ..held.profile.clone()
        })
    }

    /// The names of the instance profiles the role keyed by `role_key` is bound to, in
    /// the profiles' stable key order.
    ///
    /// This is what tells a delete that an instance profile still depends on the role:
    /// "before you delete a role, remove the following attached items: inline policies,
    /// attached managed policies, instance profile" (`DeleteRole`). The names are the
    /// ones the profiles were created with, so a refusal can say which they are.
    pub fn instance_profiles_holding_role(&self, role_key: &str) -> Vec<String> {
        self.instance_profiles
            .values()
            .filter(|record| record.role_keys.iter().any(|held| held == role_key))
            .map(|record| record.profile.instance_profile_name.clone())
            .collect()
    }

    /// The modelled `LoginProfile` of the user keyed by `user_key`, or `None` when
    /// that user has no password — or no longer exists.
    ///
    /// `LoginProfile$UserName` reports the name as the ACCOUNT holds it, not the
    /// case a request happened to spell, because IAM names "are not distinguished
    /// by case"; the name is therefore read from the user rather than copied into
    /// the login profile record. One definition, shared by the create that returns
    /// the new profile and the read that reports it, so both answer with the same
    /// values from the same state.
    pub fn login_profile_report(&self, user_key: &str) -> Option<LoginProfile> {
        let user = self.users.get(user_key)?;
        let held = self.login_profiles.get(user_key)?;
        Some(LoginProfile {
            user_name: user.user_name.clone(),
            create_date: held.create_date,
            password_reset_required: held.password_reset_required,
        })
    }
}

/// The aggregate size, in characters, of the inline policies `policies` holds for
/// the entity keyed by `entity_key`, counting every policy except the one keyed by
/// `replacing`.
///
/// A put of an existing policy name REPLACES that document rather than adding to
/// it, so the document being replaced does not count toward the quota the new one
/// has to fit inside. One definition, shared by every entity kind that embeds
/// inline policies.
fn inline_policy_characters(
    policies: &BTreeMap<String, BTreeMap<String, InlinePolicy>>,
    entity_key: &str,
    replacing: &str,
) -> usize {
    policies
        .get(entity_key)
        .map(|policies| {
            policies
                .iter()
                .filter(|(key, _)| key.as_str() != replacing)
                .map(|(_, policy)| policy.policy_document.chars().count())
                .sum()
        })
        .unwrap_or(0)
}

/// The case-folded form of an IAM entity name, used as its key.
///
/// IAM entity names "are not distinguished by case", so `MyUser` and `myuser`
/// name one entity. Entity names are constrained to `^[\w+=,.@-]+$`, so ASCII
/// folding is total over the accepted alphabet.
pub fn name_key(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// The ARN of an IAM user: `arn:aws:iam::<account>:user<path><name>`.
///
/// `path` already begins and ends with `/` (the model's `pathType` pattern), so
/// it joins the `user` resource type to the name without extra separators. IAM is
/// a global service, so the region segment is empty.
pub fn user_arn(path: &str, user_name: &str) -> String {
    ::engine::emulator_arn("iam", "", &format!("user{path}{user_name}"))
}

/// The ARN of an IAM managed policy: `arn:aws:iam::<account>:policy<path><name>`.
///
/// Built the same way [`user_arn`] is, from the `policy` resource type.
pub fn policy_arn(path: &str, policy_name: &str) -> String {
    ::engine::emulator_arn("iam", "", &format!("policy{path}{policy_name}"))
}

/// The ARN of an IAM role: `arn:aws:iam::<account>:role<path><name>`.
///
/// Built the same way [`user_arn`] is, from the `role` resource type: `path`
/// already begins and ends with `/`, and IAM is a global service, so the region
/// segment is empty.
pub fn role_arn(path: &str, role_name: &str) -> String {
    ::engine::emulator_arn("iam", "", &format!("role{path}{role_name}"))
}

/// The ARN of an IAM instance profile:
/// `arn:aws:iam::<account>:instance-profile<path><name>`.
///
/// Built the same way [`user_arn`] is, from the `instance-profile` resource type.
pub fn instance_profile_arn(path: &str, instance_profile_name: &str) -> String {
    ::engine::emulator_arn(
        "iam",
        "",
        &format!("instance-profile{path}{instance_profile_name}"),
    )
}

/// The wall-clock instant a resource was created or last changed, in the
/// modelled timestamp type.
pub fn now() -> ::aws_smithy_types::DateTime {
    let since_epoch = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_millis(since_epoch.as_millis() as i64)
}

/// Whether `path` is the path of a service-linked role — see
/// [`SERVICE_LINKED_ROLE_PATH_PREFIX`].
pub fn is_service_linked_role_path(path: &str) -> bool {
    path.starts_with(SERVICE_LINKED_ROLE_PATH_PREFIX)
}

/// The service principal a service-linked role's `path` names, or `None` when
/// `path` is not one — a refinement of [`is_service_linked_role_path`] for the
/// refusals whose message has to name that service.
///
/// A service-linked role lives at `/aws-service-role/<principal>/`, so the segment
/// between that prefix and the closing `/` names the service that depends on the
/// role — the one service AWS lets modify or delete it, which is what
/// `UnmodifiableEntityException`'s documented message identifies.
pub fn service_linked_role_principal(path: &str) -> Option<&str> {
    let principal = path
        .strip_prefix(SERVICE_LINKED_ROLE_PATH_PREFIX)?
        .strip_suffix('/')?;
    (!principal.is_empty()).then_some(principal)
}

/// What every service-linked role name starts with. AWS's "Using service-linked
/// roles" guidance, echoed by `CreateServiceLinkedRole$CustomSuffix`'s
/// documentation ("a string that you provide, which is combined with the
/// service-provided prefix to form the complete role name"), is that the role a
/// service links to itself is named for the service behind this prefix.
pub const SERVICE_LINKED_ROLE_NAME_PREFIX: &str = "AWSServiceRoleFor";

/// The IAM policy language version a generated trust policy declares. It is the
/// current version of the policy grammar IAM documents, and the one AWS emits in
/// the trust policy of a role it creates.
pub const POLICY_LANGUAGE_VERSION: &str = "2012-10-17";

/// The role name AWS PUBLISHES for a service principal's service-linked role.
///
/// The service-provided prefix is [`SERVICE_LINKED_ROLE_NAME_PREFIX`] followed by
/// the service's own AWS-assigned label, and that label is NOT the first label of
/// the principal and is not derivable from it: `ssm-sap.amazonaws.com` links
/// `AWSServiceRoleForAWSSSMForSAP`, `ops.apigateway.amazonaws.com` links
/// `AWSServiceRoleForAPIGateway`, `replication.cassandra.amazonaws.com` links
/// `AWSServiceRoleForKeyspacesReplication`, and each Application Auto Scaling
/// principal links a name suffixed with the resource type it scales. No part of the
/// IAM model carries that label — `CreateServiceLinkedRole$CustomSuffix`'s
/// documentation only says a suffix "is combined with the service-provided prefix to
/// form the complete role name" — so the published names are listed here, and a
/// principal this list does not name falls back to the name
/// `CreateServiceLinkedRole` derives.
///
/// This is also the catalog of the roles the OWNING SERVICE keeps provisioned in
/// the account ([`service_provisioned_role`]), which is why it lives beside the
/// state rather than in one handler: the create and the read resolve one table.
///
/// Source: the "Using service-linked roles for <service>" page of each service's own
/// user guide, which names the role that service creates; AWS's "Amazon Web Services
/// services that work with IAM" table links them all. Rows marked `[model]` are
/// additionally corroborated by the documentation a Smithy model in THIS repository
/// carries, quoted in the named handler.
///
/// Sorted by principal, which is also the lookup key
/// ([`published_service_linked_role_name`]).
pub const PUBLISHED_SERVICE_LINKED_ROLE_NAMES: &[(&str, &str)] = &[
    ("access-analyzer.amazonaws.com", "AWSServiceRoleForAccessAnalyzer"),
    // [model] service-com-amazonaws-applicationsignals-applicationsignals
    // `src/ops/start_discovery.rs`: "creating the
    // AWSServiceRoleForCloudWatchApplicationSignals service-linked role".
    (
        "application-signals.amazonaws.com",
        "AWSServiceRoleForCloudWatchApplicationSignals",
    ),
    // [model] service-com-amazonaws-autoscaling-autoscaling-2011-01-01
    // `src/ops/create_auto_scaling_group.rs`: "service-linked role named
    // AWSServiceRoleForAutoScaling".
    ("autoscaling.amazonaws.com", "AWSServiceRoleForAutoScaling"),
    ("backup.amazonaws.com", "AWSServiceRoleForBackup"),
    // [model] service-com-amazonaws-batch-awsbatchv20160810
    // `src/ops/update_compute_environment.rs`: "the AWSServiceRoleForBatch
    // service-linked role".
    ("batch.amazonaws.com", "AWSServiceRoleForBatch"),
    ("cloudtrail.amazonaws.com", "AWSServiceRoleForCloudTrail"),
    // AWS CodeStar Notifications' own "Using service-linked roles for AWS CodeStar
    // Notifications" page names the role it creates to publish notifications to a
    // rule's targets: AWSServiceRoleForCodeStarNotifications, for the principal
    // codestar-notifications.amazonaws.com. Without the row the derived fallback name
    // is not the one AWS publishes, so a client that creates the role and then reads
    // it back under the published name never converges.
    (
        "codestar-notifications.amazonaws.com",
        "AWSServiceRoleForCodeStarNotifications",
    ),
    // [model] service-com-amazonaws-configservice-starlingdoveservice
    // `src/ops/put_service_linked_configuration_recorder.rs`: "creates a
    // service-linked role AWSServiceRoleForConfig in your account".
    ("config.amazonaws.com", "AWSServiceRoleForConfig"),
    // Application Auto Scaling publishes one role per scaled service, named
    // `AWSServiceRoleForApplicationAutoScaling_<resource type>` — the suffixes this
    // repository already lists in
    // service-com-amazonaws-applicationautoscaling-anyscalefrontendservice
    // `src/state.rs` (`SERVICE_LINKED_ROLE_SUFFIXES`).
    (
        "dynamodb.application-autoscaling.amazonaws.com",
        "AWSServiceRoleForApplicationAutoScaling_DynamoDBTable",
    ),
    ("ec2fleet.amazonaws.com", "AWSServiceRoleForEC2Fleet"),
    // [model] service-com-amazonaws-ecs-amazonec2containerservicev20141113
    // `src/state.rs`: `role/aws-service-role/ecs.amazonaws.com/AWSServiceRoleForECS`.
    ("ecs.amazonaws.com", "AWSServiceRoleForECS"),
    (
        "ecs.application-autoscaling.amazonaws.com",
        "AWSServiceRoleForApplicationAutoScaling_ECSService",
    ),
    ("eks-fargate.amazonaws.com", "AWSServiceRoleForAmazonEKSForFargate"),
    ("eks-nodegroup.amazonaws.com", "AWSServiceRoleForAmazonEKSNodegroup"),
    ("eks.amazonaws.com", "AWSServiceRoleForAmazonEKS"),
    ("elasticache.amazonaws.com", "AWSServiceRoleForElastiCache"),
    ("elasticbeanstalk.amazonaws.com", "AWSServiceRoleForElasticBeanstalk"),
    (
        "elasticloadbalancing.amazonaws.com",
        "AWSServiceRoleForElasticLoadBalancing",
    ),
    // [model] service-com-amazonaws-fms-awsfms-20180101
    // `src/ops/put_notification_channel.rs`: "a role other than the
    // AWSServiceRoleForFMS".
    ("fms.amazonaws.com", "AWSServiceRoleForFMS"),
    ("globalaccelerator.amazonaws.com", "AWSServiceRoleForGlobalAccelerator"),
    ("guardduty.amazonaws.com", "AWSServiceRoleForAmazonGuardDuty"),
    // [model] service-com-amazonaws-lakeformation-awslakeformation
    // `src/ops/register_resource.rs`: "choose the
    // AWSServiceRoleForLakeFormationDataAccess".
    (
        "lakeformation.amazonaws.com",
        "AWSServiceRoleForLakeFormationDataAccess",
    ),
    ("lex.amazonaws.com", "AWSServiceRoleForLexBots"),
    ("macie.amazonaws.com", "AWSServiceRoleForAmazonMacie"),
    (
        "opensearchservice.amazonaws.com",
        "AWSServiceRoleForAmazonOpenSearchService",
    ),
    ("ops.apigateway.amazonaws.com", "AWSServiceRoleForAPIGateway"),
    ("organizations.amazonaws.com", "AWSServiceRoleForOrganizations"),
    ("rds.amazonaws.com", "AWSServiceRoleForRDS"),
    (
        "rds.application-autoscaling.amazonaws.com",
        "AWSServiceRoleForApplicationAutoScaling_RDSCluster",
    ),
    // [model] service-com-amazonaws-keyspaces-keyspacesservice
    // `src/ops/update_keyspace.rs`:
    // `role/aws-service-role/replication.cassandra.amazonaws.com/AWSServiceRoleForKeyspacesReplication`.
    (
        "replication.cassandra.amazonaws.com",
        "AWSServiceRoleForKeyspacesReplication",
    ),
    // AWS Backup Audit Manager: reporting is a FEATURE of AWS Backup with its own
    // principal, and the role it links is named for the feature
    // (`...ForBackupReports`), not for the principal's first label (`reports`) —
    // "Using service-linked roles for AWS Backup" names
    // `AWSServiceRoleForBackupReports` as the role AWS Backup creates to run report
    // jobs, which is the name a client that provisions Backup reporting reads back.
    ("reports.backup.amazonaws.com", "AWSServiceRoleForBackupReports"),
    ("securityhub.amazonaws.com", "AWSServiceRoleForSecurityHub"),
    // Amazon EC2 Spot and Spot Fleet: each role is assumed through the FEATURE's own
    // principal (`spot.amazonaws.com`, `spotfleet.amazonaws.com`), which is neither
    // the role's name nor the `ec2` endpoint prefix.
    ("spot.amazonaws.com", "AWSServiceRoleForEC2Spot"),
    ("spotfleet.amazonaws.com", "AWSServiceRoleForEC2SpotFleet"),
    // AWS Systems Manager for SAP: the role AWS SSM for SAP requires in the account
    // before it will register an application.
    ("ssm-sap.amazonaws.com", "AWSServiceRoleForAWSSSMForSAP"),
    ("ssm.amazonaws.com", "AWSServiceRoleForAmazonSSM"),
    ("sso.amazonaws.com", "AWSServiceRoleForSSO"),
    ("support.amazonaws.com", "AWSServiceRoleForSupport"),
    ("trustedadvisor.amazonaws.com", "AWSServiceRoleForTrustedAdvisor"),
];

/// AWS's own role name for `service_principal`, when it publishes one.
///
/// Matched without regard to ASCII case: a service principal is a DNS-style name
/// (`CreateServiceLinkedRole$AWSServiceName` — "a string similar to a URL but
/// without the http:// in front"), so a differently-cased spelling addresses the
/// same service, and IAM holds the resulting role name case-folded anyway
/// ([`name_key`]).
pub fn published_service_linked_role_name(service_principal: &str) -> Option<&'static str> {
    PUBLISHED_SERVICE_LINKED_ROLE_NAMES
        .iter()
        .find(|(principal, _)| principal.eq_ignore_ascii_case(service_principal))
        .map(|(_, role_name)| *role_name)
}

/// The catalog row whose PUBLISHED role name is `role_name` — its POSITION in
/// [`PUBLISHED_SERVICE_LINKED_ROLE_NAMES`], the service principal the role is linked
/// to, and the name AWS publishes for it — or `None` when no service publishes that
/// name.
///
/// The inverse of [`published_service_linked_role_name`], matched the same way IAM
/// resolves any entity name: "names are not distinguished by case"
/// (`CreateRole$RoleName`), so a request spelling the name in any case addresses the
/// same role, and the row carries the canonical spelling a read reports. The position
/// comes back with it because it is what identifies the row to
/// [`service_provisioned_role_id`].
pub fn published_service_linked_role(
    role_name: &str,
) -> Option<(usize, &'static str, &'static str)> {
    PUBLISHED_SERVICE_LINKED_ROLE_NAMES
        .iter()
        .enumerate()
        .find(|(_, (_, published))| published.eq_ignore_ascii_case(role_name))
        .map(|(row, (principal, published))| (row, *principal, *published))
}

/// The path a service-linked role for `service_principal` is filed under:
/// [`SERVICE_LINKED_ROLE_PATH_PREFIX`] — the path AWS reserves for service-linked
/// roles, since a role's path is what tells every operation the role is
/// service-linked — followed by the principal and a closing `/`, which is
/// `pathType`'s required form (`^(\u002F)|(\u002F[\u0021-\u007E]+\u002F)$`).
pub fn service_linked_role_path(service_principal: &str) -> String {
    format!("{SERVICE_LINKED_ROLE_PATH_PREFIX}{service_principal}/")
}

/// The trust policy of a service-linked role: the one statement that lets the
/// service principal the role is linked to assume it.
///
/// Computed from the principal, so the document a read returns says which service
/// controls the role rather than being a fixed blob.
pub fn service_linked_role_trust_policy(service_principal: &str) -> String {
    ::serde_json::json!({
        "Version": POLICY_LANGUAGE_VERSION,
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": service_principal },
            "Action": ["sts:AssumeRole"],
        }],
    })
    .to_string()
}

/// The `CreateDate` a service-provisioned role reports: the epoch.
///
/// Nothing in this account created the role — the service that owns it did, before
/// any request this emulator served — so there is no creation instant to report, and
/// a derived record that read the clock would answer a different `CreateDate` to
/// every read. A FIXED instant keeps the record stable across reads, which is what a
/// client polling for the role depends on, and the epoch is the value this
/// repository already gives a derived record's timestamps: the xray crate's
/// `default_sampling_rule` reports `CreatedAt`/`ModifiedAt` as the epoch "because
/// nothing created it and nothing has modified it".
const SERVICE_PROVISIONED_ROLE_CREATE_MILLIS: i64 = 0;

/// The service-linked role the OWNING SERVICE keeps provisioned in the account under
/// the name AWS publishes for it, or `None` when `role_name` is not one of those
/// names.
///
/// A service-linked role "is a unique type of IAM role that is linked directly to an
/// Amazon Web Services service … predefined by the service and includes all the
/// permissions that the service requires"; the service, not the account, controls it
/// ("to attach a policy to this service-linked role, you must make the request using
/// the Amazon Web Services service that depends on this role",
/// `CreateServiceLinkedRole`), and "only the service that depends on the
/// service-linked role can modify or delete the role on your behalf"
/// (`UnmodifiableEntityException`). For every service this emulator serves, that role
/// is therefore already provisioned: the service provisions it on the account's
/// behalf when the account uses the service, and no caller has to ask for it — which
/// is why a client that waits for IAM to report one (the documented pattern before
/// using a dependent resource) converges instead of spinning on `NoSuchEntity`.
///
/// DERIVED, never inserted into the store, exactly as this crate keeps AWS-managed
/// policies out of [`ComAmazonawsIamAwsidentitymanagementv20100508State::policies`]:
/// the record belongs to AWS rather than to the account, so the account's own
/// collections stay the record of what the ACCOUNT holds, and no read writes
/// anything. Every member is a pure function of the catalog row — the path and trust
/// policy of the principal, the ARN of that path and name, an id keyed to the name
/// ([`service_provisioned_role_id`]) and a fixed `CreateDate`
/// ([`SERVICE_PROVISIONED_ROLE_CREATE_MILLIS`]) — so two reads answer the same
/// record. A record the account creates for that name (`CreateServiceLinkedRole`)
/// takes precedence: a reader looks in the store first.
///
/// WHAT RESOLVES THROUGH THIS, and what deliberately does not:
///
/// * `GetRole` — the role REPORT — answers the account's own record where there is
///   one and this derived record otherwise.
/// * `CreateServiceLinkedRole` still MINTS the account's own record for such a
///   principal and answers 200 rather than refusing the name as taken. That is
///   deliberate: the role a service predefines is not a record of the account's, and
///   every client that follows a service's documented bootstrap (create the role,
///   then read it back under the published name) depends on that create succeeding.
///   The created record then supersedes this one for every later read.
/// * The operations that CHANGE a role or report the ACCOUNT's own record of its
///   attachments (`DeleteRole`, `UpdateRole`, `UpdateAssumeRolePolicy`,
///   `UpdateRoleDescription`, `Put`/`Get`/`DeleteRolePolicy`,
///   `Attach`/`DetachRolePolicy`, `Tag`/`UntagRole`, the `ListRole*` listings and
///   `ListRoles`) keep answering from the store alone, so a name only this function
///   resolves is still `NoSuchEntity` there. None of them could legally change a
///   service-linked role ("only the service that depends on the service-linked role
///   can modify or delete the role on your behalf"), and reporting the policies a
///   service attaches to its OWN role would mean inventing a catalog no model in
///   this repository carries.
pub fn service_provisioned_role(role_name: &str) -> Option<Role> {
    let (row, service_principal, published_name) = published_service_linked_role(role_name)?;
    let path = service_linked_role_path(service_principal);
    Some(Role {
        arn: role_arn(&path, published_name),
        role_id: service_provisioned_role_id(row),
        create_date: ::aws_smithy_types::DateTime::from_millis(
            SERVICE_PROVISIONED_ROLE_CREATE_MILLIS,
        ),
        // The service that the role is linked to is the one that can assume it.
        assume_role_policy_document: Some(service_linked_role_trust_policy(service_principal)),
        // A role the service predefined carries no description of the account's, no
        // permissions boundary and no tags of its own, and nothing in this account
        // has assumed it, so there is no last-used activity to report.
        description: None,
        max_session_duration: Some(DEFAULT_MAX_SESSION_DURATION_SECONDS),
        permissions_boundary: None,
        tags: None,
        role_last_used: None,
        // Only a role created from a role template carries one
        // (`smithy_query(query="shape", shape="Role")`), which this is not.
        source_role_template: None,
        role_name: published_name.to_string(),
        path,
    })
}

/// The `RoleId` of the service-provisioned role at position `row` of
/// [`PUBLISHED_SERVICE_LINKED_ROLE_NAMES`]: an id minted by [`unique_id`], this
/// module's one id minter, from the ROW rather than from a counter.
///
/// Keying it to the row is what makes it stable — the same name resolves the same row
/// and so reads the same id, call after call — and distinct per role, since
/// [`unique_id`] is injective in the position it is given. The position is offset by
/// [`SERVICE_PROVISIONED_ROLE_ID_BASE`], past every value
/// [`ComAmazonawsIamAwsidentitymanagementv20100508State::role_id_seq`] can reach, so a
/// provisioned role's id can never collide with one minted for a created role.
fn service_provisioned_role_id(row: usize) -> String {
    unique_id(ROLE_ID_PREFIX, SERVICE_PROVISIONED_ROLE_ID_BASE + row as u64)
}

/// Where the id positions of the service-provisioned roles start: the top half of the
/// `u64` range.
///
/// [`ComAmazonawsIamAwsidentitymanagementv20100508State::role_id_seq`] starts at zero
/// and is bumped once per created role, so it cannot reach this base, and the
/// catalog's length cannot carry a row past the range — which is what makes the two
/// id spaces disjoint by construction.
const SERVICE_PROVISIONED_ROLE_ID_BASE: u64 = 1 << 63;

/// The marker scope of the attachment set of the role keyed by `role_key`.
pub fn role_attached_policies_marker_scope(role_key: &str) -> String {
    format!("{MARKER_SCOPE_ROLE_ATTACHED_POLICIES}:{role_key}")
}

/// The marker scope of the inline-policy set of the role keyed by `role_key`.
pub fn role_policies_marker_scope(role_key: &str) -> String {
    format!("{MARKER_SCOPE_ROLE_POLICIES}:{role_key}")
}

/// The marker scope of the entity collection of the managed policy `policy_arn`
/// names. See [`MARKER_SCOPE_ENTITIES_FOR_POLICY`].
pub fn entities_for_policy_marker_scope(policy_arn: &str) -> String {
    format!("{MARKER_SCOPE_ENTITIES_FOR_POLICY}:{policy_arn}")
}

/// The page key of ONE entity in the order `ListEntitiesForPolicy` enumerates the
/// entities a managed policy relates to: the entity's TYPE tag followed by
/// [`name_key`] of its name.
///
/// The byte order of these keys IS the enumeration order, which is what lets the
/// operation resume strictly after the key a `Marker` names even when the entity it
/// named has since been deleted. The three tags ([`ENTITY_KEY_TAG_GROUP`],
/// [`ENTITY_KEY_TAG_ROLE`], [`ENTITY_KEY_TAG_USER`]) sort in that order and each is
/// followed by the same separator, so the keys group by entity type and then run
/// ascending by name within a type — the order each store is already keyed in.
pub fn entity_page_key(type_tag: &str, name_key: &str) -> String {
    format!("{type_tag}:{name_key}")
}

/// The [`entity_page_key`] type tag of an IAM group.
pub const ENTITY_KEY_TAG_GROUP: &str = "group";

/// The [`entity_page_key`] type tag of an IAM role.
pub const ENTITY_KEY_TAG_ROLE: &str = "role";

/// The [`entity_page_key`] type tag of an IAM user.
pub const ENTITY_KEY_TAG_USER: &str = "user";

/// The managed-policy ARN the entity's permissions boundary names, if it carries
/// one.
///
/// A permissions boundary is "the ARN of the managed policy that is used to set the
/// permissions boundary" (`AttachedPermissionsBoundary$PermissionsBoundaryArn`),
/// which is what makes the entity one the boundary policy relates to.
pub fn permissions_boundary_arn(
    boundary: Option<&AttachedPermissionsBoundary>,
) -> Option<&str> {
    boundary?.permissions_boundary_arn.as_deref()
}

/// The parts of an IAM managed-policy ARN that an operation taking one reads.
#[derive(Debug, Clone)]
pub struct PolicyArn {
    /// The ARN's account segment: an account id, or
    /// [`AWS_MANAGED_POLICY_ACCOUNT`] for an AWS-managed policy.
    pub account: String,
    /// The policy's path, beginning and ending with `/` — `/` for a policy at the
    /// root, `/service-role/` for one AWS files under that path.
    pub path: String,
    /// The policy's friendly name: the last segment of the ARN's resource.
    pub policy_name: String,
}

/// The parts of `arn` when it is an IAM managed-policy ARN, or `None` when it is
/// not one — which is what makes an operation taking a `PolicyArn` refuse it as an
/// "invalid … value … supplied for an input parameter".
///
/// A managed policy's ARN is `arn:<partition>:iam::<account>:policy/<path><name>`:
/// IAM is a global service, so the region segment is EMPTY, and the resource is
/// the `policy` type followed by the policy's path and friendly name (the form
/// `arnType`'s documentation points at Amazon Resource Names in the AWS General
/// Reference for). Path segments are non-empty, since `policyPathType`'s pattern
/// (`^((/[A-Za-z0-9\.,\+@=_-]+)*)/$`) admits no empty one.
pub fn parse_policy_arn(arn: &str) -> Option<PolicyArn> {
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    if fields.next()?.is_empty() {
        return None;
    }
    if fields.next()? != "iam" {
        return None;
    }
    if !fields.next()?.is_empty() {
        return None;
    }
    let account = fields.next()?;
    if account.is_empty() {
        return None;
    }
    let resource = fields.next()?.strip_prefix("policy/")?;
    // Everything before the LAST `/` is the policy's path, which is absent for a
    // policy at the root. An empty path segment (`policy//Name`) is not a path the
    // `policyPathType` pattern admits, so such an ARN names no policy.
    let (path, policy_name) = match resource.rfind('/') {
        Some(last) => {
            let path_body = &resource[..last];
            if path_body.split('/').any(str::is_empty) {
                return None;
            }
            (format!("/{path_body}/"), &resource[last + 1..])
        }
        None => (DEFAULT_PATH.to_string(), resource),
    };
    if policy_name.is_empty() {
        return None;
    }
    Some(PolicyArn {
        account: account.to_string(),
        path,
        policy_name: policy_name.to_string(),
    })
}

/// Encode a state map key as the opaque pagination `Marker` a client echoes back.
///
/// The codec is `aws_smithy_types::base64` — already in this crate's dependency
/// tree, since it is what the wire protocols use for blob members — so the token
/// needs no dependency of its own. Base64's alphabet is inside `markerType`'s
/// `^[\u0020-\u00FF]+$` pattern and its 1..320 length for any key the entity-name
/// constraints admit, and the token carries no structure a client could depend on
/// beyond being echo-able.
pub fn encode_marker(scope: &str, key: &str) -> String {
    ::aws_smithy_types::base64::encode(format!("{MARKER_PREFIX}{scope}:{key}"))
}

/// The state map key an opaque `Marker` names within `scope`, or `None` when the
/// token was not produced by [`encode_marker`] for that scope — which the prefix
/// and the scope are what establish, since a foreign token that happens to be
/// valid base64, or one this service issued while paging another collection, does
/// not carry them.
pub fn decode_marker(scope: &str, marker: &str) -> Option<String> {
    let decoded = String::from_utf8(::aws_smithy_types::base64::decode(marker).ok()?).ok()?;
    decoded
        .strip_prefix(&format!("{MARKER_PREFIX}{scope}:"))
        .map(str::to_string)
}

/// One stored role as a resource-LISTING operation reports it: the attributes
/// `ListRoles` documents it withholds are dropped, every other one is reported as
/// the account holds it.
///
/// `ListRoles`' documentation: "IAM resource-listing operations return a subset of
/// the available attributes for the resource. This operation does not return the
/// following attributes, even though they are an attribute of the returned object:
/// PermissionsBoundary RoleLastUsed Tags. To view all of the information for a
/// role, see GetRole."
pub fn listed_role(role: &Role) -> Role {
    Role {
        permissions_boundary: None,
        role_last_used: None,
        tags: None,
        ..role.clone()
    }
}

/// One stored user as a resource-LISTING operation reports it, for the same reason
/// [`listed_role`] exists.
///
/// `ListUsers`' documentation: "IAM resource-listing operations return a subset of
/// the available attributes for the resource. This operation does not return the
/// following attributes, even though they are an attribute of the returned object:
/// PermissionsBoundary Tags. To view all of the information for a user, see
/// GetUser." `PasswordLastUsed` is NOT among them, so it stays.
pub fn listed_user(user: &User) -> User {
    User {
        permissions_boundary: None,
        tags: None,
        ..user.clone()
    }
}

/// A unique entity id: `prefix` followed by [`UNIQUE_ID_SUFFIX_LEN`] base32
/// characters that spell out `seq` mixed through SplitMix64.
///
/// The mix is a bijection on `u64` and the first 13 suffix characters carry all
/// 64 of its bits, so distinct `seq` values always yield distinct ids. The
/// remaining characters come from a second, independently mixed word purely so
/// the id has the shape and entropy of an AWS unique id rather than a visible
/// counter.
fn unique_id(prefix: &str, seq: u64) -> String {
    let mut id = String::with_capacity(prefix.len() + UNIQUE_ID_SUFFIX_LEN);
    id.push_str(prefix);
    let mut word = splitmix64(seq);
    for i in 0..UNIQUE_ID_SUFFIX_LEN {
        // 13 base32 characters cover all 64 bits of the first word; after that,
        // continue from a second word keyed to the same counter.
        if i == 13 {
            word = splitmix64(seq ^ 0x9E37_79B9_7F4A_7C15);
        }
        id.push(BASE32_ALPHABET[(word & 31) as usize] as char);
        word >>= 5;
    }
    id
}

/// SplitMix64's finalizer: a bijection on `u64` that scatters consecutive inputs.
fn splitmix64(seq: u64) -> u64 {
    let mut z = seq.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIamAwsidentitymanagementv20100508State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIamAwsidentitymanagementv20100508State>)
}

/// The reserved tag key among `keys`, if any — the lowest one, so the refusal names
/// the same key however the request's tags were ordered.
/// See [`RESERVED_TAG_KEY_PREFIX`].
pub fn reserved_tag_key<'a, I: IntoIterator<Item = &'a str>>(keys: I) -> Option<&'a str> {
    let prefix = RESERVED_TAG_KEY_PREFIX.as_bytes();
    keys.into_iter()
        .filter(|key| {
            let key = key.as_bytes();
            key.len() >= prefix.len() && key[..prefix.len()].eq_ignore_ascii_case(prefix)
        })
        .min()
}

/// The tag set an entity holding `held` carries after a tag call supplying
/// `incoming`, or `None` when the merged set would pass
/// [`MAX_TAGS_PER_ENTITY`].
///
/// The rule every IAM tag operation follows, in one place: "if a tag with the same
/// key name already exists, then that tag is overwritten with the new value"
/// (`TagRole`), so a supplied key REPLACES the held value in place — the held
/// order is preserved and only a key the entity does not hold yet is appended.
/// Keys compare exactly: "tag keys and values are case sensitive" ("Tagging IAM
/// resources", IAM User Guide), so two spellings of one word are two tags. A key
/// repeated inside one request settles on its LAST value, the same way a second
/// call would overwrite the first.
///
/// The merged set is what the bound is taken against, so re-tagging a held key is
/// never refused however many tags the entity already carries.
pub fn merge_tags(held: Option<&Vec<Tag>>, incoming: Vec<Tag>) -> Option<Vec<Tag>> {
    let mut merged: Vec<Tag> = held.cloned().unwrap_or_default();
    for tag in incoming {
        match merged.iter_mut().find(|existing| existing.key == tag.key) {
            Some(existing) => existing.value = tag.value,
            None => merged.push(tag),
        }
    }
    (merged.len() <= MAX_TAGS_PER_ENTITY).then_some(merged)
}

/// Whether `document` is an IAM policy document.
///
/// An IAM policy document is a JSON object whose top-level elements are `Version`
/// and `Id` (both optional) and `Statement` (required) — the IAM JSON policy
/// grammar. So a document that does not parse, is not an object, or carries no
/// `Statement` is not a policy, and the operation taking it is refused with the
/// modelled `MalformedPolicyDocumentException`.
///
/// Validation deliberately stops at that top level: the grammar BELOW `Statement`
/// is not checked here, so a 200 from an operation taking a policy document is the
/// claim "stored", not "every statement is valid IAM". Deepening it is its own
/// change. This is the ONE definition of the rule, shared by every operation that
/// accepts a policy document (`PutUserPolicy`'s inline policy, `CreateRole`'s trust
/// policy).
pub fn is_policy_document(document: &str) -> bool {
    ::serde_json::from_str::<::serde_json::Value>(document).is_ok_and(|document| {
        document
            .as_object()
            .is_some_and(|document| document.contains_key("Statement"))
    })
}

/// The size a managed policy document is measured at against
/// [`MAX_MANAGED_POLICY_CHARACTERS`]: its characters except whitespace, because
/// AWS "doesn't count whitespace when calculating the size of a policy against
/// this limitation".
pub fn managed_policy_characters(document: &str) -> usize {
    document.chars().filter(|c| !c.is_whitespace()).count()
}

