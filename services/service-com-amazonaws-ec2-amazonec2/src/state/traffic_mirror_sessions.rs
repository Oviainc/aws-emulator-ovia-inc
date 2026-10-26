// SPDX-License-Identifier: CC0-1.0

//! The Traffic Mirror session store: the ids, bounds, byte charge and error channel the
//! Traffic Mirror session operations share.
//!
//! The sessions themselves live on the service's one state struct
//! ([`ComAmazonawsEc2Amazonec2State::traffic_mirror_sessions`]) as the MODELLED
//! [`TrafficMirrorSession`] shape, the way [`super::traffic_mirror_filters`] holds a
//! filter, so a create, a modify and a describe cannot report one session differently;
//! only the primitives over that store live here.

use super::{tag_list_bytes, ComAmazonawsEc2Amazonec2State};
use crate::ops::types::TrafficMirrorSession;
use crate::scaffolded::Scaffolded;

/// The id prefix a Traffic Mirror session is minted under.
///
/// EC2 names the resource's ids `tms-<hex>`, the same `<prefix>-<hex>` form every other
/// EC2 resource id takes, so [`ComAmazonawsEc2Amazonec2State::next_id`] mints them.
pub const TRAFFIC_MIRROR_SESSION_ID_PREFIX: &str = "tms";

/// The `ResourceType` a Traffic Mirror session is tagged under, spelled as the modelled
/// `ResourceType` enum spells it (`smithy_query(query="shape",shape="ResourceType")`).
pub const TRAFFIC_MIRROR_SESSION_TAGGABLE_RESOURCE: &str = "traffic-mirror-session";

/// How many Traffic Mirror sessions this emulator stores before a create is refused.
///
/// CHOSEN, like [`super::MAX_TRAFFIC_MIRROR_FILTERS`] and for the same reason: nothing in
/// this emulator reclaims the id of a session a delete removed, so the store needs a
/// ceiling that is not the process's memory. A count ceiling is not a memory bound on its
/// own — see [`super::MAX_STORED_BYTES`] — so the caller-chosen bytes of each record are charged
/// against that budget too ([`traffic_mirror_session_payload_bytes`]).
pub const MAX_TRAFFIC_MIRROR_SESSIONS: usize = 1_000;

/// The largest page `DescribeTrafficMirrorSessions` serves, and the page it serves when the
/// request names no size.
///
/// The top of the modelled `MaxResults` range, `TrafficMirroringMaxResults`'s
/// `range 5..1000`
/// (`smithy_query(query="operation",operation="DescribeTrafficMirrorSessions")`), so a
/// request that names no page size is answered with the largest page the operation can be
/// ASKED for and the `NextToken` path behaves the same either way — the same contract as
/// [`super::MAX_TRAFFIC_MIRROR_FILTERS_PAGE_SIZE`].
pub const MAX_TRAFFIC_MIRROR_SESSIONS_PAGE_SIZE: usize = 1_000;

/// EC2's code for a Traffic Mirror session id that names no session of the account.
///
/// EC2's model declares no error shape at all (see [`crate::scaffolded::ServiceError`]),
/// and the service answers `Invalid<Resource>Id.NotFound` for an id of a resource it
/// cannot resolve — `InvalidTrafficMirrorFilterId.NotFound`,
/// `InvalidLaunchTemplateId.NotFound` — so that is the code this resource's id gets.
pub const INVALID_TRAFFIC_MIRROR_SESSION_ID_NOT_FOUND: &str =
    "InvalidTrafficMirrorSessionId.NotFound";

/// The caller-chosen bytes one stored Traffic Mirror session holds for as long as the
/// store holds it: its tags and its description.
///
/// Everything else on the record — its own id, the ids of the filter, the target and the
/// network interface it names, the account, and the bounded integers — is derived by this
/// crate or bounded by the model, so it is not counted here. The ONE definition of that
/// charge, so a writer cannot store bytes the store's budget never saw and a removal gives
/// back exactly what the insert took.
pub fn traffic_mirror_session_payload_bytes(session: &TrafficMirrorSession) -> usize {
    tag_list_bytes(session.tags.as_deref().unwrap_or_default())
        .saturating_add(session.description.as_ref().map_or(0, String::len))
}

/// Refuse a request that named a Traffic Mirror session id this account holds no session
/// for: HTTP 400 [`INVALID_TRAFFIC_MIRROR_SESSION_ID_NOT_FOUND`].
///
/// `TrafficMirrorSessionId` publishes no pattern
/// (`smithy_query(query="shape",shape="TrafficMirrorSessionId")`), so there is no spelling
/// to call an id malformed by, and the recorded catalog this emulator is measured against
/// shows EC2 answering this one code for an id in the long resource-id form that names
/// nothing: one code covers an id of any shape, because an id this service never minted
/// names no session of the account whether or not it reads like one. The message names the
/// id the REQUEST gave, so a client reading the text sees which id it was.
pub fn invalid_traffic_mirror_session_id_not_found<E>(
    traffic_mirror_session_id: &str,
) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_TRAFFIC_MIRROR_SESSION_ID_NOT_FOUND,
        400,
        format!("{traffic_mirror_session_id} does not exist"),
    )
}

/// Resolve `traffic_mirror_session_id` against the store, or refuse the request with
/// [`invalid_traffic_mirror_session_id_not_found`].
///
/// The ONE place an id of this kind is resolved, so no two operations can disagree about
/// which ids name a session of the account.
pub fn require_known_traffic_mirror_session<E>(
    store: &ComAmazonawsEc2Amazonec2State,
    traffic_mirror_session_id: &str,
) -> Result<(), Scaffolded<E>> {
    if store
        .traffic_mirror_sessions
        .contains_key(traffic_mirror_session_id)
    {
        return Ok(());
    }
    Err(invalid_traffic_mirror_session_id_not_found(
        traffic_mirror_session_id,
    ))
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another Traffic Mirror session fits inside the store's ceiling
    /// ([`MAX_TRAFFIC_MIRROR_SESSIONS`]) and its byte budget.
    ///
    /// The same contract as [`Self::has_traffic_mirror_filter_capacity`]: a create asks
    /// first so it can refuse with EC2's own quota code instead of overshooting, and
    /// [`Self::insert_traffic_mirror_session`] asks it again for itself.
    pub fn has_traffic_mirror_session_capacity(&self, session: &TrafficMirrorSession) -> bool {
        self.traffic_mirror_sessions.len() < MAX_TRAFFIC_MIRROR_SESSIONS
            && !self.would_exceed_stored_bytes(traffic_mirror_session_payload_bytes(session))
    }

    /// Store `session` under a freshly minted `TrafficMirrorSessionId`, charge the
    /// caller-chosen bytes it retains against the store's byte budget, and answer the id it
    /// was given — or `None`, having stored NOTHING, when the store is at a ceiling.
    ///
    /// The id is minted HERE rather than by the writer, so a session never shares an id
    /// with another resource of this store, and the record is written with that id on it so
    /// no reader has to learn the key twice. The ceilings are enforced here as well as
    /// asked by the writer ([`Self::has_traffic_mirror_session_capacity`]), the way
    /// [`Self::insert_coip_pool`] enforces its own, so a writer that forgets to ask cannot
    /// grow the store without bound.
    ///
    /// `CreateTrafficMirrorSession` is still a scaffold stub with an issue of its own, so
    /// for now this is reached only from this crate's tests, which seed sessions the way
    /// that create will. It lives here rather than in a test so the create lands on this
    /// one record instead of a second copy of a session beside it.
    pub fn insert_traffic_mirror_session(
        &mut self,
        mut session: TrafficMirrorSession,
    ) -> Option<String> {
        if !self.has_traffic_mirror_session_capacity(&session) {
            return None;
        }
        let traffic_mirror_session_id = self.next_id(TRAFFIC_MIRROR_SESSION_ID_PREFIX);
        session.traffic_mirror_session_id = Some(traffic_mirror_session_id.clone());
        self.add_stored_bytes(traffic_mirror_session_payload_bytes(&session));
        self.traffic_mirror_sessions
            .insert(traffic_mirror_session_id.clone(), session);
        Some(traffic_mirror_session_id)
    }

    /// Remove the Traffic Mirror session `traffic_mirror_session_id` names and answer it as
    /// it was last reported, or `None` when the store holds no such session.
    ///
    /// The answer is the record the removal took OUT of the store, so what a delete reports
    /// is what the readers reported — never a second copy built beside it. The bytes the
    /// record held go back to the store's budget on exactly the terms
    /// [`Self::insert_traffic_mirror_session`] took them on
    /// ([`traffic_mirror_session_payload_bytes`]).
    pub fn remove_traffic_mirror_session(
        &mut self,
        traffic_mirror_session_id: &str,
    ) -> Option<TrafficMirrorSession> {
        let session = self
            .traffic_mirror_sessions
            .remove(traffic_mirror_session_id)?;
        self.release_stored_bytes(traffic_mirror_session_payload_bytes(&session));
        Some(session)
    }
}
