// SPDX-License-Identifier: CC0-1.0

//! The bundle-task store's identity and refusals: the prefix a bundle task id carries, the
//! code EC2 answers for ids it cannot resolve, and the reading every operation that names a
//! bundle task goes through.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there. The store itself is
//! [`ComAmazonawsEc2Amazonec2State::bundle_tasks`], which holds a task as the modelled
//! [`BundleTask`] shape every report of one answers with.

use super::*;

/// The prefix of a bundle task id: EC2 resource ids are `<prefix>-<hex>`
/// ([`ComAmazonawsEc2Amazonec2State::next_id`]), and a bundle task's is `bun-…` — "the ID of
/// the bundle task" (`CancelBundleTaskRequest$BundleId`, `BundleTask$BundleId`), whose shape
/// the model publishes no `@pattern` for
/// (`smithy_query(query="shape",shape="BundleId")` is a plain `String`).
pub const BUNDLE_TASK_ID_PREFIX: &str = "bun";

/// The code EC2 answers for bundle task ids it cannot resolve to tasks of this account:
/// `InvalidBundleID.NotFound` (HTTP 400).
pub const INVALID_BUNDLE_ID_NOT_FOUND: &str = "InvalidBundleID.NotFound";

/// Refuse a request naming bundle task ids this account holds no bundle task for: HTTP 400
/// [`INVALID_BUNDLE_ID_NOT_FOUND`].
///
/// The sentence is this resource's own — `Invalid bundle ids: <id>[, <id>…]` — which names
/// EVERY id at fault rather than quoting one, so it does not go through [`id_not_found`]: the
/// request member is a LIST, and the refusal is about the ids of it that do not resolve. The
/// ids come from the REQUEST, each cut to [`MAX_ECHOED_RESOURCE_ID_CHARS`]
/// ([`echoed_resource_id`]) the way every refusal that quotes a caller's value is, and how
/// MANY may be named is bounded by [`MAX_RESOURCE_IDS`], the cap a caller's id list is held
/// to before it is looked at.
pub fn invalid_bundle_ids<E>(bundle_ids: &[&str]) -> Scaffolded<E> {
    let named: Vec<String> = bundle_ids
        .iter()
        .map(|bundle_id| echoed_resource_id(bundle_id))
        .collect();
    Scaffolded::service_error(
        INVALID_BUNDLE_ID_NOT_FOUND,
        400,
        format!("Invalid bundle ids: {}", named.join(", ")),
    )
}

impl ComAmazonawsEc2Amazonec2State {
    /// The ids of `requested` this account holds no bundle task for, in the order the request
    /// named them and without a repeat — the ids [`invalid_bundle_ids`] names.
    ///
    /// A value that is not a bundle task id at all is one of them: there is no bundle task
    /// under it either, and EC2 publishes the one code for the member. The ONE reading of a
    /// request's `BundleIds`, so every operation that reports or acts on bundle tasks refuses
    /// an id it cannot resolve the same way.
    pub fn unresolved_bundle_ids<'a>(&self, requested: &[&'a str]) -> Vec<&'a str> {
        // The ids a request may name are capped at [`MAX_RESOURCE_IDS`], so what bounds this
        // is the cost PER id: a set membership and a map lookup, both logarithmic. Checking
        // the ids already collected by scanning them instead would be quadratic in a
        // caller-chosen count, under the state lock.
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut unresolved: Vec<&'a str> = Vec::new();
        for bundle_id in requested {
            // A repeat names the same task, so it is named once in the refusal.
            if !seen.insert(*bundle_id) {
                continue;
            }
            if self.bundle_tasks.contains_key(*bundle_id) {
                continue;
            }
            unresolved.push(*bundle_id);
        }
        unresolved
    }
}
