// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iam-awsidentitymanagementv20100508 service.
//!
//! This module owns the persisted IAM entities plus the small helpers every
//! handler needs (name keying, ARN building, id minting, policy-document checks,
//! opaque pagination markers). Handlers reach it through [`service_state`]; they
//! never keep a second copy of a resource.
//!
//! [`authorization`] is the sub-module that answers who a signed request speaks for
//! and whether that principal's identity-based policies let it do what it asks.

use std::collections::{BTreeMap, BTreeSet};

use crate::ops::types::{
    AttachedPermissionsBoundary, DelegationRequest, Group, InstanceProfile, LoginProfile, Policy,
    PolicyVersion, Role, SAMLPrivateKey, ServerCertificateMetadata,
    ServiceSpecificCredentialMetadata, SigningCertificate, Tag, User,
};

pub mod authorization;

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

/// The `EntityType` value that names an IAM group. See [`ENTITY_FILTER_ROLE`].
pub const ENTITY_FILTER_GROUP: &str = "Group";

/// The `EntityType` value that names a managed policy the ACCOUNT owns — "use the
/// value LocalManagedPolicy to include customer managed policies"
/// (`GetAccountAuthorizationDetails$Filter`). See [`ENTITY_FILTER_ROLE`].
pub const ENTITY_FILTER_LOCAL_MANAGED_POLICY: &str = "LocalManagedPolicy";

/// The `EntityType` value that names a managed policy AWS owns, the counterpart of
/// [`ENTITY_FILTER_LOCAL_MANAGED_POLICY`]. See [`ENTITY_FILTER_ROLE`].
pub const ENTITY_FILTER_AWS_MANAGED_POLICY: &str = "AWSManagedPolicy";

/// The `PolicyUsageType` value that names a policy used as a permissions POLICY:
/// "to list only permissions policies, set PolicyUsageFilter to PermissionsPolicy"
/// (`ListEntitiesForPolicy$PolicyUsageFilter`). One of the two values the enum
/// declares, which `ops::types::POLICY_USAGE_TYPE_VALUES` records.
pub const POLICY_USAGE_PERMISSIONS_POLICY: &str = "PermissionsPolicy";

/// The `PolicyUsageType` value that names a policy used to set a permissions
/// BOUNDARY: "to list only the policies used to set permissions boundaries, set the
/// value to PermissionsBoundary". See [`POLICY_USAGE_PERMISSIONS_POLICY`].
pub const POLICY_USAGE_PERMISSIONS_BOUNDARY: &str = "PermissionsBoundary";

/// The ARN resource type of a delegation request: the entity type
/// `GetHumanReadableSummaryRequest$EntityArn` documents a summary is served for
/// ("at this time, the only supported entity type is delegation-request"), and the
/// resource type an ARN naming such a request therefore carries.
pub const DELEGATION_REQUEST_RESOURCE_TYPE: &str = "delegation-request";

/// The `policyScopeType` value that selects only the managed policies AWS owns: "to
/// list only Amazon Web Services managed policies, set Scope to AWS"
/// (`ListPolicies$Scope`). One of the three values the enum declares, which
/// `ops::types::POLICY_SCOPE_TYPE_VALUES` records.
pub const POLICY_SCOPE_AWS: &str = "AWS";

/// The `policyScopeType` value that selects only the managed policies the ACCOUNT
/// owns: "to list only the customer managed policies in your Amazon Web Services
/// account, set Scope to Local". See [`POLICY_SCOPE_AWS`]; the third value (`All`)
/// selects both, which is also what an absent `Scope` means.
pub const POLICY_SCOPE_LOCAL: &str = "Local";

/// The prefix AWS gives an IAM user's unique id. `User$UserId` is modelled as
/// `idType` (len 16..128, pattern `^[\w]+$`), which this prefix plus
/// [`UNIQUE_ID_SUFFIX_LEN`] base32 characters satisfies.
const USER_ID_PREFIX: &str = "AIDA";

/// The prefix AWS gives an IAM role's unique id. `Role$RoleId` is modelled as
/// `idType` (len 16..128, pattern `^[\w]+$`), which this prefix plus
/// [`UNIQUE_ID_SUFFIX_LEN`] base32 characters satisfies.
const ROLE_ID_PREFIX: &str = "AROA";

/// The prefix AWS gives an IAM group's unique id. `Group$GroupId` is modelled as
/// `idType` (len 16..128, pattern `^[\w]+$`), which this prefix plus
/// [`UNIQUE_ID_SUFFIX_LEN`] base32 characters satisfies.
const GROUP_ID_PREFIX: &str = "AGPA";

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
/// ([`policy_document_characters`]) skips whitespace too.
pub const MAX_MANAGED_POLICY_CHARACTERS: usize = 6_144;

/// How many versions one managed policy may carry. `CreatePolicyVersion`'s
/// documentation: "A managed policy can have up to five versions. If the policy has
/// five versions, you must delete an existing version using DeletePolicyVersion
/// before you create a new version." A create past it is refused with
/// `LimitExceededException`, whose documentation says "the error message describes
/// the limit exceeded", and it is what stops one policy's version map growing
/// without end.
pub const MAX_POLICY_VERSIONS: usize = 5;

/// How many IAM users one account may hold. AWS's "IAM and STS quotas" page gives
/// the quota "Users in an account: 5,000", and a create past it is refused with
/// `LimitExceededException`, whose documentation says "the error message describes
/// the limit exceeded".
pub const MAX_USERS_PER_ACCOUNT: usize = 5_000;

/// How many IAM groups one account may hold. AWS's "IAM and STS quotas" page gives
/// the quota "IAM groups in an account: 300", and a create past it is refused with
/// `LimitExceededException`, whose documentation says "the error message describes
/// the limit exceeded". It is also what stops the group map growing without end.
pub const MAX_GROUPS_PER_ACCOUNT: usize = 300;

/// How many IAM groups one IAM user may belong to. AWS's "IAM and STS quotas" page
/// gives the quota "IAM groups that an IAM user can be a member of: 10", and an add
/// past it is refused with `LimitExceededException`, whose documentation says "the
/// error message describes the limit exceeded" — here, the maximum number of groups
/// per user, for the user the add names. It is also what bounds the memberships one
/// user can accumulate in [`group_members`].
///
/// [`group_members`]: ComAmazonawsIamAwsidentitymanagementv20100508State::group_members
pub const MAX_GROUPS_PER_USER: usize = 10;

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

/// How many X.509 signing certificates one IAM user may hold. AWS's "IAM and STS
/// quotas" page gives the quota "Signing certificates per IAM user: 2", and an
/// upload past it is refused with `LimitExceededException`, whose documentation says
/// "the error message describes the limit exceeded".
pub const MAX_SIGNING_CERTIFICATES_PER_USER: usize = 2;

/// The aggregate size, in non-whitespace characters, of all inline policies
/// embedded in one IAM user. AWS's "IAM and STS quotas" page gives the quota "User
/// policy size cannot exceed 2,048 characters" and states that IAM "doesn't count
/// whitespace when calculating the size of a policy against this limitation", so
/// the measure ([`policy_document_characters`]) skips whitespace; a put that would
/// carry the user past the quota is refused with `LimitExceededException`.
pub const MAX_USER_INLINE_POLICY_CHARACTERS: usize = 2_048;

/// The aggregate size, in non-whitespace characters, of all inline policies
/// embedded in one IAM role. AWS's "IAM and STS quotas" page gives the quota "Role
/// policy size cannot exceed 10,240 characters", measured the same way the other
/// policy-size quotas are ([`policy_document_characters`]); a put that would carry
/// the role past it is refused with `LimitExceededException`.
pub const MAX_ROLE_INLINE_POLICY_CHARACTERS: usize = 10_240;

/// The aggregate size, in non-whitespace characters, of all inline policies
/// embedded in one IAM group. AWS's "IAM and STS quotas" page gives the quota
/// "Group policy size cannot exceed 5,120 characters", measured the same way the
/// other policy-size quotas are ([`policy_document_characters`]); a put that would
/// carry the group past it is refused with `LimitExceededException`.
pub const MAX_GROUP_INLINE_POLICY_CHARACTERS: usize = 5_120;

/// The aggregate RAW size, in characters, of the inline-policy documents this
/// emulator retains for one entity.
///
/// The quotas above are measured with whitespace left out, so they do not bound the
/// TEXT an entity's inline policies carry: a document that fits its quota may be
/// formatted with any amount of whitespace, and every document is held verbatim so
/// a read reports it exactly as it was embedded. This is the ceiling on that text,
/// set to `policyDocumentType`'s own published maximum (`@length 1..131072`,
/// `smithy_query(query="shape", shape="policyDocumentType")`) — the longest single
/// document the service accepts at all — so one entity never retains more policy
/// text than one document may carry, and a put that would carry it past this
/// ceiling is refused with `LimitExceededException` instead of stored.
pub const MAX_INLINE_POLICY_TEXT_CHARACTERS: usize = 131_072;

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

/// The instant every entry of the AWS-managed policy catalog
/// ([`AWS_MANAGED_POLICIES`]) reports as its `CreateDate` and `UpdateDate`:
/// 2015-02-06T18:39:46Z in seconds since the epoch, the creation time the AWS
/// Managed Policy Reference publishes for the policies AWS created when managed
/// policies launched (`AdministratorAccess` among them).
///
/// AWS publishes a creation instant per managed policy; this emulator carries ONE
/// for the catalog as a whole, so a catalog entry's dates are of the modelled type
/// (`dateType`) and stable across reads — the same value for every reader and every
/// run — but they are the catalog's instant rather than that one policy's. Nothing
/// derives them from a request, and no read moves them.
const AWS_MANAGED_POLICY_CREATE_DATE_SECS: i64 = 1_423_247_986;

/// How many managed policies one IAM role may carry. AWS's "IAM and STS quotas"
/// page gives the quota "Managed policies attached to an IAM role: 10", and an
/// attach past it is refused with `LimitExceededException`, whose documentation
/// says "the error message describes the limit exceeded". It is also what stops a
/// role's attachment set growing without end.
pub const MAX_MANAGED_POLICIES_PER_ROLE: usize = 10;

/// How many managed policies one IAM user may carry. AWS's "IAM and STS quotas"
/// page gives the quota "Managed policies attached to an IAM user: 10", and an
/// attach past it is refused with `LimitExceededException`, exactly as a role's
/// quota ([`MAX_MANAGED_POLICIES_PER_ROLE`]) is. It is also what stops a user's
/// attachment set growing without end.
pub const MAX_MANAGED_POLICIES_PER_USER: usize = 10;

/// How many managed policies one IAM group may carry. AWS's "IAM and STS quotas"
/// page gives the quota "Managed policies attached to an IAM group: 10", and an
/// attach past it is refused with `LimitExceededException`, exactly as a role's
/// quota ([`MAX_MANAGED_POLICIES_PER_ROLE`]) is. It is also what stops a group's
/// attachment set growing without end.
pub const MAX_MANAGED_POLICIES_PER_GROUP: usize = 10;

/// The scheme an OIDC provider's URL must carry.
/// `CreateOpenIDConnectProvider$Url`'s documentation: "The URL must begin with
/// https://".
pub const OPEN_ID_CONNECT_URL_SCHEME: &str = "https://";

/// How many client IDs one IAM OIDC provider may carry.
/// `CreateOpenIDConnectProvider$ClientIDList`'s documentation: "You cannot register
/// more than 100 client IDs with a single IAM OIDC provider."
pub const MAX_CLIENT_IDS_PER_OPEN_ID_CONNECT_PROVIDER: usize = 100;

/// How many certificate thumbprints one IAM OIDC provider may carry.
/// `CreateOpenIDConnectProvider$ThumbprintList`'s documentation: "IAM lets you have
/// up to five thumbprints for an OIDC provider."
pub const MAX_THUMBPRINTS_PER_OPEN_ID_CONNECT_PROVIDER: usize = 5;

/// How many OIDC providers one account may hold. AWS's "IAM and STS quotas" page
/// gives the quota "OIDC identity providers in an account: 100", and a create past
/// it is refused with `LimitExceededException`, whose documentation says "the error
/// message describes the limit exceeded". It is also what stops the provider map
/// growing without end.
pub const MAX_OPEN_ID_CONNECT_PROVIDERS_PER_ACCOUNT: usize = 100;

/// The name that stands for EVERY OIDC provider in the account, so
/// [`open_id_connect_provider_arn`] builds the ARN of the provider collection rather than
/// of one provider. An operation that names no provider is authorized on that collection,
/// and a refusal names it, because the request is about all of them.
pub const ANY_OPEN_ID_CONNECT_PROVIDER: &str = "*";

/// How many SAML 2.0 identity providers one account may hold. AWS's "IAM and STS
/// quotas" page gives the quota "SAML providers in an account: 100", and a create past
/// it is refused with `LimitExceededException`, whose documentation says "the error
/// message describes the limit exceeded". It is also what stops the provider map
/// growing without end.
pub const MAX_SAML_PROVIDERS_PER_ACCOUNT: usize = 100;

/// How many private keys ONE SAML provider may hold: `privateKeyList` — the shape
/// `GetSAMLProvider$PrivateKeyList` reports the keys through — is modelled len 0..2
/// (`smithy_query(query="shape", shape="privateKeyList")`), so a provider holding more
/// could never be reported. It is also why "to rotate private keys, add your new private
/// key and then remove the old key in a separate request" (`UpdateSAMLProvider`): with
/// the second slot taken, the next upload is refused with `LimitExceededException`,
/// whose documentation says "the error message describes the limit exceeded".
pub const MAX_PRIVATE_KEYS_PER_SAML_PROVIDER: usize = 2;

/// How many server certificates one account may hold. AWS's "IAM and STS quotas" page
/// gives the quota "Server certificates stored in IAM per account: 20", and an upload
/// past it is refused with `LimitExceededException`, whose documentation says "the error
/// message describes the limit exceeded". It is also what stops the server-certificate
/// map growing without end.
pub const MAX_SERVER_CERTIFICATES_PER_ACCOUNT: usize = 20;

/// The XML namespace a SAML 2.0 metadata document's descriptors are declared in
/// (`urn:oasis:names:tc:SAML:2.0:metadata`, OASIS "Metadata for the OASIS Security
/// Assertion Markup Language (SAML) V2.0"). A document whose root descriptor is not in
/// it is not "an XML document generated by an identity provider (IdP) that supports
/// SAML 2.0" (`CreateSAMLProvider$SAMLMetadataDocument`).
pub const SAML_METADATA_NAMESPACE: &str = "urn:oasis:names:tc:SAML:2.0:metadata";

/// The root element of a SAML 2.0 metadata document describing ONE entity, which
/// carries that entity's `entityID` — "the issuer's name"
/// (`CreateSAMLProvider$SAMLMetadataDocument`).
const SAML_ENTITY_DESCRIPTOR: &str = "EntityDescriptor";

/// The root element of a SAML 2.0 metadata document describing a GROUP of entities;
/// the entity descriptors are nested inside it.
const SAML_ENTITIES_DESCRIPTOR: &str = "EntitiesDescriptor";

/// The metadata attribute carrying "expiration information"
/// (`CreateSAMLProvider$SAMLMetadataDocument`) — an `xs:dateTime`, which is the
/// instant `GetSAMLProvider$ValidUntil` reports for the provider.
const SAML_VALID_UNTIL_ATTRIBUTE: &str = "validUntil";

/// The metadata attribute carrying a SAML entity's issuer name.
const SAML_ENTITY_ID_ATTRIBUTE: &str = "entityID";

/// The prefix of a minted `SAMLProviderUUID`. `privateKeyIdType` (which
/// `GetSAMLProvider$SAMLProviderUUID` targets) is len 22..64 with pattern
/// `^[A-Z0-9]+$`; this prefix plus [`UNIQUE_ID_SUFFIX_LEN`] base32 characters is 22
/// characters of uppercase alphanumerics, so it satisfies both. AWS publishes no
/// prefix for this identifier, so one is chosen here that cannot collide with the
/// entity-id prefixes above.
const SAML_PROVIDER_UUID_PREFIX: &str = "SAMLP";

/// The prefix of a minted `SAMLPrivateKey$KeyId`, which targets the same
/// `privateKeyIdType` as [`SAML_PROVIDER_UUID_PREFIX`] and is built the same way, from
/// its own prefix so a key id is never a provider uuid.
const SAML_PRIVATE_KEY_ID_PREFIX: &str = "SAMLK";

/// The prefix AWS gives a server certificate's unique id — the `ASCA` prefix the "IAM
/// identifiers" page of the IAM User Guide lists for a certificate under "unique ID
/// prefixes". `ServerCertificateMetadata$ServerCertificateId` is modelled as `idType`
/// (len 16..128, pattern `^[\w]+$`), which this prefix plus [`UNIQUE_ID_SUFFIX_LEN`]
/// base32 characters satisfies.
const SERVER_CERTIFICATE_ID_PREFIX: &str = "ASCA";

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

/// How many base32 characters one mixed 64-bit word of a minted unique id spells
/// out: five bits per character, so thirteen characters carry all 64 bits.
const BASE32_CHARACTERS_PER_WORD: usize = 13;

/// How many characters a minted `CertificateId` carries. A signing certificate's id
/// has no entity-type prefix, so the whole value is base32 characters, and 32 of them
/// sit inside `certificateIdType`'s length 24..128
/// (`smithy_query(query="shape", shape="certificateIdType")`).
const SIGNING_CERTIFICATE_ID_CHARACTERS: usize = 32;

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

/// The marker scope of the account's group collection, which `ListGroups` pages
/// over. It is its OWN scope, for the same reason the role collection is: a marker
/// issued while paging groups is not read as a position among the users or roles.
pub const MARKER_SCOPE_GROUPS: &str = "groups";

/// The marker scope of the MEMBERSHIP of ONE group — the users that group holds —
/// which `GetGroup` pages over. The group's key follows it, so a marker issued
/// while paging one group's members does not name a position in another's, nor in
/// the account's group collection: see [`group_members_marker_scope`].
pub const MARKER_SCOPE_GROUP_MEMBERS: &str = "group-members";

/// The marker scope of the groups ONE user belongs to, which `ListGroupsForUser`
/// pages over. The user's key follows it, so a marker issued while paging one
/// user's memberships names no position in another user's — and it is its OWN
/// scope, the other way round from [`MARKER_SCOPE_GROUP_MEMBERS`], so a token
/// issued while paging a group's members is not read as a position among a user's
/// groups: see [`user_groups_marker_scope`].
pub const MARKER_SCOPE_USER_GROUPS: &str = "user-groups";

/// The marker scope of the account's instance profile collection, which
/// `ListInstanceProfiles` pages over. It is its OWN scope, exactly as the user and
/// role collections are, so a marker issued while paging another collection is not
/// read as a position among the instance profiles.
pub const MARKER_SCOPE_INSTANCE_PROFILES: &str = "instance-profiles";

/// The marker scope of the managed policies an account can read — the ones it holds
/// itself and the ones AWS owns ([`AWS_MANAGED_POLICIES`]) — which `ListPolicies`
/// pages over. It is its OWN scope, like the user and role collections, and ONE
/// scope covers the whole collection because that operation pages over the same key
/// order (the policy's ARN) whichever scope, path prefix, attachment state or usage
/// it filters by: the filters select within the order rather than changing it.
pub const MARKER_SCOPE_POLICIES: &str = "policies";

/// The marker scope of ONE role's attached managed policies, which
/// `ListAttachedRolePolicies` pages over. The role's key follows it, so a marker
/// issued while paging one role's attachments does not name a position in
/// another's: see [`role_attached_policies_marker_scope`].
pub const MARKER_SCOPE_ROLE_ATTACHED_POLICIES: &str = "role-attached-policies";

/// The marker scope of ONE user's attached managed policies, which
/// `ListAttachedUserPolicies` pages over. It is its OWN scope, with the user's key
/// following it, so a marker issued while paging one user's attachments names no
/// position in another user's — nor in any role's attachment order:
/// see [`user_attached_policies_marker_scope`].
pub const MARKER_SCOPE_USER_ATTACHED_POLICIES: &str = "user-attached-policies";

/// The marker scope of ONE group's attached managed policies, which
/// `ListAttachedGroupPolicies` pages over. It is its OWN scope, with the group's
/// key following it, exactly as the user's and role's attachment scopes carry
/// theirs, so a marker issued while paging one group's attachments names no
/// position in another group's — nor in any user's or role's attachment order, nor
/// among that group's EMBEDDED inline policies:
/// see [`group_attached_policies_marker_scope`].
pub const MARKER_SCOPE_GROUP_ATTACHED_POLICIES: &str = "group-attached-policies";

/// The marker scope of ONE role's EMBEDDED inline policies, which
/// `ListRolePolicies` pages over. The role's key follows it, exactly as it does
/// for the attachment set, so a marker issued while paging one role's inline
/// policies names no position in another role's — and a marker issued for a
/// role's attachments is not read as a position among its inline policies:
/// see [`role_policies_marker_scope`].
pub const MARKER_SCOPE_ROLE_POLICIES: &str = "role-policies";

/// The marker scope of ONE user's EMBEDDED inline policies, which
/// `ListUserPolicies` pages over. It is its OWN scope, with the user's key
/// following it, exactly as the role's inline-policy scope carries the role's, so a
/// marker issued while paging one user's inline policies names no position in
/// another user's — nor among that user's ATTACHED managed policies, nor among any
/// role's inline policies: see [`user_policies_marker_scope`].
pub const MARKER_SCOPE_USER_POLICIES: &str = "user-policies";

/// The marker scope of ONE group's EMBEDDED inline policies, which
/// `ListGroupPolicies` pages over. It is its OWN scope, with the group's key
/// following it, exactly as the user's and role's inline-policy scopes carry
/// theirs, so a marker issued while paging one group's inline policies names no
/// position in another group's — nor among that group's ATTACHED managed
/// policies, nor among the account's group collection:
/// see [`group_policies_marker_scope`].
pub const MARKER_SCOPE_GROUP_POLICIES: &str = "group-policies";

/// The marker scope of the entities ONE managed policy relates to, which
/// `ListEntitiesForPolicy` pages over. The policy's ARN follows it, so a marker
/// issued while paging the entities of one policy does not name a position in
/// another's: see [`entities_for_policy_marker_scope`]. One scope covers that
/// policy's whole entity collection because the operation pages over the same key
/// order whichever entity type, usage or path prefix it filters by — the filters
/// select within the order rather than changing it.
pub const MARKER_SCOPE_ENTITIES_FOR_POLICY: &str = "entities-for-policy";

/// The marker scope of the versions of ONE managed policy, which
/// `ListPolicyVersions` pages over. The policy's ARN follows it, so a marker
/// issued while paging one policy's versions does not name a position among
/// another's: see [`policy_versions_marker_scope`].
pub const MARKER_SCOPE_POLICY_VERSIONS: &str = "policy-versions";

/// The marker scope of the account's alias collection, which
/// `ListAccountAliases` pages over. It is its OWN scope, exactly as the user and
/// role collections are, so a marker issued while paging another collection is
/// not read as a position among the aliases.
pub const MARKER_SCOPE_ACCOUNT_ALIASES: &str = "account-aliases";

/// The marker scope of the account's service-specific credentials, which
/// `ListServiceSpecificCredentials` pages over. One scope covers the whole
/// collection because that operation pages over the same key order whichever user
/// or service it filters by — the filters select within the order rather than
/// changing it.
pub const MARKER_SCOPE_SERVICE_SPECIFIC_CREDENTIALS: &str = "service-specific-credentials";

/// The marker scope of the account's access keys, which `ListAccessKeys` pages
/// over. One scope covers the whole collection because that operation pages over
/// the same key order (the access key id) whichever user it reports on — the
/// user selects within the order rather than changing it.
pub const MARKER_SCOPE_ACCESS_KEYS: &str = "access-keys";

/// The marker scope of the account's signing certificates, which
/// `ListSigningCertificates` pages over. One scope covers the whole collection for
/// the same reason [`MARKER_SCOPE_ACCESS_KEYS`] does: that operation pages over the
/// same key order (the certificate id) whichever user it reports on.
pub const MARKER_SCOPE_SIGNING_CERTIFICATES: &str = "signing-certificates";

/// The marker scope of a policy simulation's evaluation results, which
/// `SimulatePrincipalPolicy` pages over. Unlike the other scopes this one names a
/// position in a list the request itself determines (its actions and resources), so
/// the key the marker carries is the INDEX of the next result rather than a store key.
pub const MARKER_SCOPE_SIMULATION_RESULTS: &str = "simulation-results";

/// The marker scope of the service namespaces ONE
/// `ListPoliciesGrantingServiceAccess` request asks about. Like
/// [`MARKER_SCOPE_SIMULATION_RESULTS`] this one names a position in a list the
/// REQUEST determines (its `ServiceNamespaces`) rather than in a store, so the key
/// the marker carries is the index of the next namespace to report on.
pub const MARKER_SCOPE_POLICIES_GRANTING_SERVICE_ACCESS: &str =
    "policies-granting-service-access";

/// The marker scope of the tag set of ONE tagged entity, which IAM's tag-listing
/// operations page over. The entity's handle follows it, so a marker issued while
/// paging one entity's tags names no position in another entity's tag set — and no
/// position in any other collection: see [`entity_tags_marker_scope`].
pub const MARKER_SCOPE_ENTITY_TAGS: &str = "entity-tags";

/// The marker scope of the account's whole authorization configuration — its users,
/// groups, roles and managed policies — which `GetAccountAuthorizationDetails` pages
/// over. One scope covers all four collections because that operation reports them
/// in a SINGLE enumeration order ([`entity_page_key`]) under one `Marker`, and the
/// entity types it is filtered by select within that order rather than changing it.
pub const MARKER_SCOPE_ACCOUNT_AUTHORIZATION_DETAILS: &str = "account-authorization-details";

/// The marker scope of the account's delegation request collection, which
/// `ListDelegationRequests` pages over. It is its OWN scope, for the same reason the
/// user and role collections are: a marker issued while paging another collection
/// names no position in this one. The key it carries is a `DelegationRequestId`, the
/// key [`ComAmazonawsIamAwsidentitymanagementv20100508State::delegation_requests`]
/// holds a request under.
pub const MARKER_SCOPE_DELEGATION_REQUESTS: &str = "delegation-requests";

/// The marker scope of the services ONE service-last-accessed report reports on, which
/// `GetServiceLastAccessedDetails` pages over. The report's own `JobId` follows it, so a
/// marker issued while paging one report's services names no position in another's: see
/// [`service_last_accessed_marker_scope`]. The key it carries is a service namespace, the
/// value that report's entries are ordered by.
pub const MARKER_SCOPE_SERVICE_LAST_ACCESSED: &str = "service-last-accessed";

/// The marker scope of the entities ONE service-last-accessed report reports for ONE
/// service, which `GetServiceLastAccessedDetailsWithEntities` pages over. Both the report's
/// `JobId` and that service's namespace follow it, so a marker issued while paging one
/// service's entities names no position in another service's: see
/// [`service_last_accessed_entities_marker_scope`]. The key it carries is an
/// [`entity_page_key`].
pub const MARKER_SCOPE_SERVICE_LAST_ACCESSED_ENTITIES: &str = "service-last-accessed-entities";

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

impl ManagedPolicy {
    /// The identifier the NEXT version of this policy carries: `v` followed by one
    /// past the greatest version number the policy holds
    /// ([`policy_version_number`]).
    ///
    /// Version identifiers are assigned in sequence — the first one a policy carries
    /// is [`FIRST_POLICY_VERSION_ID`], which is what this answers for a policy
    /// holding no version — so the number is also the creation order every reader of
    /// a policy's versions enumerates them in
    /// ([`policy_version_page_key`]). Taking it one past the GREATEST held number,
    /// rather than from the count, is what keeps a new version from reusing the
    /// identifier of one the policy still carries after an earlier version was
    /// deleted.
    pub fn next_version_id(&self) -> String {
        let highest = self
            .versions
            .keys()
            .map(|version_id| policy_version_number(version_id))
            .max()
            .unwrap_or(0);
        format!("v{}", highest + 1)
    }
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

/// One X.509 signing certificate an IAM user holds, keyed in
/// [`ComAmazonawsIamAwsidentitymanagementv20100508State::signing_certificates`] by its
/// `CertificateId` — the handle every other operation on one takes
/// (`UpdateSigningCertificate` and `DeleteSigningCertificate` name the certificate by
/// id, and `ListSigningCertificates` reports it).
///
/// The owning user's NAME is not copied in: it is read from the user the record's
/// [`user_key`](SigningCertificateRecord::user_key) names, so the name lives in
/// exactly one place and a user renamed by an update does not leave a stale copy here.
#[derive(Debug, Clone)]
pub struct SigningCertificateRecord {
    /// [`name_key`] of the IAM user that holds the certificate.
    pub user_key: String,
    /// "The contents of the signing certificate"
    /// (`SigningCertificate$CertificateBody`), verbatim as the upload carried it.
    pub certificate_body: String,
    /// The DER encoding the body decodes to: what tells one certificate from another,
    /// independently of the whitespace that framed it.
    ///
    /// Kept beside the body so recognising a repeat upload costs one comparison per held
    /// certificate instead of re-reading every held body. It is DERIVED from
    /// [`certificate_body`](Self::certificate_body) and reported by nothing: no modelled
    /// member carries it.
    pub certificate_der: Vec<u8>,
    /// The certificate's `statusType` value: [`CREDENTIAL_STATUS_ACTIVE`] when it was
    /// uploaded ("when you upload the certificate, its default status is Active",
    /// `UploadSigningCertificate`), until an update moves it.
    pub status: String,
    /// "The date when the signing certificate was uploaded"
    /// (`SigningCertificate$UploadDate`), reported unchanged by every later read.
    pub upload_date: ::aws_smithy_types::DateTime,
}

impl SigningCertificateRecord {
    /// This record as the one modelled shape every operation reports a signing
    /// certificate through, under the id it is keyed by and the user name as the
    /// account holds it.
    ///
    /// ONE place builds the shape, so the upload response and every later list report
    /// the same certificate identically.
    pub fn report(&self, certificate_id: &str, user_name: &str) -> SigningCertificate {
        SigningCertificate {
            user_name: user_name.to_string(),
            certificate_id: certificate_id.to_string(),
            certificate_body: self.certificate_body.clone(),
            status: self.status.clone(),
            upload_date: Some(self.upload_date),
        }
    }
}

/// One IAM OpenID Connect (OIDC) identity provider the account holds, keyed in
/// [`ComAmazonawsIamAwsidentitymanagementv20100508State::open_id_connect_providers`]
/// by its ARN — the handle every other operation on one takes
/// (`GetOpenIDConnectProvider`, `DeleteOpenIDConnectProvider` and
/// `AddClientIDToOpenIDConnectProvider` all name the provider by ARN, and
/// `ListOpenIDConnectProviders` reports nothing else about it).
///
/// The ARN is DERIVED from the URL ([`open_id_connect_provider_arn`]), so keying by
/// it is also what makes a provider's URL its identity: "you cannot register the same
/// provider multiple times in a single Amazon Web Services account"
/// (`CreateOpenIDConnectProvider$Url`).
///
/// No modelled shape carries a whole provider — `OpenIDConnectProviderListEntry`
/// holds only the ARN (`smithy_query(query="shape",
/// shape="OpenIDConnectProviderListEntry")`) — so the record holds the members
/// `GetOpenIDConnectProvider` reports, and that read is what defines them.
#[derive(Debug, Clone)]
pub struct OpenIdConnectProviderRecord {
    /// "The URL that the IAM OIDC provider resource object is associated with"
    /// (`GetOpenIDConnectProvider$Url`), WITHOUT its scheme: the host-and-path form
    /// the provider's ARN is built from, which is also the issuer form a client
    /// compares an OIDC token's `iss` claim against.
    pub url: String,
    /// "A list of client IDs (also known as audiences) that are associated with the
    /// specified IAM OIDC provider resource object"
    /// (`GetOpenIDConnectProvider$ClientIDList`), in the order they were registered.
    /// At most [`MAX_CLIENT_IDS_PER_OPEN_ID_CONNECT_PROVIDER`] of them.
    pub client_id_list: Vec<String>,
    /// "A list of certificate thumbprints that are associated with the specified IAM
    /// OIDC provider resource object" (`GetOpenIDConnectProvider$ThumbprintList`), in
    /// the order they were supplied. At most
    /// [`MAX_THUMBPRINTS_PER_OPEN_ID_CONNECT_PROVIDER`] of them, and empty when the
    /// create named none.
    pub thumbprint_list: Vec<String>,
    /// "The date and time when the IAM OIDC provider resource object was created in
    /// the Amazon Web Services account" (`GetOpenIDConnectProvider$CreateDate`):
    /// minted once and reported unchanged by every later read.
    pub create_date: ::aws_smithy_types::DateTime,
    /// The provider's tags, sorted by tag key, because "the returned list of tags is
    /// sorted by tag key" (`GetOpenIDConnectProvider$Tags`). `None` when the provider
    /// carries none, so the member is reported only when there is a tag to report.
    pub tags: Option<Vec<Tag>>,
}

/// One IAM SAML 2.0 identity provider the account holds, keyed in
/// [`ComAmazonawsIamAwsidentitymanagementv20100508State::saml_providers`] by its ARN —
/// the handle every other operation on one takes (`GetSAMLProvider`,
/// `UpdateSAMLProvider`, `DeleteSAMLProvider` and the tag calls all name the provider
/// by ARN).
///
/// The ARN is DERIVED from the provider's name ([`saml_provider_arn`]), so keying by it
/// is also what makes the name the provider's identity and refuses a second create for
/// a name the account already holds.
///
/// No modelled shape carries a whole provider — `SAMLProviderListEntry` holds the ARN
/// and its two instants (`smithy_query(query="shape",
/// shape="SAMLProviderListEntry")`) — so the record holds the members
/// `GetSAMLProvider` reports, and that read is what defines them.
///
/// The uploaded PRIVATE KEY MATERIAL is deliberately not among them, for the reason
/// [`LoginProfileRecord`] does not hold a password: `SAMLPrivateKey` reports only a key
/// id and the instant the key was uploaded (`smithy_query(query="shape",
/// shape="SAMLPrivateKey")`), the model marks `CreateSAMLProvider$AddPrivateKey`
/// sensitive, and no operation here decrypts an assertion, so nothing could ever read
/// the key back.
#[derive(Debug, Clone)]
pub struct SamlProviderRecord {
    /// "The unique identifier assigned to the SAML provider"
    /// (`GetSAMLProvider$SAMLProviderUUID`): minted once by
    /// [`next_saml_provider_uuid`](ComAmazonawsIamAwsidentitymanagementv20100508State::next_saml_provider_uuid)
    /// and reported unchanged by every later read.
    pub provider_uuid: String,
    /// "The XML metadata document that includes information about an identity
    /// provider" (`GetSAMLProvider$SAMLMetadataDocument`), held exactly as the request
    /// supplied it so the read hands back the same bytes.
    pub metadata_document: String,
    /// "The date and time when the SAML provider was created"
    /// (`GetSAMLProvider$CreateDate`): minted once and reported unchanged by every
    /// later read.
    pub create_date: ::aws_smithy_types::DateTime,
    /// "The expiration date and time for the SAML provider"
    /// (`GetSAMLProvider$ValidUntil`) — the `validUntil` of the metadata document
    /// ([`parse_saml_metadata`]), so it is a property of the document the request
    /// carried. `None` when the document states no expiration.
    pub valid_until: Option<::aws_smithy_types::DateTime>,
    /// "The encryption setting for the SAML provider"
    /// (`GetSAMLProvider$AssertionEncryptionMode`), one value of
    /// `assertionEncryptionModeType`. `None` when no call has set one: the model
    /// declares no default for the member, so nothing is reported until a request
    /// chooses a value.
    pub assertion_encryption_mode: Option<String>,
    /// "The private key metadata for the SAML provider"
    /// (`GetSAMLProvider$PrivateKeyList`), in the order the keys were uploaded. At most
    /// [`MAX_PRIVATE_KEYS_PER_SAML_PROVIDER`] of them, the maximum `privateKeyList`
    /// declares (len 0..2, `smithy_query(query="shape", shape="privateKeyList")`), and
    /// empty when no call uploaded one.
    pub private_keys: Vec<SAMLPrivateKey>,
    /// The provider's tags, sorted by tag key, because "the returned list of tags is
    /// sorted by tag key" (`GetSAMLProvider$Tags`). `None` when the provider carries
    /// none, so the member is reported only when there is a tag to report.
    pub tags: Option<Vec<Tag>>,
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

/// One Organizations service-last-accessed report job the account has asked for, keyed in
/// [`ComAmazonawsIamAwsidentitymanagementv20100508State::organizations_access_reports`] by
/// the `JobId` the generate answered with.
///
/// `GenerateOrganizationsAccessReport` "generates a report for service last accessed data
/// for Organizations" and answers only a job id: the report itself is what
/// `GetOrganizationsAccessReport` reports, which is why the job is a stored record rather
/// than an answer computed on the spot.
#[derive(Debug, Clone)]
pub struct OrganizationsAccessReportJob {
    /// "The path of the Organizations entity (root, OU, or account)"
    /// (`GenerateOrganizationsAccessReport$EntityPath`) the report was asked for, which
    /// the report's own entries name.
    pub entity_path: String,
    /// "The identifier of the Organizations service control policy (SCP)" the request
    /// narrowed the report to, when it named one.
    pub organizations_policy_id: Option<String>,
    /// "The date and time, in ISO 8601 date-time format, when the report job was created"
    /// (`GetOrganizationsAccessReport$JobCreationDate`).
    pub created: ::aws_smithy_types::DateTime,
    /// The instant the generation is DUE: until it, the read reports the job
    /// `IN_PROGRESS`; from it, `COMPLETED` with this instant as its
    /// `JobCompletionDate`. Stored rather than slept for, so a caller that polls
    /// straight after the generate still sees the job in flight.
    pub completes: ::aws_smithy_types::DateTime,
}

/// One service-last-accessed report job the account has asked for, keyed in
/// [`ComAmazonawsIamAwsidentitymanagementv20100508State::service_last_accessed_reports`]
/// by the `JobId` the generate answered with.
///
/// `GenerateServiceLastAccessedDetails` "generates a report that includes details about
/// when an IAM resource (user, group, role, or policy) was last used in an attempt to
/// access Amazon Web Services services" and answers only a job id: the report itself is
/// what `GetServiceLastAccessedDetails` and `GetServiceLastAccessedDetailsWithEntities`
/// report, so the job records WHICH resource and WHICH level of detail were asked for
/// rather than an answer computed on the spot.
#[derive(Debug, Clone)]
pub struct ServiceLastAccessedJob {
    /// "The ARN of the IAM resource (user, group, role, or managed policy) used to
    /// generate information about when the resource was last used in an attempt to
    /// access an Amazon Web Services service"
    /// (`GenerateServiceLastAccessedDetails$Arn`), as the request supplied it: the
    /// resource whose permissions policies the report is about.
    pub arn: String,
    /// "The level of detail that you want to generate"
    /// (`GenerateServiceLastAccessedDetails$Granularity`), reported back by the read as
    /// its `JobType`. Held RESOLVED: a request that omits the parameter asks for
    /// service data ("if you don't include this optional parameter, the operation
    /// generates service data"), which is [`USAGE_GRANULARITY_SERVICE_LEVEL`].
    pub granularity: String,
    /// "The date and time, in ISO 8601 date-time format, when the report job was
    /// created" (`GetServiceLastAccessedDetails$JobCreationDate`).
    pub created: ::aws_smithy_types::DateTime,
    /// The instant the generation is DUE, read through [`report_job_status`] exactly as
    /// an Organizations report job's is ([`OrganizationsAccessReportJob::completes`]):
    /// stored rather than slept for, so a caller polling straight after the generate
    /// still sees the job in flight.
    pub completes: ::aws_smithy_types::DateTime,
}

/// One persisted server certificate: the metadata every operation reports about it,
/// beside the PEM material a read may hand back.
///
/// The uploaded PRIVATE KEY is deliberately not a field. `ServerCertificate` — the only
/// shape any operation reports a server certificate through — carries just the metadata,
/// the certificate body and the chain
/// (`smithy_query(query="shape", shape="ServerCertificate")`), and the model marks
/// `UploadServerCertificate$PrivateKey` sensitive, so nothing can ever read the key back:
/// holding it would be keeping a secret no caller may be given. It is read once, by the
/// upload, to check that it belongs to the certificate's public key.
#[derive(Debug, Clone)]
pub struct ServerCertificateRecord {
    /// "The meta information of the uploaded server certificate without its certificate
    /// body, certificate chain, and private key"
    /// (`UploadServerCertificate$ServerCertificateMetadata`): the path, the name, the
    /// minted id, the ARN, the instant it was uploaded and the instant the certificate
    /// itself expires. Stored whole, so the upload response and every later read report
    /// one record.
    pub metadata: ServerCertificateMetadata,
    /// "The contents of the public key certificate" (`ServerCertificate$CertificateBody`),
    /// verbatim as the upload carried it.
    pub certificate_body: String,
    /// "The contents of the public key certificate chain"
    /// (`ServerCertificate$CertificateChain`), when the upload carried one.
    pub certificate_chain: Option<String>,
    /// The tags attached to the certificate, sorted by tag key, because "the returned list
    /// of tags is sorted by tag key" (`UploadServerCertificate$Tags`). `None` when the
    /// upload attached none.
    pub tags: Option<Vec<Tag>>,
}

/// The organization the account belongs to, as IAM sees it: which organization it is,
/// and which account manages it.
///
/// IAM reads this where an operation of its own is served only to an organization's
/// management account — `GenerateOrganizationsAccessReport` ("to call this operation, you
/// must be signed in using your Organizations management account credentials") — so the
/// two facts such a decision needs are held in ONE place: the organization's id, which an
/// Organizations entity path names, and the account that manages it.
#[derive(Debug, Clone)]
pub struct Organization {
    /// "The unique identifier (ID) of an organization" (`OrganizationIdType`, pattern
    /// `^o-[a-z0-9]{10,32}$`) — the value an entity path's first segment carries.
    pub organization_id: String,
    /// The account that manages the organization: the one whose credentials IAM serves
    /// the organization-wide operations to, and the account every other member is
    /// refused as.
    pub management_account_id: String,
}

/// The organization an Organizations entity path names: the `o-…` segment it begins with.
///
/// "You can build an entity path using the known structure of your organization … your
/// entity path is `o-a1b2c3d4e5/r-f6g7h8i9j0example/ou-rge0-awsabcde/123456789012`"
/// (`GenerateOrganizationsAccessReport$EntityPath`), so the organization is the first
/// segment and the root, the organizational units and the account follow it. `None` for a
/// value that carries no such segment — which the member's own pattern
/// (`^o-[0-9a-z]{10,32}\/r-…`) already refuses before a handler sees it.
pub fn organizations_entity_path_organization(entity_path: &str) -> Option<&str> {
    let organization = entity_path.split('/').next()?;
    organization
        .strip_prefix("o-")
        .is_some_and(|id| !id.is_empty())
        .then_some(organization)
}

/// Per-service state for com-amazonaws-iam-awsidentitymanagementv20100508. A handler reaches it through [`service_state`].
///
/// Every map is a `BTreeMap` so enumeration order is total and stable, which is
/// what lets a list operation hand out a resumable marker: the next page starts
/// strictly after the key the marker names.
///
/// `Default` is NOT derived: the account does not start out empty. It starts out holding
/// the IAM user this emulator's own long-term credential belongs to
/// ([`Self::default`]), which is why [`Self::empty`] — the account with nothing at all in
/// it — is a separate constructor.
#[derive(Debug)]
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
    /// The persisted IAM groups, keyed by [`name_key`] of the group name, for the
    /// same reason users and roles are: IAM entity names are unique within the
    /// account and "are not distinguished by case" (`CreateGroup$GroupName`), while
    /// `Group::group_name` keeps the case the creator chose. `CreateGroup` fills
    /// this map and `ListGroups` pages over it.
    pub groups: BTreeMap<String, Group>,
    /// The users each group holds: [`name_key`] of the group name -> [`name_key`] of
    /// every member user's name. A `BTreeSet` because a user belongs to a group at
    /// most once (adding a member it already holds is the same membership) and
    /// because the total key order is what lets `GetGroup` hand out a resumable
    /// marker over the membership.
    ///
    /// The membership is held HERE rather than on either entity, so the group and the
    /// user records stay what their own reads report and one place answers which
    /// users a group holds. `AddUserToGroup` is what fills it and
    /// `RemoveUserFromGroup` what empties it.
    pub group_members: BTreeMap<String, BTreeSet<String>>,
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
    /// Inline group policies: [`name_key`] of the group name -> [`name_key`] of the
    /// policy name -> the policy. Keyed exactly as
    /// [`user_inline_policies`](Self::user_inline_policies) is, and for the same
    /// reason: "user, group, role, and policy names must be unique within the
    /// account. Names are not distinguished by case." `PutGroupPolicy` fills this
    /// map, `GetGroupPolicy` reads one entry and `ListGroupPolicies` pages over
    /// the names.
    pub group_inline_policies: BTreeMap<String, BTreeMap<String, InlinePolicy>>,
    /// Access key id -> the key's record, including [`name_key`] of the user that
    /// owns it. `GetUser` with no `UserName` resolves the caller through this map,
    /// and `CreateAccessKey` is what populates it.
    pub access_keys: BTreeMap<String, AccessKeyRecord>,
    /// `CertificateId` -> the signing certificate's record, including [`name_key`] of
    /// the user that holds it. `UploadSigningCertificate` is what fills this map,
    /// `ListSigningCertificates` pages over it, `UpdateSigningCertificate` moves one
    /// record's status and `DeleteSigningCertificate` empties it. Keyed by the id
    /// (rather than held on the user) so enumeration order is total and stable, which
    /// is what lets the list hand out a resumable marker.
    pub signing_certificates: BTreeMap<String, SigningCertificateRecord>,
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
    /// are owned by AWS, not by the account ([`AWS_MANAGED_POLICY_ACCOUNT`]), and
    /// are served from [`AWS_MANAGED_POLICIES`]; a read addressed by ARN resolves
    /// against both ([`Self::resolve_policy`]).
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
    /// The managed policies attached to each user: [`name_key`] of the user name
    /// -> the attached policies' ARNs. Keyed and ordered exactly as
    /// [`Self::role_attached_policies`] is, and for the same reasons: a policy is
    /// attached to a user at most once, and the total ARN order is what lets a
    /// reader of the set page over it.
    pub user_attached_policies: BTreeMap<String, BTreeSet<String>>,
    /// The managed policies attached to each group: [`name_key`] of the group name
    /// -> the attached policies' ARNs. Keyed and ordered exactly as
    /// [`Self::role_attached_policies`] is, and for the same reasons: a policy is
    /// attached to a group at most once, and the total ARN order is what lets a
    /// reader of the set page over it. `AttachGroupPolicy` is what fills it, and
    /// `ListAttachedGroupPolicies` is what pages over it.
    pub group_attached_policies: BTreeMap<String, BTreeSet<String>>,
    /// The persisted IAM OpenID Connect (OIDC) identity providers, keyed by their
    /// ARN — the handle every operation on one takes, and the value
    /// [`open_id_connect_provider_arn`] derives from the provider's URL. Keying by
    /// the ARN keeps enumeration order total and stable, and makes the URL the
    /// provider's identity, which is what refuses a second create for one the
    /// account already holds.
    pub open_id_connect_providers: BTreeMap<String, OpenIdConnectProviderRecord>,
    /// The persisted IAM SAML 2.0 identity providers, keyed by their ARN — the handle
    /// every operation on one takes, and the value [`saml_provider_arn`] derives from
    /// the provider's name. Keying by the ARN keeps enumeration order total and stable,
    /// and makes the name the provider's identity, which is what refuses a second
    /// create for one the account already holds.
    pub saml_providers: BTreeMap<String, SamlProviderRecord>,
    /// The persisted server certificates, keyed by [`name_key`] of the server certificate
    /// name — the handle every operation on one takes
    /// (`GetServerCertificate$ServerCertificateName`) — and case-folded for the same
    /// reason user and role names are: IAM entity names are unique within the account and
    /// "are not distinguished by case", while
    /// `ServerCertificateMetadata::server_certificate_name` keeps the case the uploader
    /// chose. `UploadServerCertificate` is what fills this map and `GetServerCertificate`
    /// reads it.
    pub server_certificates: BTreeMap<String, ServerCertificateRecord>,
    /// The account's alias, when one has been created. An `Option` rather than a
    /// collection because an account holds AT MOST ONE: "Lists the account alias
    /// associated with the Amazon Web Services account (Note: you can have only
    /// one)" (`ListAccountAliases`). `CreateAccountAlias` is what fills it —
    /// "your AWS account can have only one alias. If you create a new alias for
    /// your AWS account, the new alias replaces the previous alias" (the AWS
    /// Sign-In User Guide page "Creating, deleting, and listing an AWS account
    /// alias", which `CreateAccountAlias`'s own documentation links to) — and
    /// `DeleteAccountAlias` is what clears it.
    ///
    /// Stored verbatim: `accountAliasType`'s pattern
    /// (`^[a-z0-9]([a-z0-9]|-(?!-)){1,61}[a-z0-9]$`) admits only lowercase, so
    /// there is no case to fold and no [`name_key`] to build.
    pub account_alias: Option<String>,
    /// The account-level properties the account has persisted, keyed by the
    /// property key itself — "keys are in the format `Namespace/PropertyName`"
    /// (`GetAccountPropertiesResponse$Properties`). `PutAccountProperties` is what
    /// fills this map and `GetAccountProperties` is what reports it.
    ///
    /// A `BTreeMap` so the store's own enumeration order is total and stable; the
    /// modelled response member is a map, so what a read reports is the SET of
    /// pairs the account holds rather than an order. Keys are stored VERBATIM: `accountPropertyKeyType`'s pattern
    /// (`^[A-Za-z][A-Za-z0-9/_-]*$`) admits both cases and the namespace part of a
    /// key is a name the caller chose, so there is no case to fold and no
    /// [`name_key`] to build.
    pub account_properties: BTreeMap<String, String>,
    /// The persisted delegation requests, keyed by their `DelegationRequestId` — the
    /// handle every operation on one takes (`GetDelegationRequest$DelegationRequestId`,
    /// "the unique identifier of the delegation request"), and the last segment of the
    /// ARN that names one ([`delegation_request_arn`]). `CreateDelegationRequest`
    /// ("creates an IAM delegation request") is what fills this map.
    ///
    /// The identifier is held VERBATIM: `delegationRequestIdType`'s pattern is
    /// `^[\w-]+$` and the identifier is minted rather than named by a caller, so there
    /// is no case to fold and no [`name_key`] to build.
    pub delegation_requests: BTreeMap<String, DelegationRequest>,
    /// The organization this account belongs to, or `None` while it belongs to none.
    ///
    /// `None` is the standalone account this emulator presents until something records
    /// otherwise: an account is put in an organization by AWS Organizations, not by IAM,
    /// so no operation of THIS service fills this field — the organization-wide operations
    /// IAM serves only consult it ([`Self::manages_organization`]) and refuse a caller it
    /// does not make the organization's management account. An `Option` rather than a
    /// collection because an account belongs to at most one organization.
    pub organization: Option<Organization>,
    /// The Organizations service-last-accessed report jobs the account has asked for,
    /// keyed by the `JobId` [`GenerateOrganizationsAccessReport`] answered with — "the job
    /// identifier … use this parameter in the GetOrganizationsAccessReport operation" —
    /// which is the handle the read takes.
    ///
    /// `GenerateOrganizationsAccessReport` is what fills this map and
    /// `GetOrganizationsAccessReport` what reports one entry.
    pub organizations_access_reports: BTreeMap<String, OrganizationsAccessReportJob>,
    /// The service-last-accessed report jobs the account has asked for, keyed by the
    /// `JobId` [`GenerateServiceLastAccessedDetails`] answered with — "the JobId that
    /// you can use in the GetServiceLastAccessedDetails or
    /// GetServiceLastAccessedDetailsWithEntities operations" — which is the handle both
    /// reads take.
    ///
    /// `GenerateServiceLastAccessedDetails` is what fills this map;
    /// `GetServiceLastAccessedDetails` and `GetServiceLastAccessedDetailsWithEntities`
    /// are what report one entry. Held apart from
    /// [`Self::organizations_access_reports`] because the two are different reports of
    /// different resources, read back by different operations: a job id minted for one
    /// names no job of the other.
    pub service_last_accessed_reports: BTreeMap<String, ServiceLastAccessedJob>,
    /// Monotonic counter feeding every report job's `JobId`
    /// ([`next_access_report_job_id`](Self::next_access_report_job_id) and
    /// [`next_service_last_accessed_job_id`](Self::next_service_last_accessed_job_id)).
    ///
    /// ONE counter for both kinds of job, so no two jobs the account ever records can
    /// share an id whichever generate minted them.
    pub report_job_seq: u64,
    /// Monotonic counter feeding
    /// [`next_saml_provider_uuid`](Self::next_saml_provider_uuid).
    pub saml_provider_uuid_seq: u64,
    /// Monotonic counter feeding
    /// [`next_saml_private_key_id`](Self::next_saml_private_key_id).
    pub saml_private_key_id_seq: u64,
    /// Monotonic counter feeding [`next_user_id`](Self::next_user_id).
    pub user_id_seq: u64,
    /// Monotonic counter feeding [`next_role_id`](Self::next_role_id).
    pub role_id_seq: u64,
    /// Monotonic counter feeding [`next_group_id`](Self::next_group_id).
    pub group_id_seq: u64,
    /// Monotonic counter feeding [`next_policy_id`](Self::next_policy_id).
    pub policy_id_seq: u64,
    /// Monotonic counter feeding
    /// [`next_instance_profile_id`](Self::next_instance_profile_id).
    pub instance_profile_id_seq: u64,
    /// Monotonic counter feeding
    /// [`next_server_certificate_id`](Self::next_server_certificate_id).
    pub server_certificate_id_seq: u64,
    /// Monotonic counter feeding
    /// [`next_signing_certificate_id`](Self::next_signing_certificate_id).
    pub signing_certificate_id_seq: u64,
}

impl Default for ComAmazonawsIamAwsidentitymanagementv20100508State {
    /// The account's IAM as it stands before any request: holding the one user this
    /// emulator's own long-term credential belongs to, and nothing else.
    ///
    /// A long-term access key belongs to an IAM USER, so the principal STS reports for a
    /// caller presenting the key the emulator publishes
    /// ([`::service_common::caller_principal`]) is a user of this account — present from
    /// the moment the account exists, exactly as the user behind an AWS access key is. It
    /// is therefore a stored record and not a derived one: a caller that reads its own
    /// identity and then manages that identity (embeds an inline policy, attaches a
    /// managed one, tags it, reads it back) is managing the ACCOUNT's own user, which the
    /// account's own collections have to carry for those writes to land and for a later
    /// read to report them.
    ///
    /// Its access key is deliberately NOT seeded beside it: the key pair the emulator
    /// publishes is held by the credential store, not by `iam:CreateAccessKey`, and
    /// [`authorization`] resolves a caller's identity-based policies through
    /// [`Self::access_keys`] — so seeding one there would start evaluating that caller
    /// against an empty policy set instead of leaving it in the position this emulator's
    /// published credential is in.
    fn default() -> Self {
        let mut account = Self::empty();
        let caller = ambient_user();
        account
            .users
            .insert(name_key(&caller.user_name), caller);
        account
    }
}

impl ComAmazonawsIamAwsidentitymanagementv20100508State {
    /// An account holding NOTHING: every collection empty and every id counter at its
    /// start.
    ///
    /// The base [`Self::default`] builds the account's starting records on top of. Written
    /// out field by field rather than derived, so a field added later has to say here what
    /// an account with nothing in it holds for it.
    fn empty() -> Self {
        Self {
            users: BTreeMap::new(),
            roles: BTreeMap::new(),
            groups: BTreeMap::new(),
            group_members: BTreeMap::new(),
            user_inline_policies: BTreeMap::new(),
            role_inline_policies: BTreeMap::new(),
            group_inline_policies: BTreeMap::new(),
            access_keys: BTreeMap::new(),
            signing_certificates: BTreeMap::new(),
            login_profiles: BTreeMap::new(),
            service_specific_credentials: BTreeMap::new(),
            policies: BTreeMap::new(),
            instance_profiles: BTreeMap::new(),
            role_attached_policies: BTreeMap::new(),
            user_attached_policies: BTreeMap::new(),
            group_attached_policies: BTreeMap::new(),
            open_id_connect_providers: BTreeMap::new(),
            saml_providers: BTreeMap::new(),
            server_certificates: BTreeMap::new(),
            account_alias: None,
            account_properties: BTreeMap::new(),
            delegation_requests: BTreeMap::new(),
            organization: None,
            organizations_access_reports: BTreeMap::new(),
            service_last_accessed_reports: BTreeMap::new(),
            report_job_seq: 0,
            saml_provider_uuid_seq: 0,
            saml_private_key_id_seq: 0,
            user_id_seq: 0,
            role_id_seq: 0,
            group_id_seq: 0,
            policy_id_seq: 0,
            instance_profile_id_seq: 0,
            server_certificate_id_seq: 0,
            signing_certificate_id_seq: 0,
        }
    }
}

/// The IAM user this emulator's own long-term credential belongs to, as the account holds
/// it.
///
/// Every member is a pure function of the account contract, stated once by
/// [`::service_common::caller_principal`] so this record and the one STS reports as the
/// calling principal cannot differ: the name, the account-root path, the ARN of that path
/// and name, and the `UserId` derived from that ARN. The `UserId` is NOT drawn from
/// [`ComAmazonawsIamAwsidentitymanagementv20100508State::user_id_seq`] for that reason —
/// the counter is what mints the id of a user a CALLER creates, and this user predates
/// every request — and it cannot collide with one of those: the two are 17 characters
/// drawn independently from the same 32-character alphabet.
///
/// Nothing in this account created the user — the key pair it holds was published before
/// any request was served — so its `CreateDate` is the fixed instant a record the account
/// starts out with reports ([`ACCOUNT_BOOTSTRAP_CREATE_MILLIS`]) rather than the clock at
/// first touch, which would make the record differ between two emulators serving the same
/// account. It carries no password (nothing here serves a console sign-in), no permissions
/// boundary and no tags, because nothing has attached any.
fn ambient_user() -> User {
    User {
        path: ::service_common::caller_principal::AMBIENT_USER_PATH.to_string(),
        user_name: ::service_common::caller_principal::AMBIENT_USER_NAME.to_string(),
        user_id: ::service_common::caller_principal::ambient_user_id(),
        arn: ::service_common::caller_principal::ambient_user_arn(),
        create_date: ::aws_smithy_types::DateTime::from_millis(ACCOUNT_BOOTSTRAP_CREATE_MILLIS),
        password_last_used: None,
        permissions_boundary: None,
        tags: None,
    }
}

/// The `CreateDate` a record the account STARTS OUT with reports: the epoch.
///
/// Nothing in this account created such a record, so there is no creation instant to
/// report, and reading the clock would answer a different `CreateDate` on every fresh
/// process. A FIXED instant keeps it stable, and the epoch is the value this repository
/// already gives a record nothing created — the same choice
/// [`SERVICE_PROVISIONED_ROLE_CREATE_MILLIS`] states for a service-linked role.
const ACCOUNT_BOOTSTRAP_CREATE_MILLIS: i64 = 0;

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

    /// Mint the next group's `GroupId`.
    ///
    /// Unique and stable for the same reason [`next_user_id`](Self::next_user_id)
    /// is: the counter is bijectively mixed into the suffix, and the id is minted
    /// once and then stored with the group.
    pub fn next_group_id(&mut self) -> String {
        self.group_id_seq += 1;
        unique_id(GROUP_ID_PREFIX, self.group_id_seq)
    }

    /// Mint the next Organizations access-report job's `JobId`.
    ///
    /// Spelled by [`report_job_id`] — unique for the same reason
    /// [`next_user_id`](Self::next_user_id) is, and minted once, then stored with the job.
    pub fn next_access_report_job_id(&mut self) -> String {
        self.report_job_seq += 1;
        report_job_id(self.report_job_seq)
    }

    /// Mint the next service-last-accessed report job's `JobId`.
    ///
    /// Drawn from the same counter and the same spelling as
    /// [`next_access_report_job_id`](Self::next_access_report_job_id), so an id is
    /// unique across BOTH kinds of report job the account records, and minted once,
    /// then stored with the job.
    pub fn next_service_last_accessed_job_id(&mut self) -> String {
        self.report_job_seq += 1;
        report_job_id(self.report_job_seq)
    }

    /// Mint the next SAML provider's `SAMLProviderUUID`.
    ///
    /// Unique and stable for the same reason [`next_user_id`](Self::next_user_id) is:
    /// the counter is bijectively mixed into the suffix, and the id is minted once and
    /// then stored with the provider.
    pub fn next_saml_provider_uuid(&mut self) -> String {
        self.saml_provider_uuid_seq += 1;
        unique_id(SAML_PROVIDER_UUID_PREFIX, self.saml_provider_uuid_seq)
    }

    /// Mint the next uploaded SAML private key's `KeyId`.
    ///
    /// Unique and stable for the same reason
    /// [`next_saml_provider_uuid`](Self::next_saml_provider_uuid) is, and drawn from its
    /// own counter and prefix so a key id is never a provider uuid.
    pub fn next_saml_private_key_id(&mut self) -> String {
        self.saml_private_key_id_seq += 1;
        unique_id(SAML_PRIVATE_KEY_ID_PREFIX, self.saml_private_key_id_seq)
    }

    /// Mint the next server certificate's `ServerCertificateId`.
    ///
    /// Unique and stable for the same reason [`next_user_id`](Self::next_user_id) is: the
    /// counter is bijectively mixed into the suffix, and the id is minted once and then
    /// stored with the certificate, so "the stable and unique string identifying the
    /// server certificate" (`ServerCertificateMetadata$ServerCertificateId`) is what every
    /// later read reports.
    pub fn next_server_certificate_id(&mut self) -> String {
        self.server_certificate_id_seq += 1;
        unique_id(
            SERVER_CERTIFICATE_ID_PREFIX,
            self.server_certificate_id_seq,
        )
    }

    /// Mint the next signing certificate's `CertificateId`.
    ///
    /// Unique and stable for the same reason [`next_user_id`](Self::next_user_id) is:
    /// the counter is bijectively mixed into the id, and the id is minted once and then
    /// stored with the certificate, so "the ID for the signing certificate"
    /// (`SigningCertificate$CertificateId`) is what every later read reports. It
    /// carries no entity-type prefix — AWS publishes none for a signing certificate —
    /// so the whole id is [`SIGNING_CERTIFICATE_ID_CHARACTERS`] base32 characters,
    /// inside `certificateIdType`'s len 24..128 and `^[\w]+$`.
    pub fn next_signing_certificate_id(&mut self) -> String {
        self.signing_certificate_id_seq += 1;
        unique_id_of_length(
            "",
            self.signing_certificate_id_seq,
            SIGNING_CERTIFICATE_ID_CHARACTERS,
        )
    }

    /// Whether `account_id` manages the organization `organization_id` — the question an
    /// operation served only to an organization's management account is decided by.
    ///
    /// False for an account that belongs to no organization at all (a standalone account,
    /// which is what [`Self::organization`] holds `None` for), for a member account of an
    /// organization it does not manage, and for the management account of ANOTHER
    /// organization than the one asked about.
    pub fn manages_organization(&self, account_id: &str, organization_id: &str) -> bool {
        self.organization.as_ref().is_some_and(|organization| {
            organization.organization_id == organization_id
                && organization.management_account_id == account_id
        })
    }

    /// How many signing certificates the user keyed by `user_key` holds.
    ///
    /// This is the measure IAM's signing-certificates-per-user quota
    /// ([`MAX_SIGNING_CERTIFICATES_PER_USER`]) is taken against, and what tells a user
    /// delete that certificates still depend on the user.
    pub fn signing_certificates_held_by(&self, user_key: &str) -> usize {
        self.signing_certificates
            .values()
            .filter(|record| record.user_key == user_key)
            .count()
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

    /// Whether a signed request presenting `access_key_id` carries a credential this
    /// account will accept.
    ///
    /// `UpdateAccessKey$Status` documents what the stored value means: "Active means
    /// that the key can be used for programmatic calls to Amazon Web Services, while
    /// Inactive means that the key cannot be used" — so a key the account holds
    /// authenticates a request only while its status is
    /// [`ACCESS_KEY_ACTIVE_STATUS`], and deactivating one makes it unusable for API
    /// calls while leaving it listed. Any other `statusType` value the member admits
    /// is likewise not `Active`, so it is not usable either.
    ///
    /// A key id the account holds NO record of is usable: such a credential is not an
    /// IAM user's at all — the key pair the emulator publishes at start, or a session
    /// another service minted — and this account's `AccessKey$Status` says nothing
    /// about it, so there is nothing here to refuse it on.
    ///
    /// A READ: the store is only consulted, so asking changes nothing a later caller
    /// sees.
    pub fn access_key_is_usable(&self, access_key_id: &str) -> bool {
        self.access_keys
            .get(access_key_id)
            .is_none_or(|record| record.status == ACCESS_KEY_ACTIVE_STATUS)
    }

    /// The groups the user keyed by `user_key` belongs to, by the key
    /// [`Self::groups`] holds each of them under, in that map's stable key order.
    ///
    /// The one place the membership is read from the group-keyed map
    /// ([`Self::group_members`]) the other way round, so every caller that asks what
    /// a USER belongs to agrees: the measure IAM's groups-per-user quota
    /// ([`MAX_GROUPS_PER_USER`]) is taken against, what tells a delete that the user
    /// still belongs to a group, what a report of the user's groups lists, and where
    /// the group policies in force on the user are collected from.
    pub fn group_keys_held_by<'a>(&'a self, user_key: &'a str) -> impl Iterator<Item = &'a String> {
        self.group_members
            .iter()
            .filter(move |(_, members)| members.contains(user_key))
            .map(|(group_key, _)| group_key)
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

    /// The managed policy `arn` names, if the account holds it, for a caller that
    /// is going to CHANGE it.
    ///
    /// The same lookup and the same identity test as
    /// [`policy_by_arn`](Self::policy_by_arn) — an ARN carrying another path,
    /// another account or another casing of the name names no policy — so a write
    /// addressed by ARN reaches exactly the record a read of that ARN reports, and
    /// the account's policies are never found under two different rules.
    pub fn policy_by_arn_mut(&mut self, arn: &str) -> Option<&mut ManagedPolicy> {
        let parsed = parse_policy_arn(arn)?;
        let held = self.policies.get_mut(&name_key(&parsed.policy_name))?;
        (held.policy.arn.as_deref() == Some(arn)).then_some(held)
    }

    /// How many entities the managed policy `arn` names is attached to —
    /// `Policy$AttachmentCount`, "the number of entities (users, groups, and roles)
    /// that the policy is attached to".
    ///
    /// Counted from the attachments themselves on every read, so the number a
    /// reader reports is the number of attachments that exist at that instant: an
    /// attach or a detach changes it without any reader having to be told, and the
    /// count can never drift from the attachment sets it describes. The same
    /// measure answers for a policy the account created and for one of AWS's
    /// ([`AWS_MANAGED_POLICIES`]) — what is counted is the attachments, not the
    /// policy's owner.
    ///
    /// All three identity kinds the measure names can hold an attachment, so all
    /// three attachment sets are counted — which is why the policy's own record does
    /// not cache the number.
    pub fn policy_attachment_count(&self, arn: &str) -> i32 {
        let attachments = self
            .role_attached_policies
            .values()
            .chain(self.user_attached_policies.values())
            .chain(self.group_attached_policies.values())
            .filter(|attached| attached.contains(arn))
            .count();
        i32::try_from(attachments).unwrap_or(i32::MAX)
    }

    /// For how many entities the managed policy `arn` names is used to set the
    /// permissions boundary — `Policy$PermissionsBoundaryUsageCount`, "the number
    /// of entities (users and roles) for which the policy is used to set the
    /// permissions boundary".
    ///
    /// Derived on every read for the same reason
    /// [`policy_attachment_count`](Self::policy_attachment_count) is: the boundary
    /// a role or user carries is the one record of that use.
    pub fn permissions_boundary_usage_count(&self, arn: &str) -> i32 {
        // One definition of "the boundary ARN an entity carries"
        // ([`permissions_boundary_arn`]), the same one `ListEntitiesForPolicy`
        // compares against, so the count and the listing cannot disagree about
        // which entities a boundary policy relates to.
        let boundary_is = |boundary: Option<&crate::ops::types::AttachedPermissionsBoundary>| {
            permissions_boundary_arn(boundary) == Some(arn)
        };
        let roles = self
            .roles
            .values()
            .filter(|role| boundary_is(role.permissions_boundary.as_ref()))
            .count();
        let users = self
            .users
            .values()
            .filter(|user| boundary_is(user.permissions_boundary.as_ref()))
            .count();
        i32::try_from(roles + users).unwrap_or(i32::MAX)
    }

    /// The managed policy `arn` names — the account's own ([`policy_by_arn`](Self::policy_by_arn))
    /// or AWS's ([`aws_managed_policy`]) — as every reader of a managed policy
    /// reports it, or `None` when neither holds that ARN.
    ///
    /// ONE resolution shared by every read addressed by `PolicyArn`, because the
    /// `arn:aws:iam::aws:policy/` namespace is part of every account's view of IAM:
    /// a policy AWS owns can be attached, so it must also be readable, and a reader
    /// that consulted only the account's store would call a policy AWS serves
    /// absent. The two counts are recomputed here rather than read out of the
    /// stored record, so a reader reports the attachments and boundary uses that
    /// exist now; nothing else about the stored policy is touched, and the
    /// resolution writes nothing.
    pub fn resolve_policy(&self, arn: &str) -> Option<ManagedPolicy> {
        // The counts are derived only for an ARN that resolves: a miss — an ARN of
        // another entity, or one no policy is held under — pays no scan of the
        // attachment sets, the roles or the users.
        if let Some(held) = self.policy_by_arn(arn) {
            let mut resolved = held.clone();
            resolved.policy.attachment_count = Some(self.policy_attachment_count(arn));
            resolved.policy.permissions_boundary_usage_count =
                Some(self.permissions_boundary_usage_count(arn));
            return Some(resolved);
        }
        aws_managed_policy(arn).map(|entry| {
            entry.record(
                self.policy_attachment_count(arn),
                self.permissions_boundary_usage_count(arn),
            )
        })
    }

    /// Whether the managed policy `arn` names may be attached to an identity, or used
    /// to set one's permissions boundary — the `IsAttachable` flag every reader of that
    /// policy reports ("specifies whether the policy can be attached to an IAM user,
    /// group, or role", `Policy$IsAttachable`).
    ///
    /// ONE decision, consulted by the attach calls and by the permissions-boundary
    /// puts, so the flag a client pre-screens an ARN with is exactly what those
    /// operations enforce: a policy a reader reports as not attachable is refused, and
    /// one it reports as attachable is accepted. The resolved record answers for every
    /// policy this emulator can report — the account's own, and the entries of AWS's
    /// catalog. An ARN in AWS's namespace the catalog does not carry resolves to no
    /// record, so AWS's published rule answers for it instead: a policy filed under the
    /// path reserved for service-linked roles belongs to that role alone
    /// ([`AwsManagedPolicy::is_attachable`]).
    ///
    /// A value that is not a managed-policy ARN at all is not this question's business
    /// and is taken as attachable: an operation taking a `PolicyArn` refuses such a
    /// value as an invalid input before it asks.
    pub fn policy_is_attachable(&self, arn: &str) -> bool {
        if let Some(held) = self.policy_by_arn(arn) {
            return held.policy.is_attachable;
        }
        if let Some(entry) = aws_managed_policy(arn) {
            return entry.is_attachable();
        }
        parse_policy_arn(arn).is_none_or(|parsed| !is_service_linked_role_path(&parsed.path))
    }

    /// The delegation request `arn` names, if the account holds it.
    ///
    /// Every read of a delegation request addressed by ARN resolves it here, and the
    /// held request answers only when its OWN ARN is the one asked for — the same
    /// identity test [`Self::policy_by_arn`] applies to a managed policy — so an ARN
    /// carrying another account's id names no request THIS account holds.
    pub fn delegation_request_by_arn(&self, arn: &str) -> Option<&DelegationRequest> {
        let parsed = parse_delegation_request_arn(arn)?;
        let held = self.delegation_requests.get(&parsed.delegation_request_id)?;
        (delegation_request_arn(&parsed.delegation_request_id) == arn).then_some(held)
    }

    /// The identity-based policies the IAM identity whose ARN is `arn` carries, or
    /// `None` when `arn` names no identity the account holds.
    ///
    /// The documents alone, which is what a caller evaluating or reading them needs;
    /// [`principal_sourced_policies`](Self::principal_sourced_policies) is the same
    /// collection with each document's provenance kept, and is where the collection
    /// rule itself lives.
    pub fn principal_policies(&self, arn: &str) -> Option<PrincipalPolicies> {
        let mut collected = PrincipalPolicies::default();
        for policy in self.principal_sourced_policies(arn)? {
            match (policy.document, policy.source) {
                (Some(document), _) => collected.documents.push(document),
                // Only an attached managed policy can resolve to no document (see
                // [`PrincipalPolicies::uncarried`]).
                (None, PolicySource::Managed { policy_arn, .. }) => {
                    collected.uncarried.push(policy_arn)
                }
                (None, PolicySource::Inline { .. }) => {}
            }
        }
        Some(collected)
    }

    /// The document of the managed policy `policy_arn` names that is IN FORCE: its
    /// DEFAULT version's, "the one that is in effect for the IAM users, groups, and
    /// roles to which the policy is attached" (`SetDefaultPolicyVersion`). `None` when
    /// this emulator holds no document for that ARN — an entry of AWS's catalog it does
    /// not carry — which is the one attachment that resolves to nothing. A record whose
    /// default version is not recorded is read at the version every policy is created
    /// with ([`FIRST_POLICY_VERSION_ID`]), the one it would be in force at.
    ///
    /// ONE resolution, shared by every reader of what an attached policy grants: the
    /// collection of an identity's own policies ([`Self::principal_sourced_policies`])
    /// and the ceiling attached to it ([`Self::permissions_boundary_document`]), so the
    /// two cannot read one policy differently.
    pub fn default_version_document(&self, policy_arn: &str) -> Option<String> {
        let policy = self.resolve_policy(policy_arn)?;
        let default = policy
            .policy
            .default_version_id
            .unwrap_or_else(|| FIRST_POLICY_VERSION_ID.to_string());
        policy
            .versions
            .get(&default)
            .and_then(|version| version.document.clone())
    }

    /// The permissions boundary document in force on the IAM identity whose ARN is
    /// `arn`, or `None` when it carries none — or carries one whose document this
    /// emulator does not hold, a ceiling it cannot read and so cannot apply.
    ///
    /// A boundary is attached as a MANAGED policy — "the ARN of the managed policy
    /// that is used to set the permissions boundary"
    /// (`AttachedPermissionsBoundary$PermissionsBoundaryArn`) — so the document in
    /// force is that policy's DEFAULT version, read through the resolution every
    /// attached policy's document is read through ([`Self::default_version_document`]).
    /// Only a user and a role can carry one, so a group's ARN names no boundary.
    ///
    /// ONE definition of "the ceiling in force on this identity", consulted by this
    /// service's own authorizer, by its policy simulator and by every other service
    /// that decides a call against this store's principals, so none of them caps a
    /// principal differently from the others. A READ: it only consults the store.
    ///
    /// The identity is resolved by NAME, the key the account holds it under, because an
    /// entity carries exactly one boundary whatever path an ARN spells for it — the
    /// question here is which ceiling that entity carries, not whether the ARN is a
    /// well-formed spelling of the entity.
    pub fn permissions_boundary_document(&self, arn: &str) -> Option<String> {
        let entity = parse_entity_arn(arn)?;
        let key = name_key(&entity.name);
        let attached = match entity.kind {
            EntityKind::User => self
                .users
                .get(&key)
                .and_then(|user| permissions_boundary_arn(user.permissions_boundary.as_ref())),
            EntityKind::Role => self
                .roles
                .get(&key)
                .and_then(|role| permissions_boundary_arn(role.permissions_boundary.as_ref())),
            EntityKind::Group => None,
        }?
        .to_string();
        self.default_version_document(&attached)
    }

    /// The identity-based policies the IAM identity whose ARN is `arn` carries, each
    /// with WHERE it comes from, or `None` when `arn` names no identity the account
    /// holds.
    ///
    /// An identity's own permissions come from the policies attached to it: the
    /// inline policies embedded in it, plus the DEFAULT version of each managed
    /// policy attached to it — "the default version is the operative version …
    /// the one that is in effect for the IAM users, groups, and roles to which the
    /// policy is attached" (`SetDefaultPolicyVersion`). A resource-based policy is
    /// not one of them, so a role's trust policy — which says who may assume the
    /// role rather than what the role may do — is not collected. For a USER, the
    /// policies in force on the groups it belongs to are collected too, each keeping
    /// the group it is attached to as its provenance.
    ///
    /// The provenance is kept because a caller can be asked about the policies as
    /// INDIVIDUAL policies rather than as one set: `SimulatePrincipalPolicy`'s
    /// `PolicyExclusionList` names policies to leave out of a simulation by type, by
    /// ARN or by inline name and the entity they are attached to, which is decidable
    /// only from where each document came from.
    ///
    /// The stored identity answers only when its OWN ARN is the one asked for, the
    /// identity test [`policy_by_arn`](Self::policy_by_arn) applies to a policy: an
    /// ARN carrying another path, another account or another resource type names no
    /// identity of this account's. The collection is read from the STORE alone, the
    /// way every other report of the account's record of an identity's attachments
    /// answers (see [`service_provisioned_role`]), and it writes nothing.
    pub fn principal_sourced_policies(&self, arn: &str) -> Option<Vec<SourcedPolicy>> {
        let source = parse_entity_arn(arn)?;
        let key = name_key(&source.name);
        let mut collected: Vec<SourcedPolicy> = Vec::new();
        let embedded = match source.kind {
            EntityKind::User => &self.user_inline_policies,
            EntityKind::Role => &self.role_inline_policies,
            EntityKind::Group => &self.group_inline_policies,
        };
        let attached: Vec<String> = match source.kind {
            EntityKind::User => {
                let user = self.users.get(&key)?;
                if user.arn != arn {
                    return None;
                }
                self.user_attached_policies
                    .get(&key)
                    .map(|attached| attached.iter().cloned().collect())
                    .unwrap_or_default()
            }
            EntityKind::Role => {
                let role = self.roles.get(&key)?;
                if role.arn != arn {
                    return None;
                }
                self.role_attached_policies
                    .get(&key)
                    .map(|attached| attached.iter().cloned().collect())
                    .unwrap_or_default()
            }
            EntityKind::Group => {
                let group = self.groups.get(&key)?;
                if group.arn != arn {
                    return None;
                }
                self.group_attached_policies
                    .get(&key)
                    .map(|attached| attached.iter().cloned().collect())
                    .unwrap_or_default()
            }
        };

        // The inline policies the identity embeds, ordered by policy name — the
        // order the embedding map holds them in, so two reads collect them alike.
        for (_, policy) in embedded.get(&key).into_iter().flatten() {
            collected.push(SourcedPolicy {
                source: PolicySource::Inline {
                    policy_name: policy.policy_name.clone(),
                    attachment_type: source.kind.resource_type(),
                    attachment_name: source.name.clone(),
                },
                document: Some(policy.policy_document.clone()),
            });
        }
        for policy_arn in attached {
            // The DEFAULT version's document, the operative one (see
            // [`Self::default_version_document`]).
            let document = self.default_version_document(&policy_arn);
            collected.push(SourcedPolicy {
                source: PolicySource::Managed {
                    aws_owned: is_aws_managed_policy_arn(&policy_arn),
                    policy_arn,
                },
                document,
            });
        }

        // A user's permissions include the policies in force on every group it belongs
        // to: "if you specify a user, the simulation also includes all policies that
        // are attached to any groups the user belongs to"
        // (`SimulatePrincipalPolicy$PolicySourceArn`). Each inherited policy keeps the
        // GROUP as its provenance, because the group is the entity it is attached to;
        // collecting it through the group's OWN ARN is what makes one rule answer for
        // both, so a policy a group carries is in force on its members however the
        // group came to carry it.
        if source.kind == EntityKind::User {
            let group_arns: Vec<String> = self
                .group_keys_held_by(&key)
                .filter_map(|group_key| self.groups.get(group_key))
                .map(|group| group.arn.clone())
                .collect();
            for group_arn in group_arns {
                collected.extend(
                    self.principal_sourced_policies(&group_arn)
                        .into_iter()
                        .flatten(),
                );
            }
        }
        Some(collected)
    }

    /// The permissions policies the resource a service-last-accessed report is about
    /// carries, or `None` when `arn` names no such resource of this account.
    ///
    /// "The ARN of the IAM resource (user, group, role, or managed policy) used to
    /// generate information about when the resource was last used in an attempt to access
    /// an Amazon Web Services service" (`GenerateServiceLastAccessedDetails$Arn`): the
    /// report is about what that resource's permissions policies ALLOW, so for an
    /// IDENTITY the collection is the one every reader of an identity's grants takes
    /// ([`Self::principal_sourced_policies`], which for a user also collects the policies
    /// the groups it belongs to carry), and for a managed POLICY it is that policy's own
    /// operative document ([`Self::default_version_document`]).
    ///
    /// Only permissions policies: "service last accessed data does not use other policy
    /// types when determining whether a resource could access a service. These other
    /// policy types include resource-based policies, access control lists, Organizations
    /// policies, IAM permissions boundaries, and STS assume role policies. It only
    /// applies permissions policy logic" (`GenerateServiceLastAccessedDetails`) — which
    /// is exactly the collection those readers make, so a role's trust policy and the
    /// ceiling an entity carries are not in it.
    ///
    /// A READ: it only consults the store.
    pub fn report_resource_policies(&self, arn: &str) -> Option<Vec<SourcedPolicy>> {
        if parse_entity_arn(arn).is_some() {
            return self.principal_sourced_policies(arn);
        }
        // A managed policy answers only through the ARN grammar that names one, and only
        // when this emulator holds that policy at all — the account's own or an entry of
        // AWS's catalog ([`Self::resolve_policy`]).
        parse_policy_arn(arn)?;
        self.resolve_policy(arn)?;
        Some(vec![SourcedPolicy {
            source: PolicySource::Managed {
                aws_owned: is_aws_managed_policy_arn(arn),
                policy_arn: arn.to_string(),
            },
            document: self.default_version_document(arn),
        }])
    }

    /// What the inline policies embedded in the user keyed by `user_key` take up,
    /// counting every policy except the one keyed by `replacing` (a put of an
    /// existing name replaces that document rather than adding to it).
    ///
    /// [`InlinePolicySize::measured`] is what IAM's user policy size quota
    /// ([`MAX_USER_INLINE_POLICY_CHARACTERS`]) is taken against.
    pub fn user_inline_policy_size(&self, user_key: &str, replacing: &str) -> InlinePolicySize {
        inline_policy_size(&self.user_inline_policies, user_key, replacing)
    }

    /// The same, for the role keyed by `role_key`, whose measured size is what IAM's
    /// role policy size quota ([`MAX_ROLE_INLINE_POLICY_CHARACTERS`]) is taken
    /// against.
    pub fn role_inline_policy_size(&self, role_key: &str, replacing: &str) -> InlinePolicySize {
        inline_policy_size(&self.role_inline_policies, role_key, replacing)
    }

    /// The same, for the group keyed by `group_key`, whose measured size is what
    /// IAM's group policy size quota ([`MAX_GROUP_INLINE_POLICY_CHARACTERS`]) is
    /// taken against.
    pub fn group_inline_policy_size(&self, group_key: &str, replacing: &str) -> InlinePolicySize {
        inline_policy_size(&self.group_inline_policies, group_key, replacing)
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

    /// Move the user keyed by `from_key` to `user_name` and `path`, carrying everything
    /// the account holds ABOUT that user with it.
    ///
    /// `UpdateUser` "updates the name and/or the path of the specified IAM user", and
    /// "the user's unique identifier remains the same" — which is the whole point of the
    /// move: a user that kept its `UserId` keeps the credentials, policies, memberships
    /// and console password it held, so every collection keyed by the user's name or
    /// carrying its key is re-pointed rather than left behind. The user's ARN is REBUILT,
    /// because it is derived from the path and the name ("if you change the user's name or
    /// path, you must update the policies that refer to it" — the ARN moves with them).
    ///
    /// One definition, so the rename cannot leave a collection behind: a caller adds to
    /// [`Self`] and this method is where the new collection joins the move.
    ///
    /// Answers `false` when `from_key` names no user, and does nothing.
    pub fn rename_user(&mut self, from_key: &str, user_name: &str, path: &str) -> bool {
        let Some(mut user) = self.users.remove(from_key) else {
            return false;
        };
        let to_key = name_key(user_name);
        user.arn = user_arn(path, user_name);
        user.user_name = user_name.to_string();
        user.path = path.to_string();
        self.users.insert(to_key.clone(), user);
        if let Some(policies) = self.user_inline_policies.remove(from_key) {
            self.user_inline_policies.insert(to_key.clone(), policies);
        }
        if let Some(attached) = self.user_attached_policies.remove(from_key) {
            self.user_attached_policies.insert(to_key.clone(), attached);
        }
        if let Some(profile) = self.login_profiles.remove(from_key) {
            self.login_profiles.insert(to_key.clone(), profile);
        }
        for members in self.group_members.values_mut() {
            if members.remove(from_key) {
                members.insert(to_key.clone());
            }
        }
        for record in self.access_keys.values_mut() {
            if record.user_key == from_key {
                record.user_key = to_key.clone();
            }
        }
        for record in self.signing_certificates.values_mut() {
            if record.user_key == from_key {
                record.user_key = to_key.clone();
            }
        }
        for credential in self.service_specific_credentials.values_mut() {
            if name_key(&credential.user_name) == from_key {
                credential.user_name = user_name.to_string();
            }
        }
        true
    }
}

/// What a set of inline policies takes up: the size the policy-size quota is
/// measured against, and the raw text the store retains for it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InlinePolicySize {
    /// Non-whitespace characters, the measure IAM's policy-size quotas are taken
    /// against ([`policy_document_characters`]).
    pub measured: usize,
    /// Raw characters, whitespace included: what the store holds, bounded by
    /// [`MAX_INLINE_POLICY_TEXT_CHARACTERS`].
    pub retained: usize,
}

/// What the inline policies `policies` holds for the entity keyed by `entity_key`
/// take up, counting every policy except the one keyed by `replacing`.
///
/// A put of an existing policy name REPLACES that document rather than adding to
/// it, so the document being replaced counts toward neither sum the new one has to
/// fit inside. One definition, shared by every entity kind that embeds inline
/// policies.
fn inline_policy_size(
    policies: &BTreeMap<String, BTreeMap<String, InlinePolicy>>,
    entity_key: &str,
    replacing: &str,
) -> InlinePolicySize {
    let mut size = InlinePolicySize::default();
    let Some(policies) = policies.get(entity_key) else {
        return size;
    };
    for (_, policy) in policies.iter().filter(|(key, _)| key.as_str() != replacing) {
        size.measured += policy_document_characters(&policy.policy_document);
        size.retained += policy.policy_document.chars().count();
    }
    size
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

/// The ARN of an IAM group: `arn:aws:iam::<account>:group<path><name>`.
///
/// Built the same way [`user_arn`] is, from the `group` resource type: `path`
/// already begins and ends with `/`, and IAM is a global service, so the region
/// segment is empty.
pub fn group_arn(path: &str, group_name: &str) -> String {
    ::engine::emulator_arn("iam", "", &format!("group{path}{group_name}"))
}

/// The ARN of an IAM role: `arn:aws:iam::<account>:role<path><name>`.
///
/// Built the same way [`user_arn`] is, from the `role` resource type: `path`
/// already begins and ends with `/`, and IAM is a global service, so the region
/// segment is empty.
pub fn role_arn(path: &str, role_name: &str) -> String {
    ::engine::emulator_arn("iam", "", &format!("role{path}{role_name}"))
}

/// The ARN of an IAM OIDC identity provider:
/// `arn:aws:iam::<account>:oidc-provider/<url without its scheme>`.
///
/// The provider is named by its URL, so its ARN is derived from that URL with the
/// `https://` scheme ([`OPEN_ID_CONNECT_URL_SCHEME`]) removed — which is what makes
/// two creates for one URL two creates for one provider. IAM is a global service, so
/// the region segment is empty, exactly as it is for [`user_arn`].
pub fn open_id_connect_provider_arn(url: &str) -> String {
    let host_and_path = url.strip_prefix(OPEN_ID_CONNECT_URL_SCHEME).unwrap_or(url);
    ::engine::emulator_arn("iam", "", &format!("oidc-provider/{host_and_path}"))
}

/// The ARN of an IAM SAML 2.0 identity provider:
/// `arn:aws:iam::<account>:saml-provider/<name>`.
///
/// The provider is named by its `Name`, so its ARN is derived from that name — which is
/// what makes two creates for one name two creates for one provider, and what lets
/// `AssumeRoleWithSAML`'s `PrincipalArn` ("the ARN of the SAML provider in IAM") name
/// the provider a role's trust policy trusts. IAM is a global service, so the region
/// segment is empty, exactly as it is for [`user_arn`].
pub fn saml_provider_arn(name: &str) -> String {
    ::engine::emulator_arn("iam", "", &format!("saml-provider/{name}"))
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

/// The ARN of an IAM server certificate:
/// `arn:aws:iam::<account>:server-certificate<path><name>`.
///
/// Built the same way [`user_arn`] is, from the `server-certificate` resource type:
/// `path` already begins and ends with `/` (the model's `pathType` pattern), and IAM is a
/// global service, so the region segment is empty.
pub fn server_certificate_arn(path: &str, server_certificate_name: &str) -> String {
    ::engine::emulator_arn(
        "iam",
        "",
        &format!("server-certificate{path}{server_certificate_name}"),
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

/// The one sentence this service states a refused change to a PROTECTED role with — a
/// role a service links itself to, which "only the service that depends on the
/// service-linked role can modify or delete … on your behalf"
/// (`UnmodifiableEntityException`).
///
/// Every word of it is fixed but the role the request named, which is quoted exactly as
/// the request spelled it. The protection is one rule about one kind of role, so every
/// operation that refuses such a change states it through here rather than wording its
/// own sentence: the sentence does not name which operation was refused, nor the service
/// the role is linked to, so there is nothing per-operation to word. It is read back by
/// `tests/all/protected_role_refusals.rs` across the operations the protection covers.
pub fn protected_role_message(role_name: &str) -> String {
    format!(
        "Cannot perform the operation on the protected role '{role_name}' - this role is \
         only modifiable by AWS"
    )
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
/// `AWSServiceRoleForAWSSSMForSAP`, `migrationhub-strategy.amazonaws.com` links
/// `AWSServiceRoleForMigrationHubStrategy`, `ops.apigateway.amazonaws.com` links
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
    // AWS Compute Optimizer: the hyphen of the principal's label is not in the role
    // AWS names in "Using service-linked roles for AWS Compute Optimizer",
    // AWSServiceRoleForComputeOptimizer.
    ("compute-optimizer.amazonaws.com", "AWSServiceRoleForComputeOptimizer"),
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
    // The AWS Migration Hub family. Each of these services is reached through a
    // HYPHENATED endpoint label (`migrationhub-orchestrator`,
    // `migrationhub-strategy` — the `cloudTrailEventSource` their own Smithy models
    // in this repository carry), while the label AWS assigns the role is the
    // service's NAME in mixed case with the hyphen gone, which no transformation of
    // the principal produces: "Using service-linked roles for AWS Migration Hub
    // Orchestrator" names AWSServiceRoleForMigrationHubOrchestrator and "Using
    // service-linked roles for Strategy Recommendations" names
    // AWSServiceRoleForMigrationHubStrategy — the role Strategy Recommendations
    // requires in the account before it will run an import or an assessment, and so
    // the name a client reads back as that prerequisite.
    (
        "migrationhub-orchestrator.amazonaws.com",
        "AWSServiceRoleForMigrationHubOrchestrator",
    ),
    (
        "migrationhub-strategy.amazonaws.com",
        "AWSServiceRoleForMigrationHubStrategy",
    ),
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
    // AWS Resource Explorer: the principal carries the service's API VERSION
    // (`resource-explorer-2`), which the role AWS names in "Using service-linked
    // roles for Resource Explorer" does not — AWSServiceRoleForResourceExplorer.
    ("resource-explorer-2.amazonaws.com", "AWSServiceRoleForResourceExplorer"),
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

/// The marker scope of the membership of the group keyed by `group_key`.
/// See [`MARKER_SCOPE_GROUP_MEMBERS`].
pub fn group_members_marker_scope(group_key: &str) -> String {
    format!("{MARKER_SCOPE_GROUP_MEMBERS}:{group_key}")
}

/// The marker scope of the groups the user keyed by `user_key` belongs to.
/// See [`MARKER_SCOPE_USER_GROUPS`].
pub fn user_groups_marker_scope(user_key: &str) -> String {
    format!("{MARKER_SCOPE_USER_GROUPS}:{user_key}")
}

/// The marker scope of the attachment set of the user keyed by `user_key`.
pub fn user_attached_policies_marker_scope(user_key: &str) -> String {
    format!("{MARKER_SCOPE_USER_ATTACHED_POLICIES}:{user_key}")
}

/// The marker scope of the attachment set of the group keyed by `group_key`.
/// See [`MARKER_SCOPE_GROUP_ATTACHED_POLICIES`].
pub fn group_attached_policies_marker_scope(group_key: &str) -> String {
    format!("{MARKER_SCOPE_GROUP_ATTACHED_POLICIES}:{group_key}")
}

/// The marker scope of the inline-policy set of the role keyed by `role_key`.
pub fn role_policies_marker_scope(role_key: &str) -> String {
    format!("{MARKER_SCOPE_ROLE_POLICIES}:{role_key}")
}

/// The marker scope of the inline-policy set of the user keyed by `user_key`.
pub fn user_policies_marker_scope(user_key: &str) -> String {
    format!("{MARKER_SCOPE_USER_POLICIES}:{user_key}")
}

/// The marker scope of the inline-policy set of the group keyed by `group_key`.
pub fn group_policies_marker_scope(group_key: &str) -> String {
    format!("{MARKER_SCOPE_GROUP_POLICIES}:{group_key}")
}

/// The marker scope of the entity collection of the managed policy `policy_arn`
/// names. See [`MARKER_SCOPE_ENTITIES_FOR_POLICY`].
pub fn entities_for_policy_marker_scope(policy_arn: &str) -> String {
    format!("{MARKER_SCOPE_ENTITIES_FOR_POLICY}:{policy_arn}")
}

/// The marker scope of the version collection of the managed policy `policy_arn`
/// names. See [`MARKER_SCOPE_POLICY_VERSIONS`].
pub fn policy_versions_marker_scope(policy_arn: &str) -> String {
    format!("{MARKER_SCOPE_POLICY_VERSIONS}:{policy_arn}")
}

/// The marker scope of the services the report job `job_id` names reports on. See
/// [`MARKER_SCOPE_SERVICE_LAST_ACCESSED`].
pub fn service_last_accessed_marker_scope(job_id: &str) -> String {
    format!("{MARKER_SCOPE_SERVICE_LAST_ACCESSED}:{job_id}")
}

/// The marker scope of the entities the report job `job_id` names reports for the service
/// whose namespace is `service_namespace`. See
/// [`MARKER_SCOPE_SERVICE_LAST_ACCESSED_ENTITIES`].
pub fn service_last_accessed_entities_marker_scope(
    job_id: &str,
    service_namespace: &str,
) -> String {
    format!("{MARKER_SCOPE_SERVICE_LAST_ACCESSED_ENTITIES}:{job_id}:{service_namespace}")
}

/// The page key of ONE policy version in the order a reader of a managed policy's
/// versions enumerates them: the version NUMBER, zero padded, followed by the
/// version identifier itself.
///
/// A version identifier is `v` then a number then an optional dotted suffix
/// (`policyVersionIdType`'s pattern `^v[1-9][0-9]*(\.[A-Za-z0-9-]*)?$`), so the
/// identifiers alone do not sort in version order — `v10` precedes `v2` byte by
/// byte. Padding the number to [`POLICY_VERSION_KEY_DIGITS`] digits makes the byte
/// order of these keys the numeric order, which is the order the versions were
/// created in, and appending the identifier keeps the key unique when two
/// identifiers carry the same number with different suffixes. The byte order IS the
/// enumeration order, which is what lets a reader resume strictly after the key a
/// `Marker` names even when the version it named has since been deleted.
pub fn policy_version_page_key(version_id: &str) -> String {
    let number = policy_version_number(version_id);
    format!("{number:0width$}:{version_id}", width = POLICY_VERSION_KEY_DIGITS)
}

/// The NUMBER a policy version identifier carries: the digit run after the leading
/// `v` of `policyVersionIdType`'s pattern `^v[1-9][0-9]*(\.[A-Za-z0-9-]*)?$`.
///
/// ONE reading of an identifier's number, shared by the enumeration order
/// ([`policy_version_page_key`]) and by the minting of the next identifier
/// ([`ManagedPolicy::next_version_id`]), so a version created is numbered in the
/// space the readers order by. An identifier the pattern admits always carries a
/// number; one that somehow does not counts as 0, so it sorts first rather than
/// being dropped from the order.
pub fn policy_version_number(version_id: &str) -> u64 {
    version_id
        .trim_start_matches('v')
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap_or(0)
}

/// How many digits [`policy_version_page_key`] pads a version number to: enough
/// for every `u64`, so no number can overflow the padding and sort out of order.
const POLICY_VERSION_KEY_DIGITS: usize = 20;

/// The marker scope of the tag set of the entity `entity_handle` names — an ARN or
/// an entity key, whichever handle the tag-listing operation takes. See
/// [`MARKER_SCOPE_ENTITY_TAGS`].
pub fn entity_tags_marker_scope(entity_handle: &str) -> String {
    format!("{MARKER_SCOPE_ENTITY_TAGS}:{entity_handle}")
}

/// The page key of ONE entity in the order an operation that enumerates entities of
/// SEVERAL types under one `Marker` walks them: the entity's TYPE tag followed by the
/// key its own store holds it under ([`name_key`] of its name, or an ARN for a
/// managed policy).
///
/// The byte order of these keys IS the enumeration order, which is what lets such an
/// operation resume strictly after the key a `Marker` names even when the entity it
/// named has since been deleted. Each tag is followed by the same separator, so the
/// keys group by entity type, in the tags' own order, and then run ascending within a
/// type — the order each store is already keyed in. `ListEntitiesForPolicy` tags the
/// three entity kinds a policy relates to ([`ENTITY_KEY_TAG_GROUP`],
/// [`ENTITY_KEY_TAG_ROLE`], [`ENTITY_KEY_TAG_USER`]);
/// `GetAccountAuthorizationDetails` tags the five the `EntityType` enum names, which
/// is also what its `Filter` selects by.
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
    let (account, resource) = iam_arn_parts(arn)?;
    let (path, policy_name) = split_path_and_name(resource.strip_prefix("policy/")?)?;
    Some(PolicyArn {
        account: account.to_string(),
        path,
        policy_name: policy_name.to_string(),
    })
}

/// The parts of a delegation request's ARN that an operation taking one reads.
#[derive(Debug, Clone)]
pub struct DelegationRequestArn {
    /// The ARN's account segment: the account that holds the request.
    pub account: String,
    /// The request's `DelegationRequestId`: the whole resource body.
    pub delegation_request_id: String,
}

/// The parts of `arn` when it is a delegation request's ARN, or `None` when it is
/// not one — which is what lets an operation taking an ARN of any entity kind tell
/// the one kind it acts on from every other.
///
/// Such an ARN is `arn:<partition>:iam::<account>:delegation-request/<id>`, read
/// through the same [`iam_arn_parts`] envelope as every other IAM ARN this crate
/// parses; the identifier is the whole resource body, since
/// `delegationRequestIdType`'s pattern (`^[\w-]+$`) admits no `/` and so no path.
/// The account segment comes back with it for the same reason it does for a managed
/// policy ([`parse_policy_arn`]): it is part of WHICH request the ARN names.
pub fn parse_delegation_request_arn(arn: &str) -> Option<DelegationRequestArn> {
    let (account, resource) = iam_arn_parts(arn)?;
    let delegation_request_id = resource
        .strip_prefix(DELEGATION_REQUEST_RESOURCE_TYPE)?
        .strip_prefix('/')?;
    if delegation_request_id.is_empty() || delegation_request_id.contains('/') {
        return None;
    }
    Some(DelegationRequestArn {
        account: account.to_string(),
        delegation_request_id: delegation_request_id.to_string(),
    })
}

/// The ARN of a delegation request:
/// `arn:aws:iam::<account>:delegation-request/<id>`.
///
/// Built the same way [`user_arn`] is, from the
/// [`DELEGATION_REQUEST_RESOURCE_TYPE`] resource type: IAM is a global service, so
/// the region segment is empty, and a delegation request carries no path — its
/// identifier is the whole resource body ([`parse_delegation_request_arn`] reads it
/// back).
pub fn delegation_request_arn(delegation_request_id: &str) -> String {
    ::engine::emulator_arn(
        "iam",
        "",
        &format!("{DELEGATION_REQUEST_RESOURCE_TYPE}/{delegation_request_id}"),
    )
}

/// The message a read of a delegation request the account does not hold is refused
/// with: the entity type that was looked for, then the identifier the request named —
/// "the error message describes the resource" (`NoSuchEntityException`).
///
/// The delegation-request API family's own wording, which is not the "The &lt;entity&gt;
/// with name &lt;name&gt; cannot be found." form IAM words its user, role and group
/// reads with (see `crate::ops::get_human_readable_summary`). Stated ONCE here so the
/// reads of a delegation request cannot word their refusal two ways; the identifier is
/// taken from the request.
pub fn delegation_request_not_found_message(delegation_request_id: &str) -> String {
    format!("DelegationRequest not found: {delegation_request_id}")
}

/// The account and resource segments of `arn` when it is an IAM resource's ARN, or
/// `None` when it is not one.
///
/// Every IAM ARN is `arn:<partition>:iam::<account>:<resource>`: IAM is a global
/// service, so the region segment is EMPTY, and the account segment names the
/// account that holds the resource (or [`AWS_MANAGED_POLICY_ACCOUNT`] for a policy
/// AWS itself owns). ONE reading of that envelope, shared by every parser of an IAM
/// ARN in this crate, so no two of them can disagree about what an IAM ARN is.
fn iam_arn_parts(arn: &str) -> Option<(&str, &str)> {
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
    Some((account, fields.next()?))
}

/// Split the body of an IAM resource's ARN — its path followed by its friendly name
/// — into that path (beginning and ending with `/`) and that name, or `None` when
/// `resource` is not such a body.
///
/// Everything before the LAST `/` is the path, which is absent for a resource at the
/// account root ([`DEFAULT_PATH`]). An empty path segment (`//Name`) is not a path
/// IAM's path patterns admit — `policyPathType`'s is
/// `^((/[A-Za-z0-9\.,\+@=_-]+)*)/$` — and an empty name names nothing, so neither is
/// a resource body.
fn split_path_and_name(resource: &str) -> Option<(String, &str)> {
    let (path, name) = match resource.rfind('/') {
        Some(last) => {
            let path_body = &resource[..last];
            if path_body.split('/').any(str::is_empty) {
                return None;
            }
            (format!("/{path_body}/"), &resource[last + 1..])
        }
        None => (DEFAULT_PATH.to_string(), resource),
    };
    (!name.is_empty()).then_some((path, name))
}

/// The kind of IAM identity an ARN names: one of the three a policy can be attached
/// to — "an IAM user, group, or role"
/// (`GetContextKeysForPrincipalPolicy$PolicySourceArn`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityKind {
    /// An IAM user, whose ARN carries the `user` resource type.
    User,
    /// An IAM group, whose ARN carries the `group` resource type.
    Group,
    /// An IAM role, whose ARN carries the `role` resource type.
    Role,
}

impl EntityKind {
    /// Every kind, in the order the documentation lists them.
    pub const ALL: &'static [Self] = &[Self::User, Self::Group, Self::Role];

    /// The ARN resource type that names this kind of identity, which is also the
    /// word a refusal naming such an identity uses.
    pub fn resource_type(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Group => "group",
            Self::Role => "role",
        }
    }
}

/// The parts of an IAM identity's ARN that an operation taking one reads.
#[derive(Debug, Clone)]
pub struct EntityArn {
    /// Which of the three identities the ARN names.
    pub kind: EntityKind,
    /// The ARN's account segment: the account that holds the identity.
    pub account: String,
    /// The identity's path, beginning and ending with `/`.
    pub path: String,
    /// The identity's friendly name: the last segment of the ARN's resource.
    pub name: String,
}

/// The parts of `arn` when it is the ARN of an IAM user, group or role, or `None`
/// when it is not one — which is what makes an operation taking a principal's ARN
/// refuse it as an "invalid … value … supplied for an input parameter" rather than
/// report a missing entity.
///
/// Such an ARN is `arn:<partition>:iam::<account>:<type><path><name>`, read through
/// the same [`iam_arn_parts`] envelope and [`split_path_and_name`] body as a managed
/// policy's ([`parse_policy_arn`]); only the resource type differs.
pub fn parse_entity_arn(arn: &str) -> Option<EntityArn> {
    let (account, resource) = iam_arn_parts(arn)?;
    let (kind, body) = EntityKind::ALL.iter().find_map(|kind| {
        let body = resource.strip_prefix(kind.resource_type())?.strip_prefix('/')?;
        Some((*kind, body))
    })?;
    let (path, name) = split_path_and_name(body)?;
    Some(EntityArn {
        kind,
        account: account.to_string(),
        path,
        name: name.to_string(),
    })
}

/// The identity-based policies an IAM identity carries, as
/// [`principal_policies`](ComAmazonawsIamAwsidentitymanagementv20100508State::principal_policies)
/// collects them.
#[derive(Debug, Clone, Default)]
pub struct PrincipalPolicies {
    /// The policy documents themselves: the identity's inline policies, then the
    /// default version of each managed policy attached to it.
    pub documents: Vec<String>,
    /// The ARNs of the attached managed policies whose document this emulator does
    /// not hold — an ARN in AWS's own namespace that [`AWS_MANAGED_POLICIES`] does
    /// not carry, the only attachment that can resolve to no document, so nothing
    /// can be reported about its content ([`uncarried_aws_managed_policy`]).
    pub uncarried: Vec<String>,
}

/// WHERE one identity-based policy in force on a principal comes from, as
/// [`principal_sourced_policies`](ComAmazonawsIamAwsidentitymanagementv20100508State::principal_sourced_policies)
/// reports it.
///
/// The two forms an identity-based policy takes are the two the IAM grammar names: a
/// policy EMBEDDED in the identity (an inline policy, identified by its own name and
/// the entity it is embedded in) and a managed policy ATTACHED to it (identified by
/// its ARN, which also says whether AWS or the account owns it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicySource {
    /// An inline policy embedded in the identity.
    Inline {
        /// The policy's own name, as the put that embedded it spelled it.
        policy_name: String,
        /// Which kind of identity embeds it: one of [`EntityKind::resource_type`]'s
        /// words, the same three values the model's `AttachmentType` enum declares.
        attachment_type: &'static str,
        /// The friendly name of the identity that embeds it.
        attachment_name: String,
    },
    /// A managed policy attached to the identity.
    Managed {
        /// The attached policy's ARN.
        policy_arn: String,
        /// Whether that ARN is in AWS's own managed-policy namespace rather than the
        /// account's ([`is_aws_managed_policy_arn`]).
        aws_owned: bool,
    },
}

/// One identity-based policy in force on a principal: where it comes from, and its
/// document when this emulator holds one for it (see
/// [`PrincipalPolicies::uncarried`] for the one case it does not).
#[derive(Debug, Clone)]
pub struct SourcedPolicy {
    /// Where the policy comes from.
    pub source: PolicySource,
    /// The document in force: an inline policy's own text, or the default version of
    /// an attached managed policy.
    pub document: Option<String>,
}

/// Every condition context key the IAM policy documents in `documents` reference,
/// folded to one entry per key and ordered.
///
/// A policy references a context key by TESTING it: "context keys can be evaluated
/// by testing against a value in an IAM policy"
/// (`GetContextKeysForPrincipalPolicy`), and the place a statement tests one is its
/// `Condition` element, whose every condition-operator block is a map from context
/// key to the values that key is tested against — so the keys of those blocks are
/// the keys the document references. A statement with no `Condition` tests none, and
/// `Statement` is read both as the list and as the single statement the IAM policy
/// grammar admits.
///
/// Context key names are not distinguished by case, so two spellings of one key are
/// one key: the folded name is the identity and the first spelling seen is the one
/// reported. A document that is not JSON contributes nothing — the operations taking
/// a policy document refuse one ([`is_policy_document`]) before reaching here.
pub fn context_keys_in_policies<'a, I: IntoIterator<Item = &'a str>>(documents: I) -> Vec<String> {
    let mut keys: BTreeMap<String, String> = BTreeMap::new();
    for document in documents {
        let Ok(parsed) = ::serde_json::from_str::<::serde_json::Value>(document) else {
            continue;
        };
        let Some(statement) = parsed.get("Statement") else {
            continue;
        };
        let statements = match statement.as_array() {
            Some(statements) => statements.as_slice(),
            None => ::std::slice::from_ref(statement),
        };
        for statement in statements {
            let Some(condition) = statement.get("Condition").and_then(|c| c.as_object()) else {
                continue;
            };
            for tested in condition.values().filter_map(|block| block.as_object()) {
                for key in tested.keys() {
                    keys.entry(key.to_ascii_lowercase())
                        .or_insert_with(|| key.clone());
                }
            }
        }
    }
    keys.into_values().collect()
}

/// The `Effect` value that makes a statement a GRANT rather than a refusal, as the
/// IAM policy grammar spells it.
const EFFECT_ALLOW: &str = "Allow";

/// The character separating a service namespace from an action name in an action
/// element's value (`iam:CreateUser`).
const ACTION_SEPARATOR: char = ':';

/// The whole-element wildcard: the value that stands for every action, and the one
/// action element the grammar admits with no service namespace in front of it.
const ANY_ACTION: &str = "*";

/// Whether the IAM policy document `document` grants access to the service whose
/// namespace is `service_namespace` — that is, whether one of its statements ALLOWS
/// at least one action belonging to that service.
///
/// A policy grants access to a service when it permits an action of that service:
/// an action element's value is `<namespace>:<action>` (or the whole-element `*`),
/// so the namespace a value reaches is the part before the separator, matched with
/// IAM's `*` and `?` wildcards and without regard to case — the comparison
/// [`::service_common::iam_authorization::wildcard_match`] makes for every action
/// name in the tree. A `NotAction` element permits everything its values do NOT, so
/// it reaches the namespace unless one of its values covers EVERY action of it.
///
/// Only a statement's EFFECT and its action element decide this: what the policy
/// grants access to is a property of the policy, so a statement's resource scope
/// narrows which resources of the service it reaches rather than whether it reaches
/// the service, and a `Deny` — in this document or another — refuses access rather
/// than granting it. A document that is not JSON grants nothing, the reading
/// [`context_keys_in_policies`] gives such a document, and `Statement` is read both
/// as the list and as the single statement the grammar admits.
pub fn grants_service_access(document: &str, service_namespace: &str) -> bool {
    let Ok(parsed) = ::serde_json::from_str::<::serde_json::Value>(document) else {
        return false;
    };
    let Some(statement) = parsed.get("Statement") else {
        return false;
    };
    let statements = match statement.as_array() {
        Some(statements) => statements.as_slice(),
        None => ::std::slice::from_ref(statement),
    };
    statements.iter().any(|statement| {
        let allows = statement
            .get("Effect")
            .and_then(::serde_json::Value::as_str)
            .is_some_and(|effect| effect == EFFECT_ALLOW);
        if !allows {
            return false;
        }
        if let Some(permitted) = statement.get("Action") {
            return element_values(permitted)
                .into_iter()
                .any(|value| reaches_namespace(value, service_namespace));
        }
        if let Some(excluded) = statement.get("NotAction") {
            return !element_values(excluded)
                .into_iter()
                .any(|value| covers_namespace(value, service_namespace));
        }
        // A statement with neither element is about no action at all.
        false
    })
}

/// The string values one policy element carries: the IAM grammar writes
/// `Action`/`NotAction` either as one string or as a list of them.
fn element_values(element: &::serde_json::Value) -> Vec<&str> {
    match element.as_array() {
        Some(values) => values.as_slice(),
        None => ::std::slice::from_ref(element),
    }
    .iter()
    .filter_map(::serde_json::Value::as_str)
    .collect()
}

/// Whether the action-element value `value` names at least ONE action of the service
/// whose namespace is `service_namespace`.
fn reaches_namespace(value: &str, service_namespace: &str) -> bool {
    match value.split_once(ACTION_SEPARATOR) {
        Some((namespace, _action)) => ::service_common::iam_authorization::wildcard_match(
            namespace,
            service_namespace,
            false,
        ),
        // Without a separator the only value that names an action is the
        // whole-element wildcard, which names every action of every service.
        None => value == ANY_ACTION,
    }
}

/// Whether the action-element value `value` names EVERY action of the service whose
/// namespace is `service_namespace` — what a `NotAction` value has to do to put the
/// whole service outside the statement's grant.
fn covers_namespace(value: &str, service_namespace: &str) -> bool {
    match value.split_once(ACTION_SEPARATOR) {
        Some((namespace, action)) => {
            action == ANY_ACTION
                && ::service_common::iam_authorization::wildcard_match(
                    namespace,
                    service_namespace,
                    false,
                )
        }
        None => value == ANY_ACTION,
    }
}

/// The most `(action, resource)` asks one delegation request's permission policy is
/// read as ([`delegated_asks`]).
///
/// Every document that reaches that reading came through an operation that bounds the
/// policy it carries, so this is a backstop such a document never meets: it keeps one
/// that somehow got past those from making a decision walk do work proportional to the
/// PRODUCT of a statement's two element lists. A document naming more asks than this is
/// read as one whose asks cannot be enumerated, which is what the reading answers for
/// every other document it cannot reduce to concrete asks.
const MAX_DELEGATED_ASKS: usize = 1_024;

/// Every `(action, resource)` pair the permission policy `document` of a delegation
/// request ASKS to have delegated, or `None` when the asks cannot be read out of it in
/// full.
///
/// A delegation request carries the permissions it asks for as an IAM policy document
/// (`DelegationRequest$PermissionPolicy`, "JSON content of the associated permission
/// policy of this delegation request"), so what it asks for is the set of
/// `(action, resource)` pairs its GRANTING statements name — the cartesian product of a
/// statement's action and resource elements, each written either as one string or as a
/// list of them ([`element_values`]). A `Deny` statement asks for nothing: it narrows
/// what the delegated session may do rather than naming a permission anybody must hold.
///
/// `None` is the answer for every document whose asks are not that set: one that is not
/// an IAM policy document at all, one that states no granting statement, one whose
/// statement is missing an element this needs, and one written with `NotAction`,
/// `NotResource` or a `Condition` — a complement names the permissions the statement
/// does NOT ask for and a condition narrows an ask to request states that are not
/// modelled here, so neither reduces to the concrete pairs a decision is made against.
/// `Statement` is read both as the list and as the single statement the IAM policy
/// grammar admits, as [`grants_service_access`] and [`context_keys_in_policies`] read
/// it.
pub fn delegated_asks(document: &str) -> Option<Vec<(String, String)>> {
    let parsed = ::serde_json::from_str::<::serde_json::Value>(document).ok()?;
    let statement = parsed.get("Statement")?;
    let statements = match statement.as_array() {
        Some(statements) => statements.as_slice(),
        None => ::std::slice::from_ref(statement),
    };

    let mut asks: Vec<(String, String)> = Vec::new();
    for statement in statements {
        let effect = statement.get("Effect").and_then(::serde_json::Value::as_str)?;
        if effect != EFFECT_ALLOW {
            continue;
        }
        if statement.get("NotAction").is_some()
            || statement.get("NotResource").is_some()
            || statement.get("Condition").is_some()
        {
            return None;
        }
        let actions = element_values(statement.get("Action")?);
        let resources = element_values(statement.get("Resource")?);
        if actions.is_empty() || resources.is_empty() {
            return None;
        }
        if asks.len() + actions.len() * resources.len() > MAX_DELEGATED_ASKS {
            return None;
        }
        for action in &actions {
            for resource in &resources {
                asks.push(((*action).to_string(), (*resource).to_string()));
            }
        }
    }
    (!asks.is_empty()).then_some(asks)
}

/// One entry of AWS's own catalog of managed policies — a policy AWS owns and
/// every account can read and attach, addressed under the
/// `arn:aws:iam::aws:policy/` namespace ([`AWS_MANAGED_POLICY_ACCOUNT`]).
///
/// An AWS-managed policy is not account state: no call creates or deletes one, so
/// an entry carries no counter and no minted id. Its identity is its path and
/// friendly name (which is what its ARN spells, [`Self::arn`]), its id is catalog
/// data fixed per entry ([`Self::policy_id`]), and its content is the ONE version
/// this emulator carries — the default version, reported under
/// [`FIRST_POLICY_VERSION_ID`].
///
/// AWS publishes a managed policy's own default version number (`v3`, `v12`, …) and
/// bumps it as it updates the policy; the number it is at right now is not something
/// this emulator can know, so the catalog promises only the invariant the API has:
/// `GetPolicy`'s `DefaultVersionId` names the version `GetPolicyVersion` serves.
/// AWS's friendly `Description` prose is not carried either — the catalog holds a
/// policy's identity and document, so `Description` is reported only for a policy
/// this ACCOUNT created with one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AwsManagedPolicy {
    /// The policy's path, beginning and ending with `/`: `/` for a policy at the
    /// root, `/service-role/` for one AWS files under that path.
    pub path: &'static str,
    /// The policy's friendly name, as the AWS Managed Policy Reference spells it.
    pub policy_name: &'static str,
    /// This entry's `PolicyId`, carried as catalog data beside the path, name and
    /// document.
    ///
    /// Stable and unique, the two properties `Policy$PolicyId` documents ("the stable
    /// and unique string identifying the policy"): it is fixed per entry, so every
    /// read in every run reports the same id for the same policy, and the suite
    /// asserts the catalog's ids are distinct. AWS's own `PolicyId` for a managed
    /// policy is not published in the reference the catalog copies from, so this is a
    /// value the EMULATOR assigns, of the modelled `idType` (len 16..128, `^[\w]+$`).
    /// Every one carries `_`, which the base32 alphabet a MINTED id is drawn from does
    /// not ([`unique_id`]), so a catalog id can never collide with the id the account's
    /// own `CreatePolicy` mints.
    pub policy_id: &'static str,
    /// The policy's default version document, an IAM JSON policy.
    pub document: &'static str,
}

impl AwsManagedPolicy {
    /// This entry's ARN: `arn:aws:iam::aws:policy<path><name>`.
    ///
    /// Built here rather than through `engine::emulator_arn` because that helper
    /// always writes the emulator's own account id into the account segment, and an
    /// AWS-managed policy is owned by AWS: its account segment is the literal
    /// [`AWS_MANAGED_POLICY_ACCOUNT`]. The rest of the ARN is the same grammar
    /// [`policy_arn`] builds and [`parse_policy_arn`] reads.
    pub fn arn(&self) -> String {
        format!(
            "arn:aws:iam::{AWS_MANAGED_POLICY_ACCOUNT}:policy{}{}",
            self.path, self.policy_name
        )
    }

    /// Whether this policy may be attached to an IAM user, group or role —
    /// `Policy$IsAttachable`, "specifies whether the policy can be attached to an IAM
    /// user, group, or role".
    ///
    /// AWS's catalog holds two kinds of policy. Most are published to be attached, and
    /// report `true`. The ones AWS files under the path it reserves for service-linked
    /// roles ([`SERVICE_LINKED_ROLE_PATH_PREFIX`]) belong to the service-linked role
    /// they were written for — "Amazon Web Services service role policies can only be
    /// attached to the service-linked role for that service"
    /// (`PolicyNotAttachableException`) — so no identity may take one as a permissions
    /// policy or as a permissions boundary, and they report `false`.
    ///
    /// DERIVED from the path rather than carried per entry, so the flag a reader reports
    /// and the refusal an attach or a boundary put answers
    /// ([`ComAmazonawsIamAwsidentitymanagementv20100508State::policy_is_attachable`])
    /// cannot disagree about one policy.
    pub fn is_attachable(&self) -> bool {
        !is_service_linked_role_path(self.path)
    }

    /// This entry as a managed policy record: the `Policy` metadata every reader
    /// reports, plus the single version `GetPolicyVersion` serves.
    ///
    /// The two counts are the caller's, not the catalog's — they are what the
    /// account's own attachments and permissions boundaries make them, which is why
    /// they are passed in (see
    /// [`ComAmazonawsIamAwsidentitymanagementv20100508State::resolve_policy`]).
    fn record(&self, attachment_count: i32, permissions_boundary_usage_count: i32) -> ManagedPolicy {
        let created = ::aws_smithy_types::DateTime::from_secs(AWS_MANAGED_POLICY_CREATE_DATE_SECS);
        let version = PolicyVersion {
            document: Some(self.document.to_string()),
            version_id: Some(FIRST_POLICY_VERSION_ID.to_string()),
            is_default_version: true,
            create_date: Some(created),
        };
        ManagedPolicy {
            policy: Policy {
                arn: Some(self.arn()),
                policy_id: Some(self.policy_id.to_string()),
                policy_name: Some(self.policy_name.to_string()),
                path: Some(self.path.to_string()),
                default_version_id: Some(FIRST_POLICY_VERSION_ID.to_string()),
                attachment_count: Some(attachment_count),
                permissions_boundary_usage_count: Some(permissions_boundary_usage_count),
                // "Specifies whether the policy can be attached to an IAM user,
                // group, or role" (`Policy$IsAttachable`): see [`Self::is_attachable`]
                // for the two kinds of entry AWS's catalog holds.
                is_attachable: self.is_attachable(),
                // See the type's documentation: AWS's prose is not carried.
                description: None,
                create_date: Some(created),
                update_date: Some(created),
                // An AWS-managed policy carries no tags: tagging is an account
                // operation on an account's own resource.
                tags: None,
            },
            versions: [(FIRST_POLICY_VERSION_ID.to_string(), version)]
                .into_iter()
                .collect(),
        }
    }
}

/// The AWS-managed policies this emulator carries, ordered by ARN.
///
/// AWS-managed policies "are created and managed by AWS" and exist in every
/// account, so a read of one is NOT a read of account state — which is why they
/// live in this constant rather than in
/// [`ComAmazonawsIamAwsidentitymanagementv20100508State::policies`], the account's
/// own policy store.
///
/// SOURCE: the AWS Managed Policy Reference ("AWS managed policies", IAM User
/// Guide), which publishes each policy's path, friendly name and default-version
/// document. AWS's catalog runs to well over a thousand policies and this is a
/// SUBSET of it — the policies the emulator's own suites and fixtures attach, plus
/// the account-wide and job-function ones a client is most likely to read, and the
/// service-linked-role policies that are published but NOT attachable
/// ([`AwsManagedPolicy::is_attachable`]), so both kinds of entry a client can meet
/// are discoverable here.
///
/// It also carries every AWS-managed policy ARN another service of this emulator
/// hands a caller as a recommendation: a control plane that names a policy ARN in a
/// response must be able to describe that ARN, so an identifier this emulator
/// publishes is never one its own API reports as missing. The EKS add-on catalog's
/// `podIdentityConfiguration[].recommendedManagedPolicies` is the one place that
/// does so today, and the five policies it names are entries here.
///
/// An `arn:aws:iam::aws:policy/` ARN this catalog does not carry resolves to no
/// policy, and a read addressed by such an ARN is refused as the missing entity
/// `NoSuchEntityException` names — the operations' modelled refusal — rather than
/// with a code none of them declares.
pub const AWS_MANAGED_POLICIES: &[AwsManagedPolicy] = &[
    AwsManagedPolicy {
        path: "/",
        policy_name: "AWSLakeFormationDataAdmin",
        policy_id: "ANPA_AWSLAKEFORMATIONDATAADMIN",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["lakeformation:*","cloudtrail:DescribeTrails","cloudtrail:LookupEvents","glue:GetDatabase","glue:GetDatabases","glue:CreateDatabase","glue:UpdateDatabase","glue:DeleteDatabase","glue:GetConnections","glue:SearchTables","glue:GetTable","glue:CreateTable","glue:UpdateTable","glue:DeleteTable","glue:GetTableVersions","glue:GetPartitions","glue:GetTables","glue:ListWorkflows","glue:BatchGetWorkflows","glue:DeleteWorkflow","glue:GetWorkflows","glue:GetTags","glue:TagResource","glue:UnTagResource","iam:ListUsers","iam:ListRoles","iam:GetRole","iam:GetRolePolicy","s3:ListAllMyBuckets","s3:GetBucketLocation"],"Resource":"*"},{"Effect":"Deny","Action":["lakeformation:PutDataLakeSettings"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "AWSLambdaExecute",
        policy_id: "ANPA_AWSLAMBDAEXECUTE",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["logs:*"],"Resource":"arn:aws:logs:*:*:*"},{"Effect":"Allow","Action":["s3:GetObject","s3:PutObject"],"Resource":"arn:aws:s3:::*"}]}"#,
    },
    // Recommended by the EKS add-on catalog for the `amazon-cloudwatch-observability`
    // add-on's `cloudwatch-agent` service account, beside
    // `CloudWatchAgentServerPolicy`.
    AwsManagedPolicy {
        path: "/",
        policy_name: "AWSXrayWriteOnlyAccess",
        policy_id: "ANPA_AWSXRAYWRITEONLYACCESS",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["xray:PutTraceSegments","xray:PutTelemetryRecords","xray:GetSamplingRules","xray:GetSamplingTargets","xray:GetSamplingStatisticSummaries"],"Resource":["*"]}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "AdministratorAccess",
        policy_id: "ANPA_ADMINISTRATORACCESS",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"*","Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonEC2ReadOnlyAccess",
        policy_id: "ANPA_AMAZONEC2READONLYACCESS",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"ec2:Describe*","Resource":"*"},{"Effect":"Allow","Action":"elasticloadbalancing:Describe*","Resource":"*"},{"Effect":"Allow","Action":["cloudwatch:ListMetrics","cloudwatch:GetMetricStatistics","cloudwatch:Describe*"],"Resource":"*"},{"Effect":"Allow","Action":"autoscaling:Describe*","Resource":"*"}]}"#,
    },
    // Recommended by the EKS add-on catalog for the `vpc-cni` add-on's `aws-node`
    // service account.
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonEKS_CNI_Policy",
        policy_id: "ANPA_AMAZONEKS_CNI_POLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:AssignPrivateIpAddresses","ec2:AttachNetworkInterface","ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeInstances","ec2:DescribeTags","ec2:DescribeNetworkInterfaces","ec2:DescribeInstanceTypes","ec2:DescribeSubnets","ec2:DetachNetworkInterface","ec2:ModifyNetworkInterfaceAttribute","ec2:UnassignPrivateIpAddresses"],"Resource":"*"},{"Effect":"Allow","Action":["ec2:CreateTags"],"Resource":["arn:aws:ec2:*:*:network-interface/*"]}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonS3FullAccess",
        policy_id: "ANPA_AMAZONS3FULLACCESS",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:*","s3-object-lambda:*"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonS3ReadOnlyAccess",
        policy_id: "ANPA_AMAZONS3READONLYACCESS",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:Get*","s3:List*","s3:Describe*","s3-object-lambda:Get*","s3-object-lambda:List*"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonSNSFullAccess",
        policy_id: "ANPA_AMAZONSNSFULLACCESS",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["sns:*"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "AmazonSQSFullAccess",
        policy_id: "ANPA_AMAZONSQSFULLACCESS",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["sqs:*"],"Resource":"*"}]}"#,
    },
    // Recommended by the EKS add-on catalog for the
    // `amazon-cloudwatch-observability` add-on's `cloudwatch-agent` service account.
    AwsManagedPolicy {
        path: "/",
        policy_name: "CloudWatchAgentServerPolicy",
        policy_id: "ANPA_CLOUDWATCHAGENTSERVERPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudwatch:PutMetricData","ec2:DescribeVolumes","ec2:DescribeTags","logs:PutLogEvents","logs:PutRetentionPolicy","logs:DescribeLogStreams","logs:DescribeLogGroups","logs:CreateLogStream","logs:CreateLogGroup","xray:PutTraceSegments","xray:PutTelemetryRecords","xray:GetSamplingRules","xray:GetSamplingTargets","xray:GetSamplingStatisticSummaries"],"Resource":"*"},{"Effect":"Allow","Action":["ssm:GetParameter"],"Resource":"arn:aws:ssm:*:*:parameter/AmazonCloudWatch-*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "CloudWatchLogsFullAccess",
        policy_id: "ANPA_CLOUDWATCHLOGSFULLACCESS",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["logs:*"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "IAMFullAccess",
        policy_id: "ANPA_IAMFULLACCESS",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["iam:*","organizations:DescribeAccount","organizations:DescribeOrganization","organizations:DescribeOrganizationalUnit","organizations:DescribePolicy","organizations:ListChildren","organizations:ListParents","organizations:ListPoliciesForTarget","organizations:ListRoots","organizations:ListPolicies","organizations:ListTargetsForPolicy"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "IAMReadOnlyAccess",
        policy_id: "ANPA_IAMREADONLYACCESS",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["iam:GenerateCredentialReport","iam:GenerateServiceLastAccessedDetails","iam:Get*","iam:List*","iam:SimulateCustomPolicy","iam:SimulatePrincipalPolicy"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/",
        policy_name: "PowerUserAccess",
        policy_id: "ANPA_POWERUSERACCESS",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","NotAction":["iam:*","organizations:*","account:*"],"Resource":"*"},{"Effect":"Allow","Action":["iam:CreateServiceLinkedRole","iam:DeleteServiceLinkedRole","iam:ListRoles","organizations:DescribeOrganization","account:ListRegions","account:GetAccountInformation"],"Resource":"*"}]}"#,
    },
    // The job-function policy a client most often resolves by name: "ReadOnlyAccess"
    // grants the read verbs of every service. AWS's own default version enumerates
    // them service by service and runs far past the managed-policy character quota
    // ([`MAX_MANAGED_POLICY_CHARACTERS`]), so the document carried here is an
    // EMULATOR-SCOPED approximation of it — the same read-only grant over the service
    // namespaces this emulator serves, and a subset of AWS's where a client reasons
    // about the grants `GetPolicyVersion` serves. The policy's identity — its path,
    // name, ARN and attachability — is AWS's.
    AwsManagedPolicy {
        path: "/",
        policy_name: "ReadOnlyAccess",
        policy_id: "ANPA_READONLYACCESS",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["cloudformation:Describe*","cloudformation:Get*","cloudformation:List*","cloudwatch:Describe*","cloudwatch:Get*","cloudwatch:List*","dynamodb:Describe*","dynamodb:Get*","dynamodb:List*","dynamodb:Query","dynamodb:Scan","ec2:Describe*","iam:Get*","iam:List*","iam:SimulateCustomPolicy","iam:SimulatePrincipalPolicy","kms:Describe*","kms:Get*","kms:List*","lambda:Get*","lambda:List*","logs:Describe*","logs:FilterLogEvents","logs:Get*","logs:List*","s3:Describe*","s3:Get*","s3:List*","sns:Get*","sns:List*","sqs:Get*","sqs:List*","sts:GetCallerIdentity"],"Resource":"*"}]}"#,
    },
    // The policies AWS files under the path it reserves for service-linked roles.
    // Each belongs to the service-linked role it was written for, so every one of
    // them reports `IsAttachable` false ([`AwsManagedPolicy::is_attachable`]) and no
    // identity can attach it or be bounded by it.
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AWSSupportServiceRolePolicy",
        policy_id: "ANPA_AWSSERVICEROLE_AWSSUPPORTSERVICEROLEPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["support:*","trustedadvisor:Describe*","trustedadvisor:Get*","trustedadvisor:List*","cloudformation:DescribeStacks","cloudwatch:DescribeAlarms","ec2:DescribeInstances","iam:GetRole","iam:ListRoles","s3:ListAllMyBuckets"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "AmazonEKSServiceRolePolicy",
        policy_id: "ANPA_AWSSERVICEROLE_AMAZONEKSSERVICEROLEPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateNetworkInterface","ec2:CreateNetworkInterfacePermission","ec2:DeleteNetworkInterface","ec2:DescribeInstances","ec2:DescribeNetworkInterfaces","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","ec2:DescribeVpcs","ec2:ModifyNetworkInterfaceAttribute","elasticloadbalancing:DescribeLoadBalancers","elasticloadbalancing:DescribeTargetGroups","iam:ListAttachedRolePolicies","kms:CreateGrant","kms:DescribeKey"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/aws-service-role/",
        policy_name: "ElasticLoadBalancingServiceRolePolicy",
        policy_id: "ANPA_AWSSERVICEROLE_ELASTICLOADBALANCINGSERVICEROLEPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:AssignIpv6Addresses","ec2:AssignPrivateIpAddresses","ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeAddresses","ec2:DescribeInstances","ec2:DescribeInternetGateways","ec2:DescribeNetworkInterfaces","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","ec2:DescribeVpcs","ec2:ModifyNetworkInterfaceAttribute","ec2:UnassignIpv6Addresses","ec2:UnassignPrivateIpAddresses","logs:CreateLogDelivery","logs:DeleteLogDelivery","logs:GetLogDelivery","logs:ListLogDeliveries","logs:UpdateLogDelivery"],"Resource":"*"}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSGlueServiceRole",
        policy_id: "ANPA_SERVICEROLE_AWSGLUESERVICEROLE",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["glue:*","s3:GetBucketLocation","s3:ListBucket","s3:ListAllMyBuckets","s3:GetBucketAcl","ec2:DescribeVpcEndpoints","ec2:DescribeRouteTables","ec2:CreateNetworkInterface","ec2:DeleteNetworkInterface","ec2:DescribeNetworkInterfaces","ec2:DescribeSecurityGroups","ec2:DescribeSubnets","ec2:DescribeVpcAttribute","iam:ListRolePolicies","iam:GetRole","iam:GetRolePolicy","cloudwatch:PutMetricData"],"Resource":["*"]},{"Effect":"Allow","Action":["s3:CreateBucket"],"Resource":["arn:aws:s3:::aws-glue-*"]},{"Effect":"Allow","Action":["s3:GetObject","s3:PutObject","s3:DeleteObject"],"Resource":["arn:aws:s3:::aws-glue-*/*","arn:aws:s3:::*/*aws-glue-*/*"]},{"Effect":"Allow","Action":["s3:GetObject"],"Resource":["arn:aws:s3:::crawler-public*","arn:aws:s3:::aws-glue-*"]},{"Effect":"Allow","Action":["logs:CreateLogGroup","logs:CreateLogStream","logs:PutLogEvents"],"Resource":["arn:aws:logs:*:*:/aws-glue/*"]}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AWSLambdaBasicExecutionRole",
        policy_id: "ANPA_SERVICEROLE_AWSLAMBDABASICEXECUTIONROLE",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["logs:CreateLogGroup","logs:CreateLogStream","logs:PutLogEvents"],"Resource":"*"}]}"#,
    },
    // Recommended by the EKS add-on catalog for the `aws-ebs-csi-driver` add-on's
    // `ebs-csi-controller-sa` service account. The conditions are the published
    // policy's: the driver may create and delete only the volumes and snapshots it
    // tagged for itself.
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonEBSCSIDriverPolicy",
        policy_id: "ANPA_SERVICEROLE_AMAZONEBSCSIDRIVERPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ec2:CreateSnapshot","ec2:AttachVolume","ec2:DetachVolume","ec2:ModifyVolume","ec2:DescribeAvailabilityZones","ec2:DescribeInstances","ec2:DescribeSnapshots","ec2:DescribeTags","ec2:DescribeVolumes","ec2:DescribeVolumesModifications"],"Resource":"*"},{"Effect":"Allow","Action":["ec2:CreateTags"],"Resource":["arn:aws:ec2:*:*:volume/*","arn:aws:ec2:*:*:snapshot/*"],"Condition":{"StringEquals":{"ec2:CreateAction":["CreateVolume","CreateSnapshot"]}}},{"Effect":"Allow","Action":["ec2:DeleteTags"],"Resource":["arn:aws:ec2:*:*:volume/*","arn:aws:ec2:*:*:snapshot/*"]},{"Effect":"Allow","Action":["ec2:CreateVolume"],"Resource":"*","Condition":{"StringLike":{"aws:RequestTag/ebs.csi.aws.com/cluster":"true"}}},{"Effect":"Allow","Action":["ec2:CreateVolume"],"Resource":"*","Condition":{"StringLike":{"aws:RequestTag/CSIVolumeName":"*"}}},{"Effect":"Allow","Action":["ec2:DeleteVolume"],"Resource":"*","Condition":{"StringLike":{"ec2:ResourceTag/ebs.csi.aws.com/cluster":"true"}}},{"Effect":"Allow","Action":["ec2:DeleteVolume"],"Resource":"*","Condition":{"StringLike":{"ec2:ResourceTag/CSIVolumeName":"*"}}},{"Effect":"Allow","Action":["ec2:DeleteSnapshot"],"Resource":"*","Condition":{"StringLike":{"ec2:ResourceTag/CSIVolumeSnapshotName":"*"}}},{"Effect":"Allow","Action":["ec2:DeleteSnapshot"],"Resource":"*","Condition":{"StringLike":{"ec2:ResourceTag/ebs.csi.aws.com/cluster":"true"}}}]}"#,
    },
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonECSTaskExecutionRolePolicy",
        policy_id: "ANPA_SERVICEROLE_AMAZONECSTASKEXECUTIONROLEPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["ecr:GetAuthorizationToken","ecr:BatchCheckLayerAvailability","ecr:GetDownloadUrlForLayer","ecr:BatchGetImage","logs:CreateLogStream","logs:PutLogEvents"],"Resource":"*"}]}"#,
    },
    // Recommended by the EKS add-on catalog for the `aws-efs-csi-driver` add-on's
    // `efs-csi-controller-sa` service account. The conditions are the published
    // policy's: the driver may create an access point only while tagging it for its
    // cluster, and may tag or delete only an access point already carrying that tag.
    AwsManagedPolicy {
        path: "/service-role/",
        policy_name: "AmazonEFSCSIDriverPolicy",
        policy_id: "ANPA_SERVICEROLE_AMAZONEFSCSIDRIVERPOLICY",
        document: r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["elasticfilesystem:DescribeAccessPoints","elasticfilesystem:DescribeFileSystems","elasticfilesystem:DescribeMountTargets","ec2:DescribeAvailabilityZones"],"Resource":"*"},{"Effect":"Allow","Action":["elasticfilesystem:CreateAccessPoint"],"Resource":"*","Condition":{"StringLike":{"aws:RequestTag/efs.csi.aws.com/cluster":"true"}}},{"Effect":"Allow","Action":"elasticfilesystem:TagResource","Resource":"*","Condition":{"StringLike":{"aws:ResourceTag/efs.csi.aws.com/cluster":"true"}}},{"Effect":"Allow","Action":"elasticfilesystem:DeleteAccessPoint","Resource":"*","Condition":{"StringEquals":{"aws:ResourceTag/efs.csi.aws.com/cluster":"true"}}}]}"#,
    },
];

/// The catalog entry `arn` names, or `None` when `arn` names no AWS-managed policy
/// this emulator carries.
///
/// The ARN is read through the one grammar this crate holds
/// ([`parse_policy_arn`]), so a value that is not a managed-policy ARN at all names
/// nothing here either; the account segment is what says the ARN is in AWS's
/// namespace rather than an account's, and the entry answers only when its own ARN
/// is exactly the one asked for — the same identity test
/// [`ComAmazonawsIamAwsidentitymanagementv20100508State::policy_by_arn`] applies to
/// the account's policies.
pub fn aws_managed_policy(arn: &str) -> Option<&'static AwsManagedPolicy> {
    let parsed = parse_policy_arn(arn)?;
    if parsed.account != AWS_MANAGED_POLICY_ACCOUNT {
        return None;
    }
    AWS_MANAGED_POLICIES
        .iter()
        .find(|entry| entry.path == parsed.path && entry.policy_name == parsed.policy_name)
        .filter(|entry| entry.arn() == arn)
}

/// Whether `arn` is an ARN in AWS's own managed-policy namespace — well formed and
/// carrying [`AWS_MANAGED_POLICY_ACCOUNT`] as its account — whether or not
/// [`AWS_MANAGED_POLICIES`] carries that policy.
///
/// This is what separates "AWS owns this policy" from "this account owns it": the
/// write operations refuse an AWS-owned ARN as an out-of-range value for their
/// parameter rather than as a missing entity, because no account can create, change
/// or delete a policy AWS owns.
pub fn is_aws_managed_policy_arn(arn: &str) -> bool {
    parse_policy_arn(arn).is_some_and(|parsed| parsed.account == AWS_MANAGED_POLICY_ACCOUNT)
}

/// The scaffold's 501 payload for a policy ATTACHED to an identity whose ARN is in
/// AWS's own managed-policy namespace and which [`AWS_MANAGED_POLICIES`] does not
/// carry — `what` naming the part of the policy the caller needed ("document").
///
/// NOT for an operation addressed by a `PolicyArn`: a read of the policy resource
/// itself resolves the ARN and refuses a miss with the `NoSuchEntityException` its
/// model declares ([`policy_not_found`]), because an operation may answer only the
/// errors it models. This payload is for the operations whose subject is an IDENTITY
/// that exists and whose attached grant the emulator holds no document for: nothing
/// they model says "a document this request needed is not available", so the honest
/// answer is that nobody has written that catalog entry down yet.
///
/// ONE wording, shared by those operations, so they cannot describe the same
/// unreadable grant three different ways.
pub fn uncarried_aws_managed_policy(arn: &str, what: &str) -> crate::scaffolded::NotImplemented {
    crate::scaffolded::NotImplemented {
        message: format!(
            "AWS-managed policy {arn} is not in this emulator's catalog of AWS-managed \
             policies, so its {what} cannot be reported; the policy is not claimed to \
             be absent"
        ),
    }
}

/// The message IAM refuses an operation addressed by `PolicyArn` with when the
/// account holds no managed policy under that ARN — the `NoSuchEntityException`
/// message, whose fixed text is `Policy ` and ` was not found.` around the ARN the
/// request supplied.
///
/// ONE wording, shared by every operation taking a `PolicyArn`, so a read, a tag call
/// and a delete cannot describe the same missing policy three different ways.
pub fn policy_not_found(arn: &str) -> String {
    format!("Policy {arn} was not found.")
}

/// The message IAM refuses an operation of the policy-ATTACHMENT family with when the
/// account holds no managed policy under the `PolicyArn` it was addressed by — the
/// `NoSuchEntityException` message, whose fixed text is `Policy ` and ` does not exist or
/// is not attachable.` around the ARN the request supplied.
///
/// The trailing clause is what separates that family — the operations whose subject is
/// whether a managed policy can be, or already is, attached to an entity (`AttachRolePolicy`,
/// `AttachUserPolicy`, `ListEntitiesForPolicy`) — from a plain read, write or delete of the
/// policy resource itself, which reports the same absence as [`policy_not_found`]. An
/// attachment call cannot tell "this ARN names nothing" from "it names something that
/// cannot take part in an attachment", and its wording says so.
pub fn policy_not_attachable(arn: &str) -> String {
    format!("Policy {arn} does not exist or is not attachable.")
}

/// The message IAM refuses an attach or a permissions-boundary put with when the policy
/// the `PolicyArn` resolves to EXISTS but reports `IsAttachable` false — the
/// `PolicyNotAttachableException` message, whose fixed text is `Policy ` and ` is not
/// attachable.` around the ARN the request supplied.
///
/// Distinct from [`policy_not_attachable`], which is the `NoSuchEntityException` text for
/// an ARN that resolves to NOTHING: here the policy is one the account can read, and what
/// the refusal says is that this policy may not take part in an attachment. The two names
/// are deliberately unalike, because the error classes are: picking one at a call site
/// picks the error code a client branches on. ONE wording, shared by the three attach calls
/// and the two boundary puts
/// ([`ComAmazonawsIamAwsidentitymanagementv20100508State::policy_is_attachable`] is the
/// one decision behind them), so a caller matching IAM's text sees one sentence whichever
/// operation it reached the policy through.
pub fn policy_cannot_be_attached(arn: &str) -> String {
    format!("Policy {arn} is not attachable.")
}

/// The message IAM refuses an operation addressed by a `PolicyArn` and a `VersionId`
/// with when the policy that ARN resolves to carries no such version — the
/// `NoSuchEntityException` message, whose fixed text is `Policy `, ` version ` and
/// ` does not exist or is not attachable.` around the two values the request supplied.
///
/// The closing clause is part of IAM's fixed text for a policy the request cannot be
/// served from, not a statement about this particular version: the same clause closes
/// the refusal of an attach or a permissions-boundary put naming a policy that cannot
/// be attached (`AttachUserPolicy`, `AttachRolePolicy`, `PutUserPermissionsBoundary`,
/// `PutRolePermissionsBoundary` in this crate all word it that way), so a caller
/// matching IAM's text sees one wording however it addressed the policy.
///
/// ONE wording, shared by every operation taking a policy version, so a read and a
/// delete cannot describe the same missing version two different ways.
pub fn policy_version_not_found(arn: &str, version_id: &str) -> String {
    format!("Policy {arn} version {version_id} does not exist or is not attachable.")
}

/// The message IAM refuses a permissions-boundary put with when the `PermissionsBoundary`
/// ARN names no policy the boundary could be set from — the fixed text `Scope ARN: ` and
/// ` does not exist or is not attachable.` around the ARN the request supplied.
///
/// What a boundary call names is a SCOPE on an entity's permissions rather than an
/// attachment of a policy to it, and IAM words the two differently: the attachment family
/// opens its sentence with `Policy ` ([`policy_not_attachable`]) and a boundary put with
/// `Scope ARN: `. ONE wording, shared by both boundary puts, so a caller matching IAM's
/// text sees one sentence whether it set the boundary of a role or of a user.
pub fn boundary_scope_not_attachable(arn: &str) -> String {
    format!("Scope ARN: {arn} does not exist or is not attachable.")
}

/// How IAM refuses a delete whose entity still holds an ATTACHED MANAGED policy: one
/// fixed sentence naming the class of subordinate entity to remove, and nothing of the
/// entity, the policy or how many there are.
///
/// Every delete-conflict refusal in this crate is one of the constants here, for the same
/// reason: IAM's `DeleteConflictException` text is fixed per class of blocking dependent
/// ("The error message describes these entities" — the CLASS, not their names), so a
/// client branching on the sentence to decide whether to detach, to delete or to remove a
/// membership and retry sees the same text whichever entity kind it addressed.
pub const DELETE_CONFLICT_DETACH_POLICIES: &str =
    "Cannot delete entity, must detach all policies first.";

/// How IAM refuses a delete whose entity still EMBEDS an inline policy.
pub const DELETE_CONFLICT_DELETE_POLICIES: &str =
    "Cannot delete entity, must delete policies first.";

/// How IAM refuses a delete blocked by an instance profile: the role is still in one, or
/// the instance profile still holds a role. One sentence for both, because what has to
/// change is the same binding.
pub const DELETE_CONFLICT_REMOVE_ROLES_FROM_INSTANCE_PROFILE: &str =
    "Cannot delete entity, must remove roles from instance profile first.";

/// How IAM refuses a delete blocked by a group membership.
pub const DELETE_CONFLICT_REMOVE_USERS_FROM_GROUP: &str =
    "Cannot delete entity, must remove users from group first.";

/// How IAM refuses a delete whose user still holds an access key.
pub const DELETE_CONFLICT_DELETE_ACCESS_KEYS: &str =
    "Cannot delete entity, must delete access keys first.";

/// How IAM refuses a delete whose user still holds a signing certificate.
pub const DELETE_CONFLICT_DELETE_SIGNING_CERTIFICATES: &str =
    "Cannot delete entity, must delete signing certificates first.";

/// How IAM refuses a delete whose user still has a console password.
pub const DELETE_CONFLICT_DELETE_LOGIN_PROFILE: &str =
    "Cannot delete entity, must delete login profile first.";

/// The message IAM refuses a MANAGED policy document past its size quota with — the fixed
/// text `Cannot exceed quota for PolicySize: ` and the quota itself.
///
/// A managed policy's document is measured against an ACCOUNT quota (`PolicySize`), so the
/// refusal names that quota and nothing of the policy, exactly as every other
/// `Cannot exceed quota for <quota>: <limit>` refusal in this crate does. An INLINE
/// policy's document is measured against the embedding entity instead, and is worded with
/// that entity ([`inline_policy_size_exceeded`]).
pub fn policy_size_quota_exceeded(quota: usize) -> String {
    format!("Cannot exceed quota for PolicySize: {quota}")
}

/// The message IAM refuses an INLINE policy document past the embedding entity's
/// policy-size quota with — the fixed text `Maximum policy size of `, ` bytes exceeded for
/// ` and the entity kind and name the request named.
///
/// The unit is `bytes`, which is IAM's own word in this refusal, and the entity is named
/// because the quota is the ENTITY's (a user's 2048, a role's 10240, a group's 5120)
/// rather than the account's.
pub fn inline_policy_size_exceeded(quota: usize, entity_kind: &str, entity_name: &str) -> String {
    format!("Maximum policy size of {quota} bytes exceeded for {entity_kind} {entity_name}")
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

/// One managed policy as a resource-LISTING operation reports it, for the same
/// reason [`listed_role`] exists.
///
/// `ListPolicies`' documentation: "IAM resource-listing operations return a subset of
/// the available attributes for the resource. For example, this operation does not
/// return tags, even though they are an attribute of the returned object. To view all
/// of the information for a customer manged policy, see GetPolicy." `Description` is
/// withheld on the same ground, stated by the member itself: "this element is included
/// in the response to the GetPolicy operation. It is not included in the response to
/// the ListPolicies operation" (`Policy$Description`). Every other member of the
/// record — including the two counts the resolution derives — is reported as it stands.
pub fn listed_policy(policy: &Policy) -> Policy {
    Policy {
        description: None,
        tags: None,
        ..policy.clone()
    }
}

/// One stored instance profile as a resource-LISTING operation reports it, for the
/// same reason [`listed_role`] exists.
///
/// `ListInstanceProfiles`' documentation: "IAM resource-listing operations return a
/// subset of the available attributes for the resource. For example, this operation
/// does not return tags, even though they are an attribute of the returned object. To
/// view all of the information for an instance profile, see GetInstanceProfile." Every
/// other member — including `Roles`, which the resolution fills from the roles the
/// account holds — is reported as it stands.
pub fn listed_instance_profile(instance_profile: InstanceProfile) -> InstanceProfile {
    InstanceProfile {
        tags: None,
        ..instance_profile
    }
}

/// A unique entity id: `prefix` followed by [`UNIQUE_ID_SUFFIX_LEN`] base32
/// characters that spell out `seq` mixed through SplitMix64.
///
/// The mix is a bijection on `u64` and the first [`BASE32_CHARACTERS_PER_WORD`]
/// suffix characters carry all 64 of its bits, so distinct `seq` values always
/// yield distinct ids. The remaining characters come from a second, independently
/// mixed word purely so the id has the shape and entropy of an AWS unique id
/// rather than a visible counter.
fn unique_id(prefix: &str, seq: u64) -> String {
    unique_id_of_length(prefix, seq, UNIQUE_ID_SUFFIX_LEN)
}

/// [`unique_id`] over a suffix of `suffix_len` characters, for an id whose modelled
/// length constraint asks for more or fewer than [`UNIQUE_ID_SUFFIX_LEN`].
///
/// Unique for the same reason: the first [`BASE32_CHARACTERS_PER_WORD`] characters
/// carry all 64 bits of the bijectively mixed counter, and every further word is
/// keyed to its own position so no two positions repeat one word's characters.
fn unique_id_of_length(prefix: &str, seq: u64, suffix_len: usize) -> String {
    // The same golden-ratio odd constant SplitMix64's step adds, used here to key each
    // word to the position it fills.
    const WORD_KEY: u64 = 0x9E37_79B9_7F4A_7C15;

    let mut id = String::with_capacity(prefix.len() + suffix_len);
    id.push_str(prefix);
    let mut word = 0;
    for i in 0..suffix_len {
        if i.is_multiple_of(BASE32_CHARACTERS_PER_WORD) {
            let position = (i / BASE32_CHARACTERS_PER_WORD) as u64;
            word = splitmix64(seq ^ position.wrapping_mul(WORD_KEY));
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

/// What the LOW word of a 128-bit job id is keyed with, so the two words of one id are
/// drawn from independent mixes of the same counter rather than repeating it
/// ([`report_job_id`]). The HIGH word carries all 64 bits of the counter's bijective mix,
/// which is what makes two distinct counter values yield two distinct ids.
const JOB_ID_LOW_WORD_KEY: u64 = 0xA076_1D64_78BD_642F;

/// The `JobId` of the report job at counter position `seq`.
///
/// `jobIDType` is exactly 36 characters, which is the 8-4-4-4-12 grouping of 128 bits AWS
/// reports a job id in, so the id is built as that grouping over two bijectively mixed
/// words of the counter ([`splitmix64`], [`JOB_ID_LOW_WORD_KEY`]).
///
/// ONE spelling, shared by every report job the account mints an id for, so a client
/// cannot tell which generate produced an id from its shape.
fn report_job_id(seq: u64) -> String {
    let high = splitmix64(seq);
    let low = splitmix64(seq ^ JOB_ID_LOW_WORD_KEY);
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        high >> 32,
        (high >> 16) & 0xFFFF,
        high & 0xFFFF,
        low >> 48,
        low & 0xFFFF_FFFF_FFFF
    )
}

/// How long after the request an Organizations access report's generation is due.
///
/// AWS publishes no duration for it — "this operation … might take a few minutes" is all
/// `GenerateOrganizationsAccessReport` says — so the span is the emulator's own, chosen
/// short enough for a caller to poll through and long enough that a caller reading straight
/// after the generate still sees the job `IN_PROGRESS`, which is the state AWS answers
/// there.
pub const ACCESS_REPORT_GENERATION_SECONDS: i64 = 3;

/// How many access-report jobs may be generating AT ONCE.
///
/// `ReportGenerationLimitExceededException` is "the maximum number of concurrent requests
/// for this account are already running", which is the quota this is: a caller that asks
/// for one report after another is refused with that modelled error rather than being
/// allowed to start work without limit. AWS publishes no figure for it, so the bound is
/// the emulator's own.
pub const MAX_CONCURRENT_ACCESS_REPORT_JOBS: usize = 5;

/// How many FINISHED report jobs of ONE kind the account keeps.
///
/// A report job is a record a caller can ask for again, so a finished one is kept — but not
/// forever: past this many the oldest are dropped, so the store cannot grow without end on
/// repeated generates. The emulator's own bound; AWS publishes no retention for a report.
pub const MAX_RETAINED_ACCESS_REPORT_JOBS: usize = 100;

/// The `JobStatus` the job whose generation is due at `completes` is in at `now`:
/// `IN_PROGRESS` until that instant, `COMPLETED` from it on.
///
/// The ONE reading of that clock, shared by every report job the account records, so the
/// status a read reports and the `JobCompletionDate` beside it cannot disagree.
pub fn report_job_status(
    completes: ::aws_smithy_types::DateTime,
    now: ::aws_smithy_types::DateTime,
) -> &'static str {
    if now.as_secs_f64() < completes.as_secs_f64() {
        JOB_STATUS_IN_PROGRESS
    } else {
        JOB_STATUS_COMPLETED
    }
}

/// The `JobStatus` an Organizations access-report job is in at `now`, read through the one
/// clock rule [`report_job_status`] states.
pub fn access_report_job_status(
    job: &OrganizationsAccessReportJob,
    now: ::aws_smithy_types::DateTime,
) -> &'static str {
    report_job_status(job.completes, now)
}

/// The message a read of a report job the account holds nothing under is refused with —
/// the `NoSuchEntityException` message, whose fixed text is `The job ID ` and ` cannot be
/// found.` around the id the request supplied.
///
/// ONE wording, shared by every reader of a report job, so two reads cannot describe the
/// same missing job differently.
pub fn report_job_not_found(job_id: &str) -> String {
    format!("The job ID {job_id} cannot be found.")
}

/// The `AccessAdvisorUsageGranularityType` value that asks for SERVICE data only: "if you
/// specify service-level granularity, this operation generates only service data"
/// (`GenerateServiceLastAccessedDetails$Granularity`), and the level an omitted
/// `Granularity` asks for. One of the two values the enum declares, which
/// `ops::types::ACCESS_ADVISOR_USAGE_GRANULARITY_TYPE_VALUES` records.
pub const USAGE_GRANULARITY_SERVICE_LEVEL: &str = "SERVICE_LEVEL";

/// The `AccessAdvisorUsageGranularityType` value that asks for action data beside the
/// service data: "if you specify action-level granularity, it generates service and action
/// data". See [`USAGE_GRANULARITY_SERVICE_LEVEL`].
pub const USAGE_GRANULARITY_ACTION_LEVEL: &str = "ACTION_LEVEL";

/// One service a service-last-accessed report covers: the namespace a policy names that
/// service's actions under, and the service's own name.
#[derive(Debug, Clone, Copy)]
pub struct ReportedService {
    /// "The namespace of the service in which access was attempted"
    /// (`ServiceLastAccessed$ServiceNamespace`): the service prefix an action element
    /// spells before the separator.
    pub service_namespace: &'static str,
    /// "The name of the service in which access was attempted"
    /// (`ServiceLastAccessed$ServiceName`): the service's published name.
    pub service_name: &'static str,
}

/// The services a service-last-accessed report reports on, ascending by namespace.
///
/// SOURCE: the Service Authorization Reference ("Actions, resources, and condition keys for
/// Amazon Web Services services"), which `ServiceLastAccessed$ServiceNamespace`'s own
/// documentation points at to learn a service's namespace — "choose the name of the service
/// to view details for that service. In the first paragraph, find the service prefix" — so
/// each row is one service of that reference: its prefix, and the name it is published
/// under.
///
/// A SUBSET of it, and that is what the report covers: IAM reports last accessed data only
/// for the services that report activity to it ("for more information about services and
/// actions for which action last accessed information is displayed, see IAM action last
/// accessed information services and actions", `GenerateServiceLastAccessedDetails`), so a
/// namespace outside this catalog is a service no entry is reported for rather than one
/// reported without a name. The rows are every namespace this emulator's own catalog of
/// AWS-managed policies ([`AWS_MANAGED_POLICIES`]) grants actions under, plus the services
/// a client's own policies are most likely to name.
///
/// Ascending by namespace, because "by default, the list is sorted by service namespace"
/// (`GetServiceLastAccessedDetails`), so the catalog's own order IS the reported order.
pub const REPORTED_SERVICES: &[ReportedService] = &[
    ReportedService { service_namespace: "account", service_name: "AWS Account Management" },
    ReportedService { service_namespace: "acm", service_name: "AWS Certificate Manager" },
    ReportedService { service_namespace: "apigateway", service_name: "Amazon API Gateway" },
    ReportedService { service_namespace: "athena", service_name: "Amazon Athena" },
    ReportedService { service_namespace: "autoscaling", service_name: "Amazon EC2 Auto Scaling" },
    ReportedService { service_namespace: "backup", service_name: "AWS Backup" },
    ReportedService { service_namespace: "batch", service_name: "AWS Batch" },
    ReportedService { service_namespace: "bedrock", service_name: "Amazon Bedrock" },
    ReportedService { service_namespace: "cassandra", service_name: "Amazon Keyspaces" },
    ReportedService { service_namespace: "cloudformation", service_name: "AWS CloudFormation" },
    ReportedService { service_namespace: "cloudfront", service_name: "Amazon CloudFront" },
    ReportedService { service_namespace: "cloudtrail", service_name: "AWS CloudTrail" },
    ReportedService { service_namespace: "cloudwatch", service_name: "Amazon CloudWatch" },
    ReportedService { service_namespace: "codebuild", service_name: "AWS CodeBuild" },
    ReportedService { service_namespace: "codecommit", service_name: "AWS CodeCommit" },
    ReportedService { service_namespace: "codepipeline", service_name: "AWS CodePipeline" },
    ReportedService { service_namespace: "config", service_name: "AWS Config" },
    ReportedService { service_namespace: "dynamodb", service_name: "Amazon DynamoDB" },
    ReportedService { service_namespace: "ec2", service_name: "Amazon Elastic Compute Cloud" },
    ReportedService { service_namespace: "ecr", service_name: "Amazon Elastic Container Registry" },
    ReportedService { service_namespace: "ecs", service_name: "Amazon Elastic Container Service" },
    ReportedService { service_namespace: "eks", service_name: "Amazon Elastic Kubernetes Service" },
    ReportedService { service_namespace: "elasticache", service_name: "Amazon ElastiCache" },
    ReportedService {
        service_namespace: "elasticbeanstalk",
        service_name: "AWS Elastic Beanstalk",
    },
    ReportedService {
        service_namespace: "elasticfilesystem",
        service_name: "Amazon Elastic File System",
    },
    ReportedService {
        service_namespace: "elasticloadbalancing",
        service_name: "Elastic Load Balancing",
    },
    ReportedService { service_namespace: "events", service_name: "Amazon EventBridge" },
    ReportedService { service_namespace: "firehose", service_name: "Amazon Data Firehose" },
    ReportedService { service_namespace: "glue", service_name: "AWS Glue" },
    ReportedService { service_namespace: "iam", service_name: "AWS Identity and Access Management" },
    ReportedService { service_namespace: "kinesis", service_name: "Amazon Kinesis" },
    ReportedService { service_namespace: "kms", service_name: "AWS Key Management Service" },
    ReportedService { service_namespace: "lakeformation", service_name: "AWS Lake Formation" },
    ReportedService { service_namespace: "lambda", service_name: "AWS Lambda" },
    ReportedService { service_namespace: "logs", service_name: "Amazon CloudWatch Logs" },
    ReportedService { service_namespace: "organizations", service_name: "AWS Organizations" },
    ReportedService { service_namespace: "rds", service_name: "Amazon Relational Database Service" },
    ReportedService { service_namespace: "redshift", service_name: "Amazon Redshift" },
    ReportedService { service_namespace: "route53", service_name: "Amazon Route 53" },
    ReportedService { service_namespace: "s3", service_name: "Amazon Simple Storage Service" },
    ReportedService {
        service_namespace: "s3-object-lambda",
        service_name: "Amazon S3 Object Lambda",
    },
    ReportedService { service_namespace: "sagemaker", service_name: "Amazon SageMaker" },
    ReportedService { service_namespace: "secretsmanager", service_name: "AWS Secrets Manager" },
    ReportedService { service_namespace: "ses", service_name: "Amazon Simple Email Service" },
    ReportedService { service_namespace: "sns", service_name: "Amazon Simple Notification Service" },
    ReportedService { service_namespace: "sqs", service_name: "Amazon Simple Queue Service" },
    ReportedService { service_namespace: "ssm", service_name: "AWS Systems Manager" },
    ReportedService { service_namespace: "states", service_name: "AWS Step Functions" },
    ReportedService { service_namespace: "sts", service_name: "AWS Security Token Service" },
    ReportedService { service_namespace: "support", service_name: "AWS Support" },
    ReportedService { service_namespace: "swf", service_name: "Amazon Simple Workflow Service" },
    ReportedService { service_namespace: "trustedadvisor", service_name: "AWS Trusted Advisor" },
    ReportedService { service_namespace: "xray", service_name: "AWS X-Ray" },
];

/// The catalog entry for `service_namespace`, or `None` when [`REPORTED_SERVICES`] carries
/// no service under that namespace.
///
/// A service prefix is published in lower case and an action element may spell it in any
/// (the comparison [`grants_service_access`] makes is case-insensitive too), so the lookup
/// folds case: two spellings of one namespace name one service.
pub fn reported_service(service_namespace: &str) -> Option<&'static ReportedService> {
    REPORTED_SERVICES
        .iter()
        .find(|service| service.service_namespace.eq_ignore_ascii_case(service_namespace))
}

/// The `jobStatusType` value of a job whose generation is not due yet.
pub const JOB_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// The `jobStatusType` value of a job whose generation is done.
pub const JOB_STATUS_COMPLETED: &str = "COMPLETED";

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

/// How many characters an account property key may carry, and the alphabet it may
/// be spelled in: `accountPropertyKeyType` is length 1..50 with pattern
/// `^[A-Za-z][A-Za-z0-9/_-]*$`
/// (`smithy_query(query="shape", shape="accountPropertiesMapType")`). A map KEY's
/// own constraints are outside the framework's constraint walk, so the operation
/// taking these keys is what takes them against the published bound.
pub const MAX_ACCOUNT_PROPERTY_KEY_CHARACTERS: usize = 50;

/// How many account-level properties one account may hold. AWS publishes no quota
/// for these properties, so this bound is the emulator's own: it is what stops the
/// property map growing without end on repeated writes of fresh keys, and it sits
/// far above any set of namespaces and properties the service configures (each
/// entry is itself bounded, by [`MAX_ACCOUNT_PROPERTY_KEY_CHARACTERS`] and the
/// published 1024-character value length). A write that would carry the account
/// past it is refused with the modelled `InvalidInputException` — this operation
/// models no limit error — and stores nothing; re-setting a property the account
/// already holds is never refused, because the bound is taken against what the
/// account would hold AFTER the write.
pub const MAX_ACCOUNT_PROPERTIES_PER_ACCOUNT: usize = 1_000;

/// Whether `key` is within the published length and alphabet of
/// `accountPropertyKeyType` — see [`MAX_ACCOUNT_PROPERTY_KEY_CHARACTERS`] for both
/// and for why this is checked here rather than by the constraint walk.
pub fn is_account_property_key_shape(key: &str) -> bool {
    (1..=MAX_ACCOUNT_PROPERTY_KEY_CHARACTERS).contains(&key.chars().count())
        && key.starts_with(|first: char| first.is_ascii_alphabetic())
        && key
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "/_-".contains(character))
}

/// The NAMESPACE an account property key names, or `None` when `key` is not in the
/// account property key format.
///
/// The one definition of that format: "each key uses the format
/// `Namespace/PropertyName`. The key must contain exactly one `/` separating the
/// namespace from the property name, and cannot start or end with `/`"
/// (`PutAccountPropertiesRequest$Properties`). So a key with no separator, with
/// more than one, or with either part empty names no property, and the namespace a
/// well-formed key names is the part before the separator — the value the
/// one-namespace-per-request rule is taken against.
pub fn account_property_namespace(key: &str) -> Option<&str> {
    let (namespace, property_name) = key.split_once('/')?;
    let names_one_property =
        !namespace.is_empty() && !property_name.is_empty() && !property_name.contains('/');
    names_one_property.then_some(namespace)
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

/// One PAGE of a tagged entity's tags, as an IAM tag-listing operation reports it:
/// the tags themselves, whether more remain, and the token that resumes after them.
#[derive(Debug)]
pub struct TagPage {
    /// The tags this page reports, sorted by tag key. Empty when the entity holds
    /// none, because "if no tags are attached to the specified resource, the response
    /// contains an empty list" (`ListSAMLProviderTags$Tags`).
    pub tags: Vec<Tag>,
    /// Whether "there are more items to return" (`$IsTruncated`).
    pub is_truncated: bool,
    /// The `Marker` to resume from, present only when the page is truncated: "when
    /// IsTruncated is true, this element is present" (`$Marker`).
    pub marker: Option<String>,
}

/// The page of the `held` tag set that follows `resume_after`, at most `page_size`
/// tags long, with markers minted in `scope`.
///
/// The rule every IAM tag-listing operation follows, in one place: "the returned list
/// of tags is sorted by tag key" (`ListSAMLProviderTags`), so the tag KEY is the page
/// order whatever order the entity's record holds its tags in, and a `Marker` names
/// the last key the previous page reported. The next page resumes STRICTLY after that
/// key, which stays well defined when the tag it named has been removed since: paging
/// resumes from the position, and the removed tag is simply no longer in the range.
pub fn tag_page(
    held: Option<&Vec<Tag>>,
    scope: &str,
    resume_after: Option<&str>,
    page_size: usize,
) -> TagPage {
    // A page holds at least one tag, so a truncated page always carries the key a
    // `Marker` can resume after: "when IsTruncated is true, this element is present"
    // (`$Marker`) cannot be honoured by a page of nothing. `maxItemsType`'s range floor
    // is 1, so no request reaches here asking for a smaller page.
    let page_size = page_size.max(1);
    let mut sorted: Vec<Tag> = held.cloned().unwrap_or_default();
    sorted.sort_by(|left, right| left.key.cmp(&right.key));

    let mut tags: Vec<Tag> = Vec::new();
    let mut is_truncated = false;
    for tag in sorted
        .into_iter()
        .filter(|tag| resume_after.is_none_or(|after| tag.key.as_str() > after))
    {
        if tags.len() == page_size {
            // At least one more tag follows, so the caller needs another page to see it.
            is_truncated = true;
            break;
        }
        tags.push(tag);
    }
    let marker = is_truncated
        .then(|| tags.last().map(|tag| encode_marker(scope, &tag.key)))
        .flatten();
    TagPage {
        tags,
        is_truncated,
        marker,
    }
}

/// The tag set an entity holding `held` carries after an untag call naming
/// `keys`.
///
/// The rule every IAM untag operation follows, in one place: the call "removes the
/// specified tags from the role" (`UntagRole`), so each held tag whose key is named
/// is dropped and every tag it does not name keeps its value and its position in
/// the held order. Keys compare exactly: "tag keys and values are case sensitive"
/// ("Tagging IAM resources", IAM User Guide), so a key spelled in another case
/// names a different tag. A named key the entity does not hold removes nothing —
/// these operations model no error for one — so the call is idempotent and naming
/// no keys at all leaves the held set as it was.
pub fn remove_tags<'a, I: IntoIterator<Item = &'a str>>(
    held: Option<&Vec<Tag>>,
    keys: I,
) -> Vec<Tag> {
    let removed: BTreeSet<&str> = keys.into_iter().collect();
    held.into_iter()
        .flatten()
        .filter(|tag| !removed.contains(tag.key.as_str()))
        .cloned()
        .collect()
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
    policy_document_defect(document).is_none()
}

/// WHY a value supplied where an IAM policy document is expected is not one.
///
/// IAM distinguishes the two, and words each with a sentence of its own
/// ([`Self::message`]): a text that is no JSON object at all failed to PARSE, while a
/// JSON object that carries no `Statement` parsed and described no statement. A client
/// matching the refusal's text is what the distinction is for — the first is a
/// serialization bug on the caller's side, the second a policy that would grant nothing —
/// so one sentence for both would lose it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDocumentDefect {
    /// The value does not parse as a JSON object: it is not JSON, or it is JSON that is
    /// not an object (a list, a number, a string), neither of which can carry the
    /// top-level elements of the IAM JSON policy grammar.
    Syntax,
    /// The value IS a JSON object, and carries no `Statement` element — the one element
    /// that grammar requires.
    NoStatements,
}

/// The fixed sentence IAM refuses a document that does not parse as a JSON object with.
pub const SYNTAX_ERRORS_IN_POLICY: &str = "Syntax errors in policy.";

/// The fixed sentence IAM refuses a JSON object carrying no `Statement` element with.
pub const POLICY_HAS_NO_STATEMENTS: &str = "Policy has no statements";

impl PolicyDocumentDefect {
    /// How IAM words this defect in the modelled `MalformedPolicyDocumentException`: a
    /// FIXED sentence per defect, carrying nothing of the document and nothing of the
    /// member that supplied it, so every operation taking a policy document refuses the
    /// same text for the same defect.
    pub fn message(self) -> &'static str {
        match self {
            Self::Syntax => SYNTAX_ERRORS_IN_POLICY,
            Self::NoStatements => POLICY_HAS_NO_STATEMENTS,
        }
    }
}

/// Why `document` is not an IAM policy document, or `None` when it is one.
///
/// The ONE reader of the rule [`is_policy_document`] states, so the refusal an operation
/// answers and the decision it answers it on cannot drift apart.
pub fn policy_document_defect(document: &str) -> Option<PolicyDocumentDefect> {
    match ::serde_json::from_str::<::serde_json::Value>(document) {
        Ok(::serde_json::Value::Object(elements)) => {
            (!elements.contains_key("Statement")).then_some(PolicyDocumentDefect::NoStatements)
        }
        Ok(_) | Err(_) => Some(PolicyDocumentDefect::Syntax),
    }
}

/// What IAM reads out of a SAML provider's metadata document.
///
/// `CreateSAMLProvider` has to READ the document rather than store it blindly, because
/// what a provider reports about itself comes from it: "that document includes the
/// issuer's name, expiration information, and keys" (`CreateSAMLProvider`), and the
/// expiration is what `GetSAMLProvider$ValidUntil` answers.
#[derive(Debug, Clone)]
pub struct SamlMetadata {
    /// The root descriptor's `validUntil`, when the document carries one: the instant
    /// the metadata — and so the provider — stops being valid.
    pub valid_until: Option<::aws_smithy_types::DateTime>,
}

/// Read `document` as a SAML 2.0 metadata document, or `None` when it is not one.
///
/// A metadata document is "an XML document generated by an identity provider (IdP) that
/// supports SAML 2.0" whose root is an entity (or entities) descriptor in the SAML
/// metadata namespace ([`SAML_METADATA_NAMESPACE`]) and which "includes the issuer's
/// name" — the entity descriptor's `entityID`
/// (`CreateSAMLProvider$SAMLMetadataDocument`, OASIS "Metadata for SAML V2.0" §2.3).
/// So a document that is not well-formed XML, is rooted at something else, or names no
/// issuer describes no identity provider, and the operation taking it is refused as
/// having been "supplied an invalid … value … for an input parameter"
/// (`InvalidInputException`).
///
/// Validation deliberately stops there: the keys and endpoints BELOW the descriptor are
/// not checked, so a 200 from an operation taking a metadata document is the claim
/// "this is SAML 2.0 metadata for an issuer", not "every binding in it is usable".
/// This is the ONE definition of the rule, shared by every operation that accepts a
/// metadata document.
///
/// WHICH of those it was is deliberately not reported: the refusal is
/// [`INVALID_SAML_METADATA_DOCUMENT`], one fixed sentence for every way a document can
/// fail to be SAML 2.0 metadata, so no reader of the document can put its own diagnostics
/// on the wire.
pub fn parse_saml_metadata(document: &str) -> Option<SamlMetadata> {
    let parsed = ::roxmltree::Document::parse(document).ok()?;
    let root = parsed.root_element();
    let in_metadata_namespace = |node: &::roxmltree::Node<'_, '_>, local: &str| {
        node.tag_name().namespace() == Some(SAML_METADATA_NAMESPACE)
            && node.tag_name().name() == local
    };
    // The descriptor that names the issuer: the root itself for a document describing
    // one entity, or the first entity descriptor nested in a group of them.
    let entity = if in_metadata_namespace(&root, SAML_ENTITY_DESCRIPTOR) {
        root
    } else if in_metadata_namespace(&root, SAML_ENTITIES_DESCRIPTOR) {
        root.descendants()
            .find(|node| in_metadata_namespace(node, SAML_ENTITY_DESCRIPTOR))?
    } else {
        return None;
    };
    if entity
        .attribute(SAML_ENTITY_ID_ATTRIBUTE)
        .is_none_or(str::is_empty)
    {
        return None;
    }
    // "Expiration information": `validUntil` on the root descriptor, or on the entity
    // descriptor itself when the root is a group that carries none. It is an
    // `xs:dateTime`, so it may state a UTC offset rather than `Z` (XML Schema Part 2,
    // §3.2.7) — read through the ONE reader every service with a text timestamp shares.
    let valid_until = match root
        .attribute(SAML_VALID_UNTIL_ATTRIBUTE)
        .or_else(|| entity.attribute(SAML_VALID_UNTIL_ATTRIBUTE))
    {
        None => None,
        Some(text) => Some(parse_xs_date_time(text)?),
    };
    Some(SamlMetadata { valid_until })
}

/// The message IAM refuses an operation taking a `SAMLMetadataDocument` with when the
/// value describes no SAML 2.0 identity provider ([`parse_saml_metadata`]) — the
/// `InvalidInputException` message, which is this ONE fixed sentence: it names neither the
/// input member, nor which of the document's properties was wrong, nor anything the reader
/// of the document observed about its text.
pub const INVALID_SAML_METADATA_DOCUMENT: &str = "Invalid SAML metadata document";

/// The instant an `xs:dateTime` names, or `None` when the text is not one.
///
/// The calendar conversion (month lengths, leap years, a fractional second) comes from
/// the maintained library, which reads the `Z` form; a value carrying the numeric UTC
/// offset `xs:dateTime` also admits is resolved by reading its civil part as UTC and
/// shifting by that offset, through the ONE reader of that offset every service with a
/// text timestamp shares. A value stating NO zone is local time of an undetermined zone
/// (XML Schema Part 2 §3.2.7), which is a form a well-formed document may use, so it is
/// read as UTC rather than refused.
fn parse_xs_date_time(text: &str) -> Option<::aws_smithy_types::DateTime> {
    let (civil, offset_secs) =
        ::service_common::rfc3339::split_utc_offset(text).unwrap_or((text, 0));
    let parsed = ::aws_smithy_types::DateTime::from_str(
        &format!("{civil}Z"),
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    Some(::aws_smithy_types::DateTime::from_secs_and_nanos(
        parsed.secs().checked_sub(offset_secs)?,
        parsed.subsec_nanos(),
    ))
}

/// The size a policy document is measured at against the policy-size quota of
/// whatever it is embedded in or held as ([`MAX_MANAGED_POLICY_CHARACTERS`],
/// [`MAX_USER_INLINE_POLICY_CHARACTERS`], [`MAX_ROLE_INLINE_POLICY_CHARACTERS`],
/// [`MAX_GROUP_INLINE_POLICY_CHARACTERS`]): its characters except whitespace,
/// because AWS "doesn't count whitespace when calculating the size of a policy
/// against this limitation" — one measure, so a document that fits a quota fits it
/// whichever operation stores it and however it is formatted.
pub fn policy_document_characters(document: &str) -> usize {
    document.chars().filter(|c| !c.is_whitespace()).count()
}

