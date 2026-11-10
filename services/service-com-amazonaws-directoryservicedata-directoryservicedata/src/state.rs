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
//!
//! What a write to one of those objects also does is produce the DOMAIN CONTROLLER
//! SECURITY LOG record the directory's controllers audit it with: a directory whose `ds`
//! log subscription forwards that log forwards this write to the subscribed CloudWatch
//! Logs group. That subscription lives on the control plane's own directory record, so the
//! emitter is that plane's ([`security_log`]), reached here through the shared engine
//! rather than copied.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Bound;
use std::time::{Duration, SystemTime};

mod attribute_schema;
mod builtin_groups;

use sha2::Digest;

use crate::ops::types::{
    AttributeValue, ConflictException, Member, ResourceNotFoundException, ValidationException,
};

/// The Kerberos realm (AD domain name) this plane reports a directory in when the
/// control plane says nothing about its domain.
///
/// A directory's domain name is chosen when the `ds` control plane creates the
/// directory, and this data-plane API has no operation that carries one, so it is read
/// from the shared link that plane writes ([`DirectoryDataAccess::realm`]). This is what
/// stands in for a `DirectoryId` that plane holds no record of — a directory nothing
/// created in this process — and it satisfies `Realm`'s
/// `^([a-zA-Z0-9]+[\\.-])+([a-zA-Z0-9])+[.]?$`.
pub const DEFAULT_REALM: &str = "corp.example.com";

/// The container inside a managed directory's DELEGATED organizational unit that the
/// objects this API creates sit in.
///
/// A Managed Microsoft AD gives the account no write access to the domain's own containers:
/// it delegates an organizational unit named after the directory's NetBIOS short name, and
/// creates the account's own containers inside it. The objects this data plane creates are
/// users and groups, which are the principals that container holds.
const DELEGATED_OBJECT_CONTAINER: &str = "Users";

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

/// How many attributes ONE object's record holds at once.
///
/// An EMULATOR ceiling, not an AWS quota: these records live in memory for the lifetime of
/// the process, and `UpdateUser` adds attributes to one object call after call, so without
/// a bound a caller looping on fresh attribute names would grow a single record without
/// end. It sits far above the 25 attributes one request may carry (`Attributes` is
/// modelled `@length(min: 1, max: 25)`), so nothing a workflow does reaches it; a write
/// that would pass it is refused ([`attribute_limit_exceeded`]) and the record is left
/// exactly as it stands. A create cannot reach it at all — one request's 25 attributes are
/// all a created object holds — so it is the UPDATE path the clamp sits on.
pub const MAX_OBJECT_ATTRIBUTES: usize = 250;

/// The `ValidationException` `Reason` for a write the directory refuses for its SIZE.
/// One of `ValidationExceptionReason`'s modelled values.
pub const LDAP_SIZE_LIMIT_EXCEEDED: &str = "LDAP_SIZE_LIMIT_EXCEEDED";

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
    /// The `OtherAttributes` the create stored, keyed by the attribute's name as the
    /// directory's own schema spells it ([`attribute_schema`]) — AD attribute names are case
    /// insensitive, so the case a request wrote is not part of the attribute.
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

    /// The `User` object `SearchUsers` serves for a user the search matched, reported as
    /// active or not by `enabled` — see [`crate::state::DirectoryDataAccess::is_enabled`],
    /// which is where an account's activity is decided.
    ///
    /// A search answers with the record a listing reports for that user — the
    /// [`Self::to_summary`] members — widened by the attributes the search NAMED: the
    /// projection is the directory's, not the client's, so an attribute nobody asked
    /// about is carried by no member of the answer.
    pub fn to_searched_user(&self, names: &[String], enabled: bool) -> crate::ops::types::User {
        crate::ops::types::User {
            // What every search result carries, named or not: the same projection
            // `ListUsers` serves.
            sid: Some(self.sid.clone()),
            sam_account_name: self.sam_account_name.clone(),
            given_name: self.given_name.clone(),
            surname: self.surname.clone(),
            enabled: Some(enabled),
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

    /// The `UserSummary` projection `ListUsers` serves, reported as active or not by
    /// `enabled` — see [`DirectoryDataAccess::is_enabled`].
    pub fn to_summary(&self, enabled: bool) -> crate::ops::types::UserSummary {
        crate::ops::types::UserSummary {
            sid: self.sid.clone(),
            sam_account_name: self.sam_account_name.clone(),
            given_name: self.given_name.clone(),
            surname: self.surname.clone(),
            enabled,
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

    /// Write `values` onto this record the way `update` says to, and leave every member
    /// `values` does NOT name exactly as it stands.
    ///
    /// What an update names is what it changes: `UpdateUser` carries the members a caller
    /// wants changed and nothing about the rest of the object, so a member the request left
    /// out is not a member it asked to clear. The naming members are not updatable — the
    /// account name is the object's key, and its SID, distinguished name and principal name
    /// are the directory's own derivations — so nothing here touches them.
    ///
    /// Answers the refusal for a write that would take this record past
    /// [`MAX_OBJECT_ATTRIBUTES`] attributes, in which case NOTHING is written: the clamp
    /// precedes every write this method performs, so one object's record stays bounded
    /// however many updates a caller sends it.
    pub fn apply_update(
        &mut self,
        update: UpdateMode,
        values: UserValues,
    ) -> Result<(), ValidationException> {
        let attributes = stored_attributes(values.other_attributes);
        // The CLAMP, ahead of the first write: how many attributes this record would hold
        // afterwards — a name it already holds takes the new value rather than a second
        // entry, and a clearing update only ever removes.
        if update == UpdateMode::Set {
            let held: BTreeSet<&String> = self
                .other_attributes
                .keys()
                .chain(attributes.keys())
                .collect();
            if held.len() > MAX_OBJECT_ATTRIBUTES {
                return Err(attribute_limit_exceeded());
            }
        }
        let named = |value: Option<String>| match update {
            UpdateMode::Set => value,
            // The value a clearing update carries says WHICH member to clear, not what to
            // leave in it.
            UpdateMode::Clear => None,
        };
        if values.email_address.is_some() {
            self.email_address = named(values.email_address);
        }
        if values.given_name.is_some() {
            self.given_name = named(values.given_name);
        }
        if values.surname.is_some() {
            self.surname = named(values.surname);
        }
        // The attribute names were resolved against the directory's schema above, so an
        // update written in another case changes the attribute the create stored rather
        // than adding a second one beside it.
        for (name, value) in attributes {
            match update {
                UpdateMode::Set => {
                    self.other_attributes.insert(name, value);
                }
                UpdateMode::Clear => {
                    self.other_attributes.remove(&name);
                }
            }
        }
        Ok(())
    }
}

/// The values of a user an update carries: the members `UpdateUser` can change, each
/// `Some` only when the request named it.
#[derive(Debug, Default)]
pub struct UserValues {
    pub email_address: Option<String>,
    pub given_name: Option<String>,
    pub surname: Option<String>,
    pub other_attributes: Option<HashMap<String, AttributeValue>>,
}

/// What an update does with the values it carries, as `UpdateType` says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateMode {
    /// Hold the values the request carries: `ADD` for a member that holds none and
    /// `REPLACE` for one that does, which write the same value either way.
    Set,
    /// Hold no value for the members the request names: `REMOVE`, which
    /// `UpdateUserRequest$UpdateType` documents as the type to "clear the attribute
    /// value".
    Clear,
}

impl UpdateMode {
    /// The mode `UpdateType` names.
    ///
    /// `UpdateType` is optional and the modelled values are `ADD`, `REPLACE` and `REMOVE`
    /// (`smithy_query(query="shape", shape="UpdateType")`); its documentation tells a
    /// caller to "use REPLACE to change an attribute value or REMOVE to clear the attribute
    /// value", so a request that names no type is one carrying the values it wants held,
    /// which is what the two writing types both do.
    pub fn of(update_type: Option<&str>) -> Self {
        match update_type {
            Some(REMOVE_UPDATE_TYPE) => Self::Clear,
            _ => Self::Set,
        }
    }
}

/// The `UpdateType` that clears the members an update names; the other two write them.
const REMOVE_UPDATE_TYPE: &str = "REMOVE";

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
/// names the directory's schema holds them under, which is the name they are stored as
/// ([`attribute_schema`]).
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
    /// The AD domain name of this directory, as the control plane that created the
    /// directory named it ([`DirectoryDataAccess::realm`]). Every realm-valued member
    /// answers from it, and a request naming another realm is asking about a domain this
    /// directory holds no objects for.
    pub realm: String,
    /// The NetBIOS short name of this directory, as the control plane that created the
    /// directory named it ([`DirectoryDataAccess::short_name`]). It names the
    /// organizational unit the directory delegates to the account, which is where the
    /// objects this API creates sit ([`Directory::distinguished_name`]).
    pub short_name: String,
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
    /// A directory with no objects yet, in the domain `realm` and delegating the
    /// organizational unit `short_name` names, whose SID authority is derived from
    /// `directory_id` so it is stable for that directory and distinct across directories.
    fn new(directory_id: &str, realm: &str, short_name: &str) -> Self {
        let mut directory = Self {
            realm: realm.to_string(),
            short_name: short_name.to_string(),
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

    /// The distinguished name of an object this API creates in the directory:
    /// `CN=<name>,OU=Users,OU=<ShortName>,DC=…`.
    ///
    /// A managed directory does not give the account write access to the domain's own
    /// containers; it delegates an organizational unit named after the directory's NetBIOS
    /// short name, holding the containers the account's own objects go in — so a user or a
    /// group this API creates is named inside that OU's `Users` container, and the name is
    /// the directory's to compose: no create carries one.
    pub fn distinguished_name(&self, sam_account_name: &str) -> String {
        format!(
            "CN={sam_account_name},OU={DELEGATED_OBJECT_CONTAINER},OU={},{}",
            self.short_name,
            self.domain_components()
        )
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

    /// A user's `UserPrincipalName`: the account name in this directory's realm, the
    /// domain name in upper case.
    ///
    /// `CreateUser` carries no UPN — the member is output-only — so Active Directory
    /// derives it from the account name and the domain the account was created in, and
    /// states the domain half of a principal name the way it states a realm: in capitals.
    pub fn user_principal_name(&self, sam_account_name: &str) -> String {
        format!("{sam_account_name}@{}", self.realm.to_uppercase())
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
    /// The directory `directory_id` names, creating the record — in the domain and under the
    /// delegated organizational unit `access` resolved for it — if this is the first write
    /// to it. Only a MUTATING operation may call this: a read uses [`Self::directories`]
    /// directly, so reporting never materialises a directory.
    ///
    /// One call materialises at most one directory, and a new record holds a fixed
    /// set of objects — the domain's built-in groups, a catalogue fixed at compile
    /// time — so nothing in a request decides how much a materialisation allocates.
    pub fn directory_for_write(
        &mut self,
        directory_id: &str,
        access: &DirectoryDataAccess,
    ) -> &mut Directory {
        self.directories
            .entry(directory_id.to_string())
            .or_insert_with(|| Directory::new(directory_id, access.realm(), access.short_name()))
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

/// The attributes of a request, stored under the name the directory's own schema holds each
/// attribute under ([`attribute_schema::resolved_name`]) and in the order a directory reports
/// them.
///
/// The name is resolved once, HERE, so one attribute written in two cases is one stored
/// attribute and every projection of it reports the schema's name without re-deciding it.
pub fn stored_attributes(
    attributes: Option<HashMap<String, AttributeValue>>,
) -> BTreeMap<String, AttributeValue> {
    attributes
        .into_iter()
        .flatten()
        .map(|(name, value)| (attribute_schema::resolved_name(&name), value))
        .collect()
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
pub fn user_not_found(directory_id: &str) -> ResourceNotFoundException {
    object_not_found(OBJECT_CLASS_USER, directory_id)
}

/// The `ResourceNotFoundException` payload for a group no directory holds.
pub fn group_not_found(directory_id: &str) -> ResourceNotFoundException {
    object_not_found(OBJECT_CLASS_GROUP, directory_id)
}

/// The `ResourceNotFoundException` payload for a member no directory holds.
pub fn member_not_found(directory_id: &str) -> ResourceNotFoundException {
    object_not_found(OBJECT_CLASS_MEMBER, directory_id)
}

/// The `ResourceNotFoundException` payload for a membership a group does not hold.
///
/// The membership is the resource a removal names, and the object of that resource is the
/// MEMBER a group does not hold — so the refusal states the same class the member's own
/// absence states, through the one sentence below.
pub fn membership_not_found(directory_id: &str) -> ResourceNotFoundException {
    object_not_found(OBJECT_CLASS_MEMBER, directory_id)
}

/// The object class a not-found refusal names for a user object.
const OBJECT_CLASS_USER: &str = "User";

/// The object class it names for a group object.
const OBJECT_CLASS_GROUP: &str = "Group";

/// The object class it names for a member of a group — which is a user or a group object,
/// reached as a MEMBER, so the refusal states the role the request named it in.
const OBJECT_CLASS_MEMBER: &str = "Member";

/// The sentence every not-found refusal of this API states: the CLASS of object the request
/// asked about and the directory it was asked about in, and no object name.
///
/// Directory Service Data states this refusal as a class and a directory: the name the
/// request named is the caller's own and the answer discloses nothing of what the directory
/// holds — so one reader of the message cannot tell a name the directory never had from one
/// it no longer has. The creates' refusal is worded the same way ([`object_already_exists`]).
fn object_not_found(object_class: &str, directory_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "{object_class} not found in directory {directory_id}."
        )),
    }
}

/// The `ConflictException` payload for a create whose `SAMAccountName` already names a
/// user of the directory.
///
/// Directory Service Data states this refusal as the CLASS of object that already exists
/// and the directory it exists in, and quotes no name: a caller tells a duplicate user
/// from any other conflict of the same call (a spent idempotency token, a membership
/// conflict) by that class, so the two creates of this API word it with
/// [`object_already_exists`] rather than each re-wording it.
pub fn user_already_exists(directory_id: &str) -> ConflictException {
    object_already_exists(OBJECT_CLASS_USER, directory_id)
}

/// The same refusal for a create whose `SAMAccountName` already names a group of the
/// directory.
pub fn group_already_exists(directory_id: &str) -> ConflictException {
    object_already_exists(OBJECT_CLASS_GROUP, directory_id)
}

/// The sentence both creates state: the class of object that already exists, then the
/// directory it already exists in.
fn object_already_exists(object_class: &str, directory_id: &str) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "{object_class} already exists in directory {directory_id}."
        )),
    }
}

/// The `ValidationException` payload for a `NextToken` this service did not issue for
/// the listing it was spent on.
///
/// The message is the sentence Directory Service Data states for an invalid parameter —
/// the member it refused, named — and the `Reason` is the modelled
/// [`INVALID_NEXT_TOKEN`] value.
pub fn invalid_next_token() -> ValidationException {
    ValidationException {
        message: Some("Invalid parameter: NextToken.".to_string()),
        reason: Some(INVALID_NEXT_TOKEN.to_string()),
    }
}

/// The `ValidationException` payload for a write that would take ONE object's record past
/// [`MAX_OBJECT_ATTRIBUTES`] attributes.
///
/// The `Reason` is the modelled [`LDAP_SIZE_LIMIT_EXCEEDED`] value — the one that names a
/// write the directory will not take for its SIZE — and the message names the member the
/// write carried and the number of attributes an object holds, read off that ceiling rather
/// than written into the sentence.
pub fn attribute_limit_exceeded() -> ValidationException {
    ValidationException {
        message: Some(format!(
            "OtherAttributes: an object holds at most {MAX_OBJECT_ATTRIBUTES} attributes."
        )),
        reason: Some(LDAP_SIZE_LIMIT_EXCEEDED.to_string()),
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
/// holds, as the control plane that creates and deletes them recorded it, whether that
/// plane has each directory's DS Data feature on, the domain each was created with, and
/// which of its accounts have been given a password. Call it with a handler's
/// `&*self.engine`.
///
/// This API creates no directory — `CreateDirectory` / `CreateMicrosoftAD` /
/// `DeleteDirectory` belong to the Directory Service control plane, as do
/// `EnableDirectoryDataAccess` / `DisableDirectoryDataAccess` and `ResetUserPassword` — so
/// whether a `DirectoryId` names a directory of this account, whether this whole API is in
/// service for it, the realm its objects live in and whether one of its accounts holds a
/// password are knowable here only from that plane's own record. It is ONE record under one
/// shared type
/// ([`::service_common::ad_directories::AdDirectoryInventory`]) rather than a copy inside
/// [`ComAmazonawsDirectoryservicedataDirectoryservicedataState`]: the engine keys state by
/// TYPE, so a copy of its own here would be invisible to the plane that writes it.
///
/// LOCK ORDER: this inventory is taken and RELEASED before [`service_state`] is locked (the
/// authorization gate in `crate::service::authorization` and
/// [`DirectoryDataAccess::resolve`] are its readers here), so a request never holds both at
/// once and the writer's order — its own store first, this one second — cannot invert
/// against it.
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

/// Record an account this plane just created in the shared [`ad_directory_inventory`], so
/// the control plane's `ResetUserPassword` can resolve it and give it a password.
///
/// A no-op for a directory the control plane holds no record of: the inventory holds the
/// accounts of the directories that plane created, and this one creates no directory
/// record of its own.
pub fn link_created_account<Eng: Send + Sync + 'static>(
    engine: &Eng,
    directory_id: &str,
    sam_account_name: &str,
) {
    if let Some(inventory) = ad_directory_inventory(engine) {
        inventory
            .lock()
            .record_account(directory_id, sam_account_name);
    }
}

/// Record that an account this plane deleted is gone, so the shared
/// [`ad_directory_inventory`] holds nothing for a name the directory no longer has an
/// object under.
pub fn link_deleted_account<Eng: Send + Sync + 'static>(
    engine: &Eng,
    directory_id: &str,
    sam_account_name: &str,
) {
    if let Some(inventory) = ad_directory_inventory(engine) {
        inventory
            .lock()
            .forget_account(directory_id, sam_account_name);
    }
}

/// What the control plane says about the directory a call names: the realm its objects are
/// reported in, the organizational unit it delegates to the account, and which of its
/// accounts have a password.
///
/// WHETHER this plane may answer for the directory at all is not here: that is the
/// authorization gate's question (`crate::service::authorization`), which reads the same
/// inventory ahead of every handler and refuses a directory the account does not hold or
/// whose DS Data feature the control plane has off. By the time a handler resolves this,
/// the directory is one this API serves.
///
/// Resolved ONCE per call, before this service's own store is reached, so a handler holds
/// the shared inventory no longer than it takes to copy these out of it.
#[derive(Debug)]
pub struct DirectoryDataAccess {
    /// The domain name the directory was created with, or [`DEFAULT_REALM`] for a
    /// directory the control plane recorded none for.
    realm: String,
    /// The NetBIOS short name the directory was created with, which names the
    /// organizational unit it delegates to the account; derived from the realm
    /// ([`default_short_name`]) for a directory the control plane recorded none for.
    short_name: String,
    /// The directory's accounts whose password the control plane has set, by folded name.
    accounts_with_password: BTreeSet<String>,
}

impl Default for DirectoryDataAccess {
    /// What this plane answers from when the control plane says nothing about the
    /// directory: its own records, in its own default realm, and no account holding a
    /// password.
    fn default() -> Self {
        Self {
            realm: DEFAULT_REALM.to_string(),
            short_name: default_short_name(DEFAULT_REALM),
            accounts_with_password: BTreeSet::new(),
        }
    }
}

/// The NetBIOS short name of a directory whose create recorded none: the first label of its
/// domain name, in upper case.
///
/// That is the default the control plane's own create documents for the member
/// (`CreateMicrosoftADRequest$ShortName`: "If you don't specify a NetBIOS name, it will
/// default to the first part of your directory DNS. For example, `CORP` for the directory DNS
/// `corp.example.com`"), so a directory always has the one name its delegated organizational
/// unit goes by.
fn default_short_name(domain_name: &str) -> String {
    domain_name
        .trim_end_matches('.')
        .split('.')
        .next()
        .unwrap_or_default()
        .to_uppercase()
}

impl DirectoryDataAccess {
    /// What the control plane recorded about `directory_id`: the domain it created the
    /// directory in, the NetBIOS short name of that create, and the accounts of it whose
    /// password it has set.
    ///
    /// The defaults stand for a directory that plane recorded nothing of the kind for —
    /// this plane's own fallback realm, the short name that realm's first label gives, and
    /// no account holding a password — so a handler always has an answer to report from.
    pub fn resolve<Eng: Send + Sync + 'static>(engine: &Eng, directory_id: &str) -> Self {
        let Some(inventory) = ad_directory_inventory(engine) else {
            return Self::default();
        };
        let inventory = inventory.lock();
        let realm = inventory
            .domain_name(directory_id)
            .unwrap_or(DEFAULT_REALM)
            .to_string();
        Self {
            short_name: inventory
                .short_name(directory_id)
                .map_or_else(|| default_short_name(&realm), str::to_string),
            realm,
            accounts_with_password: inventory.accounts_with_password(directory_id),
        }
    }

    /// The realm this directory's objects are reported in: the domain name the control
    /// plane created it with.
    pub fn realm(&self) -> &str {
        &self.realm
    }

    /// The NetBIOS short name of this directory, which names the organizational unit it
    /// delegates to the account — the unit the objects this API creates are named inside.
    pub fn short_name(&self) -> &str {
        &self.short_name
    }

    /// Whether the account `sam_account_name` names is ACTIVE.
    ///
    /// `CreateUser` carries no password member, so an account created through this API
    /// has none — and Active Directory leaves an account without a password inactive. A
    /// `ResetUserPassword` on the control plane is what gives it one, which is what makes
    /// the account active.
    pub fn is_enabled(&self, sam_account_name: &str) -> bool {
        self.accounts_with_password
            .contains(&sam_account_name.to_lowercase())
    }
}

/// The DOMAIN CONTROLLER SECURITY LOG a write to a directory's objects is audited in, and
/// the forwarding of it the directory's `ds` log subscription performs.
///
/// Re-exported from the Directory Service CONTROL plane rather than restated here: the log
/// subscription that decides whether a record is forwarded, and the log group it names,
/// live on that plane's directory record, and the records go into the CloudWatch Logs store
/// of the same shared engine. A handler calls
/// [`security_log::record`] / [`security_log::record_membership`] with its OWN store guard
/// already released — that emitter takes the control plane's store and then CloudWatch
/// Logs', so no request holds two of them at once.
pub use ::service_com_amazonaws_directoryservice_directoryservice_20150416::state::security_log;
