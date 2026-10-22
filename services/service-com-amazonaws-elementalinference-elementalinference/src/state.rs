// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-elementalinference-elementalinference service.
//!
//! The feed is this service's primary resource: `CreateFeed` mints one, `GetFeed` /
//! `ListFeeds` report it, `UpdateFeed` mutates it and `DeleteFeed` retires it. The
//! record below is the single copy every one of those handlers reads and writes, so
//! the shapes the handlers hand back are all built from it.
//!
//! The dictionary is this service's other resource — a named, language-tagged payload
//! of custom words a subtitling output can reference. `CreateDictionary` mints one and
//! `GetDictionary` reports it; both work on the single [`Dictionary`] record stored
//! here, exactly as the feed handlers work on [`Feed`].

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

/// Length of a `DictionaryId`: the model constrains it to `len 1..19`, so this is
/// the longest id the shape admits.
const DICTIONARY_ID_LEN: usize = 19;

/// The alphabet a `DictionaryId` is drawn from, i.e. exactly the characters its
/// modelled pattern `^[a-zA-Z0-9]+$` admits (and which the `resourceArn` pattern
/// `…:(feed|dictionary)/[a-zA-Z0-9]{1,19}$` also admits).
const DICTIONARY_ID_ALPHABET: &[u8; 62] =
    b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// How long a clock-owed transition takes: a feed's `CREATING`/`UPDATING` ->
/// `AVAILABLE` and `DELETING` -> gone, and a dictionary's `CREATING` -> `AVAILABLE`.
///
/// AWS documents no duration for these, so this span is chosen rather than
/// specified: it is non-zero, so a caller that reads a resource straight after
/// creating it still observes the transient `CREATING` status the real service
/// reports, and it is short enough that no caller has to wait long for the settled
/// state. The due instant is stored on the record and materialised on the next
/// call — nothing ever sleeps for it.
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

/// The most dictionaries this emulator keeps at once.
///
/// As with [`MAX_FEEDS`], Elemental Inference limits how many dictionaries an account
/// may hold (the model declares `ServiceQuotaExceededException` for
/// `CreateDictionary`) without stating the figure, so this one is the emulator's own:
/// it BOUNDS the dictionary store, which would otherwise grow by one record per
/// request forever. Past it `CreateDictionary` refuses with the modelled
/// `ServiceQuotaExceededException` (HTTP 402), which is also the answer the real
/// service gives once the quota is exhausted.
pub const MAX_DICTIONARIES: usize = 100;

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

/// The `DictionaryStatus` values this slice of the service produces.
///
/// The model's enum also carries `REFERENCED` (a dictionary a feed output points at),
/// `DELETING` and `DELETED`; the operations that would produce those
/// (`AssociateFeed`, `DeleteDictionary`) are not implemented yet, so this enum names
/// only the statuses a dictionary can actually reach today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DictionaryStatus {
    /// The dictionary has been accepted and is not usable yet.
    Creating,
    /// The dictionary is settled and can be referenced by an output.
    Available,
}

impl DictionaryStatus {
    /// The value this status takes on the wire, as spelled by the `DictionaryStatus`
    /// enum.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Available => "AVAILABLE",
        }
    }
}

/// A dictionary: custom words and phrases a subtitling output can reference.
#[derive(Debug, Clone)]
pub struct Dictionary {
    pub id: String,
    pub name: String,
    pub arn: String,
    /// An ISO 639-2/T code from the modelled `DictionaryLanguage` enum.
    pub language: String,
    /// The entries payload as submitted; empty when the request carried none.
    pub entries: String,
    pub status: DictionaryStatus,
    /// The ids of the feeds whose outputs reference this dictionary. Empty until a
    /// feed points at it.
    pub references: Vec<String>,
    pub tags: BTreeMap<String, String>,
    /// When the clock owes this dictionary's transient status its next move, if any.
    pub transition_due_at: Option<Instant>,
}

impl Dictionary {
    /// The tags of this dictionary as a `TagMap`, absent when it carries none.
    pub fn tag_map(&self) -> Option<std::collections::HashMap<String, String>> {
        if self.tags.is_empty() {
            return None;
        }
        Some(self.tags.clone().into_iter().collect())
    }

    /// The `FeedReferences` list every dictionary response carries: the feeds that
    /// reference it, which is an empty list rather than an absent member while none
    /// do.
    pub fn feed_references(&self) -> Vec<String> {
        self.references.clone()
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
    /// Every live dictionary, keyed by its `DictionaryId`. `BTreeMap` so
    /// `ListDictionaries` has one stable order to paginate over. BOUNDED at
    /// [`MAX_DICTIONARIES`] entries: `CreateDictionary` refuses past that with the
    /// modelled `ServiceQuotaExceededException`.
    pub dictionaries: BTreeMap<String, Dictionary>,
    /// Monotonic source for minted dictionary ids: unique and stable within a run.
    pub next_dictionary_seq: u64,
    /// The earliest instant at which some record owes the clock a move, when any does.
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

    /// Mint a fresh `DictionaryId`: up to [`DICTIONARY_ID_LEN`] characters of
    /// `[a-zA-Z0-9]`, as the model requires.
    ///
    /// Same construction as [`Self::mint_feed_id`] — the per-run sequence through a
    /// bit mixer, with the loop as the collision guard — over the wider alphabet the
    /// `DictionaryId` pattern admits.
    pub fn mint_dictionary_id(&mut self) -> String {
        loop {
            self.next_dictionary_seq += 1;
            let id = id_from_seq(
                self.next_dictionary_seq,
                DICTIONARY_ID_LEN,
                DICTIONARY_ID_ALPHABET,
            );
            if !self.dictionaries.contains_key(&id) {
                return id;
            }
        }
    }

    /// Record that the clock owes a move at `due`, so the next settle pass runs.
    pub fn note_due(&mut self, due: Instant) {
        self.earliest_due = Some(self.earliest_due.map_or(due, |earliest| earliest.min(due)));
    }

    /// Materialise every transition the clock already owes at `now`, for feeds and
    /// for dictionaries alike.
    ///
    /// Called at the top of every handler, including the reporting ones: these moves
    /// were due at this instant whether or not anybody called, so applying them on a
    /// read changes nothing a later caller would not have seen anyway.
    ///
    /// Returns immediately while no move is owed; the scan it then does is over at
    /// most [`MAX_FEEDS`] + [`MAX_DICTIONARIES`] entries.
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
        for dictionary in self.dictionaries.values_mut() {
            if dictionary.transition_due_at.is_none_or(|due| now < due) {
                continue;
            }
            match dictionary.status {
                // "After creation succeeds, the status will be AVAILABLE."
                DictionaryStatus::Creating => dictionary.status = DictionaryStatus::Available,
                DictionaryStatus::Available => {}
            }
            dictionary.transition_due_at = None;
        }
        self.earliest_due = self
            .feeds
            .values()
            .filter_map(|feed| feed.transition_due_at)
            .chain(
                self.dictionaries
                    .values()
                    .filter_map(|dictionary| dictionary.transition_due_at),
            )
            .min();
    }
}

/// The ARN of a feed in `region`: `arn:aws:elemental-inference:<region>:<account>:feed/<id>`.
pub fn feed_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(SERVICE_NAMESPACE, region, &format!("feed/{id}"))
}

/// The ARN of a dictionary in `region`:
/// `arn:aws:elemental-inference:<region>:<account>:dictionary/<id>`, the form the
/// modelled `resourceArn` pattern `…:(feed|dictionary)/[a-zA-Z0-9]{1,19}$` admits.
pub fn dictionary_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(SERVICE_NAMESPACE, region, &format!("dictionary/{id}"))
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
    id_from_seq(seq, FEED_ID_LEN, FEED_ID_ALPHABET)
}

/// Mix `seq` into `len` characters of `alphabet`.
///
/// Shared by every id this service mints, so one construction backs them all: only
/// the length and the alphabet the id's modelled pattern admits differ.
fn id_from_seq(seq: u64, len: usize, alphabet: &[u8]) -> String {
    // SplitMix64: a well-known 64-bit mixer, used here only to spread consecutive
    // sequence numbers over the id space.
    fn mix(state: &mut u64) -> u64 {
        *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = *state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    let base = alphabet.len() as u64;
    // The most characters one mixed word can carry while `base^n` still fits in a
    // u64 — twelve for the feed alphabet's 36 symbols, ten for the dictionary
    // alphabet's 62.
    let chars_per_word = {
        let mut chars = 0usize;
        let mut capacity = 1u64;
        while capacity <= u64::MAX / base {
            capacity *= base;
            chars += 1;
        }
        chars
    };

    let mut state = seq;
    let mut id = String::with_capacity(len);
    while id.len() < len {
        let mut word = mix(&mut state);
        for _ in 0..chars_per_word {
            if id.len() == len {
                break;
            }
            let digit = (word % base) as usize;
            id.push(char::from(alphabet[digit]));
            word /= base;
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
