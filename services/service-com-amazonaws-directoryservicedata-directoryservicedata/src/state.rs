// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-directoryservicedata-directoryservicedata service.
//!
//! Directory Service Data is the data plane of a Managed Microsoft AD directory: it
//! creates, reads and removes the directory's own objects — users, groups and the
//! memberships between them. Everything the handlers in `src/ops/` answer with is
//! computed from the records below, which are the ONE copy of each object:
//!
//! * [`Directory`] — one per `DirectoryId`, holding that directory's realm, its SID
//!   authority, its users and its groups. The directory itself is created by the `ds`
//!   control plane, which this data-plane API does not model, so the first write to a
//!   `DirectoryId` materialises the record (a read never does) — and only for a
//!   `DirectoryId` that plane recorded the account as holding
//!   ([`ad_directory_inventory`], which every request is gated on ahead of any handler
//!   by [`crate::service::authorization`]), so no record here names a directory the
//!   account does not have.
//! * [`User`] / [`Group`] — keyed by the case-folded `SAMAccountName`, because AD
//!   treats that name as case insensitive. A `BTreeMap` keyed that way is also the
//!   total order the paginated list operations index into, so a page boundary is
//!   stable across calls. A directory's groups include the built-in groups every AD
//!   domain is created with ([`builtin_groups`]), which is why its inventory is never
//!   empty.
//! * [`Membership`] — a group's members, plus the reverse index that answers "which
//!   groups is this member in" without walking every group.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Bound;
use std::time::{Duration, SystemTime};

mod builtin_groups;

use sha2::Digest;

use crate::ops::types::{AttributeValue, Member, ResourceNotFoundException, ValidationException};

/// The Kerberos realm (AD domain name) the emulator's directories are created in.
///
/// A directory's domain name is chosen when the `ds` control plane creates the
/// directory; this data-plane API has no operation that carries one, so the emulator
/// states it once here. Every realm-valued member answers from this, and it satisfies
/// `Realm`'s `^([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+[.]?$`.
pub const DEFAULT_REALM: &str = "corp.example.com";

/// The `GroupType` a group gets when `CreateGroup` names none.
///
/// `GroupSummary$GroupType` is `@required`, so a stored group always has one, and a
/// group Active Directory creates without being told otherwise is a security group.
pub const DEFAULT_GROUP_TYPE: &str = "Security";

/// The `GroupScope` a group gets when `CreateGroup` names none.
///
/// `GroupSummary$GroupScope` is `@required` for the same reason, and the scope
/// Active Directory gives a new group by default is global.
pub const DEFAULT_GROUP_SCOPE: &str = "Global";

/// The name of the group every user account of a Managed Microsoft AD domain
/// belongs to.
///
/// Active Directory makes this group — one of the groups a domain is created with
/// ([`builtin_groups`]) — the primary group of every user account, so a user is a
/// member of it from the moment it is created, without anything adding it.
pub const PRIMARY_USER_GROUP: &str = "Domain Users";

/// The first RID the directory hands out to an object it creates.
///
/// A Windows SID ends in a relative identifier (RID) that is unique within the
/// domain; Microsoft reserves RIDs below 1000 for the built-in principals, so a
/// newly created user or group gets 1000 or above.
pub const FIRST_RID: u64 = 1000;

/// The largest page the list operations serve, and the page they serve when the
/// request names no `MaxResults`: `MaxResults` is modelled `@range(min: 1, max: 250)`.
pub const MAX_PAGE_SIZE: i32 = 250;

/// How long a `ClientToken` keeps naming the result of the call that first used it.
///
/// `CreateUserRequest$ClientToken` documents it: "A client token is valid for 8 hours
/// after the first request that uses it completes. After 8 hours, any request with the
/// same client token is treated as a new request."
pub const IDEMPOTENCY_WINDOW: Duration = Duration::from_secs(8 * 60 * 60);

/// The `ValidationException` `Reason` for a `NextToken` this service did not issue.
/// One of `ValidationExceptionReason`'s modelled values.
pub const INVALID_NEXT_TOKEN: &str = "INVALID_NEXT_TOKEN";

/// `MemberType` for a member that is a user object.
pub const USER_MEMBER_TYPE: &str = "USER";

/// `MemberType` for a member that is a group object.
pub const GROUP_MEMBER_TYPE: &str = "GROUP";

/// The page kind [`encode_cursor`] stamps on a `ListUsers` token.
pub const USERS_PAGE: &str = "users";

/// The page kind [`encode_cursor`] stamps on a `ListGroups` token.
pub const GROUPS_PAGE: &str = "groups";

/// The page kind [`encode_cursor`] stamps on a `ListGroupMembers` token.
pub const GROUP_MEMBERS_PAGE: &str = "group-members";

/// The page kind [`encode_cursor`] stamps on a `ListGroupsForMember` token.
pub const GROUPS_FOR_MEMBER_PAGE: &str = "groups-for-member";

/// The page kind [`encode_cursor`] stamps on a `SearchUsers` token.
pub const SEARCH_USERS_PAGE: &str = "search-users";

/// The page kind [`encode_cursor`] stamps on a `SearchGroups` token.
pub const SEARCH_GROUPS_PAGE: &str = "search-groups";

/// The cursor format [`encode_cursor`] writes, so a token from an older encoding is
/// refused rather than misread.
const CURSOR_VERSION: &str = "v1";

/// The cursor field separator: ASCII unit separator, which no field can contain
/// because a decoded field is the remainder of the split.
const CURSOR_SEPARATOR: char = '\u{1f}';

/// The case-folded form of a `SAMAccountName`, which is how users, groups and
/// memberships are keyed: AD resolves an account name case insensitively.
pub fn fold(name: &str) -> String {
    name.to_lowercase()
}

// The attribute names each of a user's STRUCTURAL members answers to, [`fold`]ed
// because an AD attribute name is case insensitive: the attribute Active Directory
// holds the value in, and beside it the name the model gives the member where the two
// differ (`smithy_query(query="shape", shape="User")`: `SID`, `SAMAccountName`,
// `DistinguishedName`, `UserPrincipalName`, `EmailAddress`, `GivenName`, `Surname`,
// `Enabled`). One table, used both to match a search and to project its answer.

/// `sAMAccountName`, and `cn`: the directory creates the object as
/// `CN=<SAMAccountName>` (see [`Directory::distinguished_name`]), so its common name
/// is that same name.
const SAM_ACCOUNT_NAME_ATTRIBUTES: &[&str] = &["samaccountname", "cn"];

/// `objectSid` is the attribute an object's SID lives in.
const SID_ATTRIBUTES: &[&str] = &["objectsid", "sid"];

const DISTINGUISHED_NAME_ATTRIBUTES: &[&str] = &["distinguishedname"];

const USER_PRINCIPAL_NAME_ATTRIBUTES: &[&str] = &["userprincipalname"];

/// `mail` is the attribute an email address lives in.
const EMAIL_ADDRESS_ATTRIBUTES: &[&str] = &["mail", "emailaddress"];

const GIVEN_NAME_ATTRIBUTES: &[&str] = &["givenname"];

/// `sn` is the attribute a surname lives in.
const SURNAME_ATTRIBUTES: &[&str] = &["sn", "surname"];

// A GROUP's own structural members, the same way. Its naming members answer to the
// tables above — one object model, one set of naming attributes — and these two are
// the group's alone (`smithy_query(query="shape", shape="Group")`: `GroupType`,
// `GroupScope`).

/// Active Directory holds a group's type in its `groupType` attribute, which this API
/// models as its own member, so that member's name is the other name it answers to.
const GROUP_TYPE_ATTRIBUTES: &[&str] = &["grouptype"];

/// The same `groupType` attribute carries a group's scope, which this API likewise
/// models as a member of its own.
const GROUP_SCOPE_ATTRIBUTES: &[&str] = &["groupscope"];

/// One user object of a directory.
#[derive(Debug, Clone)]
pub struct User {
    /// The name as the create stored it; the map key is its folded form.
    pub sam_account_name: String,
    pub sid: String,
    pub distinguished_name: String,
    pub user_principal_name: String,
    pub email_address: Option<String>,
    pub given_name: Option<String>,
    pub surname: Option<String>,
    /// Whether the account is active. `DisableUser` is what clears it.
    pub enabled: bool,
    /// The `OtherAttributes` the create stored, keyed by the attribute's
    /// `LdapDisplayName` as given — AD attribute names are case insensitive, so a
    /// projection matches them case insensitively.
    pub other_attributes: BTreeMap<String, AttributeValue>,
    pub created_at: SystemTime,
}

impl User {
    /// The value this user holds for the attribute `folded_name` names, when that is
    /// one of its STRUCTURAL members.
    ///
    /// `Enabled` is not readable this way: Active Directory holds it as a bit of
    /// `userAccountControl`, not as a value a string can equal.
    fn structural_attribute(&self, folded_name: &str) -> Option<String> {
        let held = |names: &[&str]| names.contains(&folded_name);
        if held(SAM_ACCOUNT_NAME_ATTRIBUTES) {
            Some(self.sam_account_name.clone())
        } else if held(SID_ATTRIBUTES) {
            Some(self.sid.clone())
        } else if held(DISTINGUISHED_NAME_ATTRIBUTES) {
            Some(self.distinguished_name.clone())
        } else if held(USER_PRINCIPAL_NAME_ATTRIBUTES) {
            Some(self.user_principal_name.clone())
        } else if held(EMAIL_ADDRESS_ATTRIBUTES) {
            self.email_address.clone()
        } else if held(GIVEN_NAME_ATTRIBUTES) {
            self.given_name.clone()
        } else if held(SURNAME_ATTRIBUTES) {
            self.surname.clone()
        } else {
            None
        }
    }

    /// Whether this user is one a search for `search_string` over `names` reports.
    /// See [`object_matches_search`], the one rule every searched object is matched
    /// by.
    pub fn matches_search(&self, names: &[String], search_string: &str) -> bool {
        object_matches_search(names, search_string, &self.other_attributes, |folded| {
            self.structural_attribute(folded)
        })
    }

    /// The `User` object `SearchUsers` serves for a user the search matched.
    ///
    /// A search answers with the record a listing reports for that user — the
    /// [`Self::to_summary`] members — widened by the attributes the search NAMED: the
    /// projection is the directory's, not the client's, so an attribute nobody asked
    /// about is carried by no member of the answer.
    pub fn to_searched_user(&self, names: &[String]) -> crate::ops::types::User {
        crate::ops::types::User {
            // What every search result carries, named or not: the same projection
            // `ListUsers` serves.
            sid: Some(self.sid.clone()),
            sam_account_name: self.sam_account_name.clone(),
            given_name: self.given_name.clone(),
            surname: self.surname.clone(),
            enabled: Some(self.enabled),
            // What widens it: the attributes this search asked about.
            distinguished_name: names_include(names, DISTINGUISHED_NAME_ATTRIBUTES)
                .then(|| self.distinguished_name.clone()),
            user_principal_name: names_include(names, USER_PRINCIPAL_NAME_ATTRIBUTES)
                .then(|| self.user_principal_name.clone()),
            email_address: names_include(names, EMAIL_ADDRESS_ATTRIBUTES)
                .then(|| self.email_address.clone())
                .flatten(),
            other_attributes: self.projected_attributes(Some(names)),
        }
    }

    /// The `UserSummary` projection `ListUsers` serves.
    pub fn to_summary(&self) -> crate::ops::types::UserSummary {
        crate::ops::types::UserSummary {
            sid: self.sid.clone(),
            sam_account_name: self.sam_account_name.clone(),
            given_name: self.given_name.clone(),
            surname: self.surname.clone(),
            enabled: self.enabled,
        }
    }

    /// The attributes of this user that `names` asks for. See
    /// [`project_attributes`], which is the one projection rule every object's
    /// `OtherAttributes` is reported through.
    pub fn projected_attributes(
        &self,
        names: Option<&[String]>,
    ) -> Option<HashMap<String, AttributeValue>> {
        project_attributes(&self.other_attributes, names)
    }
}

/// One group object of a directory.
#[derive(Debug, Clone)]
pub struct Group {
    /// The name as the create stored it; the map key is its folded form.
    pub sam_account_name: String,
    pub sid: String,
    pub distinguished_name: String,
    pub group_type: String,
    pub group_scope: String,
    pub other_attributes: BTreeMap<String, AttributeValue>,
    /// This group's members, keyed by the folded member name — the order
    /// `ListGroupMembers` pages through.
    pub members: BTreeMap<String, Membership>,
}

impl Group {
    /// The value this group holds for the attribute `folded_name` names, when that is
    /// one of its STRUCTURAL members.
    fn structural_attribute(&self, folded_name: &str) -> Option<String> {
        let held = |names: &[&str]| names.contains(&folded_name);
        if held(SAM_ACCOUNT_NAME_ATTRIBUTES) {
            Some(self.sam_account_name.clone())
        } else if held(SID_ATTRIBUTES) {
            Some(self.sid.clone())
        } else if held(DISTINGUISHED_NAME_ATTRIBUTES) {
            Some(self.distinguished_name.clone())
        } else if held(GROUP_TYPE_ATTRIBUTES) {
            Some(self.group_type.clone())
        } else if held(GROUP_SCOPE_ATTRIBUTES) {
            Some(self.group_scope.clone())
        } else {
            None
        }
    }

    /// Whether this group is one a search for `search_string` over `names` reports.
    /// See [`object_matches_search`], the one rule every searched object is matched
    /// by.
    pub fn matches_search(&self, names: &[String], search_string: &str) -> bool {
        object_matches_search(names, search_string, &self.other_attributes, |folded| {
            self.structural_attribute(folded)
        })
    }

    /// The `Group` object `SearchGroups` serves for a group the search matched.
    ///
    /// A search answers with the record a listing reports for that group — the
    /// [`Self::to_summary`] members — widened by the attributes the search NAMED: the
    /// projection is the directory's, not the client's, so an attribute nobody asked
    /// about is carried by no member of the answer.
    pub fn to_searched_group(&self, names: &[String]) -> crate::ops::types::Group {
        crate::ops::types::Group {
            // What every search result carries, named or not: the same projection
            // `ListGroups` serves.
            sid: Some(self.sid.clone()),
            sam_account_name: self.sam_account_name.clone(),
            group_type: Some(self.group_type.clone()),
            group_scope: Some(self.group_scope.clone()),
            // What widens it: the attributes this search asked about.
            distinguished_name: names_include(names, DISTINGUISHED_NAME_ATTRIBUTES)
                .then(|| self.distinguished_name.clone()),
            other_attributes: self.projected_attributes(Some(names)),
        }
    }

    /// The `GroupSummary` projection the group listings serve.
    pub fn to_summary(&self) -> crate::ops::types::GroupSummary {
        crate::ops::types::GroupSummary {
            sid: self.sid.clone(),
            sam_account_name: self.sam_account_name.clone(),
            group_type: self.group_type.clone(),
            group_scope: self.group_scope.clone(),
        }
    }

    /// The attributes of this group that `names` asks for. See
    /// [`project_attributes`], which is the one projection rule every object's
    /// `OtherAttributes` is reported through.
    pub fn projected_attributes(
        &self,
        names: Option<&[String]>,
    ) -> Option<HashMap<String, AttributeValue>> {
        project_attributes(&self.other_attributes, names)
    }
}

/// Whether a search for `search_string` over the attributes `names` reports an object
/// holding `other_attributes`, whose structural members `structural` reads.
///
/// A match is an attribute the request NAMED whose value IS the search string:
/// nothing else about the object is looked at, no other attribute is consulted, and
/// the string is not treated as a prefix or a pattern. Names and values are both
/// compared case insensitively, which is the matching rule Active Directory applies to
/// an attribute name and to a string-valued attribute. A multi-valued attribute
/// matches when ANY of its values does. One rule, so a user and a group are searched
/// alike.
fn object_matches_search(
    names: &[String],
    search_string: &str,
    other_attributes: &BTreeMap<String, AttributeValue>,
    structural: impl Fn(&str) -> Option<String>,
) -> bool {
    let wanted = fold(search_string);
    names.iter().any(|name| {
        let folded_name = fold(name);
        structural(&folded_name)
            .into_iter()
            .chain(
                other_attributes
                    .iter()
                    .filter(|(held, _)| fold(held) == folded_name)
                    .flat_map(|(_, value)| attribute_value_strings(value)),
            )
            .any(|held| fold(&held) == wanted)
    })
}

/// Whether the attributes a search NAMED include one of `attributes`, the names a
/// structural member answers to — which is what widens the record the search reports.
fn names_include(names: &[String], attributes: &[&str]) -> bool {
    names
        .iter()
        .any(|name| attributes.contains(&fold(name).as_str()))
}

/// The attributes of a stored object that `names` asks for, matched case
/// insensitively (AD attribute names are case insensitive) and reported under the
/// names they are stored as.
///
/// `None` when nothing was asked for or nothing asked for is held: `Attributes` is
/// modelled `@length(min: 1, max: 25)`, so an empty map is not a value that member
/// can carry.
pub fn project_attributes(
    stored: &BTreeMap<String, AttributeValue>,
    names: Option<&[String]>,
) -> Option<HashMap<String, AttributeValue>> {
    let names = names?;
    let wanted: BTreeSet<String> = names.iter().map(|name| fold(name)).collect();
    let projected: HashMap<String, AttributeValue> = stored
        .iter()
        .filter(|(name, _)| wanted.contains(&fold(name)))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    (!projected.is_empty()).then_some(projected)
}


/// One member of a group: which object is in it, and what kind of object that is.
///
/// The member's SID is NOT copied here; it is read from the member's own record, so
/// one object has one SID. [`Directory::member_view`] does that read.
#[derive(Debug, Clone)]
pub struct Membership {
    /// The member's name as the `AddGroupMember` call gave it.
    pub member_name: String,
    /// One of `MemberType`'s modelled values.
    pub member_type: String,
}

/// One directory's objects, as this emulator persists them.
#[derive(Debug)]
pub struct Directory {
    /// The AD domain name of this directory. Every realm-valued member answers from
    /// it, and a request naming another realm is asking about a domain this
    /// directory holds no objects for.
    pub realm: String,
    /// `S-1-5-21-<three sub-authorities>`: the SID of this directory's domain, which
    /// every object's SID extends with its own RID.
    pub domain_sid: String,
    /// The next RID [`Directory::mint_sid`] hands out.
    pub next_rid: u64,
    pub users: BTreeMap<String, User>,
    pub groups: BTreeMap<String, Group>,
    /// Reverse index over [`Group::members`]: folded member name → the folded names
    /// of the groups holding it. Derived state, maintained only by
    /// [`Directory::add_member`], [`Directory::remove_member`] and
    /// [`Directory::delete_user`].
    groups_by_member: HashMap<String, BTreeSet<String>>,
}

impl Directory {
    /// A directory with no objects yet, whose SID authority is derived from
    /// `directory_id` so it is stable for that directory and distinct across
    /// directories.
    fn new(directory_id: &str) -> Self {
        let mut directory = Self {
            realm: DEFAULT_REALM.to_string(),
            domain_sid: domain_sid(directory_id),
            next_rid: FIRST_RID,
            users: BTreeMap::new(),
            groups: BTreeMap::new(),
            groups_by_member: HashMap::new(),
        };
        // A domain holds its built-in groups from the moment it exists, so they are
        // part of the record rather than something a reader adds.
        builtin_groups::seed(&mut directory);
        directory
    }

    /// Record `user_name` in the domain's primary user group.
    ///
    /// Called by the create that stores a user: a domain user is a member of that
    /// group from the moment it exists, so the membership is part of the create and
    /// not something a later read invents. The group itself comes with the directory
    /// record ([`builtin_groups`]), which is why no caller has to create it.
    pub fn join_primary_group(&mut self, user_name: &str) {
        let joined = self.add_member(
            &fold(PRIMARY_USER_GROUP),
            Membership {
                member_name: user_name.to_string(),
                member_type: USER_MEMBER_TYPE.to_string(),
            },
        );
        debug_assert!(
            joined,
            "the primary user group comes with the directory record"
        );
    }

    /// Mint the SID of a new object of this directory: the domain SID plus the next
    /// unused RID. Within `SID`'s `@length(min: 1, max: 256)`.
    pub fn mint_sid(&mut self) -> String {
        let rid = self.next_rid;
        self.next_rid = self.next_rid.saturating_add(1);
        format!("{}-{rid}", self.domain_sid)
    }

    /// The realm's `DC=` components: `corp.example.com` → `DC=corp,DC=example,DC=com`.
    fn domain_components(&self) -> String {
        self.realm
            .trim_end_matches('.')
            .split('.')
            .map(|label| format!("DC={label}"))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// The distinguished name of an object created in this directory's default
    /// container: `CN=<name>,CN=Users,DC=…`.
    pub fn distinguished_name(&self, sam_account_name: &str) -> String {
        self.distinguished_name_in("Users", sam_account_name)
    }

    /// The distinguished name of an object of this directory sitting in `container`:
    /// `CN=<name>,CN=<container>,DC=…`. An object's container is part of where it IS,
    /// so a group of the domain's `BUILTIN` authority does not report itself under the
    /// container a created object goes in.
    pub fn distinguished_name_in(&self, container: &str, sam_account_name: &str) -> String {
        format!(
            "CN={sam_account_name},CN={container},{}",
            self.domain_components()
        )
    }

    /// A user's `UserPrincipalName`: the account name in this directory's realm.
    pub fn user_principal_name(&self, sam_account_name: &str) -> String {
        format!("{sam_account_name}@{}", self.realm)
    }

    /// Whether any object of this directory already answers to `key`.
    ///
    /// AD requires a `sAMAccountName` to be unique across the domain's security
    /// principals, so a user and a group cannot share one.
    pub fn holds_object(&self, key: &str) -> bool {
        self.users.contains_key(key) || self.groups.contains_key(key)
    }

    /// Whether `group_key`'s group already holds `member_key`.
    pub fn holds_member(&self, group_key: &str, member_key: &str) -> bool {
        self.groups
            .get(group_key)
            .is_some_and(|group| group.members.contains_key(member_key))
    }

    /// Record `membership` in `group_key`'s group and in the reverse index.
    ///
    /// Answers `false` when `group_key` names no group, so a caller cannot record a
    /// membership the index would then outlive.
    pub fn add_member(&mut self, group_key: &str, membership: Membership) -> bool {
        let member_key = fold(&membership.member_name);
        let Some(group) = self.groups.get_mut(group_key) else {
            return false;
        };
        group.members.insert(member_key.clone(), membership);
        self.groups_by_member
            .entry(member_key)
            .or_default()
            .insert(group_key.to_string());
        true
    }

    /// Forget the membership of `member_key` in `group_key`'s group, on the group's
    /// record and in the reverse index at once, so the two sides of one membership
    /// never disagree about whether it is still held.
    ///
    /// Answers the membership that was removed, or `None` when no group of that name
    /// holds that member — which is how a caller tells a removal it performed from
    /// there being nothing to remove.
    pub fn remove_member(&mut self, group_key: &str, member_key: &str) -> Option<Membership> {
        let membership = self.groups.get_mut(group_key)?.members.remove(member_key)?;
        if let Some(groups) = self.groups_by_member.get_mut(member_key) {
            groups.remove(group_key);
            // An index entry naming no group would answer for a member in no group,
            // so the entry goes with the last membership it held.
            if groups.is_empty() {
                self.groups_by_member.remove(member_key);
            }
        }
        Some(membership)
    }

    /// The groups of this directory that hold `member_key`, in folded-group-name
    /// order — the same total order the group listings page through.
    ///
    /// Read through the reverse index, then confirmed against the group's own member
    /// set, so one membership is reported identically from either side.
    pub fn groups_of_member<'a>(
        &'a self,
        member_key: &'a str,
    ) -> impl Iterator<Item = (&'a String, &'a Group)> + 'a {
        self.groups_by_member
            .get(member_key)
            .into_iter()
            .flatten()
            .filter_map(move |group_key| {
                let group = self.groups.get(group_key)?;
                group
                    .members
                    .contains_key(member_key)
                    .then_some((group_key, group))
            })
    }

    /// The `Member` view of a membership: the member's own record is where its SID
    /// comes from. `None` when that record is gone, which is how a membership no
    /// delete reached would be reported.
    pub fn member_view(&self, membership: &Membership) -> Option<Member> {
        let key = fold(&membership.member_name);
        let sid = match membership.member_type.as_str() {
            USER_MEMBER_TYPE => self.users.get(&key)?.sid.clone(),
            GROUP_MEMBER_TYPE => self.groups.get(&key)?.sid.clone(),
            _ => return None,
        };
        Some(Member {
            sid,
            sam_account_name: membership.member_name.clone(),
            member_type: membership.member_type.clone(),
        })
    }

    /// Remove the user `key` names, and with it every membership that user held.
    ///
    /// `None` when no such user is stored — the caller answers
    /// `ResourceNotFoundException`.
    pub fn delete_user(&mut self, key: &str) -> Option<User> {
        let user = self.users.remove(key)?;
        for group_key in self.groups_by_member.remove(key).unwrap_or_default() {
            if let Some(group) = self.groups.get_mut(&group_key) {
                group.members.remove(key);
            }
        }
        Some(user)
    }
}

/// What a `ClientToken` the store has seen before says about the call carrying it.
#[derive(Debug)]
pub enum TokenClaim {
    /// The token is new (or none was given): do the work.
    Fresh,
    /// The same token already did this exact call: answer with its result rather
    /// than doing the work twice.
    Replayed(Option<String>),
    /// The same token was used for a DIFFERENT call inside the idempotency window,
    /// which `ConflictException` documents as a conflict.
    Conflict,
}

/// What a `ClientToken` named: the fingerprint of the call that used it, the SID that
/// call minted (if any), and when the token stops naming it.
#[derive(Debug)]
struct TokenRecord {
    fingerprint: String,
    sid: Option<String>,
    expires: SystemTime,
}

/// Per-service state for com-amazonaws-directoryservicedata-directoryservicedata. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsDirectoryservicedataDirectoryservicedataState {
    /// Every directory this emulator holds objects for, keyed by `DirectoryId`.
    pub directories: BTreeMap<String, Directory>,
    /// The `@idempotencyToken` ledger: a client token to the call it first served.
    client_tokens: HashMap<String, TokenRecord>,
}

impl ComAmazonawsDirectoryservicedataDirectoryservicedataState {
    /// The directory `directory_id` names, creating the record if this is the first
    /// write to it. Only a MUTATING operation may call this: a read uses
    /// [`Self::directories`] directly, so reporting never materialises a directory.
    ///
    /// One call materialises at most one directory, and a new record holds a fixed
    /// set of objects — the domain's built-in groups, a catalogue fixed at compile
    /// time — so nothing in a request decides how much a materialisation allocates.
    pub fn directory_for_write(&mut self, directory_id: &str) -> &mut Directory {
        self.directories
            .entry(directory_id.to_string())
            .or_insert_with(|| Directory::new(directory_id))
    }

    /// What `token` says about a call whose inputs fingerprint to `fingerprint`.
    ///
    /// Tokens whose 8-hour window has closed are forgotten first, which is both what
    /// the documented window means and what keeps this ledger bounded.
    pub fn claim_token(
        &mut self,
        token: Option<&str>,
        fingerprint: &str,
        now: SystemTime,
    ) -> TokenClaim {
        self.client_tokens.retain(|_, record| record.expires > now);
        let Some(token) = token else {
            return TokenClaim::Fresh;
        };
        match self.client_tokens.get(token) {
            Some(record) if record.fingerprint == fingerprint => {
                TokenClaim::Replayed(record.sid.clone())
            }
            Some(_) => TokenClaim::Conflict,
            None => TokenClaim::Fresh,
        }
    }

    /// Remember that `token` served this call, so the next 8 hours of replays answer
    /// with `sid` instead of doing the work again.
    pub fn record_token(
        &mut self,
        token: Option<String>,
        fingerprint: String,
        sid: Option<String>,
        now: SystemTime,
    ) {
        let Some(token) = token else {
            return;
        };
        self.client_tokens.insert(
            token,
            TokenRecord {
                fingerprint,
                sid,
                expires: now + IDEMPOTENCY_WINDOW,
            },
        );
    }
}

/// The domain SID of the directory `directory_id` names.
///
/// `S-1-5` is the NT authority and `21` the non-unique sub-authority every domain
/// SID carries; the three sub-authorities after it identify the domain. They are
/// derived from the directory id so that one directory always answers with one
/// domain SID and two directories never share one.
fn domain_sid(directory_id: &str) -> String {
    let high = fnv1a64(directory_id.as_bytes());
    let low = fnv1a64(&[directory_id.as_bytes(), &[0]].concat());
    // Each sub-authority is a 32-bit value, and 0 is not one a domain SID uses.
    let authority = |value: u64| (value as u32).max(1);
    format!(
        "S-1-5-21-{}-{}-{}",
        authority(high >> 32),
        authority(high),
        authority(low)
    )
}

/// FNV-1a over `bytes`: a stable, non-cryptographic digest, used only to spread a
/// directory id across the three sub-authorities of its domain SID.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// A digest of the inputs of one call, for comparing a `ClientToken` replay against
/// the call that token first served.
///
/// Length-prefixed so two different field splits cannot digest alike, and hashed so
/// the ledger holds no copy of the (sensitive) values themselves.
pub fn fingerprint(fields: &[&str]) -> String {
    let mut hasher = sha2::Sha256::new();
    for field in fields {
        hasher.update(u64::try_from(field.len()).unwrap_or(u64::MAX).to_be_bytes());
        hasher.update(field.as_bytes());
    }
    hex::encode(hasher.finalize())
}

/// The `OtherAttributes` of a request, in a form [`fingerprint`] can digest: names
/// folded (AD treats them case insensitively) and in a stable order.
pub fn attributes_fingerprint(attributes: Option<&HashMap<String, AttributeValue>>) -> String {
    let normalized: BTreeMap<String, String> = attributes
        .into_iter()
        .flatten()
        .map(|(name, value)| (fold(name), format!("{value:?}")))
        .collect();
    format!("{normalized:?}")
}

/// The values an attribute holds, as a search compares them.
///
/// One string per value, because a multi-valued attribute (`SS`) matches when any of
/// its values does; a number or a boolean is compared as the directory renders it.
pub fn attribute_value_strings(value: &AttributeValue) -> Vec<String> {
    match value {
        AttributeValue::S(text) => vec![text.clone()],
        AttributeValue::N(number) => vec![number.to_string()],
        AttributeValue::Bool(flag) => vec![flag.to_string()],
        AttributeValue::Ss(values) => values.clone(),
    }
}

/// The attributes of a request, stored in the order a directory reports them.
pub fn stored_attributes(
    attributes: Option<HashMap<String, AttributeValue>>,
) -> BTreeMap<String, AttributeValue> {
    attributes.into_iter().flatten().collect()
}

/// The scope a pagination cursor is stamped with: everything about the request that
/// decides WHICH records a listing walks, so a token cannot be spent on another one.
pub fn list_scope(parts: &[&str]) -> String {
    // A record separator, distinct from the cursor's own field separator.
    parts.join("\u{1e}")
}

/// An opaque cursor naming the key a page resumes AFTER.
///
/// `kind` and `scope` are carried so a token issued for one listing cannot be spent
/// on another, and the whole body is stamped with a digest of itself, so a token that
/// was edited, truncated or built by a client is refused instead of being read as a
/// position in the walk. Rendered as hex: a client has no business reading one.
pub fn encode_cursor(kind: &str, scope: &str, after: &str) -> String {
    let body = format!(
        "{CURSOR_VERSION}{CURSOR_SEPARATOR}{kind}{CURSOR_SEPARATOR}{scope}{CURSOR_SEPARATOR}{after}"
    );
    hex::encode(format!(
        "{}{CURSOR_SEPARATOR}{body}",
        cursor_digest(&body)
    ))
}

/// The key a page resumes after, from a token this service issued for this `kind`
/// and `scope`. `None` for anything else — the caller refuses it with
/// `ValidationException`.
pub fn decode_cursor(token: &str, kind: &str, scope: &str) -> Option<String> {
    let text = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let (digest, body) = text.split_once(CURSOR_SEPARATOR)?;
    if digest != cursor_digest(body) {
        return None;
    }
    let mut fields = body.splitn(4, CURSOR_SEPARATOR);
    if fields.next()? != CURSOR_VERSION || fields.next()? != kind || fields.next()? != scope {
        return None;
    }
    Some(fields.next()?.to_string())
}

/// The digest [`encode_cursor`] stamps a cursor's body with.
///
/// This is an integrity check, not a signature: it is what tells an edited or
/// truncated token from one this service issued, and nothing in the emulator depends
/// on a cursor being unforgeable.
fn cursor_digest(body: &str) -> String {
    let digest = sha2::Sha256::digest(body.as_bytes());
    hex::encode(&digest[..8])
}

/// The page size a list request asks for: its `MaxResults`, or the largest page when
/// it names none.
///
/// `MaxResults` is modelled `@range(min: 1, max: 250)` and the boundary validator
/// refused anything outside that before the handler ran; the clamp states the
/// invariant rather than repairing a caller, and the debug assertion surfaces a
/// validator that let one through.
pub fn page_size(max_results: Option<i32>) -> usize {
    let requested = max_results.unwrap_or(MAX_PAGE_SIZE);
    debug_assert!(
        (1..=MAX_PAGE_SIZE).contains(&requested),
        "MaxResults {requested} should have been refused at the boundary"
    );
    usize::try_from(requested.clamp(1, MAX_PAGE_SIZE)).expect("a page size clamped into 1..=250")
}

/// One page of a keyed set of records, as a list operation reports it.
#[derive(Debug)]
pub struct Page<'a, V> {
    /// The records this page reports, in key order.
    pub records: Vec<&'a V>,
    /// The key the next page resumes after, set only when another record follows the
    /// last one reported — which is exactly when the answer carries a `NextToken`.
    pub resume_after: Option<String>,
}

impl<'a, V> Page<'a, V> {
    /// The page of a set this request reports nothing from: no records, no next page.
    pub fn empty() -> Self {
        Self {
            records: Vec::new(),
            resume_after: None,
        }
    }
}

/// The page of `records` that follows the key `after`, at most `page_size` long.
///
/// The walk SEEKS to `after` in the map's order rather than scanning past the records
/// earlier pages reported, and stops at the first record beyond the page, so one call
/// reads at most `page_size + 1` records — bounded by `MaxResults`'s modelled ceiling
/// however many records the set holds, and so is the page it allocates.
pub fn page_of<'a, V>(
    records: &'a BTreeMap<String, V>,
    page_size: usize,
    after: Option<&str>,
) -> Page<'a, V> {
    let start = match after {
        // Strictly after the key the previous page ended on, so a record deleted
        // meanwhile cannot hide the records that follow it.
        Some(after) => Bound::Excluded(after),
        None => Bound::Unbounded,
    };
    let mut page = Page::empty();
    let mut last_key: Option<&String> = None;
    for (index, (key, record)) in records
        .range::<str, _>((start, Bound::Unbounded))
        .enumerate()
    {
        if index == page_size {
            // One record past the page is what proves another follows it.
            page.resume_after = last_key.cloned();
            break;
        }
        page.records.push(record);
        last_key = Some(key);
    }
    page
}

/// True when a request's `Realm` names the realm of the directory it is addressed to.
///
/// `Realm` is optional and "when no value is defined, only your Managed Microsoft AD
/// users are returned", so naming none means the directory's own realm; the value is
/// case insensitive, and a realm pattern admits ASCII only.
pub fn realm_matches(requested: Option<&str>, directory_realm: &str) -> bool {
    requested.is_none_or(|realm| realm.eq_ignore_ascii_case(directory_realm))
}

/// The realm a call reports: the one it named, or the directory's own.
pub fn resolved_realm(requested: Option<&str>, directory_realm: &str) -> String {
    requested.unwrap_or(directory_realm).to_string()
}

/// The `ResourceNotFoundException` payload for a user no directory holds.
pub fn user_not_found(directory_id: &str, sam_account_name: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "user {sam_account_name} was not found in directory {directory_id}"
        )),
    }
}

/// The `ResourceNotFoundException` payload for a group no directory holds.
pub fn group_not_found(directory_id: &str, sam_account_name: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "group {sam_account_name} was not found in directory {directory_id}"
        )),
    }
}

/// The `ResourceNotFoundException` payload for a member no directory holds.
pub fn member_not_found(directory_id: &str, member_name: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "member {member_name} was not found in directory {directory_id}"
        )),
    }
}

/// The `ResourceNotFoundException` payload for a membership a group does not hold.
///
/// The membership is the resource a removal names, so a group that does not hold the
/// member has nothing of that name to remove.
pub fn membership_not_found(
    directory_id: &str,
    group_name: &str,
    member_name: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "member {member_name} is not a member of group {group_name} in directory {directory_id}"
        )),
    }
}

/// The `ValidationException` payload for a `NextToken` this service did not issue for
/// the listing it was spent on.
pub fn invalid_next_token() -> ValidationException {
    ValidationException {
        message: Some("NextToken is not a token this operation issued".to_string()),
        reason: Some(INVALID_NEXT_TOKEN.to_string()),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDirectoryservicedataDirectoryservicedataState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDirectoryservicedataDirectoryservicedataState>)
}

/// The SHARED AD directory inventory in the shared engine: which directories the account
/// holds, as the control plane that creates and deletes them recorded it. Call it with a
/// handler's `&*self.engine`.
///
/// This API creates no directory — `CreateDirectory` / `CreateMicrosoftAD` /
/// `DeleteDirectory` belong to the Directory Service control plane — so whether a
/// `DirectoryId` names a directory of this account is knowable here only from that plane's
/// own record. It is ONE record under one shared type
/// ([`::service_common::ad_directories::AdDirectoryInventory`]) rather than a copy inside
/// [`ComAmazonawsDirectoryservicedataDirectoryservicedataState`]: the engine keys state by
/// TYPE, so a copy of its own here would be invisible to the plane that writes it.
///
/// LOCK ORDER: this inventory is taken and RELEASED before [`service_state`] is locked (the
/// authorization gate in `crate::service::authorization` is its only reader here), so a
/// request never holds both at once and the writer's order — its own store first, this one
/// second — cannot invert against it.
pub fn ad_directory_inventory<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::ad_directories::AdDirectoryInventory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::ad_directories::AdDirectoryInventory,
            >,
        )
}
