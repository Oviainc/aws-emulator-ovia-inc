// SPDX-License-Identifier: CC0-1.0

//! UPDATING a registered thing: what a change to a thing record asks for, what refuses
//! one, and the single registry write that applies it.
//!
//! `UpdateThing` "Updates the data for a thing": it addresses a thing by the name it was
//! registered under ("You can't change a thing's name"), and changes the two things about
//! a thing that are not its identity — the thing type it is associated with and its
//! attributes — under OPTIMISTIC CONCURRENCY: "The expected version of the thing record
//! in the registry. If the version of the record in the registry does not match the
//! expected version specified in the request, the UpdateThing request is rejected with a
//! VersionConflictException."
//!
//! The write lives here rather than in the handler for the same reason
//! [`super::ComAmazonawsIotAwsiotserviceState::register_thing`] does: it is the registry's
//! own rule about which changes a thing record takes, and a handler maps the refusal onto
//! the error ITS model declares.

use std::collections::BTreeMap;

use super::{
    missing_thing_message, thing_version_conflict_message, ComAmazonawsIotAwsiotserviceState,
    RegisteredThing, ThingRegistrationRefusal,
};

/// The attribute list one update provides, and what the registry does with it.
///
/// `AttributePayload$merge` decides that: "Specifies whether the list of attributes
/// provided in the `AttributePayload` is merged with the attributes stored in the
/// registry, instead of overwriting them." So the list either merges into what the record
/// holds or replaces it wholesale — and in both cases "To remove an attribute, call
/// `UpdateThing` with an empty attribute value", so an entry whose value is empty takes
/// the attribute out of the record rather than storing a blank one.
#[derive(Debug, Clone, Default)]
pub struct AttributeUpdate {
    /// The attributes the request provided, ordered the way the registry keeps them.
    pub attributes: BTreeMap<String, String>,
    /// `AttributePayload$merge`: merge the list into the stored attributes instead of
    /// overwriting them.
    pub merge: bool,
}

impl AttributeUpdate {
    /// Apply the provided list to `attributes`, the attributes a record holds — a thing's
    /// or a thing group's, since `merge` is modelled for `UpdateThing` and
    /// `UpdateThingGroup` alike.
    pub fn apply_to(&self, attributes: &mut BTreeMap<String, String>) {
        if !self.merge {
            // Not a merge: the provided list is what the thing has afterwards, so
            // whatever the record held and the list does not name is gone.
            attributes.clear();
        }
        for (name, value) in &self.attributes {
            if value.is_empty() {
                attributes.remove(name);
            } else {
                attributes.insert(name.clone(), value.clone());
            }
        }
    }
}

/// What an update asks the registry to change about one thing.
///
/// It is a value of its own, like [`super::ThingConfiguration`], because an update is NOT
/// a configuration: each member says what to do with one part of the record, and a member
/// the request left out changes nothing. `attributes` is `None` for a request that
/// provided no attribute list at all, which is a different request from one providing the
/// EMPTY list (that one replaces the record's attributes with none).
#[derive(Debug, Clone, Default)]
pub struct ThingUpdate {
    /// `UpdateThing$thingTypeName` — the thing type to associate the thing with.
    pub thing_type_name: Option<String>,
    /// `UpdateThing$removeThingType` — "Remove a thing type association. If true, the
    /// association is removed."
    pub remove_thing_type: bool,
    /// The attribute list the request provided, with its merge flag; see
    /// [`AttributeUpdate`].
    pub attributes: Option<AttributeUpdate>,
    /// `UpdateThing$expectedVersion` — the version the caller believes the record is at.
    pub expected_version: Option<i64>,
}

/// Why a thing record cannot take an update — what
/// [`ComAmazonawsIotAwsiotserviceState::update_thing`] answers instead of the updated
/// record.
///
/// It is the registry's reason, not a wire error: the handler maps it onto the error its
/// own model declares, and the wording lives here so every writer states one rule once.
#[derive(Debug, Clone)]
pub enum ThingUpdateRefusal {
    /// `thingName` names no thing in the registry.
    MissingThing(String),
    /// `expectedVersion` does not match the version the record holds, so the caller is
    /// updating a record it has not seen.
    VersionConflict {
        /// The thing the request addressed.
        thing_name: String,
        /// The version the request expected.
        expected: i64,
        /// The version the registry holds.
        registered: i64,
    },
    /// The request both names a thing type and asks for the association to be removed, so
    /// it states two contradictory things about one member of the record.
    ContradictoryThingType,
    /// `thingTypeName` names no type the thing may be associated with — see
    /// [`ComAmazonawsIotAwsiotserviceState::resolve_associable_thing_type`]. The deprecated
    /// case is reported in the wording of THIS call, which associates a type with a thing
    /// that already exists rather than creating one.
    ThingType(ThingRegistrationRefusal),
}

impl ThingUpdateRefusal {
    /// What the refusal reports, in the wording AWS IoT states each of these in.
    pub fn message(&self) -> String {
        match self {
            Self::MissingThing(thing_name) => missing_thing_message(thing_name),
            Self::VersionConflict {
                thing_name,
                expected,
                registered,
            } => thing_version_conflict_message(thing_name, *expected, *registered),
            Self::ContradictoryThingType => {
                "Can not remove thing type and set thing type in the same request".to_string()
            }
            Self::ThingType(ThingRegistrationRefusal::DeprecatedThingType(thing_type_name)) => {
                format!("Can not update thing with deprecated thing type:{thing_type_name}")
            }
            Self::ThingType(refusal) => refusal.message(),
        }
    }
}

impl ComAmazonawsIotAwsiotserviceState {
    /// Apply `update` to the thing registered as `thing_name`: the ONE registry write that
    /// changes a thing record, answering the record as it stands afterwards.
    ///
    /// Everything that can refuse the update is decided BEFORE the record is touched, so a
    /// refused update leaves the thing exactly as it was — the point of
    /// `expectedVersion`. A successful one advances the record's `version`, which is "The
    /// current version of the thing record in the registry" a later reader reports and a
    /// later `expectedVersion` has to match.
    pub fn update_thing(
        &mut self,
        thing_name: &str,
        update: ThingUpdate,
    ) -> Result<RegisteredThing, ThingUpdateRefusal> {
        let registered = self
            .things
            .get(thing_name)
            .ok_or_else(|| ThingUpdateRefusal::MissingThing(thing_name.to_string()))?;
        if let Some(expected) = update.expected_version {
            if registered.version != expected {
                return Err(ThingUpdateRefusal::VersionConflict {
                    thing_name: registered.thing_name.clone(),
                    expected,
                    registered: registered.version,
                });
            }
        }
        if update.remove_thing_type && update.thing_type_name.is_some() {
            return Err(ThingUpdateRefusal::ContradictoryThingType);
        }
        if let Some(thing_type_name) = &update.thing_type_name {
            self.resolve_associable_thing_type(thing_type_name)
                .map_err(ThingUpdateRefusal::ThingType)?;
        }

        let thing = self
            .things
            .get_mut(thing_name)
            .ok_or_else(|| ThingUpdateRefusal::MissingThing(thing_name.to_string()))?;
        if let Some(thing_type_name) = update.thing_type_name {
            thing.configuration.thing_type_name = Some(thing_type_name);
        } else if update.remove_thing_type {
            thing.configuration.thing_type_name = None;
        }
        if let Some(attributes) = &update.attributes {
            attributes.apply_to(&mut thing.configuration.attributes);
        }
        // One update is one write of the record, so the registry version it carries
        // advances by one — which is what a caller holding the old version then conflicts
        // against.
        thing.version += 1;
        Ok(thing.clone())
    }
}
