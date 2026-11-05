// SPDX-License-Identifier: CC0-1.0

//! The tag set a taggable WorkSpaces resource carries, and the three operations
//! that address it.
//!
//! `CreateTags`, `DescribeTags` and `DeleteTags` take a `ResourceId` rather than
//! a resource-kind-specific identifier, so ONE tag set per resource has to serve
//! all three, and it has to be the SAME set the create that minted the resource
//! wrote: `CreateWorkspaces`, `RegisterWorkspaceDirectory`, `CreateIpGroup` and
//! `CreateConnectionAlias` each persist a `Tags` list on their record, so a
//! `DescribeTags` on any of those resources reports the creation-time tags and a
//! `CreateTags` adds to them.
//!
//! Which resources are taggable comes from the model's
//! `CreateTagsRequest$ResourceId` doc — "The supported resource types are
//! WorkSpaces, registered directories, images, custom bundles, IP access control
//! groups, and connection aliases". Of those, this emulator persists WorkSpaces,
//! registered directories, IP access control groups and connection aliases; it
//! holds no image record and no CUSTOM bundle (the bundles in the store are the
//! Amazon-provided catalog, owned by AWS rather than by this account, and
//! `CreateWorkspaceBundle` — which is what mints a custom one — is still a
//! scaffold stub), so no id of those kinds names a tag set here and such a
//! request is refused as a resource that could not be found.

use super::*;

/// The message a tag request naming an id this service holds no resource for is
/// refused with.
///
/// `CreateTags`, `DescribeTags` and `DeleteTags` take a kind-agnostic
/// `ResourceId`, so an id that resolves to nothing names no resource KIND the
/// refusal could name either: the message is the sentence for the class, entirely
/// fixed, and the id the caller asked about travels in the error shape's own
/// `ResourceId` member ("The ID of the resource that could not be found").
pub const MESSAGE_TAG_RESOURCE_NOT_FOUND: &str = "The resource could not be found.";

/// Why a tag request could not be served, as the modelled error the handler
/// answers with.
#[derive(Debug, Clone)]
pub enum TagRefusal {
    /// No stored resource answers to the `ResourceId`:
    /// `ResourceNotFoundException`.
    NotFound,
    /// The resource would be left holding more than [`MAX_TAGS_PER_RESOURCE`]
    /// tags: `ResourceLimitExceededException`.
    LimitExceeded,
}

impl ComAmazonawsWorkspacesWorkspacesserviceState {
    /// The tag set `resource_id` names, for reading.
    ///
    /// `None` means no stored resource answers to the id — which is what
    /// separates "the resource holds no tags" (an empty set) from "there is no
    /// such resource" (a refusal).
    pub fn resource_tags(&self, resource_id: &str) -> Option<&[Tag]> {
        if let Some(record) = self.workspaces.get(resource_id) {
            return Some(&record.tags);
        }
        if let Some(record) = self.ip_groups.get(resource_id) {
            return Some(&record.tags);
        }
        if let Some(record) = self.connection_aliases.get(resource_id) {
            return Some(&record.tags);
        }
        if let Some(record) = self.directories.get(resource_id) {
            return Some(&record.tags);
        }
        None
    }

    /// The tag set `resource_id` names, for writing. See [`Self::resource_tags`].
    fn resource_tags_mut(&mut self, resource_id: &str) -> Option<&mut Vec<Tag>> {
        if let Some(record) = self.workspaces.get_mut(resource_id) {
            return Some(&mut record.tags);
        }
        if let Some(record) = self.ip_groups.get_mut(resource_id) {
            return Some(&mut record.tags);
        }
        if let Some(record) = self.connection_aliases.get_mut(resource_id) {
            return Some(&mut record.tags);
        }
        if let Some(record) = self.directories.get_mut(resource_id) {
            return Some(&mut record.tags);
        }
        None
    }

    /// Create `tags` on the resource `resource_id` names.
    ///
    /// A key the resource already holds has its value REPLACED rather than
    /// doubled: a tag key identifies at most one tag on a resource, so the stored
    /// set holds one entry per key. A key the resource does not hold is appended,
    /// which keeps the order a reader reports stable across calls. Within one
    /// request the last entry for a key wins, for the same reason.
    ///
    /// The whole request is applied or none of it is: a request that would leave
    /// the resource holding more than [`MAX_TAGS_PER_RESOURCE`] tags is refused
    /// with nothing written, and refused as soon as the key that crosses the
    /// limit is seen rather than after the whole list has been walked, so the
    /// work a request costs is bounded by the limit and not by its own size.
    /// `tags` is itself no longer than the limit: a caller's list is bounded
    /// before this is reached (see `crate::ops::create_tags`).
    pub fn create_resource_tags(
        &mut self,
        resource_id: &str,
        tags: &[Tag],
    ) -> Result<(), TagRefusal> {
        let held = self
            .resource_tags_mut(resource_id)
            .ok_or(TagRefusal::NotFound)?;

        // Only a requested key the resource does not hold yet grows the set,
        // since a key it already holds is replaced in place. The set is at most
        // the limit large, and the membership test is logarithmic rather than a
        // scan, so collecting the keys costs no more than the limit allows.
        let mut added_keys: BTreeSet<&str> = BTreeSet::new();
        for tag in tags {
            if held.iter().any(|stored| stored.key == tag.key) {
                continue;
            }
            added_keys.insert(tag.key.as_str());
            if held.len() + added_keys.len() > MAX_TAGS_PER_RESOURCE {
                return Err(TagRefusal::LimitExceeded);
            }
        }

        for tag in tags {
            match held.iter_mut().find(|stored| stored.key == tag.key) {
                Some(stored) => stored.value = tag.value.clone(),
                None => held.push(tag.clone()),
            }
        }
        Ok(())
    }

    /// Delete the tags `tag_keys` names from the resource `resource_id` names, and
    /// report whether a stored resource answered to the id.
    ///
    /// A key the resource does not hold removes nothing: `DeleteTags` names the
    /// keys to be rid of, so a resource that is already rid of one is in the
    /// state the caller asked for and the call is not a refusal.
    pub fn delete_resource_tags(&mut self, resource_id: &str, tag_keys: &[String]) -> bool {
        let Some(held) = self.resource_tags_mut(resource_id) else {
            return false;
        };
        held.retain(|stored| !tag_keys.contains(&stored.key));
        true
    }
}
