// SPDX-License-Identifier: CC0-1.0

//! The Traffic Mirror target store: the ids, bounds, byte charge and error channel the
//! Traffic Mirror target operations share, with the one write and the one removal path a
//! target enters and leaves the store by.
//!
//! The targets themselves live on the service's one state struct
//! ([`ComAmazonawsEc2Amazonec2State::traffic_mirror_targets`]) as the MODELLED
//! [`TrafficMirrorTarget`] shape, the way [`super::traffic_mirror_filters`] holds a filter
//! and [`super::traffic_mirror_sessions`] a session, so a create, a delete and a describe
//! cannot report one target differently; only the primitives over that store live here.

use super::{tag_list_bytes, ComAmazonawsEc2Amazonec2State};
use crate::ops::types::TrafficMirrorTarget;
use crate::scaffolded::Scaffolded;

/// The id prefix a Traffic Mirror target is minted under.
///
/// EC2 names the resource's ids `tmt-<hex>`, the same `<prefix>-<hex>` form every other
/// EC2 resource id takes, so [`ComAmazonawsEc2Amazonec2State::next_id`] mints them.
pub const TRAFFIC_MIRROR_TARGET_ID_PREFIX: &str = "tmt";

/// The `ResourceType` a Traffic Mirror target is tagged under, spelled as the modelled
/// `ResourceType` enum spells it (`smithy_query(query="shape",shape="ResourceType")`).
pub const TRAFFIC_MIRROR_TARGET_TAGGABLE_RESOURCE: &str = "traffic-mirror-target";

/// How many Traffic Mirror targets this emulator stores before a create is refused.
///
/// CHOSEN, like [`super::MAX_TRAFFIC_MIRROR_SESSIONS`] and for the same reason: nothing in
/// this emulator reclaims the id of a target a delete removed, so the store needs a ceiling
/// that is not the process's memory. A count ceiling is not a memory bound on its own — see
/// [`super::MAX_STORED_BYTES`] — so the caller-chosen bytes of each record are charged
/// against that budget too ([`traffic_mirror_target_payload_bytes`]).
pub const MAX_TRAFFIC_MIRROR_TARGETS: usize = 1_000;

/// The largest page `DescribeTrafficMirrorTargets` serves, and the page it serves when the
/// request names no size.
///
/// The top of the modelled `MaxResults` range, `TrafficMirroringMaxResults`'s
/// `range 5..1000`
/// (`smithy_query(query="operation",operation="DescribeTrafficMirrorTargets")`), so a
/// request that names no page size is answered with the largest page the operation can be
/// ASKED for and the `NextToken` path behaves the same either way — the same contract as
/// [`super::MAX_TRAFFIC_MIRROR_SESSIONS_PAGE_SIZE`].
pub const MAX_TRAFFIC_MIRROR_TARGETS_PAGE_SIZE: usize = 1_000;

/// EC2's code for a Traffic Mirror target id that names no target of the account.
///
/// EC2's model declares no error shape at all (see [`crate::scaffolded::ServiceError`]),
/// and the service answers `Invalid<Resource>Id.NotFound` for an id of a resource it
/// cannot resolve — `InvalidTrafficMirrorFilterId.NotFound`,
/// `InvalidTrafficMirrorSessionId.NotFound` — so that is the code this resource's id gets.
pub const INVALID_TRAFFIC_MIRROR_TARGET_ID_NOT_FOUND: &str = "InvalidTrafficMirrorTargetId.NotFound";

/// The caller-chosen bytes one stored Traffic Mirror target holds for as long as the store
/// holds it: its tags, its description, and the Network Load Balancer ARN a caller named it
/// by.
///
/// Everything else on the record — its own id, the owning account, the type word drawn from
/// a modelled enum, and the ids of the network interface and the Gateway Load Balancer
/// endpoint it points at — is derived by this crate or bounded by the model, so it is not
/// counted here. The ONE definition of that charge, so a writer cannot store bytes the
/// store's budget never saw and a removal gives back exactly what the insert took.
pub fn traffic_mirror_target_payload_bytes(target: &TrafficMirrorTarget) -> usize {
    tag_list_bytes(target.tags.as_deref().unwrap_or_default())
        .saturating_add(target.description.as_ref().map_or(0, String::len))
        .saturating_add(
            target
                .network_load_balancer_arn
                .as_ref()
                .map_or(0, String::len),
        )
}

/// Refuse a request that named a Traffic Mirror target id this account holds no target for:
/// HTTP 400 [`INVALID_TRAFFIC_MIRROR_TARGET_ID_NOT_FOUND`].
///
/// `TrafficMirrorTargetId` publishes no pattern
/// (`smithy_query(query="shape",shape="TrafficMirrorTargetId")`), so there is no spelling to
/// call an id malformed by: one code covers an id of any shape, because an id this service
/// never minted names no target of the account whether or not it reads like one. The message
/// names the id the REQUEST gave, so a client reading the text sees which id it was.
pub fn invalid_traffic_mirror_target_id_not_found<E>(traffic_mirror_target_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_TRAFFIC_MIRROR_TARGET_ID_NOT_FOUND,
        400,
        format!("{traffic_mirror_target_id} does not exist"),
    )
}

/// Resolve `traffic_mirror_target_id` against the store, or refuse the request with
/// [`invalid_traffic_mirror_target_id_not_found`].
///
/// The ONE place an id of this kind is resolved, so no two operations can disagree about
/// which ids name a target of the account — the same seam
/// [`super::require_known_traffic_mirror_session`] is for a session.
pub fn require_known_traffic_mirror_target<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    traffic_mirror_target_id: &str,
) -> Result<(), Scaffolded<E>> {
    if store
        .traffic_mirror_targets
        .contains_key(traffic_mirror_target_id)
    {
        return Ok(());
    }
    Err(invalid_traffic_mirror_target_id_not_found(
        traffic_mirror_target_id,
    ))
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another Traffic Mirror target fits inside the store's ceiling
    /// ([`MAX_TRAFFIC_MIRROR_TARGETS`]) and its byte budget.
    ///
    /// The same contract as [`Self::has_traffic_mirror_session_capacity`]: a create asks
    /// first so it can refuse with EC2's own quota code instead of overshooting, and
    /// [`Self::insert_traffic_mirror_target`] asks it again for itself.
    pub fn has_traffic_mirror_target_capacity(&self, target: &TrafficMirrorTarget) -> bool {
        self.traffic_mirror_targets.len() < MAX_TRAFFIC_MIRROR_TARGETS
            && !self.would_exceed_stored_bytes(traffic_mirror_target_payload_bytes(target))
    }

    /// Store `target` under a freshly minted `TrafficMirrorTargetId`, charge the
    /// caller-chosen bytes it retains against the store's byte budget, and answer the id it
    /// was given — or `None`, having stored NOTHING, when the store is at a ceiling.
    ///
    /// The id is minted HERE rather than by the writer, so a target never shares an id with
    /// another resource of this store, and the record is written with that id on it so no
    /// reader has to learn the key twice. The ceilings are enforced here as well as asked by
    /// the writer ([`Self::has_traffic_mirror_target_capacity`]), the way
    /// [`Self::insert_traffic_mirror_session`] enforces its own, so a writer that forgets to
    /// ask cannot grow the store without bound.
    ///
    /// `CreateTrafficMirrorTarget` is still a scaffold stub with an issue of its own, so for
    /// now this is reached only from this crate's tests, which seed targets the way that
    /// create will. It lives here rather than in a test so the create lands on this one
    /// record instead of a second copy of a target beside it.
    pub fn insert_traffic_mirror_target(
        &mut self,
        mut target: TrafficMirrorTarget,
    ) -> Option<String> {
        if !self.has_traffic_mirror_target_capacity(&target) {
            return None;
        }
        let traffic_mirror_target_id = self.next_id(TRAFFIC_MIRROR_TARGET_ID_PREFIX);
        target.traffic_mirror_target_id = Some(traffic_mirror_target_id.clone());
        self.add_stored_bytes(traffic_mirror_target_payload_bytes(&target));
        self.traffic_mirror_targets
            .insert(traffic_mirror_target_id.clone(), target);
        Some(traffic_mirror_target_id)
    }

    /// Remove the Traffic Mirror target `traffic_mirror_target_id` names and answer it as it
    /// was last reported, or `None` when the store holds no such target.
    ///
    /// The ONE way a target leaves the store, so a deletion is observable through every path
    /// that resolves a target id: [`TrafficMirrorTarget`] carries no state member
    /// (`smithy_query(query="shape",shape="TrafficMirrorTarget")`), so there is no terminal
    /// state for a deleted target to be reported in and the id simply stops resolving.
    ///
    /// The answer is the record the removal took OUT of the store, so what a delete reports
    /// is what the readers reported — never a second copy built beside it. The bytes the
    /// record held go back to the store's budget on exactly the terms
    /// [`Self::insert_traffic_mirror_target`] took them on
    /// ([`traffic_mirror_target_payload_bytes`]).
    pub fn remove_traffic_mirror_target(
        &mut self,
        traffic_mirror_target_id: &str,
    ) -> Option<TrafficMirrorTarget> {
        let target = self.traffic_mirror_targets.remove(traffic_mirror_target_id)?;
        self.release_stored_bytes(traffic_mirror_target_payload_bytes(&target));
        Some(target)
    }

    /// The ids of the Traffic Mirror sessions that mirror TO the target
    /// `traffic_mirror_target_id`, in store order.
    ///
    /// "You cannot delete a Traffic Mirror target that is in use by a Traffic Mirror session"
    /// (`DeleteTrafficMirrorTarget`): the delete asks this and refuses while the answer is not
    /// empty, so the target a stored session mirrors to is still there for that session to
    /// mirror to — the same seam [`Self::traffic_mirror_sessions_using_filter`] is for a
    /// filter.
    pub fn traffic_mirror_sessions_using_target(
        &self,
        traffic_mirror_target_id: &str,
    ) -> Vec<String> {
        self.traffic_mirror_sessions
            .iter()
            .filter(|(_, session)| {
                session.traffic_mirror_target_id.as_deref() == Some(traffic_mirror_target_id)
            })
            .map(|(session_id, _)| session_id.clone())
            .collect()
    }
}
