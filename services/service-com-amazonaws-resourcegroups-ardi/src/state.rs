// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-resourcegroups-ardi service.
//!
//! Every resource group this emulator holds lives in ONE map, [`ComAmazonawsResourcegroupsArdiState::groups`],
//! so `CreateGroup`, `GetGroup`, `GetGroupConfiguration`, `ListGroups`, `GroupResources`,
//! `ListGroupResources`, `ListGroupingStatuses`, `UpdateGroup` and `DeleteGroup` all read
//! and write the same records. The map is a
//! [`BTreeMap`] keyed by [`GroupKey`] — the region the group was created in and its
//! name — because this service is REGIONAL: a group belongs to the region whose
//! endpoint created it, and only that region's callers see it. Within one region
//! the key order is group-name order, which is the listing order, and what makes
//! `ListGroups` paging deterministic and resumable from an opaque token even after
//! a group in the middle of the listing is deleted.
//!
//! Every tag-sync task lives in a second map,
//! [`ComAmazonawsResourcegroupsArdiState::tag_sync_tasks`], keyed by the task's own
//! ARN — the identifier `GetTagSyncTask` and `CancelTagSyncTask` address a task by —
//! so `StartTagSyncTask` and every reader of a task read one record.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types::{
    Group, GroupConfiguration, GroupConfigurationItem, GroupIdentifier, GroupingStatusesItem,
    ListGroupResourcesItem, ResourceIdentifier, ResourceQuery, ResourceStatus, TagSyncTaskItem,
    QUERY_TYPE_VALUES,
};

/// Per-service state for com-amazonaws-resourcegroups-ardi. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsResourcegroupsArdiState {
    /// Every persisted resource group, keyed by the region it lives in and its
    /// name. Ordered by region and then by name, so one region's listing and its
    /// continuation token are stable.
    pub groups: BTreeMap<GroupKey, GroupRecord>,
    /// Every persisted tag-sync task, keyed by `TagSyncTaskRecord::task_arn`.
    /// Ordered, so a listing of tasks and its continuation token are stable.
    pub tag_sync_tasks: BTreeMap<String, TagSyncTaskRecord>,
}

/// Where one resource group lives: the region whose endpoint created it, and the
/// group's name.
///
/// A region-qualified key is what makes this service's state regional. A group
/// name only has to be unique within one region of one account, so the name alone
/// does not identify a group; the pair does. Ordered by region first and name
/// second, which keeps one region's groups a contiguous, name-ordered range of the
/// map ([`ComAmazonawsResourcegroupsArdiState::groups_in`]).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct GroupKey {
    /// The region the group was created in — the one in the creating caller's
    /// SigV4 credential scope, and the one its stored ARN names.
    pub region: String,
    /// The group's name, unique within [`GroupKey::region`].
    pub name: String,
}

impl GroupKey {
    /// The key of the group named `name` in `region`.
    pub fn new(region: &str, name: &str) -> Self {
        Self {
            region: region.to_string(),
            name: name.to_string(),
        }
    }

    /// A key that sorts at or before every key of `region`: the start of that
    /// region's range of the map. A group name is at least one character long (the
    /// modelled `GroupName`), so no stored key sorts before this one.
    fn first_in(region: &str) -> Self {
        Self::new(region, "")
    }
}

impl ComAmazonawsResourcegroupsArdiState {
    /// The key of the group `identifier` names IN `region`, or `None` when nothing
    /// stored there answers to it.
    ///
    /// `region` is the region the request addressed (its SigV4 credential scope),
    /// and resolution never leaves it: a group created through another region's
    /// endpoint is another region's resource, so it is invisible here however it is
    /// addressed.
    ///
    /// An identifier is either a group ARN or a bare group name, and it is read by
    /// SHAPE, not by prefix:
    ///
    /// * a full `arn:<partition>:resource-groups:<region>:<account>:group/<name>`
    ///   names exactly one resource, so it resolves only when it equals that
    ///   group's STORED ARN — which keeps an ARN for another account, partition or
    ///   region from resolving to a same-named local group;
    /// * the APPLICATION-group form of that ARN, `…:group/<name>/<id>`, names the
    ///   same group through the id that qualifies its name (see
    ///   [`GroupRecord::application_group_arn`]), so an ARN this service itself
    ///   reported — in a tag-sync task, say — resolves back to the group it was
    ///   minted from, and only while that group still carries that id;
    /// * anything else must be a bare `GroupName`, which the model constrains to
    ///   `[a-zA-Z0-9_\.-]{1,300}` or `<name>/<ulid>` — no colon — so an identifier
    ///   that is neither a well-formed ARN nor a well-formed name resolves to
    ///   nothing instead of slipping past the ARN check.
    ///
    /// The caller picks the identifier: an operation that models both members takes
    /// `Group` and falls back to the deprecated `GroupName`, so the group a request
    /// resolves to never depends on which of the two a handler happens to read.
    pub fn lookup(&self, region: &str, identifier: &str) -> Option<GroupKey> {
        if let Some((service, resource)) = parse_arn(identifier) {
            if service != "resource-groups" {
                return None;
            }
            let named = resource.strip_prefix("group/")?;
            let key = GroupKey::new(region, named);
            if let Some(record) = self.groups.get(&key) {
                if identifier != record.group_arn {
                    return None;
                }
                return Some(key);
            }
            // The application-group form: the trailing segment is the id that
            // qualifies the group's name, so the group it names is the one stored
            // under the head segment whose own qualifying id is that segment.
            let (head, id) = named.rsplit_once('/')?;
            if !is_qualifying_id(id) {
                return None;
            }
            let key = GroupKey::new(region, head);
            let record = self.groups.get(&key)?;
            if record.application_id.as_deref() != Some(id)
                || identifier != format!("{}/{id}", record.group_arn)
            {
                return None;
            }
            return Some(key);
        }
        if identifier.contains(':') {
            return None;
        }
        let key = GroupKey::new(region, identifier);
        self.groups.contains_key(&key).then_some(key)
    }

    /// Every group stored in `region`, in group-name order — the listing order.
    ///
    /// A region's groups are a contiguous range of the map, so this walks that
    /// range alone and a group of another region is never reached.
    pub fn groups_in<'a>(&'a self, region: &'a str) -> impl Iterator<Item = &'a GroupRecord> + 'a {
        self.groups
            .range(GroupKey::first_in(region)..)
            .take_while(move |(key, _)| key.region == region)
            .map(|(_, record)| record)
    }
}

/// The most groups this emulator holds at once.
///
/// NOT an AWS quota: real Resource Groups enforces its own, and answers its own
/// limit error. This is the emulator's bound on in-memory fixture state, so a
/// client looping on `CreateGroup` is refused rather than growing the store
/// without end. Reaching it is a `BadRequestException`, and deleting a group
/// makes room again.
pub const MAX_GROUPS: usize = 1_000;

/// The most static memberships one group holds at once, for the same reason as
/// [`MAX_GROUPS`] and with the same refusal.
pub const MAX_MEMBERS_PER_GROUP: usize = 1_000;

/// The most tags one group carries at once, for the same reason as
/// [`MAX_GROUPS`] and with the same refusal: it bounds in-memory fixture state,
/// so a client looping on `Tag` is refused rather than growing a record without
/// end. Removing a tag makes room again.
pub const MAX_TAGS_PER_GROUP: usize = 50;

/// The most tag-sync tasks this emulator holds at once, for the same reason as
/// [`MAX_GROUPS`] and with the same refusal: it bounds in-memory fixture state,
/// so a client looping on `StartTagSyncTask` is refused rather than growing the
/// store without end. Cancelling a task makes room again.
pub const MAX_TAG_SYNC_TASKS: usize = 1_000;

/// The prefix AWS keeps for the tags it sets itself.
///
/// AWS's resource-tagging documentation states that a caller can neither edit nor
/// delete a tag whose key carries this prefix, so a request that writes one is
/// refused rather than stored. Tag keys are case-sensitive, but a key does not
/// leave the reserved namespace by re-casing the prefix, so the comparison below
/// ignores case.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` names a tag in the reserved [`RESERVED_TAG_KEY_PREFIX`]
/// namespace.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// The longest tag key this service accepts: the maximum the modelled `TagKey`
/// declares (`length 1..128`).
///
/// Source: `smithy_query(query="shape",shape="TagKey")`. A tag key is a map KEY,
/// whose constraints the request boundary does not walk, so this service decides
/// them itself — in [`check_tag`], the one place it decides them.
pub const MAX_TAG_KEY_LENGTH: usize = 128;

/// The longest tag value this service accepts: the maximum the modelled
/// `TagValue` declares (`length 0..256`), whose minimum of zero makes an empty
/// value legal.
///
/// Source: `smithy_query(query="shape",shape="TagValue")`.
pub const MAX_TAG_VALUE_LENGTH: usize = 256;

/// Whether `c` is a character a tag key or a tag value may carry.
///
/// The modelled `TagKey` and `TagValue` share one pattern,
/// `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`: Unicode letters and numbers (which is what
/// [`char::is_alphanumeric`] answers), the separator categories, and the eight
/// punctuation characters it lists. A whitespace character that is not a control
/// character is exactly a separator, so a tab or a newline is NOT one.
fn is_tag_character(c: char) -> bool {
    c.is_alphanumeric()
        || (c.is_whitespace() && !c.is_control())
        || matches!(c, '_' | '.' | ':' | '/' | '=' | '+' | '-' | '@')
}

/// The request member a tagging call carries its pairs in, as the refusal names it.
pub const TAGS_MEMBER: &str = "Tags";

/// The request member an untagging call carries its keys in, as the refusal names
/// it.
pub const TAG_KEYS_MEMBER: &str = "Keys";

/// `Ok(())` when `key` names a tag a caller may change on a group, else the
/// message the refusal reports — prefixed with `member`, the request member the
/// key arrived in, so a caller reading the message alone knows what to fix.
///
/// The ONE place this service decides whether a string names a tag key a caller
/// controls, so every path that writes tags and the one that removes them judge a
/// key the same way:
///
/// * it stays inside the modelled key length and character set
///   ([`MAX_TAG_KEY_LENGTH`], [`is_tag_character`]);
/// * it is outside the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace, which a
///   caller can neither write nor remove.
pub fn check_tag_key(member: &str, key: &str) -> Result<(), String> {
    let key_length = key.chars().count();
    if !(1..=MAX_TAG_KEY_LENGTH).contains(&key_length) {
        return Err(format!(
            "{member} is not valid: a tag key is 1 to {MAX_TAG_KEY_LENGTH} characters long, \
             and this one is {key_length}",
        ));
    }
    if let Some(character) = key.chars().find(|c| !is_tag_character(*c)) {
        return Err(format!(
            "{member} is not valid: the tag key {key} carries {character:?}, which is not one of \
             the letters, numbers, separators and `_ . : / = + - @` a tag may use",
        ));
    }
    if is_reserved_tag_key(key) {
        return Err(format!(
            "{member} is not valid: the {RESERVED_TAG_KEY_PREFIX} prefix is reserved, so the tag \
             key {key} is not one a caller can change",
        ));
    }
    Ok(())
}

/// `Ok(())` when every key in `keys` is one a caller may remove from a group, else
/// the message the refusal reports.
///
/// The whole batch is judged BEFORE anything is removed, so a request holding one
/// key this service refuses removes none of its keys.
pub fn check_tag_keys<'a>(keys: impl IntoIterator<Item = &'a str>) -> Result<(), String> {
    for key in keys {
        check_tag_key(TAG_KEYS_MEMBER, key)?;
    }
    Ok(())
}

/// `Ok(())` when the pair `key`/`value` is one a caller may write to a group,
/// else the message the refusal reports.
///
/// The ONE place this service decides whether a tag may be written, so every path
/// that stores tags — a create that carries them and a later tagging call —
/// refuses the same pairs the same way: the key is one [`check_tag_key`] admits,
/// and the value stays inside the modelled length and character set
/// ([`MAX_TAG_VALUE_LENGTH`], [`is_tag_character`]).
pub fn check_tag(key: &str, value: &str) -> Result<(), String> {
    check_tag_key(TAGS_MEMBER, key)?;
    let value_length = value.chars().count();
    if value_length > MAX_TAG_VALUE_LENGTH {
        return Err(format!(
            "{TAGS_MEMBER} is not valid: a tag value is at most {MAX_TAG_VALUE_LENGTH} \
             characters long, and the value of {key} is {value_length}",
        ));
    }
    if let Some(character) = value.chars().find(|c| !is_tag_character(*c)) {
        return Err(format!(
            "{TAGS_MEMBER} is not valid: the value of the tag key {key} carries {character:?}, \
             which is not one of the letters, numbers, separators and `_ . : / = + - @` a tag \
             may use",
        ));
    }
    Ok(())
}

/// `Ok(())` when every pair in `tags` is one a caller may write, else the message
/// the refusal reports.
///
/// The whole batch is judged BEFORE anything is written, so a request holding one
/// pair this service refuses stores none of its pairs.
pub fn check_tags<'a>(tags: impl IntoIterator<Item = (&'a str, &'a str)>) -> Result<(), String> {
    for (key, value) in tags {
        check_tag(key, value)?;
    }
    Ok(())
}

/// `Ok(())` when `query` is a resource query this service stores, else the
/// message the refusal reports.
///
/// The ONE place this service decides whether a `ResourceQuery` is well formed,
/// so every operation that takes one — a create and a tag-sync task — refuses the
/// same documents the same way:
///
/// * `Type` is a value the modelled `QueryType` declares
///   ([`crate::ops::types::QUERY_TYPE_VALUES`]);
/// * `Query` is a JSON object, which is what the model documents it as — "the
///   query that defines a group or a search", `{"ResourceTypeFilters":[..],
///   "TagFilters":[..]}` for a tag query and `{"StackIdentifier":".."}` for a
///   stack query — so a body that is not an object defines no query.
///
/// Which keys a given `Type` accepts is NOT refused here: the query is stored
/// without being run.
pub fn check_resource_query(query: &ResourceQuery) -> Result<(), String> {
    if !QUERY_TYPE_VALUES.contains(&query.r#type.as_str()) {
        return Err(format!(
            "ResourceQuery.Type {} is not valid: expected one of {}",
            query.r#type,
            QUERY_TYPE_VALUES.join(", "),
        ));
    }
    if !serde_json::from_str::<serde_json::Value>(&query.query)
        .is_ok_and(|parsed| parsed.is_object())
    {
        return Err("ResourceQuery.Query is not valid: it must be a JSON object".to_string());
    }
    Ok(())
}

/// The modelled `GroupConfigurationStatus` of a service configuration that is in
/// effect: nothing is being applied to the group.
///
/// `CreateGroup` is synchronous — the model gives it HTTP 200 and a
/// `GroupConfiguration` in its own response, unlike `PutGroupConfiguration`
/// (HTTP 202) — so the configuration a create stores is already the one in
/// effect, with no update in flight to report.
pub const CONFIGURATION_IN_EFFECT: &str = "UPDATE_COMPLETE";

/// The service-configuration parameter that decides whether a group may be
/// deleted.
///
/// A parameter is named in the AWS "Service configurations for resource groups"
/// reference, which is the vocabulary `GroupConfigurationParameter$Name` is
/// documented in; the modelled name pattern `^[a-z-]+$` admits exactly one
/// spelling, so the comparison in [`GroupRecord::configuration_parameter_has_value`]
/// is exact.
pub const DELETION_PROTECTION_PARAMETER: &str = "deletion-protection";

/// The [`DELETION_PROTECTION_PARAMETER`] value that keeps a group which still
/// holds resources from being deleted. Its documented alternative (`DISABLED`),
/// any other value, and the parameter being absent all leave deletion
/// unprotected.
pub const DELETION_PROTECTION_UNLESS_EMPTY: &str = "UNLESS_EMPTY";

/// One persisted resource group: its identity, its describable attributes, the
/// query or the service configuration it was created with, its tags, and its
/// static members.
#[derive(Debug, Clone)]
pub struct GroupRecord {
    /// The group's name, which with the region it lives in is its
    /// [`GroupKey`] in [`ComAmazonawsResourcegroupsArdiState::groups`].
    pub name: String,
    /// `arn:aws:resource-groups:<region>:<account>:group/<name>`, minted once at
    /// create time and never recomputed, so every later read returns the same ARN.
    pub group_arn: String,
    /// The id of the account that owns the group, captured once at create time
    /// from the emulator's canonical account id — the same account the stored
    /// ARN names — so every later read reports the same owner.
    pub owner_id: String,
    pub description: Option<String>,
    pub criticality: Option<i32>,
    pub owner: Option<String>,
    pub display_name: Option<String>,
    /// The resource query the group was created with, if any.
    pub resource_query: Option<ResourceQueryRecord>,
    /// The service configuration the group was created with, if any. A group
    /// contains either a configuration or a resource query, never both, and a
    /// group created with neither carries `None` here.
    pub configuration: Option<Vec<GroupConfigurationItem>>,
    /// The group's tags, kept in key order so the stored record is deterministic
    /// (the wire map itself is unordered).
    pub tags: BTreeMap<String, String>,
    /// Static members added by `GroupResources`, kept sorted by ARN: the sort is
    /// the listing order `ListGroupResources` pages through.
    pub members: Vec<MemberRecord>,
    /// The last grouping or ungrouping action recorded against each resource of
    /// this group, keyed by resource ARN — what `ListGroupingStatuses` reports.
    /// Ordered by ARN, so that listing and its continuation token are stable.
    pub grouping_statuses: BTreeMap<String, GroupingStatusRecord>,
    /// The id that qualifies this group's name where an ARN addresses it as an
    /// APPLICATION group, minted once the group first needs one and kept
    /// afterwards. `None` until then. See
    /// [`GroupRecord::application_group_arn`].
    pub application_id: Option<String>,
}

impl GroupRecord {
    /// This group's ARN in the APPLICATION-group form, in which the group's name
    /// is qualified by a 26-character id:
    /// `arn:aws:resource-groups:<region>:<account>:group/<name>/<id>`.
    ///
    /// A tag-sync task is addressed by an ARN NESTED inside this form (the
    /// modelled `TagSyncTaskArn` is
    /// `…:group/<name>/<26 chars>/tag-sync-task/<26 chars>`), while a group is
    /// created and addressed by its bare name here, so the qualifying id is
    /// minted on first use and then stored: every ARN this group hosts carries
    /// the same one, and repeated reads of a hosted resource resolve back to
    /// this group. A group whose stored name ALREADY carries such an id (the
    /// `<name>/<id>` form the modelled `GroupName` also admits) is already
    /// qualified, so its stored ARN is returned unchanged.
    pub fn application_group_arn(&mut self) -> String {
        if self
            .name
            .rsplit_once('/')
            .is_some_and(|(head, id)| !head.is_empty() && is_qualifying_id(id))
        {
            return self.group_arn.clone();
        }
        let id = self
            .application_id
            .get_or_insert_with(new_qualifying_id)
            .as_str();
        format!("{}/{id}", self.group_arn)
    }

    /// The modelled `Group` structure for this record.
    pub fn to_group(&self) -> Group {
        Group {
            owner_id: Some(self.owner_id.clone()),
            group_arn: self.group_arn.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            criticality: self.criticality,
            owner: self.owner.clone(),
            display_name: self.display_name.clone(),
            // Only an application group (created through the Service Catalog
            // AppRegistry integration) carries one, and this slice creates none.
            application_tag: None,
        }
    }

    /// The modelled `GroupIdentifier` for this record, as `ListGroups` reports it.
    pub fn to_identifier(&self) -> GroupIdentifier {
        GroupIdentifier {
            group_name: Some(self.name.clone()),
            group_arn: Some(self.group_arn.clone()),
            description: self.description.clone(),
            criticality: self.criticality,
            owner: self.owner.clone(),
            display_name: self.display_name.clone(),
        }
    }

    /// The modelled `GroupConfiguration` for this record, if it was created with
    /// a service configuration, and `None` for a group that has none.
    ///
    /// `ProposedConfiguration` is reported only while an update is in flight, and
    /// `FailureReason` only for one that failed; a stored configuration is in
    /// effect, so neither is reported here.
    pub fn to_group_configuration(&self) -> Option<GroupConfiguration> {
        self.configuration.as_ref().map(|items| GroupConfiguration {
            configuration: Some(items.clone()),
            proposed_configuration: None,
            status: Some(CONFIGURATION_IN_EFFECT.to_string()),
            failure_reason: None,
        })
    }

    /// Whether this record's stored service configuration sets `parameter` to
    /// `value`.
    ///
    /// Read across every configuration item the group holds and every value the
    /// parameter carries: a parameter is set when any item sets it, because the
    /// configuration is one document whatever item type declares the parameter.
    /// A group with no configuration, an item with no parameters and a parameter
    /// with no values all set nothing.
    pub fn configuration_parameter_has_value(&self, parameter: &str, value: &str) -> bool {
        self.configuration
            .iter()
            .flatten()
            .filter_map(|item| item.parameters.as_ref())
            .flatten()
            .filter(|p| p.name == parameter)
            .filter_map(|p| p.values.as_ref())
            .flatten()
            .any(|v| v == value)
    }

    /// Whether this group's own stored service configuration forbids deleting it
    /// as it stands.
    ///
    /// [`DELETION_PROTECTION_PARAMETER`] set to
    /// [`DELETION_PROTECTION_UNLESS_EMPTY`] protects a group that still contains
    /// resources, so protection is decided from the configuration AND the
    /// membership the record holds at the moment of the call: emptying the group
    /// lifts it, and filling it again restores it.
    pub fn deletion_is_protected(&self) -> bool {
        !self.members.is_empty()
            && self.configuration_parameter_has_value(
                DELETION_PROTECTION_PARAMETER,
                DELETION_PROTECTION_UNLESS_EMPTY,
            )
    }

    /// The modelled `ResourceQuery` for this record, if it was created with one.
    pub fn to_resource_query(&self) -> Option<ResourceQuery> {
        self.resource_query.as_ref().map(|q| ResourceQuery {
            r#type: q.query_type.clone(),
            query: q.query.clone(),
        })
    }

    /// This record's tags in the shape the wire uses.
    pub fn tags_map(&self) -> std::collections::HashMap<String, String> {
        self.tags
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Merge `tags` into this record's tags: a key the batch names is set or
    /// overwritten, and a key it does not name keeps the value it had ("Existing
    /// tags on a resource group are not changed if they are not specified in the
    /// request parameters").
    ///
    /// The whole batch is judged against [`MAX_TAGS_PER_GROUP`] BEFORE anything
    /// is written, so a batch that would push the record past the bound is
    /// refused with the message in the `Err` and stores none of its pairs.
    ///
    /// `Ok` carries the pairs now STORED for the keys the batch named, read back
    /// out of the record, so a caller reports what the store holds rather than
    /// what the request said.
    pub fn merge_tags<'a>(
        &mut self,
        tags: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<std::collections::HashMap<String, String>, String> {
        let batch: Vec<(&str, &str)> = tags.into_iter().collect();
        let new_keys = batch
            .iter()
            .filter(|(key, _)| !self.tags.contains_key(*key))
            .count();
        if self.tags.len() + new_keys > MAX_TAGS_PER_GROUP {
            return Err(format!(
                "this emulator holds at most {MAX_TAGS_PER_GROUP} tags per resource group; \
                 remove a tag before adding another",
            ));
        }
        for (key, value) in &batch {
            self.tags.insert((*key).to_string(), (*value).to_string());
        }
        Ok(batch
            .iter()
            .filter_map(|(key, _)| self.tags.get_key_value(*key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect())
    }

    /// Remove the tags `keys` names from this record, and report the keys whose
    /// tag this call actually removed.
    ///
    /// A key the record does not carry removes nothing and is reported in
    /// nothing, so removing the same key twice is idempotent: the second call
    /// reports no key and leaves the same map behind. A key the batch repeats is
    /// likewise reported once. A key the batch does not name keeps the value it
    /// had, so the removal touches only the keys the request asked for.
    ///
    /// The reported keys come from what the record HELD, in the order the batch
    /// named them, so a caller reports the tags the store lost rather than the
    /// list the request sent.
    pub fn remove_tags<'a>(&mut self, keys: impl IntoIterator<Item = &'a str>) -> Vec<String> {
        let mut removed = Vec::new();
        for key in keys {
            if self.tags.remove(key).is_some() {
                removed.push(key.to_string());
            }
        }
        removed
    }

    /// Record `resource_arn` as a member, keeping [`GroupRecord::members`] sorted
    /// by ARN. Adding an ARN that is already a member changes nothing, so
    /// repeating a `GroupResources` call does not duplicate a membership.
    pub fn add_member(&mut self, resource_arn: &str) {
        // A membership recorded here is the outcome of a GROUP action that
        // succeeded, so the same call records the grouping status
        // `ListGroupingStatuses` reports. It is recorded for an ARN that is
        // already a member too: that call is the resource's LAST grouping
        // action, and it is the one `GroupResources` reports in `Succeeded`.
        self.record_grouping_status(
            resource_arn,
            GROUPING_ACTION_GROUP,
            GROUPING_STATUS_SUCCESS,
            SystemTime::now(),
        );
        let at = self
            .members
            .partition_point(|m| m.resource_arn.as_str() < resource_arn);
        if self
            .members
            .get(at)
            .is_some_and(|m| m.resource_arn == resource_arn)
        {
            return;
        }
        self.members.insert(
            at,
            MemberRecord {
                resource_arn: resource_arn.to_string(),
                resource_type: resource_type_of_arn(resource_arn).map(str::to_string),
                // The modelled `ResourceStatusValue` has one value, `PENDING`,
                // and the model documents it as "the process isn't completed
                // yet". A member this emulator reports in `Succeeded` is already
                // part of the group, so it carries no status.
                status: None,
            },
        );
    }

    /// Record that the last grouping action on `resource_arn` in this group was
    /// `action` (a modelled `GroupingType`) and ended in `status` (a modelled
    /// `GroupingStatus`), at `at`.
    ///
    /// The ONE place a grouping status is written, so every producer of grouping
    /// or ungrouping actions records them the same way and `ListGroupingStatuses`
    /// reads one record per resource. Recording a resource again REPLACES its
    /// entry: the operation reports each resource's last action, not a history.
    /// This slice records only the successful GROUP actions
    /// [`GroupRecord::add_member`] performs, so the map holds one entry per
    /// member and is bounded by [`MAX_MEMBERS_PER_GROUP`].
    pub fn record_grouping_status(
        &mut self,
        resource_arn: &str,
        action: &str,
        status: &str,
        at: SystemTime,
    ) {
        self.grouping_statuses.insert(
            resource_arn.to_string(),
            GroupingStatusRecord {
                action: action.to_string(),
                status: status.to_string(),
                updated_at: at,
            },
        );
    }
}

/// The modelled `GroupingType` value for adding a resource to a group — the
/// action `GroupResources` performs.
pub const GROUPING_ACTION_GROUP: &str = "GROUP";

/// The modelled `GroupingType` value for removing a resource from a group.
pub const GROUPING_ACTION_UNGROUP: &str = "UNGROUP";

/// Every value the modelled `GroupingStatus` enum declares, which is also the
/// domain of the `status` filter `ListGroupingStatuses` accepts.
///
/// Source: `smithy_query(query="shape",shape="GroupingStatus")`.
pub const GROUPING_STATUS_VALUES: [&str; 4] = ["SUCCESS", "FAILED", "IN_PROGRESS", "SKIPPED"];

/// The modelled `GroupingStatus` of an action that completed: the only outcome
/// this emulator records, because a grouping request it accepts is applied
/// synchronously and in full, and one it cannot apply is refused before any
/// state changes.
pub const GROUPING_STATUS_SUCCESS: &str = GROUPING_STATUS_VALUES[0];

/// The last grouping or ungrouping action recorded against one resource of one
/// group.
#[derive(Debug, Clone)]
pub struct GroupingStatusRecord {
    /// A modelled `GroupingType` value: [`GROUPING_ACTION_GROUP`] or
    /// [`GROUPING_ACTION_UNGROUP`].
    pub action: String,
    /// A modelled `GroupingStatus` value, one of [`GROUPING_STATUS_VALUES`].
    pub status: String,
    /// When the action this record reports was performed.
    pub updated_at: SystemTime,
}

impl GroupingStatusRecord {
    /// The modelled `GroupingStatusesItem` for this record, as
    /// `ListGroupingStatuses` reports it for `resource_arn`.
    ///
    /// `ErrorCode` and `ErrorMessage` report why an action FAILED; a recorded
    /// action that did not fail carries neither.
    pub fn to_item(&self, resource_arn: &str) -> GroupingStatusesItem {
        GroupingStatusesItem {
            resource_arn: Some(resource_arn.to_string()),
            action: Some(self.action.clone()),
            status: Some(self.status.clone()),
            error_code: None,
            error_message: None,
            updated_at: Some(to_date_time(self.updated_at)),
        }
    }
}

/// How many characters the id that qualifies an application group's name, and
/// the id of a tag-sync task, is written with.
///
/// Both modelled ARN productions spell it `[a-z0-9]{26}`, which is the length
/// and alphabet of a ULID.
///
/// Source: `smithy_query(query="shape",shape="TagSyncTaskArn")`.
pub const QUALIFYING_ID_LENGTH: usize = 26;

/// Whether `id` is shaped like the [`QUALIFYING_ID_LENGTH`]-character id the
/// modelled ARNs use.
pub fn is_qualifying_id(id: &str) -> bool {
    id.len() == QUALIFYING_ID_LENGTH
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// A fresh [`QUALIFYING_ID_LENGTH`]-character id: a ULID, lowercased.
///
/// A ULID is a millisecond timestamp followed by entropy, written in 26
/// Crockford base32 characters, so lowercasing it satisfies the modelled
/// `[a-z0-9]{26}`; the timestamp leads, so ids sort in minting order and a
/// listing keyed by one pages in creation order.
pub fn new_qualifying_id() -> String {
    ::ulid::Ulid::new().to_string().to_lowercase()
}

/// The longest group name a tag-sync task's ARN can carry.
///
/// The modelled `TagSyncTaskArn` writes the group-name segment as
/// `[a-zA-Z0-9_\.-]{1,150}`, narrower than the 300 characters a group name may
/// have, so a task cannot be addressed on a group with a longer name.
///
/// Source: `smithy_query(query="shape",shape="TagSyncTaskArn")`.
pub const MAX_TAG_SYNC_TASK_GROUP_NAME_LENGTH: usize = 150;

/// Whether a tag-sync task's ARN can carry the group name `name`.
///
/// The modelled `TagSyncTaskArn` writes that segment as
/// `[a-zA-Z0-9_\.-]{1,150}`, where the modelled `GroupName` is wider (up to 300
/// characters, and the `<name>/<id>` form besides), so the name decides whether a
/// task on this group is addressable at all. A name already qualified by an id is
/// judged on its leading segment, which is the one the task ARN carries.
pub fn is_tag_sync_task_group_name(name: &str) -> bool {
    let segment = name.split('/').next().unwrap_or_default();
    (1..=MAX_TAG_SYNC_TASK_GROUP_NAME_LENGTH).contains(&segment.len())
        && segment
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// The modelled `TagSyncTaskStatus` of a task that is managing its application's
/// resources: the model documents `ACTIVE` as the task "actively managing
/// resources" and `ERROR` as one that is not, with the reason in
/// `ErrorMessage`. A task this emulator accepts is managing its group from the
/// moment it is stored, and nothing here fails a task later, so a stored task
/// is `ACTIVE` and carries no error message.
pub const TAG_SYNC_TASK_ACTIVE: &str = "ACTIVE";

/// The message this service answers when a task ARN addresses no stored
/// tag-sync task.
///
/// The service's own FIXED wording, which names nothing the caller sent — the
/// ARN is already in the request — so every operation that addresses one task
/// answers the same sentence.
pub const TAG_SYNC_TASK_NOT_FOUND_MESSAGE: &str = "The specified tag-sync task does not exist";

/// The ARN of the tag-sync task `task_id` on the application group
/// `application_group_arn`, in the one form every operation of this service
/// reports: the modelled `TagSyncTaskArn` nests a task under the group it syncs,
/// so the task's ARN is the group's own application-group ARN (see
/// [`GroupRecord::application_group_arn`]) followed by `/tag-sync-task/<id>`.
/// Reported from the group's STORED ARN, so a task carries the region and the
/// account of the group it belongs to rather than of the request that started
/// it.
pub fn tag_sync_task_arn(application_group_arn: &str, task_id: &str) -> String {
    format!("{application_group_arn}/tag-sync-task/{task_id}")
}

/// One persisted tag-sync task: what it syncs, the group it syncs into, the role
/// it assumes, and its status.
#[derive(Debug, Clone)]
pub struct TagSyncTaskRecord {
    /// The task's own ARN, which is also its key in
    /// [`ComAmazonawsResourcegroupsArdiState::tag_sync_tasks`]. Minted once when
    /// the task is started and never recomputed, so every later read returns the
    /// same ARN.
    pub task_arn: String,
    /// The region the task lives in: the region of the group it syncs into, which
    /// is also the region its own ARN names. This service is REGIONAL, so a task
    /// is visible only to that region's callers.
    pub region: String,
    /// The STORED ARN of the group the task syncs resources into, so a reader
    /// reports the same ARN `GetGroup` does for that group.
    pub group_arn: String,
    /// The stored name of that group.
    pub group_name: String,
    /// The tag key the task syncs on, for a task started with a tag key-value
    /// pair; `None` for one started with a resource query.
    pub tag_key: Option<String>,
    /// The tag value the task syncs on, present exactly when
    /// [`TagSyncTaskRecord::tag_key`] is.
    pub tag_value: Option<String>,
    /// The query the task syncs on, for a task started with one; `None` for a
    /// task started with a tag key-value pair.
    pub resource_query: Option<ResourceQueryRecord>,
    /// The role the service assumes to tag and untag resources, stored as the
    /// caller gave it.
    pub role_arn: String,
    /// A modelled `TagSyncTaskStatus` value: [`TAG_SYNC_TASK_ACTIVE`] for every
    /// task this emulator stores.
    pub status: String,
    /// Why the task is not managing resources, reported only for a task whose
    /// status says it failed.
    pub error_message: Option<String>,
    /// When the task was started.
    pub created_at: SystemTime,
}

impl TagSyncTaskRecord {
    /// The modelled `ResourceQuery` for this record, if it was started with one.
    pub fn to_resource_query(&self) -> Option<ResourceQuery> {
        self.resource_query.as_ref().map(|q| ResourceQuery {
            r#type: q.query_type.clone(),
            query: q.query.clone(),
        })
    }

    /// The modelled `TagSyncTaskItem` for this record, as `ListTagSyncTasks`
    /// reports it.
    pub fn to_item(&self) -> TagSyncTaskItem {
        TagSyncTaskItem {
            group_arn: Some(self.group_arn.clone()),
            group_name: Some(self.group_name.clone()),
            task_arn: Some(self.task_arn.clone()),
            tag_key: self.tag_key.clone(),
            tag_value: self.tag_value.clone(),
            resource_query: self.to_resource_query(),
            role_arn: Some(self.role_arn.clone()),
            status: Some(self.status.clone()),
            error_message: self.error_message.clone(),
            created_at: Some(to_date_time(self.created_at)),
        }
    }
}

/// `at` as the modelled `timestamp` a response carries. A time before the epoch
/// cannot be reported by this service — nothing it stores predates the process —
/// so one reads as the epoch rather than panicking.
pub fn to_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    // A whole-second count past `i64::MAX` is 292 billion years away; saturating
    // keeps the conversion total without a panic path.
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_nanos(),
    )
}

/// The resource query a group was created with.
#[derive(Debug, Clone)]
pub struct ResourceQueryRecord {
    /// A modelled `QueryType` value: `TAG_FILTERS_1_0` or `CLOUDFORMATION_STACK_1_0`.
    pub query_type: String,
    /// The query document, stored verbatim as the caller sent it.
    pub query: String,
}

/// One static membership: a resource ARN added to a group by `GroupResources`.
#[derive(Debug, Clone)]
pub struct MemberRecord {
    pub resource_arn: String,
    /// The CloudFormation-style type name derived from the ARN, when
    /// [`resource_type_of_arn`] knows it; `None` otherwise.
    pub resource_type: Option<String>,
    /// A modelled `ResourceStatusValue`, set only while a membership is still
    /// being applied asynchronously.
    pub status: Option<String>,
}

impl MemberRecord {
    /// The modelled `ResourceIdentifier` for this membership.
    pub fn to_identifier(&self) -> ResourceIdentifier {
        ResourceIdentifier {
            resource_arn: Some(self.resource_arn.clone()),
            resource_type: self.resource_type.clone(),
        }
    }

    /// The modelled `ListGroupResourcesItem` for this membership.
    pub fn to_item(&self) -> ListGroupResourcesItem {
        ListGroupResourcesItem {
            identifier: Some(self.to_identifier()),
            status: self
                .status
                .as_ref()
                .map(|name| ResourceStatus {
                    name: Some(name.clone()),
                }),
        }
    }
}

/// The region every ARN this emulator mints for an unsigned request carries.
/// `emulator_server::emulator_arn`'s own documentation names `us-east-1` as the
/// emulator default.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region to mint an ARN in for this request: the one in the caller's SigV4
/// credential scope, or [`DEFAULT_REGION`] when the request carries no scope or
/// a scope region that is not shaped like an AWS region (the modelled `GroupArn`
/// pattern requires `[a-z]{2}(-[a-z]+)+-\d{1}`).
pub fn request_region(parts: &http::request::Parts) -> String {
    emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_arn_region(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The `target` every audit record this service emits carries, so a consumer can
/// select the access trail without reading the rest of the emulator's traces.
///
/// A state-changing operation emits ONE record, at `INFO`, naming the operation,
/// the caller's presented access key id, the region the request resolved to, the
/// resource it changed and the outcome. Metadata only — never a request body, a
/// tag value or a credential secret.
pub const AUDIT_TARGET: &str = "emulator::audit";

/// The identity a request presents, for an audit record: the access key id in its
/// SigV4 credential scope. `emulator_server::extract_akid` answers `"default"`
/// when the request carries no scope. An access key id NAMES a caller and is not
/// a secret — the signature and the secret key are never read here.
pub fn request_caller(parts: &http::request::Parts) -> String {
    emulator_server::extract_akid(&parts.headers)
}

/// Whether `partition` matches the partition production the modelled ARN patterns
/// use: `aws(-[a-z]+)*`.
fn is_arn_partition(partition: &str) -> bool {
    let mut segments = partition.split('-');
    segments.next() == Some("aws")
        && segments.all(|segment| {
            !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_lowercase())
        })
}

/// Whether `region` matches the region production the modelled ARN patterns use:
/// `[a-z]{2}(-[a-z]+)+-\d{1}`.
fn is_arn_region(region: &str) -> bool {
    let Some((head, digit)) = region.rsplit_once('-') else {
        return false;
    };
    if digit.len() != 1 || !digit.as_bytes()[0].is_ascii_digit() {
        return false;
    }
    let mut segments = head.split('-');
    let lowercase = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase());
    match segments.next() {
        Some(first) if first.len() == 2 && lowercase(first) => {}
        _ => return false,
    }
    let mut middle = 0usize;
    for segment in segments {
        if !lowercase(segment) {
            return false;
        }
        middle += 1;
    }
    middle >= 1
}

/// The message this service answers when a group identifier resolves to no
/// stored group.
///
/// It is the service's own FIXED wording, and it names nothing the caller sent:
/// the identifier is already in the request, so echoing it back tells the caller
/// nothing the request does not already say. Every operation that addresses one
/// group answers the same sentence, so a client cannot tell them apart by their
/// 404 text.
pub const GROUP_NOT_FOUND_MESSAGE: &str = "The specified group does not exist";

/// The ARN of the group named `name` in `region`, in the one form every
/// operation of this service reports: `arn:aws:resource-groups:<region>:<account>:group/<name>`.
/// The account segment is the emulator's canonical account id.
pub fn group_arn(region: &str, name: &str) -> String {
    emulator_server::emulator_arn("resource-groups", region, &format!("group/{name}"))
}

/// The id of the account that owns every group this emulator holds: the
/// emulator's canonical account id, which is also the account segment of every
/// ARN [`group_arn`] mints.
pub fn owning_account_id() -> String {
    emulator_server::emulator_account_id().to_string()
}

/// The marker every pagination token this service mints carries, before its
/// listing name and resume key. It is what makes a token verifiable: a string
/// that merely happens to be base64 is not a resume point, and a token issued by
/// one listing is not accepted by another.
const PAGE_TOKEN_PREFIX: &str = "resourcegroups/v1/";

/// Encode a listing's resume point — the key of the last item returned — as the
/// opaque `NextToken` the model describes (`^[a-zA-Z0-9+/]*={0,2}$`, i.e.
/// base64). `listing` names the listing the token belongs to.
pub fn encode_page_token(listing: &str, last_key: &str) -> String {
    aws_smithy_types::base64::encode(format!("{PAGE_TOKEN_PREFIX}{listing}/{last_key}"))
}

/// The resume point a `NextToken` carries, or `None` when the token was not
/// minted by `listing`.
pub fn decode_page_token(listing: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(aws_smithy_types::base64::decode(token).ok()?).ok()?;
    decoded
        .strip_prefix(PAGE_TOKEN_PREFIX)?
        .strip_prefix(listing)?
        .strip_prefix('/')
        .map(str::to_string)
}

/// The CloudFormation-style resource type an ARN names, e.g.
/// `arn:aws:ec2:us-east-1:000000000000:instance/i-0abc` -> `AWS::EC2::Instance`.
///
/// Derived from the ARN's own service segment and resource-type segment through
/// the table below; a resource whose type this table does not name resolves to
/// `None`, and the membership is then reported with no `ResourceType` rather
/// than with a guessed one (the member is optional in `ResourceIdentifier`).
///
/// The table's keys are the ARN formats in the AWS "Actions, resources, and
/// condition keys" service reference; its values are the type names from the AWS
/// CloudFormation resource and property types reference
/// (<https://docs.aws.amazon.com/AWSCloudFormation/latest/UserGuide/aws-template-resource-type-ref.html>),
/// which is the vocabulary `ResourceIdentifier$ResourceType` is documented in
/// ("such as AWS::EC2::Instance") and whose shape its `^AWS::[a-zA-Z0-9]+::\w+$`
/// pattern requires. An empty resource-type key is an ARN whose resource part is
/// a bare id (`arn:aws:sqs:<region>:<account>:<queue-name>`).
const RESOURCE_TYPES: &[(&str, &str, &str)] = &[
    ("cloudformation", "stack", "AWS::CloudFormation::Stack"),
    ("cloudwatch", "alarm", "AWS::CloudWatch::Alarm"),
    ("dynamodb", "table", "AWS::DynamoDB::Table"),
    ("ec2", "instance", "AWS::EC2::Instance"),
    ("ec2", "security-group", "AWS::EC2::SecurityGroup"),
    ("ec2", "snapshot", "AWS::EC2::Snapshot"),
    ("ec2", "subnet", "AWS::EC2::Subnet"),
    ("ec2", "volume", "AWS::EC2::Volume"),
    ("ec2", "vpc", "AWS::EC2::VPC"),
    ("ecs", "cluster", "AWS::ECS::Cluster"),
    ("ecs", "service", "AWS::ECS::Service"),
    ("iam", "role", "AWS::IAM::Role"),
    ("iam", "user", "AWS::IAM::User"),
    ("kms", "key", "AWS::KMS::Key"),
    ("lambda", "function", "AWS::Lambda::Function"),
    ("logs", "log-group", "AWS::Logs::LogGroup"),
    ("rds", "cluster", "AWS::RDS::DBCluster"),
    ("rds", "db", "AWS::RDS::DBInstance"),
    ("resource-groups", "group", "AWS::ResourceGroups::Group"),
    ("s3", "", "AWS::S3::Bucket"),
    ("sns", "", "AWS::SNS::Topic"),
    ("sqs", "", "AWS::SQS::Queue"),
    ("ssm", "document", "AWS::SSM::Document"),
    ("states", "stateMachine", "AWS::StepFunctions::StateMachine"),
];

/// The CloudFormation-style type name for `arn`, per [`RESOURCE_TYPES`].
pub fn resource_type_of_arn(arn: &str) -> Option<&'static str> {
    let (service, resource) = parse_arn(arn)?;
    // `type/id`, `type:id` and a bare `id` are the three resource forms an ARN
    // uses; the bare form has no type segment at all.
    let resource_type = resource
        .find([':', '/'])
        .map_or("", |split| &resource[..split]);
    RESOURCE_TYPES
        .iter()
        .find(|(s, t, _)| *s == service && *t == resource_type)
        .map(|(_, _, cfn)| *cfn)
}

/// Whether `arn` is shaped like the modelled `ResourceArn`
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`).
pub fn is_resource_arn(arn: &str) -> bool {
    parse_arn(arn).is_some()
}

/// The service and resource portions of `arn`, or `None` when the string is not
/// an ARN.
///
/// The shape is the modelled `ResourceArn`'s own:
/// `^arn:aws(-[a-z]+)*:[a-z0-9\-]*:([a-z]{2}(-[a-z]+)+-\d{1})?:([0-9]{12})?:.+$`
/// — six fields; a partition matching `aws(-[a-z]+)*`; a lowercase service; a
/// region that is either empty (a global service, as in `arn:aws:s3:::bucket`) or
/// shaped like an AWS region; an account that is either empty or twelve digits;
/// and a non-empty resource.
fn parse_arn(arn: &str) -> Option<(&str, &str)> {
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    let partition = fields.next()?;
    let service = fields.next()?;
    let region = fields.next()?;
    let account = fields.next()?;
    let resource = fields.next()?;
    if !is_arn_partition(partition) {
        return None;
    }
    if service.is_empty()
        || !service
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return None;
    }
    if !region.is_empty() && !is_arn_region(region) {
        return None;
    }
    if !account.is_empty() && (account.len() != 12 || !account.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    if resource.is_empty() {
        return None;
    }
    Some((service, resource))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsResourcegroupsArdiState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsResourcegroupsArdiState>)
}
