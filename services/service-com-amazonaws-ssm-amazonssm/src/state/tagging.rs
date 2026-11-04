// SPDX-License-Identifier: CC0-1.0

//! The tag plane: the one tag set each resource this store holds carries, and how a
//! request addresses it.
//!
//! Tagging is a single rule that spans resource types. A request names a resource by
//! `(ResourceType, ResourceId)` — "Use the ID of the resource", which for `Document` and
//! `Parameter` is "the name of the resource", for `ManagedInstance` is the `mi-ID_number`
//! node id (`AddTagsToResourceRequest$ResourceId`) and for an `OpsItem` or a
//! `CloudConnector` is the id its create minted — and that pair
//! resolves to the ONE record the Region holds for it. Each such record owns its tag
//! set, so a tag written through `AddTagsToResource` is the tag `ListTagsForResource`
//! reports and the tag `RemoveTagsFromResource` drops, and a resource's tags live with
//! the resource rather than in a second store beside it. A record is reached through
//! [`ComAmazonawsSsmAmazonssmState::resource_tags`] and
//! [`ComAmazonawsSsmAmazonssmState::resource_tags_mut`]; a pair that resolves to none is
//! what the tag operations refuse as an invalid resource id.
//!
//! "Adds or overwrites one or more tags" (`AddTagsToResource`): a key the resource
//! already carries takes the submitted value rather than appearing twice, which is what
//! makes the set a map from key to value ([`merge_tags`]).

use std::collections::BTreeMap;

use crate::ops::types::Tag;

use super::ComAmazonawsSsmAmazonssmState;

/// The `ResourceTypeForTagging` value whose id is a Parameter Store parameter's name
/// (from the modelled enum, `crate::ops::types::RESOURCE_TYPE_FOR_TAGGING_VALUES`).
pub const TAGGING_TYPE_PARAMETER: &str = "Parameter";

/// The `ResourceTypeForTagging` value whose id is an SSM document's name (same enum).
pub const TAGGING_TYPE_DOCUMENT: &str = "Document";

/// The `ResourceTypeForTagging` value whose id is a managed node's id (same enum).
pub const TAGGING_TYPE_MANAGED_INSTANCE: &str = "ManagedInstance";

/// The `ResourceTypeForTagging` value whose id is an OpsItem's own id (same enum).
pub const TAGGING_TYPE_OPS_ITEM: &str = "OpsItem";

/// The `ResourceTypeForTagging` value whose id is a cloud connector's minted id
/// (`super::cloud_connectors::CLOUD_CONNECTOR_RESOURCE_TYPE`, from the same enum).
pub const TAGGING_TYPE_CLOUD_CONNECTOR: &str = super::cloud_connectors::CLOUD_CONNECTOR_RESOURCE_TYPE;

/// How many tags one resource may carry: "Most resources can have a maximum of 50 tags"
/// (`AddTagsToResource`'s own doc). The same doc states one exception — "Automations can
/// have a maximum of 5 tags" — for a resource type whose records this store does not
/// hold, so there is no automation here to measure that lower maximum against.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// A tag set as the modelled `TagList` reports it, in key order — the order the store
/// holds it in, so two reads of one resource report its tags alike.
pub fn to_tag_list(held: &BTreeMap<String, String>) -> Vec<Tag> {
    held.iter()
        .map(|(key, value)| Tag {
            key: key.clone(),
            value: value.clone(),
        })
        .collect()
}

/// Merge `adding` into `held` — "Adds or overwrites one or more tags", so a key already
/// held takes the submitted value and the set grows only by the keys it did not hold.
///
/// Answers `Err` with the number of tags the resource WOULD then carry when that is more
/// than [`MAX_TAGS_PER_RESOURCE`], leaving `held` untouched: the merge is all or nothing,
/// so a refused request writes no part of its tags.
pub fn merge_tags(held: &mut BTreeMap<String, String>, adding: &[Tag]) -> Result<(), usize> {
    let mut merged = held.clone();
    for tag in adding {
        merged.insert(tag.key.clone(), tag.value.clone());
    }
    if merged.len() > MAX_TAGS_PER_RESOURCE {
        return Err(merged.len());
    }
    *held = merged;
    Ok(())
}

/// Drop every key in `keys` from `held`, which is what a remove does to the resource's
/// tag set. A key the resource does not carry leaves the set as it was: the request
/// asks for those keys to be absent, and they are.
pub fn remove_tags(held: &mut BTreeMap<String, String>, keys: &[String]) {
    for key in keys {
        held.remove(key);
    }
}

impl ComAmazonawsSsmAmazonssmState {
    /// The tag set the resource `(resource_type, resource_id)` names in `region`
    /// carries, or `None` when the Region holds no such resource — which is what the tag
    /// operations refuse as an invalid resource id.
    ///
    /// A resource type this store holds no records for resolves to nothing for the same
    /// reason: nothing has created a resource of that type, so no id names one.
    ///
    /// Documents resolve through the records a caller CREATED here. The documents
    /// Systems Manager publishes are owned by Amazon rather than by the account reading
    /// them (`super::PUBLISHED_DOCUMENT_OWNER`), so the account carries no tag set on
    /// one and a published name addresses no resource of its own to tag.
    pub fn resource_tags(
        &self,
        region: &str,
        resource_type: &str,
        resource_id: &str,
    ) -> Option<&BTreeMap<String, String>> {
        match resource_type {
            TAGGING_TYPE_PARAMETER => self.parameter(region, resource_id).map(|held| &held.tags),
            TAGGING_TYPE_DOCUMENT => self.document(region, resource_id).map(|held| &held.tags),
            TAGGING_TYPE_MANAGED_INSTANCE => self
                .managed_node(region, resource_id)
                .map(|held| &held.tags),
            TAGGING_TYPE_OPS_ITEM => self.ops_item(region, resource_id).map(|held| &held.tags),
            TAGGING_TYPE_CLOUD_CONNECTOR => self
                .cloud_connector(region, resource_id)
                .map(|held| &held.tags),
            _ => None,
        }
    }

    /// The same tag set, to write it. See [`ComAmazonawsSsmAmazonssmState::resource_tags`]
    /// for how a request addresses a resource.
    pub fn resource_tags_mut(
        &mut self,
        region: &str,
        resource_type: &str,
        resource_id: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        match resource_type {
            TAGGING_TYPE_PARAMETER => self
                .parameter_mut(region, resource_id)
                .map(|held| &mut held.tags),
            TAGGING_TYPE_DOCUMENT => self
                .document_mut(region, resource_id)
                .map(|held| &mut held.tags),
            TAGGING_TYPE_MANAGED_INSTANCE => self
                .managed_node_mut(region, resource_id)
                .map(|held| &mut held.tags),
            TAGGING_TYPE_OPS_ITEM => self
                .ops_item_mut(region, resource_id)
                .map(|held| &mut held.tags),
            TAGGING_TYPE_CLOUD_CONNECTOR => self
                .cloud_connector_mut(region, resource_id)
                .map(|held| &mut held.tags),
            _ => None,
        }
    }
}
