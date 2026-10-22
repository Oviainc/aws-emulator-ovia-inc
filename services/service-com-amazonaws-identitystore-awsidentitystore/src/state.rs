// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-identitystore-awsidentitystore service.
//!
//! Besides the state struct itself this module holds the helpers the identity-store
//! handlers share — identifier resolution (every id member also accepts the
//! resource's ARN), ARN and id minting, the records the resources persist as, the
//! record-to-modelled-shape conversions, the list cursor codec and the modelled
//! failures — so the operation files agree on one implementation of each instead of
//! carrying a copy apiece that can drift.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    Address, ConflictException, Email, ExternalId, GroupMembership, MemberId, Name, PhoneNumber,
    Photo, ResourceNotFoundException, Role, ServiceQuotaExceededException,
};

/// The number of identity stores this emulator will hold at once.
///
/// `IdentityStoreId` is a free-form request member and a store springs into being the
/// first time a mutating operation names one (see
/// [`ComAmazonawsIdentitystoreAwsidentitystoreState::try_store_mut`]), so without a cap
/// a caller could multiply the per-request allocation by naming a fresh store every
/// time. AWS publishes no figure for how many identity stores an account reaches, so
/// the number itself is CHOSEN: its purpose is that the store map cannot grow without
/// bound, and that the bound is reached through the modelled
/// `ServiceQuotaExceededException` rather than by exhausting memory.
pub const MAX_IDENTITY_STORES: usize = 16;

/// The number of users one identity store will hold.
///
/// `CreateUser` models `ServiceQuotaExceededException` (HTTP 402) for "the request
/// would cause the number of users or groups in the identity store to exceed the
/// maximum allowed", which is exactly this bound. AWS's own figure is far larger than
/// anything an in-memory emulator should hold, so the number is CHOSEN; capacity is
/// released by `DeleteUser` once that operation is implemented.
pub const MAX_USERS_PER_STORE: usize = 500;

/// The number of groups one identity store will hold. Chosen for the same reason, and
/// answered through the same modelled error, as [`MAX_USERS_PER_STORE`]; capacity is
/// released by `DeleteGroup` once that operation is implemented.
pub const MAX_GROUPS_PER_STORE: usize = 500;

/// The number of group memberships one identity store will hold. CHOSEN for the same
/// reason as [`MAX_USERS_PER_STORE`]; capacity is released by `DeleteGroupMembership`.
pub const MAX_MEMBERSHIPS_PER_STORE: usize = 2_000;

/// The number of members one group will hold.
///
/// Besides bounding memory this bounds WORK under the single service-wide state mutex:
/// a group's membership ids live in an insertion-ordered `Vec` that
/// [`IdentityStoreData::membership_of`] and [`IdentityStoreData::remove_membership`]
/// scan linearly, so without a cap one caller's group size would set every other
/// caller's latency. CHOSEN, and answered through the modelled
/// `ServiceQuotaExceededException`.
pub const MAX_MEMBERSHIPS_PER_GROUP: usize = 200;

/// The page size a list operation uses when the caller sends no `MaxResults`: the
/// model's own declared default, shared by every `MaxResults` member of this service
/// (`@default: 100`, range 1..100 — `smithy_query(query="operation",operation="ListUsers")`,
/// `smithy_query(query="operation",operation="ListGroups")`,
/// `smithy_query(query="operation",operation="ListGroupMemberships")`).
pub const DEFAULT_MAX_RESULTS: i32 = 100;

/// The largest page a list operation will answer with: the upper bound of the modelled
/// `MaxResults` range (1..100).
pub const MAX_MAX_RESULTS: i32 = 100;

/// The only attribute path `ListGroups`'s (deprecated) `Filters` member searches.
///
/// `Filter$AttributePath` is "the attribute path that is used to specify which
/// attribute name to search … `DisplayName` is a valid attribute path for the
/// `ListGroups` API" (`smithy_query(query="doc",shape="Filter",member="AttributePath")`),
/// so a filter naming any other attribute is a syntax error in the request.
pub const GROUP_FILTER_ATTRIBUTE_PATH: &str = "DisplayName";

/// The only attribute path `ListUsers`'s (deprecated) `Filters` member searches.
///
/// `Filter$AttributePath` is "the attribute path that is used to specify which
/// attribute name to search … `UserName` is a valid attribute path for the `ListUsers`
/// API" (`smithy_query(query="doc",shape="Filter",member="AttributePath")`), so a
/// filter naming any other attribute is a syntax error in the request.
pub const USER_FILTER_ATTRIBUTE_PATH: &str = "UserName";

/// The `arnNamespace` / `endpointPrefix` of this service, from the model's
/// `aws.api#service` trait (`smithy_query(query="service_info")`).
pub const ARN_NAMESPACE: &str = "identitystore";

/// Which identity-store resource an id or ARN names. The modelled `ResourceArn`
/// pattern admits exactly these three resource kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    User,
    Group,
    Membership,
}

impl ResourceKind {
    /// The resource segment this kind carries in an ARN.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Group => "group",
            Self::Membership => "membership",
        }
    }

    /// The `ResourceType` enum value naming this kind in a
    /// `ResourceNotFoundException` (`RESOURCE_TYPE_VALUES`).
    pub fn resource_type(self) -> &'static str {
        match self {
            Self::User => "USER",
            Self::Group => "GROUP",
            Self::Membership => "GROUP_MEMBERSHIP",
        }
    }
}

/// The ARN of an identity-store resource: `arn:aws:identitystore:::<kind>/<id>`.
///
/// Deliberately NOT `engine::emulator_arn`: the modelled `ResourceArn` pattern
/// (`^arn:aws[a-z-]*:identitystore:::(user|group|membership)/…$`, from
/// `smithy_query(query="shape",shape="ResourceArn")`) leaves BOTH the region and the
/// account field empty, while `emulator_arn` always writes the emulator account id
/// into the fifth field. An identity STORE's ARN does carry the account, so that one
/// is built with the shared helper — see [`identity_store_arn`].
pub fn resource_arn(kind: ResourceKind, id: &str) -> String {
    format!("arn:aws:{ARN_NAMESPACE}:::{}/{id}", kind.as_str())
}

/// The ARN of an identity store: `arn:aws:identitystore::<account>:identitystore/<id>`,
/// the ARN form the modelled `IdentityStoreId` pattern admits. The account is the
/// emulator's canonical one and the region field is empty, so this is
/// `engine::emulator_arn` with an empty region.
pub fn identity_store_arn(identity_store_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        "",
        &format!("identitystore/{identity_store_id}"),
    )
}

/// The store key an `IdentityStoreId` member names.
///
/// The modelled pattern admits both the bare id (`d-0123456789`) and the store's ARN
/// (`arn:aws:identitystore::<account>:identitystore/d-0123456789`), and both name the
/// same store, so the ARN form is reduced to the id it ends with.
pub fn identity_store_key(identifier: &str) -> String {
    bare_id(identifier)
}

/// The resource key a `ResourceId` member (`UserId`, `GroupId`, `MembershipId`,
/// `MemberId.UserId`) names.
///
/// The modelled pattern admits both the bare id and the resource's ARN, and both name
/// the same resource, so the ARN form is reduced to the id it ends with. The id is a
/// hexadecimal UUID whose pattern accepts either letter case (`[A-Fa-f0-9]`), so it is
/// folded to lower case: `A1B2…` and `a1b2…` are the same resource.
pub fn resource_key(identifier: &str) -> String {
    bare_id(identifier)
}

/// The last `/`-separated segment of an ARN, or the whole value when it is not one,
/// folded to lower case.
fn bare_id(identifier: &str) -> String {
    let bare = if identifier.starts_with("arn:") {
        identifier.rsplit('/').next().unwrap_or(identifier)
    } else {
        identifier
    };
    bare.to_ascii_lowercase()
}

/// One stored user: every member `CreateUser` was given, plus the id, ARN and revision
/// the service minted for it, so a later `DescribeUser` reports exactly what was
/// stored instead of echoing a request or synthesising a profile.
#[derive(Debug, Clone)]
pub struct User {
    pub user_id: String,
    pub user_arn: String,
    pub revision: u64,
    pub user_name: Option<String>,
    pub name: Option<Name>,
    pub display_name: Option<String>,
    pub nick_name: Option<String>,
    pub profile_url: Option<String>,
    pub emails: Vec<Email>,
    pub addresses: Vec<Address>,
    pub phone_numbers: Vec<PhoneNumber>,
    pub user_type: Option<String>,
    pub title: Option<String>,
    pub preferred_language: Option<String>,
    pub locale: Option<String>,
    pub timezone: Option<String>,
    /// `UserStatus` is reported only once something sets it. `CreateUser` models no
    /// `UserStatus` member, so a freshly created user has none and the response omits
    /// it rather than inventing `ENABLED`.
    pub user_status: Option<String>,
    pub photos: Vec<Photo>,
    pub website: Option<String>,
    pub birthdate: Option<String>,
    pub roles: Vec<Role>,
    /// External ids are set by an identity provider's provisioning, which no modelled
    /// operation of this slice performs, so this stays empty until one does.
    pub external_ids: Vec<ExternalId>,
    pub extensions: HashMap<String, ::aws_smithy_types::Document>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The identity that created / last updated the record. The emulator does not
    /// resolve a caller to an identity-store principal, so these stay `None` rather
    /// than carrying a fabricated or credential-derived value; the members are
    /// optional in the model.
    pub created_by: Option<String>,
    pub updated_by: Option<String>,
}

impl User {
    /// The stored revision as the modelled `ResourceRevision` (pattern `^[0-9]+$`).
    pub fn revision(&self) -> String {
        self.revision.to_string()
    }

    /// Record that this user was modified at `at`: the next revision, and the new
    /// `UpdatedAt`.
    ///
    /// `User$Revision` is "the current revision of the user in the identity store. This
    /// value changes each time the user is modified", so every successful update moves
    /// it on.
    pub fn record_update(&mut self, at: SystemTime) {
        self.revision = self.revision.saturating_add(1);
        self.updated_at = at;
    }

    /// The value of the user attribute `attribute_path` names, for a `ListUsers`
    /// filter. `None` when the user does not carry that attribute.
    ///
    /// Matched case-insensitively for the same reason as [`Group::attribute`]: SCIM
    /// attribute names are case-insensitive (RFC 7643 §2.1).
    pub fn attribute(&self, attribute_path: &str) -> Option<&str> {
        if attribute_path.eq_ignore_ascii_case(USER_FILTER_ATTRIBUTE_PATH) {
            return self.user_name.as_deref();
        }
        None
    }

    /// This record as the modelled `User` a list reports.
    ///
    /// Every optional member is reported only when the record holds one, so a user
    /// stored without a profile member comes back without it rather than with an
    /// invented or empty value. `requested_extensions` is why `Extensions` travels in:
    /// the member is "a map of explicitly requested attribute extensions", so only the
    /// names the request asked for are reported.
    pub fn to_user(
        &self,
        identity_store_id: &str,
        requested_extensions: Option<&[String]>,
    ) -> crate::ops::types::User {
        crate::ops::types::User {
            identity_store_id: identity_store_id.to_string(),
            user_id: self.user_id.clone(),
            user_arn: self.user_arn.clone(),
            revision: self.revision(),
            user_name: self.user_name.clone(),
            external_ids: modeled_list(&self.external_ids),
            name: self.name.clone(),
            display_name: self.display_name.clone(),
            nick_name: self.nick_name.clone(),
            profile_url: self.profile_url.clone(),
            emails: modeled_list(&self.emails),
            addresses: modeled_list(&self.addresses),
            phone_numbers: modeled_list(&self.phone_numbers),
            user_type: self.user_type.clone(),
            title: self.title.clone(),
            preferred_language: self.preferred_language.clone(),
            locale: self.locale.clone(),
            timezone: self.timezone.clone(),
            user_status: self.user_status.clone(),
            photos: modeled_list(&self.photos),
            website: self.website.clone(),
            birthdate: self.birthdate.clone(),
            roles: modeled_list(&self.roles),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            created_by: self.created_by.clone(),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
            updated_by: self.updated_by.clone(),
            extensions: self.requested_extensions(requested_extensions),
        }
    }

    /// The extensions a `DescribeUser` naming `requested` must report: the stored
    /// entries under exactly those names.
    ///
    /// `DescribeUserRequest$Extensions` is "a collection of extension names indicating
    /// what extensions the service should retrieve alongside other user attributes",
    /// so a request that names none asks for none.
    pub fn requested_extensions(
        &self,
        requested: Option<&[String]>,
    ) -> Option<HashMap<String, ::aws_smithy_types::Document>> {
        let names = requested?;
        let selected: HashMap<String, ::aws_smithy_types::Document> = names
            .iter()
            .filter_map(|name| {
                self.extensions
                    .get(name)
                    .map(|value| (name.clone(), value.clone()))
            })
            .collect();
        (!selected.is_empty()).then_some(selected)
    }
}

/// One stored group: what `CreateGroup` was given plus the id, ARN and revision the
/// service minted for it.
#[derive(Debug, Clone)]
pub struct Group {
    pub group_id: String,
    pub group_arn: String,
    pub revision: u64,
    pub display_name: Option<String>,
    pub description: Option<String>,
    /// Empty for the same reason as [`User::external_ids`].
    pub external_ids: Vec<ExternalId>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub created_by: Option<String>,
    pub updated_by: Option<String>,
}

impl Group {
    /// The stored revision as the modelled `ResourceRevision` (pattern `^[0-9]+$`).
    pub fn revision(&self) -> String {
        self.revision.to_string()
    }

    /// This record as the modelled `Group` a list reports.
    ///
    /// Every optional member is reported only when the record holds one, so a group
    /// stored without a `Description` (or a `DisplayName`, or an external id) comes
    /// back without that member rather than with an invented or empty value.
    pub fn to_group(&self, identity_store_id: &str) -> crate::ops::types::Group {
        crate::ops::types::Group {
            identity_store_id: identity_store_id.to_string(),
            group_id: self.group_id.clone(),
            group_arn: self.group_arn.clone(),
            revision: self.revision(),
            display_name: self.display_name.clone(),
            external_ids: modeled_list(&self.external_ids),
            description: self.description.clone(),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
            created_by: self.created_by.clone(),
            updated_by: self.updated_by.clone(),
        }
    }

    /// The value of the group attribute `attribute_path` names, for a `ListGroups`
    /// filter. `None` when the group does not carry that attribute.
    ///
    /// The path is matched case-insensitively: SCIM attribute names are
    /// case-insensitive (RFC 7643 §2.1), which is also how `GetGroupId` reads its
    /// `UniqueAttribute` path.
    pub fn attribute(&self, attribute_path: &str) -> Option<&str> {
        if attribute_path.eq_ignore_ascii_case(GROUP_FILTER_ATTRIBUTE_PATH) {
            return self.display_name.as_deref();
        }
        None
    }
}

/// One stored group membership.
///
/// The member is held as the user id alone because the modelled `MemberId` union
/// declares exactly one variant, `UserId`
/// (`smithy_query(query="shape",shape="MemberId")`), so that id IS the whole member
/// identity; [`GroupMembershipRecord::modeled_member_id`] puts it back in the union.
#[derive(Debug, Clone)]
pub struct GroupMembershipRecord {
    pub membership_id: String,
    pub membership_arn: String,
    pub group_id: String,
    pub member_user_id: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl GroupMembershipRecord {
    /// The stored member as the modelled union.
    pub fn modeled_member_id(&self) -> MemberId {
        MemberId::UserId(self.member_user_id.clone())
    }

    /// This record as the modelled `GroupMembership` a list or describe reports.
    pub fn to_group_membership(&self, identity_store_id: &str) -> GroupMembership {
        GroupMembership {
            identity_store_id: identity_store_id.to_string(),
            membership_id: self.membership_id.clone(),
            membership_arn: self.membership_arn.clone(),
            group_id: Some(self.group_id.clone()),
            member_id: Some(self.modeled_member_id()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
            created_by: None,
            updated_by: None,
        }
    }
}

/// One identity store: its identity plus the users, groups and memberships it holds.
#[derive(Debug, Default)]
pub struct IdentityStoreData {
    pub identity_store_id: String,
    pub identity_store_arn: String,
    /// Users by `UserId`.
    pub users: HashMap<String, User>,
    /// `UserName` -> `UserId`. Backs `CreateUser`'s uniqueness claim and, in a later
    /// slice, `GetUserId` by `AlternateIdentifier`.
    pub user_name_index: HashMap<String, String>,
    /// Groups by `GroupId`.
    pub groups: HashMap<String, Group>,
    /// `DisplayName` -> `GroupId`, backing `CreateGroup`'s uniqueness claim.
    pub group_display_name_index: HashMap<String, String>,
    /// Memberships by `MembershipId`.
    pub memberships: HashMap<String, GroupMembershipRecord>,
    /// `GroupId` -> its membership ids in creation order, so a list's order — and
    /// therefore its pagination — is deterministic.
    pub memberships_by_group: HashMap<String, Vec<String>>,
    /// `UserId` -> the membership ids naming that user, ready for
    /// `ListGroupMembershipsForMember` / `IsMemberInGroups`.
    pub memberships_by_member: HashMap<String, Vec<String>>,
    /// Bumped per minted id, so two ids minted in the same clock tick still differ.
    id_counter: u64,
}

impl IdentityStoreData {
    /// An empty store with `identity_store_id`'s identity.
    fn new(identity_store_id: String) -> Self {
        Self {
            identity_store_arn: identity_store_arn(&identity_store_id),
            identity_store_id,
            ..Self::default()
        }
    }

    /// A fresh `[0-9a-f]{8}-[0-9a-f]{4}-…` identifier no resource in this store holds.
    ///
    /// The three resource kinds share one id space because they share one ARN space:
    /// the modelled `ResourceId` admits any of them, so an id handed out for a user is
    /// never also handed out for a group or a membership.
    pub fn mint_resource_id(&mut self) -> String {
        for _ in 0..1024 {
            self.id_counter = self.id_counter.wrapping_add(1);
            let candidate = uuid_from_seed(self.id_counter, now_nanos());
            if !self.holds_id(&candidate) {
                return candidate;
            }
        }
        // Bounded fallback, so a pathological clock can never spin here: these are
        // well-formed UUIDs too, and there are always more of them than this store
        // holds resources.
        (0u64..=self.resource_count() as u64)
            .map(|n| format!("00000000-0000-4000-8000-{n:012x}"))
            .find(|candidate| !self.holds_id(candidate))
            .unwrap_or_else(|| format!("00000000-0000-4000-8000-{:012x}", u64::MAX))
    }

    /// Whether any resource of this store already carries `id`.
    fn holds_id(&self, id: &str) -> bool {
        self.users.contains_key(id)
            || self.groups.contains_key(id)
            || self.memberships.contains_key(id)
    }

    /// How many resources this store holds, across the three kinds.
    fn resource_count(&self) -> usize {
        self.users.len() + self.groups.len() + self.memberships.len()
    }

    /// Whether another user fits inside [`MAX_USERS_PER_STORE`].
    pub fn has_user_capacity(&self) -> bool {
        self.users.len() < MAX_USERS_PER_STORE
    }

    /// Whether another group fits inside [`MAX_GROUPS_PER_STORE`].
    pub fn has_group_capacity(&self) -> bool {
        self.groups.len() < MAX_GROUPS_PER_STORE
    }

    /// Whether another membership of `group_id` fits inside both
    /// [`MAX_MEMBERSHIPS_PER_STORE`] and [`MAX_MEMBERSHIPS_PER_GROUP`].
    pub fn has_membership_capacity(&self, group_id: &str) -> bool {
        self.memberships.len() < MAX_MEMBERSHIPS_PER_STORE
            && self.members_of(group_id) < MAX_MEMBERSHIPS_PER_GROUP
    }

    /// How many members `group_id` has.
    pub fn members_of(&self, group_id: &str) -> usize {
        self.memberships_by_group
            .get(group_id)
            .map_or(0, Vec::len)
    }

    /// The group whose `DisplayName` is `display_name`, or `None` when this store
    /// holds no such group.
    ///
    /// Resolved through [`Self::group_display_name_index`] — the same index
    /// `CreateGroup` enforces its uniqueness claim with — so a name resolves to the
    /// one group that currently holds it and never to a stale one.
    pub fn group_by_display_name(&self, display_name: &str) -> Option<&Group> {
        let group_id = self.group_display_name_index.get(display_name)?;
        self.groups.get(group_id)
    }

    /// The group carrying the external id `issuer`/`id`, or `None` when this store
    /// holds no such group.
    ///
    /// An external id pair identifies at most one resource, so the scan's answer does
    /// not depend on the order the groups are walked in. No modelled operation of this
    /// slice writes a group's external ids (they come from an identity provider's
    /// provisioning — see [`Group::external_ids`]), so today this resolves nothing;
    /// it is the lookup, not a hard-coded refusal, so it starts answering the moment
    /// a provisioning path stores one.
    pub fn group_by_external_id(&self, issuer: &str, id: &str) -> Option<&Group> {
        self.groups.values().find(|group| {
            group
                .external_ids
                .iter()
                .any(|external| external.issuer == issuer && external.id == id)
        })
    }

    /// The membership of `group_id` by `member_user_id`, or `None` when that user is
    /// not a member of that group.
    pub fn membership_of(&self, group_id: &str, member_user_id: &str) -> Option<&GroupMembershipRecord> {
        self.memberships_by_group
            .get(group_id)?
            .iter()
            .filter_map(|id| self.memberships.get(id))
            .find(|membership| membership.member_user_id == member_user_id)
    }

    /// Store `membership` and index it by group and by member, in creation order.
    pub fn insert_membership(&mut self, membership: GroupMembershipRecord) {
        self.memberships_by_group
            .entry(membership.group_id.clone())
            .or_default()
            .push(membership.membership_id.clone());
        self.memberships_by_member
            .entry(membership.member_user_id.clone())
            .or_default()
            .push(membership.membership_id.clone());
        self.memberships
            .insert(membership.membership_id.clone(), membership);
    }

    /// Remove the membership `membership_id` names, with both of its indexes.
    /// `None` when the store holds no such membership.
    pub fn remove_membership(&mut self, membership_id: &str) -> Option<GroupMembershipRecord> {
        let membership = self.memberships.remove(membership_id)?;
        if let Some(ids) = self.memberships_by_group.get_mut(&membership.group_id) {
            ids.retain(|id| id != membership_id);
            if ids.is_empty() {
                self.memberships_by_group.remove(&membership.group_id);
            }
        }
        if let Some(ids) = self.memberships_by_member.get_mut(&membership.member_user_id) {
            ids.retain(|id| id != membership_id);
            if ids.is_empty() {
                self.memberships_by_member.remove(&membership.member_user_id);
            }
        }
        Some(membership)
    }

    /// This store's users in a deterministic order: when each was created, ties broken
    /// by `UserId`.
    ///
    /// The order is DERIVED from the records rather than held as a second index, so it
    /// cannot drift from `users`, and it is total — two users minted in the same clock
    /// tick still have different ids — which is what makes a paginated walk of the list
    /// stable across calls.
    pub fn users_in_order(&self) -> Vec<&User> {
        let mut users: Vec<&User> = self.users.values().collect();
        users.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.user_id.cmp(&right.user_id))
        });
        users
    }

    /// This store's groups in a stable listing order.
    ///
    /// Creation time first, then `GroupId` for two groups created inside one clock
    /// tick, so a listing — and therefore its pagination — is deterministic and a
    /// group keeps its place in the sequence across calls.
    pub fn groups_in_order(&self) -> Vec<&Group> {
        let mut groups: Vec<&Group> = self.groups.values().collect();
        groups.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.group_id.cmp(&right.group_id))
        });
        groups
    }

    /// The memberships of `group_id` in creation order.
    pub fn memberships_in_order(&self, group_id: &str) -> Vec<&GroupMembershipRecord> {
        self.memberships_by_group
            .get(group_id)
            .map(|ids| ids.iter().filter_map(|id| self.memberships.get(id)).collect())
            .unwrap_or_default()
    }
}

/// Per-service state for com-amazonaws-identitystore-awsidentitystore. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsIdentitystoreAwsidentitystoreState {
    /// Every identity store, keyed by its `IdentityStoreId`.
    pub stores: HashMap<String, IdentityStoreData>,
}

impl ComAmazonawsIdentitystoreAwsidentitystoreState {
    /// The store `identifier` names, created empty on first use; `None` when it does
    /// not exist and [`MAX_IDENTITY_STORES`] are already held.
    ///
    /// This API models no operation that creates an identity store — in AWS one comes
    /// with an IAM Identity Center instance — so a client has no provisioning call to
    /// make and a store springs into being the first time a mutating operation names
    /// it. `IdentityStoreId` is caller-chosen, so that vivification is CAPPED: past
    /// the cap a create is refused with the modelled quota error instead of allocating
    /// store number N+1. Reporting operations use [`Self::store`] instead, so a read
    /// never conjures one.
    pub fn try_store_mut(&mut self, identifier: &str) -> Option<&mut IdentityStoreData> {
        let key = identity_store_key(identifier);
        if !self.stores.contains_key(&key) && self.stores.len() >= MAX_IDENTITY_STORES {
            return None;
        }
        Some(
            self.stores
                .entry(key.clone())
                .or_insert_with(|| IdentityStoreData::new(key)),
        )
    }

    /// The store `identifier` names, or `None` when none was ever created.
    pub fn store(&self, identifier: &str) -> Option<&IdentityStoreData> {
        self.stores.get(&identity_store_key(identifier))
    }

    /// The store `identifier` names, for writing, or `None` when none was ever
    /// created.
    ///
    /// Unlike [`Self::try_store_mut`] this NEVER brings a store into being: an
    /// operation that changes or removes an existing resource has nothing to change in
    /// a store that holds none, so it answers `ResourceNotFoundException` instead of
    /// vivifying an empty store the caller never had.
    pub fn store_mut(&mut self, identifier: &str) -> Option<&mut IdentityStoreData> {
        self.stores.get_mut(&identity_store_key(identifier))
    }
}

/// The modelled form of a stored list: `None` when it is empty.
///
/// Every one of the user's lists is `length 1..` in the model, so a user carrying none
/// of them reports an absent member rather than an empty list the model would refuse.
pub fn modeled_list<T: Clone>(items: &[T]) -> Option<Vec<T>> {
    (!items.is_empty()).then(|| items.to_vec())
}

/// The `ResourceNotFoundException` for an identity store this emulator does not hold.
///
/// A store springs into being on its first mutating call (see
/// [`ComAmazonawsIdentitystoreAwsidentitystoreState::try_store_mut`]),
/// so an identifier no create has ever named holds nothing at all — and a reporting
/// operation says so with the modelled 404 rather than vivifying one. `IDENTITY_STORE`
/// is the `ResourceType` value for this kind (`RESOURCE_TYPE_VALUES`).
pub fn identity_store_not_found(identifier: &str) -> ResourceNotFoundException {
    let identity_store_id = identity_store_key(identifier);
    ResourceNotFoundException {
        resource_type: Some("IDENTITY_STORE".to_string()),
        resource_id: Some(identity_store_id.clone()),
        reason: None,
        message: Some(format!("identity store {identity_store_id} not found")),
        request_id: None,
    }
}

/// The `ResourceNotFoundException` for a resource this store does not hold.
pub fn not_found(kind: ResourceKind, resource_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        resource_type: Some(kind.resource_type().to_string()),
        resource_id: Some(resource_id.to_string()),
        reason: None,
        message: Some(format!(
            "{} {resource_id} not found in this identity store",
            kind.as_str()
        )),
        request_id: None,
    }
}

/// The `ResourceNotFoundException` for a group membership the named identity store does
/// not hold.
///
/// AWS's not-found message for this class is a FIXED sentence that names the identity
/// store ("pool") and the request member rather than the id that was probed:
/// `DescribeGroupMembership` on a membership id nothing holds answers `Group membership
/// not found for given pool and membershipId.` (recorded behaviour, repeated across
/// every recorded rejection of that class), so the message is a rule rather than prose
/// rebuilt from state. The modelled `ResourceType` / `ResourceId` members still report
/// which resource was asked for, so [`not_found`] supplies them.
pub fn membership_not_found(membership_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(MEMBERSHIP_NOT_FOUND_MESSAGE.to_string()),
        ..not_found(ResourceKind::Membership, membership_id)
    }
}

/// AWS's fixed `ResourceNotFoundException` message for a membership id the identity
/// store does not hold.
pub const MEMBERSHIP_NOT_FOUND_MESSAGE: &str =
    "Group membership not found for given pool and membershipId.";

/// The `ResourceNotFoundException` for a resource an ALTERNATE identifier names and
/// this store does not hold.
///
/// `ResourceId` is left unset rather than carrying what the caller sent: the modelled
/// member is a `ResourceId` (pattern
/// `^(arn:…)?([0-9a-f]{10}-|)[A-Fa-f0-9]{8}-…$`, from
/// `smithy_query(query="shape",shape="ResourceId")`), which an external id or a unique
/// attribute value is not. `subject` names the identifier that failed to resolve, not
/// its value — a `DisplayName` is `@sensitive`.
pub fn not_found_by_alternate_identifier(
    kind: ResourceKind,
    subject: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        resource_type: Some(kind.resource_type().to_string()),
        resource_id: None,
        reason: None,
        message: Some(format!(
            "no {} in this identity store matches the given {subject}",
            kind.as_str()
        )),
        request_id: None,
    }
}

/// The `ConflictException` for a request that would violate a uniqueness claim —
/// a second user with one `UserName`, a second group with one `DisplayName`, or a
/// membership that already exists.
///
/// `reason` is the modelled `ConflictExceptionReason` value for exactly that case
/// (`CONFLICT_EXCEPTION_REASON_VALUES`).
pub fn uniqueness_conflict(message: String) -> ConflictException {
    ConflictException {
        message: Some(message),
        request_id: None,
        reason: Some("UNIQUENESS_CONSTRAINT_VIOLATION".to_string()),
    }
}

/// The `ConflictException` for a conditional request whose `Revision` precondition no
/// longer holds: the caller's expected revision is not the one the store carries, so
/// something else changed the resource first.
///
/// `reason` is the modelled `ConflictExceptionReason` value for exactly that case
/// (`CONFLICT_EXCEPTION_REASON_VALUES`).
pub fn concurrent_modification(message: String) -> ConflictException {
    ConflictException {
        message: Some(message),
        request_id: None,
        reason: Some("CONCURRENT_MODIFICATION".to_string()),
    }
}

/// The modelled `ServiceQuotaExceededException` (HTTP 402) for a create that would
/// pass one of this emulator's capacity bounds: `subject` says which bound and what is
/// already held.
pub fn quota_exceeded(subject: String) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: Some(format!(
            "the request would exceed the maximum this emulator holds: {subject}"
        )),
        request_id: None,
    }
}

/// The cursor for a page that starts at the resource `resource_id` names.
///
/// Hex of the id rather than the id itself, so a caller cannot read structure into the
/// token and has nothing to build one from: a `NextToken` is opaque and only ever
/// travels back unchanged. Hex also satisfies the modelled `NextToken` pattern
/// `^[-a-zA-Z0-9+=/:_]*$`.
pub fn encode_cursor(resource_id: &str) -> String {
    ::hex::encode(resource_id.as_bytes())
}

/// The resource id a cursor minted by [`encode_cursor`] carries, or `None` when the
/// token was not minted by this service.
pub fn decode_cursor(token: &str) -> Option<String> {
    String::from_utf8(::hex::decode(token).ok()?).ok()
}

/// Now, in nanoseconds since the Unix epoch — only an id-minting seed.
fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos())
}

/// One RFC 9562 version-4-shaped UUID from `counter` and `nanos`.
///
/// The bytes come from SHA-256 of the two seeds (the `sha2` crate), so consecutive
/// counters produce unrelated ids, and the version and variant bits are then set so
/// the value is a well-formed random UUID — which is the form the modelled
/// `ResourceId` pattern demands.
fn uuid_from_seed(counter: u64, nanos: u128) -> String {
    use ::sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(counter.to_be_bytes());
    hasher.update(nanos.to_be_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let hex = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIdentitystoreAwsidentitystoreState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIdentitystoreAwsidentitystoreState>)
}
