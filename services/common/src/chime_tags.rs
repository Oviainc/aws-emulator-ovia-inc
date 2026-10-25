// SPDX-License-Identifier: CC0-1.0

//! The TAG directory the Amazon Chime SDK family shares: which tags are on which Chime
//! resource.
//!
//! Five of the six Chime services bind the SAME three generic tagging routes
//! (`GET /tags?arn=…`, `POST /tags?operation=tag-resource`,
//! `POST /tags?operation=untag-resource`) over resources of their own, and the only thing
//! in a tagging request that says which service's resource it is about is the resource ARN
//! — which travels in the request BODY for the write operations, where a claim predicate
//! reading [`http::request::Parts`] cannot see it. So a request that names no family
//! endpoint host has to be served by ONE member whichever resource it names
//! ([`crate::chime_family::claims_shared_tagging_space`]), and that is answerable only
//! because the tags themselves live HERE, keyed by resource ARN, instead of inside one
//! service's own state struct:
//!
//! * the engine keys state by TYPE, so one directory under one shared type is visible to
//!   every member; a copy inside a service's own state would be invisible to the member
//!   that serves the call, which would then report "no tags" for a resource that has them;
//! * a tag written through whichever member serves the write is read back by whichever
//!   member serves the read, so the three routes are one consistent read-write set.
//!
//! Nothing here names a wire shape: the directory holds resource ARNs and their key/value
//! pairs, and each reader projects them onto the `Tag` shape ITS model declares.
//!
//! The directory is a record of what this emulator TAGGED, so it is partial by
//! construction: a resource it holds no entry for simply has no tags. A member that still
//! keeps create-time tags in its own state has not been migrated onto it yet — tags given
//! at create time are visible to a tagging call only once that member writes them here.

use std::collections::{BTreeMap, VecDeque};

/// How many tags one resource carries at once.
///
/// From the models themselves: every Chime `TagList` is `@length(min: 1, max: 50)`
/// (`smithy_query(query="operation",operation="TagResource")` on any of the five), so a
/// resource cannot hold more than 50 tags and a request that would take it past that is
/// refused with the resource-limit error those models declare.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// How many distinct resources the directory holds tags for at once.
///
/// An EMULATOR ceiling, not an AWS quota: the directory lives in memory for the lifetime
/// of the process, so without a bound a caller tagging fresh ARNs in a loop would grow it
/// without end. Past it the OLDEST tagged resource is dropped — never a newer one — so the
/// resources a running workflow just tagged still report their tags.
pub const MAX_TAGGED_RESOURCES: usize = 10_000;

/// What [`ChimeTagDirectory::tag`] did, so the caller maps it to the outcome its model
/// declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tagged {
    /// The pairs were stored on the resource.
    Stored,
    /// Storing them would take the resource past [`MAX_TAGS_PER_RESOURCE`]; nothing was
    /// written. Carries how many tags the resource would have ended up with.
    TooManyTags(usize),
}

/// The directory itself: the tags of every tagged resource, keyed by resource ARN.
///
/// `BTreeMap` (not `HashMap`) for the tags of one resource, so a listing walks them in a
/// stable key order rather than a per-process random one.
#[derive(Debug, Default)]
pub struct ChimeTagDirectory {
    tags: BTreeMap<String, BTreeMap<String, String>>,
    /// The order resources were first tagged in, so the oldest can be dropped first.
    order: VecDeque<String>,
}

impl ChimeTagDirectory {
    /// Apply `pairs` to `resource_arn`: a key the resource does not carry is added, and a
    /// key it already carries is RE-VALUED — tagging is a merge, not a replacement, so
    /// tags a previous call set are kept.
    ///
    /// Nothing is written when the merged set would exceed [`MAX_TAGS_PER_RESOURCE`], so a
    /// refused call leaves the resource's tags exactly as they were.
    pub fn tag<I>(&mut self, resource_arn: &str, pairs: I) -> Tagged
    where
        I: IntoIterator<Item = (String, String)>,
    {
        let mut merged = self.tags.get(resource_arn).cloned().unwrap_or_default();
        for (key, value) in pairs {
            merged.insert(key, value);
        }
        if merged.len() > MAX_TAGS_PER_RESOURCE {
            return Tagged::TooManyTags(merged.len());
        }
        if self.tags.insert(resource_arn.to_string(), merged).is_none() {
            self.order.push_back(resource_arn.to_string());
            while self.order.len() > MAX_TAGGED_RESOURCES {
                if let Some(oldest) = self.order.pop_front() {
                    self.tags.remove(&oldest);
                }
            }
        }
        Tagged::Stored
    }

    /// Remove the tags `keys` names from `resource_arn`, and answer how many were
    /// actually removed.
    ///
    /// A key the resource does not carry is not an error: untagging is stated as the
    /// absence of those keys afterwards, so repeating one call answers the same as making
    /// it once.
    pub fn untag(&mut self, resource_arn: &str, keys: &[String]) -> usize {
        let Some(tags) = self.tags.get_mut(resource_arn) else {
            return 0;
        };
        let before = tags.len();
        tags.retain(|key, _| !keys.iter().any(|dropped| dropped == key));
        let removed = before - tags.len();
        if tags.is_empty() {
            self.forget(resource_arn);
        }
        removed
    }

    /// The tags of `resource_arn`, in key order; empty for a resource the directory holds
    /// no entry for.
    pub fn tags_of(&self, resource_arn: &str) -> Vec<(String, String)> {
        self.tags
            .get(resource_arn)
            .map(|tags| {
                tags.iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Drop every tag of `resource_arn` — what the service that owns the resource calls
    /// when the resource itself is deleted, so a deleted resource leaves no tags behind.
    /// Answers whether the directory held an entry for it.
    pub fn forget(&mut self, resource_arn: &str) -> bool {
        self.order.retain(|held| held != resource_arn);
        self.tags.remove(resource_arn).is_some()
    }

    /// How many resources the directory currently holds tags for.
    pub fn tagged_resources(&self) -> usize {
        self.tags.len()
    }
}

#[cfg(test)]
mod tests {
    //! The directory over what the tagging rule generalises over: how many pairs one call
    //! carries, a key written twice, keys removed one by one, a key that was never there,
    //! and the two bounds.

    use super::*;

    fn pairs(entries: &[(&str, &str)]) -> Vec<(String, String)> {
        entries
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn tagging_merges_and_untagging_removes_only_the_keys_it_names() {
        let mut directory = ChimeTagDirectory::default();
        let arn = "arn:aws:chime:us-east-1:000000000000:app-instance/ai1";

        assert_eq!(
            directory.tag(arn, pairs(&[("env", "dev"), ("team", "core")])),
            Tagged::Stored,
            "case a first tagging call: expected it stored"
        );
        assert_eq!(
            directory.tags_of(arn),
            pairs(&[("env", "dev"), ("team", "core")]),
            "case a first tagging call: expected both pairs in key order"
        );

        // A second call merges: the repeated key is re-valued, the new one added, and the
        // untouched one kept.
        directory.tag(arn, pairs(&[("env", "prod"), ("tier", "gold")]));
        assert_eq!(
            directory.tags_of(arn),
            pairs(&[("env", "prod"), ("team", "core"), ("tier", "gold")]),
            "case a merging call: expected the re-valued, kept and added pairs"
        );

        // Untagging removes exactly the keys it names; a key the resource never carried
        // removes nothing and is not an error.
        assert_eq!(
            directory.untag(arn, &["team".to_string(), "absent".to_string()]),
            1,
            "case untagging one present and one absent key: expected one removal"
        );
        assert_eq!(
            directory.tags_of(arn),
            pairs(&[("env", "prod"), ("tier", "gold")]),
            "case untagging: expected only the named key gone"
        );

        // Removing the rest leaves the resource with no tags at all.
        directory.untag(arn, &["env".to_string(), "tier".to_string()]);
        assert!(
            directory.tags_of(arn).is_empty() && directory.tagged_resources() == 0,
            "case untagging every key: expected no tags and no entry, got {:?}",
            directory.tags_of(arn)
        );
    }

    #[test]
    fn the_directory_is_bounded_by_the_models_tag_count_and_its_own_capacity() {
        let mut directory = ChimeTagDirectory::default();
        let arn = "arn:aws:chime:us-east-1:000000000000:app-instance/ai2";

        // Rows: a resource filled to the modelled maximum, one more past it, and a
        // re-value of an existing key AT the maximum (which adds nothing, so it fits).
        let full: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE)
            .map(|index| (format!("k{index:02}"), "v".to_string()))
            .collect();
        assert_eq!(
            directory.tag(arn, full),
            Tagged::Stored,
            "case exactly MAX_TAGS_PER_RESOURCE tags: expected them stored"
        );
        assert_eq!(
            directory.tag(arn, pairs(&[("one-too-many", "v")])),
            Tagged::TooManyTags(MAX_TAGS_PER_RESOURCE + 1),
            "case one tag past the maximum: expected the refusal"
        );
        assert_eq!(
            directory.tags_of(arn).len(),
            MAX_TAGS_PER_RESOURCE,
            "case one tag past the maximum: expected the refused call to write nothing"
        );
        assert_eq!(
            directory.tag(arn, pairs(&[("k00", "re-valued")])),
            Tagged::Stored,
            "case re-valuing a key at the maximum: expected it stored"
        );

        // The capacity bound drops the OLDEST tagged resource, never a newer one.
        let mut directory = ChimeTagDirectory::default();
        for index in 0..=MAX_TAGGED_RESOURCES {
            directory.tag(&format!("arn:aws:chime:::app-instance/{index}"), pairs(&[("k", "v")]));
        }
        assert_eq!(
            directory.tagged_resources(),
            MAX_TAGGED_RESOURCES,
            "case one resource past capacity: expected the directory to stay at its bound"
        );
        assert!(
            directory
                .tags_of("arn:aws:chime:::app-instance/0")
                .is_empty(),
            "case one resource past capacity: expected the oldest resource dropped"
        );
        assert_eq!(
            directory
                .tags_of(&format!("arn:aws:chime:::app-instance/{MAX_TAGGED_RESOURCES}"))
                .len(),
            1,
            "case one resource past capacity: expected the newest resource kept"
        );
    }
}
