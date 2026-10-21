// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-elementalinference-elementalinference service.
//!
//! The feed is this service's primary resource: `CreateFeed` mints one, `GetFeed` /
//! `ListFeeds` report it, `UpdateFeed` mutates it and `DeleteFeed` retires it. The
//! record below is the single copy every one of those handlers reads and writes, so
//! the shapes the handlers hand back are all built from it.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::ops::types::{FeedAssociation, FeedSummary, GetOutput, OutputConfig};

/// This service's SigV4 signing name and ARN namespace (`OPERATIONS.md`, "Wire
/// protocol"), used for every ARN the service mints.
pub const SERVICE_NAMESPACE: &str = "elemental-inference";

/// The region an unsigned request is attributed to. `emulator_arn` documents
/// `us-east-1` as the generated emulator's default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// Length of a `FeedId` — the modelled pattern is `^[a-z0-9]{19}$`.
const FEED_ID_LEN: usize = 19;

/// The alphabet a `FeedId` is drawn from, i.e. exactly the characters its
/// modelled pattern `^[a-z0-9]{19}$` admits.
const FEED_ID_ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// How long a clock-owed feed transition takes: `CREATING`/`UPDATING` -> `AVAILABLE`,
/// and `DELETING` -> gone.
///
/// AWS documents no duration for these, so this span is chosen rather than
/// specified: it is non-zero, so a caller that reads a feed straight after creating
/// it still observes the transient `CREATING` status the real service reports, and
/// it is short enough that no caller has to wait long for the settled state. The
/// due instant is stored on the feed and materialised on the next call — nothing
/// ever sleeps for it.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(400);

/// The most feeds this emulator keeps at once.
///
/// Elemental Inference limits how many feeds an account may hold (the model declares
/// `ServiceQuotaExceededException` for `CreateFeed` precisely for that), but it does
/// not state the figure, so this one is the emulator's own: it is the BOUND on the
/// feed store, which would otherwise grow by one record per request forever. Past it
/// `CreateFeed` refuses with the modelled `ServiceQuotaExceededException` (HTTP 402),
/// which is both the bound and the answer the real service gives when the quota is
/// exhausted, and it caps the work every settle pass and every page can do.
pub const MAX_FEEDS: usize = 100;

/// The `FeedStatus` values this slice of the service produces.
///
/// The model's enum also carries `ACTIVE` (a feed with an associated resource),
/// `ARCHIVED` and `DELETED`; the operations that would produce those
/// (`AssociateFeed`, `DisassociateFeed`) are not implemented yet, so this enum
/// names only the statuses a feed can actually reach today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedStatus {
    /// The feed has been accepted and is not serviceable yet.
    Creating,
    /// The feed is settled and serviceable.
    Available,
    /// An `UpdateFeed` has been accepted and is being applied.
    Updating,
    /// A `DeleteFeed` has been accepted; the feed disappears when it completes.
    Deleting,
}

impl FeedStatus {
    /// The value this status takes on the wire, as spelled by the `FeedStatus` enum.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Available => "AVAILABLE",
            Self::Updating => "UPDATING",
            Self::Deleting => "DELETING",
        }
    }
}

/// One output of a feed: an Elemental Inference feature plus where its results go.
#[derive(Debug, Clone)]
pub struct Output {
    pub name: String,
    pub output_config: OutputConfig,
    pub status: String,
    pub description: Option<String>,
    /// True only for an output that entered the feed through `AssociateFeed`;
    /// `CreateFeed` and `UpdateFeed` outputs are false (`GetOutput$fromAssociation`).
    pub from_association: bool,
}

impl Output {
    /// The `GetOutput` projection of this output, as every feed response reports it.
    pub fn to_get_output(&self) -> GetOutput {
        GetOutput {
            name: self.name.clone(),
            output_config: self.output_config.clone(),
            status: self.status.clone(),
            description: self.description.clone(),
            from_association: Some(self.from_association),
        }
    }
}

/// A feed: the target a calling application streams live media to.
#[derive(Debug, Clone)]
pub struct Feed {
    pub id: String,
    pub name: String,
    pub arn: String,
    pub data_endpoints: Vec<String>,
    pub outputs: Vec<Output>,
    pub access_role_arn: Option<String>,
    pub status: FeedStatus,
    /// The resource associated with the feed; `None` until `AssociateFeed` runs.
    pub association: Option<FeedAssociation>,
    pub tags: BTreeMap<String, String>,
    /// When the clock owes this feed's transient status its next move, if any.
    pub transition_due_at: Option<Instant>,
}

impl Feed {
    /// Put this feed into a transient status the clock will move on by itself, and
    /// return the instant that move falls due. The caller hands that instant to
    /// [`ComAmazonawsElementalinferenceElementalinferenceState::note_due`], which is
    /// what lets the settle pass skip the store entirely while nothing is owed.
    #[must_use]
    pub fn enter_transient(&mut self, status: FeedStatus, now: Instant) -> Instant {
        let due = now + TRANSITION_SPAN;
        self.status = status;
        self.transition_due_at = Some(due);
        due
    }

    /// The outputs of this feed as the `GetOutputList` every feed response carries.
    pub fn get_outputs(&self) -> Vec<GetOutput> {
        self.outputs.iter().map(Output::to_get_output).collect()
    }

    /// The tags of this feed as a `TagMap`, absent when the feed carries none.
    pub fn tag_map(&self) -> Option<std::collections::HashMap<String, String>> {
        if self.tags.is_empty() {
            return None;
        }
        Some(self.tags.clone().into_iter().collect())
    }

    /// The `FeedSummary` projection `ListFeeds` reports.
    pub fn summary(&self) -> FeedSummary {
        FeedSummary {
            arn: self.arn.clone(),
            id: self.id.clone(),
            name: self.name.clone(),
            association: self.association.clone(),
            status: self.status.as_wire().to_string(),
        }
    }
}

/// Per-service state for com-amazonaws-elementalinference-elementalinference. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsElementalinferenceElementalinferenceState {
    /// Every live feed, keyed by its `FeedId`. `BTreeMap` so `ListFeeds` has one
    /// stable order to paginate over. BOUNDED at [`MAX_FEEDS`] entries: `CreateFeed`
    /// refuses past that with the modelled `ServiceQuotaExceededException`, so this
    /// map cannot grow without limit however many requests arrive.
    pub feeds: BTreeMap<String, Feed>,
    /// Monotonic source for minted feed ids: unique and stable within a run.
    pub next_feed_seq: u64,
    /// The earliest instant at which some feed owes the clock a move, when any does.
    /// [`Self::settle_due_transitions`] reads it first and returns without touching
    /// the store while nothing is owed, so the common request path is O(1).
    pub earliest_due: Option<Instant>,
}

impl ComAmazonawsElementalinferenceElementalinferenceState {
    /// Mint a fresh `FeedId`: 19 characters of `[a-z0-9]`, as the model requires.
    ///
    /// Derived from the per-run sequence through a bit mixer, so ids are unique and
    /// stable within a run without consecutive ones sharing a prefix. The loop is
    /// the collision guard: a mixed value that happens to land on a live feed's id
    /// takes the next sequence number instead.
    pub fn mint_feed_id(&mut self) -> String {
        loop {
            self.next_feed_seq += 1;
            let id = feed_id_from_seq(self.next_feed_seq);
            if !self.feeds.contains_key(&id) {
                return id;
            }
        }
    }

    /// Record that the clock owes a move at `due`, so the next settle pass runs.
    pub fn note_due(&mut self, due: Instant) {
        self.earliest_due = Some(self.earliest_due.map_or(due, |earliest| earliest.min(due)));
    }

    /// Materialise every feed transition the clock already owes at `now`.
    ///
    /// Called at the top of every feed handler, including the reporting ones: these
    /// moves were due at this instant whether or not anybody called, so applying
    /// them on a read changes nothing a later caller would not have seen anyway.
    ///
    /// Returns immediately while no move is owed; the scan it then does is over at
    /// most [`MAX_FEEDS`] entries.
    pub fn settle_due_transitions(&mut self, now: Instant) {
        if self.earliest_due.is_none_or(|due| now < due) {
            return;
        }
        self.feeds.retain(|_, feed| {
            let due = feed.transition_due_at.is_some_and(|due| now >= due);
            if !due {
                return true;
            }
            match feed.status {
                // A feed whose deletion has completed is gone, so a later GetFeed
                // answers ResourceNotFoundException.
                FeedStatus::Deleting => false,
                FeedStatus::Creating | FeedStatus::Updating => {
                    feed.status = FeedStatus::Available;
                    feed.transition_due_at = None;
                    true
                }
                FeedStatus::Available => {
                    feed.transition_due_at = None;
                    true
                }
            }
        });
        self.earliest_due = self
            .feeds
            .values()
            .filter_map(|feed| feed.transition_due_at)
            .min();
    }
}

/// The ARN of a feed in `region`: `arn:aws:elemental-inference:<region>:<account>:feed/<id>`.
pub fn feed_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(SERVICE_NAMESPACE, region, &format!("feed/{id}"))
}

/// The data endpoints of a feed in `region`.
///
/// One endpoint per feed ("Typically, there is only one endpoint"), named after the
/// feed so it is stable for the life of the feed and distinct per feed. The media
/// data plane those endpoints serve (`PutMedia` / `GetMetadata`) is not part of this
/// service's Smithy model, so no listener backs them — the field reports where the
/// feed would be reached, which is all the control plane models.
pub fn feed_data_endpoints(region: &str, id: &str) -> Vec<String> {
    vec![format!(
        "https://{id}.data.{SERVICE_NAMESPACE}.{region}.amazonaws.com"
    )]
}

/// The region this request was signed for, or [`DEFAULT_REGION`] when it carries no
/// SigV4 credential scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Mix `seq` into `FEED_ID_LEN` characters of [`FEED_ID_ALPHABET`].
fn feed_id_from_seq(seq: u64) -> String {
    // SplitMix64: a well-known 64-bit mixer, used here only to spread consecutive
    // sequence numbers over the id space.
    fn mix(state: &mut u64) -> u64 {
        *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = *state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    // 36^12 < 2^64, so twelve characters per mixed word stay within one u64.
    const CHARS_PER_WORD: usize = 12;

    let mut state = seq;
    let mut id = String::with_capacity(FEED_ID_LEN);
    while id.len() < FEED_ID_LEN {
        let mut word = mix(&mut state);
        for _ in 0..CHARS_PER_WORD {
            if id.len() == FEED_ID_LEN {
                break;
            }
            let digit = (word % FEED_ID_ALPHABET.len() as u64) as usize;
            id.push(char::from(FEED_ID_ALPHABET[digit]));
            word /= FEED_ID_ALPHABET.len() as u64;
        }
    }
    id
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsElementalinferenceElementalinferenceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsElementalinferenceElementalinferenceState>)
}
