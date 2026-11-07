// SPDX-License-Identifier: CC0-1.0

//! The TAGGING slice: the bounds a tag map is held to, the reserved tag-key
//! namespace, which resource an ARN names and the reads and writes of one
//! resource's tags.
//!
//! One file for the whole slice, so the tagging operations and every create
//! that carries `Tags` cannot drift into several readings of one rule. Moved
//! here unchanged from the state root, which re-exports it
//! (`pub use tags::*;`), so every `crate::state::…` path a handler already uses
//! still resolves.

use super::*;

/// The most tags one resource carries.
///
/// AWS's documented tag restriction for a MediaLive resource ("Tagging AWS
/// Elemental MediaLive Resources": "Maximum number of tags per resource — 50"),
/// and at the same time the ceiling that keeps `CreateTags` from growing the
/// emulator's heap without bound: the keys it writes are caller-chosen, so
/// without a cap a create loop against one resource would accumulate forever.
/// `CreateTags` models `BadRequestException`, which is the refusal once a
/// resource is full.
pub const MAX_RESOURCE_TAGS: usize = 50;

/// The longest tag key a resource stores, from the same AWS tag restrictions
/// ("Maximum key length — 128 Unicode characters"). The model declares no
/// constraint on `Tags` (`smithy_query(query="shape", shape="Tags")`: a plain
/// `__string` to `__string` map), so the boundary validator cannot bound it and
/// this is the service-side rule.
pub const MAX_TAG_KEY_LEN: usize = 128;

/// The longest tag value a resource stores, from the same AWS tag restrictions
/// ("Maximum value length — 256 Unicode characters"), unbounded by the model for
/// the same reason as [`MAX_TAG_KEY_LEN`].
pub const MAX_TAG_VALUE_LEN: usize = 256;

/// The tag-key prefix AWS keeps for its own, system-applied tags.
///
/// The same AWS tag restrictions reserve it ("Do not use the aws: prefix in your
/// tag names or values because it is reserved for AWS use. You can't edit or
/// delete tag names or values with this prefix"), so a tag key carrying it is
/// not a key a caller owns: a request asking to create, change or remove one is
/// refused and the resource's stored tags are left as they were.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` lies in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace.
///
/// The prefix is matched without regard to case, so the namespace cannot be
/// reached through a differently cased spelling of it.
pub fn is_reserved_tag_key(key: &str) -> bool {
    let mut characters = key.chars();
    RESERVED_TAG_KEY_PREFIX.chars().all(|expected| {
        characters
            .next()
            .is_some_and(|got| got.eq_ignore_ascii_case(&expected))
    })
}

/// The reserved key among `keys`, when they carry one.
///
/// The SMALLEST such key, so a request naming several is refused with the same
/// message however the map it came out of happened to be iterated.
pub fn first_reserved_tag_key<'k, I>(keys: I) -> Option<&'k str>
where
    I: IntoIterator<Item = &'k String>,
{
    keys.into_iter()
        .map(String::as_str)
        .filter(|key| is_reserved_tag_key(key))
        .min()
}

/// The reserved key a request's `Tags` member carries, when it carries one.
///
/// What an operation that stores a caller's whole tag map itself — a create, an
/// update — checks before it writes, so the reserved namespace is refused
/// wherever a tag key reaches the store and not only on the tag operations.
pub fn reserved_tag_key_in(tags: Option<&HashMap<String, String>>) -> Option<&str> {
    first_reserved_tag_key(tags.into_iter().flat_map(HashMap::keys))
}

/// What to tell a caller that named a key in the reserved namespace.
///
/// One wording for every operation that can be handed such a key, so the tag
/// operations and the creates cannot drift into several readings of one
/// restriction.
pub fn reserved_tag_key_message(key: &str) -> String {
    format!(
        "Tag key '{key}' cannot start with '{RESERVED_TAG_KEY_PREFIX}'. \
         That prefix is reserved for AWS use."
    )
}

impl ComAmazonawsMedialiveMedialiveState {
    /// Which of this store's taggable resources `arn` names, if it holds one.
    ///
    /// The tag operations address a resource ONLY by its ARN, and every resource
    /// this store holds carries its own tag map, so the ARN is resolved once
    /// here and both the read ([`Self::resource_tags`]) and the write
    /// ([`Self::resource_tags_mut`]) go through the same resolution: a tag
    /// written through one of them is the tag the resource's own describe
    /// reports, because there is no second copy to write.
    pub(super) fn tagged_resource(&self, arn: &str) -> Option<TaggedResource> {
        if let Some(record) = self.channel_by_arn(arn) {
            return Some(TaggedResource::Channel(record.id.clone()));
        }
        if let Some(record) = self.input_by_arn(arn) {
            return Some(TaggedResource::Input(record.id.clone()));
        }
        if let Some(record) = self
            .input_security_groups
            .values()
            .find(|record| record.arn == arn)
        {
            return Some(TaggedResource::InputSecurityGroup(record.id.clone()));
        }
        if let Some(record) = self.sdi_sources.values().find(|record| record.arn == arn) {
            return Some(TaggedResource::SdiSource(record.id.clone()));
        }
        if let Some(record) = self.networks.values().find(|record| record.arn == arn) {
            return Some(TaggedResource::Network(record.id.clone()));
        }
        if let Some(record) = self.signal_maps.values().find(|record| record.arn == arn) {
            return Some(TaggedResource::SignalMap(record.id.clone()));
        }
        if let Some(record) = self.multiplexes.values().find(|record| record.arn == arn) {
            return Some(TaggedResource::Multiplex(record.id.clone()));
        }
        if let Some(record) = self.nodes.values().find(|record| record.arn == arn) {
            return Some(TaggedResource::Node(record.id.clone()));
        }
        if let Some(record) = self.input_devices.values().find(|record| record.arn == arn) {
            return Some(TaggedResource::InputDevice(record.id.clone()));
        }
        if let Some(record) = self
            .alarm_template_groups
            .values()
            .find(|record| record.arn == arn)
        {
            return Some(TaggedResource::AlarmTemplateGroup(record.id.clone()));
        }
        if let Some(record) = self
            .event_bridge_rule_template_groups
            .values()
            .find(|record| record.arn == arn)
        {
            return Some(TaggedResource::EventBridgeRuleTemplateGroup(
                record.id.clone(),
            ));
        }
        // The monitor TEMPLATES, each a taggable resource of its own with its
        // own ARN and its own stored tag map, exactly like the GROUPS above:
        // the tags a template create supplied are readable, writable and
        // removable through the generic tag operations, not only through the
        // template's own reader.
        if let Some(record) = self.alarm_templates.values().find(|record| record.arn == arn) {
            return Some(TaggedResource::AlarmTemplate(record.id.clone()));
        }
        if let Some(record) = self
            .event_bridge_rule_templates
            .values()
            .find(|record| record.arn == arn)
        {
            return Some(TaggedResource::EventBridgeRuleTemplate(record.id.clone()));
        }
        if let Some(record) = self.clusters.values().find(|record| record.arn == arn) {
            return Some(TaggedResource::Cluster(record.id.clone()));
        }
        if let Some(record) = self
            .channel_placement_groups
            .values()
            .find(|record| record.arn == arn)
        {
            return Some(TaggedResource::ChannelPlacementGroup(record.id.clone()));
        }
        if let Some(record) = self.reservations.values().find(|record| record.arn == arn) {
            return Some(TaggedResource::Reservation(record.id.clone()));
        }
        None
    }

    /// The tags stored on the resource `arn` names, or `None` when this store
    /// holds no resource with that ARN.
    pub fn resource_tags(&self, arn: &str) -> Option<&HashMap<String, String>> {
        match self.tagged_resource(arn)? {
            TaggedResource::Channel(id) => self.channels.get(&id).map(|record| &record.tags),
            TaggedResource::Input(id) => self.inputs.get(&id).map(|record| &record.tags),
            TaggedResource::InputSecurityGroup(id) => self
                .input_security_groups
                .get(&id)
                .map(|record| &record.tags),
            TaggedResource::SdiSource(id) => self.sdi_sources.get(&id).map(|record| &record.tags),
            TaggedResource::Network(id) => self.networks.get(&id).map(|record| &record.tags),
            TaggedResource::SignalMap(id) => self.signal_maps.get(&id).map(|record| &record.tags),
            TaggedResource::Multiplex(id) => self.multiplexes.get(&id).map(|record| &record.tags),
            TaggedResource::Node(id) => self.nodes.get(&id).map(|record| &record.tags),
            TaggedResource::InputDevice(id) => {
                self.input_devices.get(&id).map(|record| &record.tags)
            }
            TaggedResource::AlarmTemplateGroup(id) => self
                .alarm_template_groups
                .get(&id)
                .map(|record| &record.tags),
            TaggedResource::EventBridgeRuleTemplateGroup(id) => self
                .event_bridge_rule_template_groups
                .get(&id)
                .map(|record| &record.tags),
            TaggedResource::AlarmTemplate(id) => {
                self.alarm_templates.get(&id).map(|record| &record.tags)
            }
            TaggedResource::EventBridgeRuleTemplate(id) => self
                .event_bridge_rule_templates
                .get(&id)
                .map(|record| &record.tags),
            TaggedResource::Cluster(id) => self.clusters.get(&id).map(|record| &record.tags),
            TaggedResource::ChannelPlacementGroup(id) => self
                .channel_placement_groups
                .get(&id)
                .map(|record| &record.tags),
            TaggedResource::Reservation(id) => {
                self.reservations.get(&id).map(|record| &record.tags)
            }
        }
    }

    /// The tag map of the resource `arn` names, to write through, or `None`
    /// when this store holds no resource with that ARN that is still live.
    ///
    /// The map handed back is the resource's OWN, so a tag written here is on
    /// the resource itself: the resource's describe and
    /// [`Self::resource_tags`] read the one map rather than two copies that
    /// could drift.
    ///
    /// A resource whose delete has been accepted is NOT handed out: tagging and
    /// untagging change a resource's configuration, so they resolve the ARN
    /// through the same live lookup every other mutation uses
    /// ([`Self::live_input`]) and the tag writers answer a dead ARN with their
    /// modelled not-found error. The READ path ([`Self::resource_tags`]) keeps
    /// resolving a deleted resource, whose record stays in the account.
    ///
    /// The INPUT SECURITY GROUP is handed out even once it is deleted, because
    /// its delete is a SOFT delete: the group stays addressable and stays
    /// mutable (see the module docs), so its tags are writable exactly as its
    /// `WhitelistRules` are.
    ///
    /// PRIVATE: the per-resource tag bounds are invariants of
    /// [`Self::insert_resource_tags`], which is the only way a tag is written,
    /// so no handler can grow a resource's tag map past them.
    pub(super) fn resource_tags_mut(&mut self, arn: &str) -> Option<&mut HashMap<String, String>> {
        match self.tagged_resource(arn)? {
            TaggedResource::Channel(id) => self
                .channels
                .get_mut(&id)
                .filter(|record| !record.is_delete_accepted())
                .map(|record| &mut record.tags),
            TaggedResource::Input(id) => self
                .inputs
                .get_mut(&id)
                .filter(|record| !record.is_delete_accepted())
                .map(|record| &mut record.tags),
            TaggedResource::InputSecurityGroup(id) => self
                .input_security_groups
                .get_mut(&id)
                .map(|record| &mut record.tags),
            TaggedResource::SdiSource(id) => self
                .sdi_sources
                .get_mut(&id)
                .filter(|record| !record.is_deleted())
                .map(|record| &mut record.tags),
            TaggedResource::Network(id) => self
                .networks
                .get_mut(&id)
                .filter(|record| !record.is_delete_accepted())
                .map(|record| &mut record.tags),
            TaggedResource::SignalMap(id) => self
                .signal_maps
                .get_mut(&id)
                .map(|record| &mut record.tags),
            TaggedResource::Multiplex(id) => self
                .multiplexes
                .get_mut(&id)
                .map(|record| &mut record.tags),
            TaggedResource::Node(id) => self
                .nodes
                .get_mut(&id)
                .filter(|record| !record.is_delete_accepted())
                .map(|record| &mut record.tags),
            // A claimed device is never deleted — nothing in this service
            // un-claims one — so there is no dead id to filter out here.
            TaggedResource::InputDevice(id) => self
                .input_devices
                .get_mut(&id)
                .map(|record| &mut record.tags),
            // The monitor template resources the SERVICE publishes are read-only
            // for the account, tags included: they are not its resources, so a
            // tag write is not handed their stored map and the tag writers answer
            // the ARN with their modelled not-found error.
            TaggedResource::AlarmTemplateGroup(id) => self
                .alarm_template_groups
                .get_mut(&id)
                .filter(|record| !is_published_id(&record.id))
                .map(|record| &mut record.tags),
            TaggedResource::EventBridgeRuleTemplateGroup(id) => self
                .event_bridge_rule_template_groups
                .get_mut(&id)
                .map(|record| &mut record.tags),
            // No delete filter on either template: a deleted template is
            // REMOVED from its store, so its ARN resolves to nothing at all and
            // never reaches this match.
            TaggedResource::AlarmTemplate(id) => self
                .alarm_templates
                .get_mut(&id)
                .filter(|record| !is_published_id(&record.id))
                .map(|record| &mut record.tags),
            TaggedResource::EventBridgeRuleTemplate(id) => self
                .event_bridge_rule_templates
                .get_mut(&id)
                .map(|record| &mut record.tags),
            TaggedResource::Cluster(id) => self
                .clusters
                .get_mut(&id)
                .filter(|record| !record.is_delete_accepted())
                .map(|record| &mut record.tags),
            TaggedResource::ChannelPlacementGroup(id) => self
                .channel_placement_groups
                .get_mut(&id)
                .filter(|record| !record.is_delete_accepted())
                .map(|record| &mut record.tags),
            // A reservation this account has DELETED is a tombstone: its record
            // stays readable but names no resource a caller may still change, so
            // its tags are no longer writable either.
            TaggedResource::Reservation(id) => self
                .reservations
                .get_mut(&id)
                .filter(|record| !record.is_deleted())
                .map(|record| &mut record.tags),
        }
    }

    /// Add `tags` to the resource `arn` names, bounded.
    ///
    /// The bounds and the reserved key namespace live here rather than in the
    /// handler, so no caller-driven write can grow a resource's tag map past
    /// them or plant a system tag in it: every refusal is checked BEFORE the
    /// first insert, and a refused request leaves the stored tags exactly as
    /// they were.
    pub fn insert_resource_tags(
        &mut self,
        arn: &str,
        tags: HashMap<String, String>,
    ) -> Result<(), TagRefusal> {
        // Request shape first — a key a caller may not name at all, or a key or
        // value past its length, is refused whatever resource it was aimed at.
        if let Some(key) = first_reserved_tag_key(tags.keys()) {
            return Err(TagRefusal::ReservedKeyPrefix {
                key: key.to_string(),
            });
        }
        for (key, value) in &tags {
            if key.chars().count() > MAX_TAG_KEY_LEN {
                return Err(TagRefusal::KeyTooLong {
                    length: key.chars().count(),
                });
            }
            if value.chars().count() > MAX_TAG_VALUE_LEN {
                return Err(TagRefusal::ValueTooLong {
                    key: key.clone(),
                    length: value.chars().count(),
                });
            }
        }

        let stored = self
            .resource_tags_mut(arn)
            .ok_or(TagRefusal::NoSuchResource)?;

        // What the resource would carry afterwards: the keys it already has,
        // plus the ones this request introduces. A key being overwritten adds
        // nothing, so re-tagging a full resource is not refused.
        let added = tags
            .keys()
            .filter(|key| !stored.contains_key(*key))
            .count();
        let merged = stored.len() + added;
        if merged > MAX_RESOURCE_TAGS {
            return Err(TagRefusal::TooManyTags { merged });
        }

        for (key, value) in tags {
            stored.insert(key, value);
        }
        Ok(())
    }

    /// Remove the tags `keys` names from the resource `arn` names.
    ///
    /// Only ever shrinks a tag map, so it needs no bound of its own; a key the
    /// resource does not carry leaves it in the state the caller asked for, so
    /// naming one is not a refusal. What it does refuse is a key in the
    /// reserved [`RESERVED_TAG_KEY_PREFIX`] namespace, which is not the
    /// caller's to remove, and an ARN this store holds no resource for.
    pub fn remove_resource_tags(
        &mut self,
        arn: &str,
        keys: &[String],
    ) -> Result<(), TagRefusal> {
        // A system tag is not the caller's to remove either, so the reserved
        // namespace is refused here exactly as it is on the write path —
        // before resolution, and before anything is removed.
        if let Some(key) = first_reserved_tag_key(keys.iter()) {
            return Err(TagRefusal::ReservedKeyPrefix {
                key: key.to_string(),
            });
        }
        let stored = self
            .resource_tags_mut(arn)
            .ok_or(TagRefusal::NoSuchResource)?;
        for key in keys {
            stored.remove(key);
        }
        Ok(())
    }
}

/// Why [`ComAmazonawsMedialiveMedialiveState::insert_resource_tags`] stored
/// nothing. Each variant is one refusal a tag write can answer with.
#[derive(Debug)]
pub enum TagRefusal {
    /// No resource in this store carries that ARN.
    NoSuchResource,
    /// A tag key lies in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace, so
    /// it is not a key a caller may create, change or remove. Carries the key,
    /// so the refusal names it.
    ReservedKeyPrefix { key: String },
    /// The merged tag set would push the resource past
    /// [`MAX_RESOURCE_TAGS`]. Carries the count it would have reached.
    TooManyTags { merged: usize },
    /// A tag key is longer than [`MAX_TAG_KEY_LEN`].
    KeyTooLong { length: usize },
    /// A tag value is longer than [`MAX_TAG_VALUE_LEN`]. Carries the key it
    /// belongs to, so the refusal names the offending member.
    ValueTooLong { key: String, length: usize },
}

/// One of this store's taggable resources: which store holds it and the key it
/// is stored under.
///
/// Returned by [`ComAmazonawsMedialiveMedialiveState::tagged_resource`] as an
/// OWNED key rather than a borrow of the record, so the same resolution serves
/// the shared and the mutable accessor.
#[derive(Debug, Clone)]
pub(super) enum TaggedResource {
    Channel(String),
    Input(String),
    InputSecurityGroup(String),
    SdiSource(String),
    Network(String),
    SignalMap(String),
    Multiplex(String),
    Node(String),
    InputDevice(String),
    AlarmTemplateGroup(String),
    EventBridgeRuleTemplateGroup(String),
    AlarmTemplate(String),
    EventBridgeRuleTemplate(String),
    Cluster(String),
    ChannelPlacementGroup(String),
    Reservation(String),
}
