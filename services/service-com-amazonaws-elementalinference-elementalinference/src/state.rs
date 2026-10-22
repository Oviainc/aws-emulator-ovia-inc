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
//!
//! A FIXTURE is not one of those resources: no operation of this service creates,
//! updates or deletes one. A fixture is a sports event "as provided by the data
//! source" (`GetFixtureResponse$name`/`$status`), an external feed of sports data that
//! both `SearchFixtures` and `GetFixture` only report — the model marks each of them
//! `@readonly`, so neither may write. The fixture catalog at the bottom of this module
//! is therefore the emulator's stand-in for that external data source: ONE deterministic
//! catalog, derived from the sport and the UTC day, that both operations project. A
//! fixture `SearchFixtures` lists and the same fixture `GetFixture` retrieves are the
//! same record by construction, and no request mutates what a later caller sees.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::ops::types::{
    Competitor, FeedAssociation, FeedSummary, FixtureSummary, GetOutput, OutputConfig,
};

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
/// The model's enum also carries `DELETED`; a feed whose deletion has completed is
/// removed from the store instead (`GetFeed` then answers
/// `ResourceNotFoundException`), so nothing ever reports that value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedStatus {
    /// The feed has been accepted and is not serviceable yet.
    Creating,
    /// The feed is settled and serviceable, with no resource associated.
    Available,
    /// A resource (the source media) is associated with the feed: `AssociateFeed`
    /// claims the feed for one resource, and that claim is what `ACTIVE` reports.
    Active,
    /// An `UpdateFeed` has been accepted and is being applied.
    Updating,
    /// A `DeleteFeed` has been accepted; the feed disappears when it completes.
    Deleting,
    /// The feed's resource has been released by `DisassociateFeed`. Terminal: a
    /// released feed takes no further association.
    Archived,
}

impl FeedStatus {
    /// The value this status takes on the wire, as spelled by the `FeedStatus` enum.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Available => "AVAILABLE",
            Self::Active => "ACTIVE",
            Self::Updating => "UPDATING",
            Self::Deleting => "DELETING",
            Self::Archived => "ARCHIVED",
        }
    }
}

/// The message this service renders for `ResourceNotFoundException`.
///
/// ElementalInference renders this error with one fixed text, the same whichever
/// operation refused and whichever id it was given, so the refusal never reports the
/// id that was probed.
pub const NOT_FOUND_MESSAGE: &str = "Resource not found";

/// The `OutputStatus` value a disabled output reports — one of the two values that
/// enum admits (`OUTPUT_STATUS_VALUES`: `DISABLED`, `ENABLED`).
pub const OUTPUT_STATUS_DISABLED: &str = "DISABLED";

/// The most outputs one feed may hold.
///
/// `AssociateFeed` APPENDS outputs ("New outputs will be appended"), so without a
/// bound a caller could grow one feed's record by another output on every call. AWS
/// does not publish the figure, so this one is the emulator's own: it is the BOUND on
/// a feed's output list, and past it `AssociateFeed` refuses with the modelled
/// `ServiceQuotaExceededException` (HTTP 402) — the answer the real service gives
/// when a request would exceed a service quota. `CreateFeed` and `UpdateFeed` need no
/// such bound: each one REPLACES the list, so one request's size bounds it.
pub const MAX_OUTPUTS_PER_FEED: usize = 20;

/// The `DictionaryStatus` values this slice of the service produces.
///
/// The model's enum also carries `REFERENCED` (a dictionary a feed output points at),
/// `DELETING` and `DELETED`; nothing records a feed output's reference to a
/// dictionary yet and `DeleteDictionary` is not implemented, so this enum names only
/// the statuses a dictionary can actually reach today.
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

    /// Mark every output of this feed `DISABLED`.
    ///
    /// `DisassociateFeed` "releases the resource (the source media) that is associated
    /// with this feed. The outputs in the feed become DISABLED."
    pub fn disable_outputs(&mut self) {
        for output in &mut self.outputs {
            output.status = OUTPUT_STATUS_DISABLED.to_string();
        }
    }

    /// The name of the resource currently associated with this feed, if any.
    pub fn associated_resource_name(&self) -> Option<&str> {
        self.association
            .as_ref()
            .map(|association| association.associated_resource_name.as_str())
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
                    // Where a transient lands depends on the feed's association, not
                    // on which call started it: a feed claimed by AssociateFeed while
                    // it was still settling becomes ACTIVE, an unclaimed one AVAILABLE.
                    feed.status = if feed.association.is_some() {
                        FeedStatus::Active
                    } else {
                        FeedStatus::Available
                    };
                    feed.transition_due_at = None;
                    true
                }
                // A settled status owes the clock nothing; clear the stale instant.
                FeedStatus::Available | FeedStatus::Active | FeedStatus::Archived => {
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

/// The refusal a role ARN naming another account earns.
///
/// Elemental Inference does not resolve a passed role offline: it evaluates the role by
/// attempting to pass it, and passing a role that lives in another account is denied.
/// The account id is in the ARN the request supplied, so the refusal is derived from
/// the request.
pub const CROSS_ACCOUNT_PASS_ROLE_MESSAGE: &str = "Cross-account pass role is not allowed.";

/// Whether `arn` names an IAM account id other than the one this emulator serves.
///
/// Reads the account segment of an ARN (`arn:<partition>:iam::<account>:<resource>`,
/// field 5) and answers true only when it is a 12-digit account id that is not
/// `engine::emulator_account_id()`. A value whose account segment is not an account id
/// at all says nothing about WHOSE role it is, so it is not a pass-role denial and is
/// left to the member's own syntax rules.
pub fn names_another_account(arn: &str) -> bool {
    let fields: Vec<&str> = arn.split(':').collect();
    let Some(account) = fields.get(4) else {
        return false;
    };
    let is_account_id =
        account.len() == 12 && account.bytes().all(|b| b.is_ascii_digit());
    is_account_id && *account != ::engine::emulator_account_id()
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
        let mut word = splitmix64(&mut state);
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

/// SplitMix64: a well-known 64-bit mixer, used here only to spread a counter or a
/// seed over the whole 64-bit range. Deterministic, so everything derived from it
/// (an id this service mints, a day's fixtures) is identical on every call with the
/// same input.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Encode an opaque `nextToken` that resumes a page after `last_id`.
///
/// The token is opaque to the caller: it is the hex encoding of the id the page
/// ended on, so a client cannot read a resource id out of it and the service can
/// resume from the next id in order. Hex comes from the `hex` crate rather than
/// being hand-rolled. Shared by every paginated operation of this service.
pub fn encode_page_token(last_id: &str) -> String {
    ::hex::encode(last_id.as_bytes())
}

/// Decode a `nextToken` back into the id a page resumes after.
///
/// `None` for a token that is not the hex encoding of a non-empty id, which the
/// handlers refuse as a `ValidationException` — an empty token names no position at
/// all. A token that does decode names a POSITION in the id order rather than an
/// item that must still be there, so paging survives an item that disappeared
/// between two pages.
pub fn decode_page_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    (!decoded.is_empty()).then_some(decoded)
}

// ---------------------------------------------------------------------------
// The fixture catalog: this emulator's stand-in for the external sports data
// source that `SearchFixtures` and `GetFixture` report (see the module docs).
// ---------------------------------------------------------------------------

/// Seconds in a UTC day. A `FixtureDate` names a UTC day ("in UTC",
/// `SearchFixturesRequest$startDate`), and UTC days have no offset changes, so a day
/// is exactly this many seconds and an epoch-second count divides into day numbers.
const SECONDS_PER_DAY: i64 = 86_400;

/// The widest search window `SearchFixtures` accepts, in days, counting both ends:
/// "The window from startDate through endDate must not exceed seven days"
/// (`SearchFixturesRequest$endDate`).
pub const MAX_SEARCH_WINDOW_DAYS: i64 = 7;

/// The most fixtures the catalog holds for one sport on one UTC day.
///
/// The data source's real schedule density is not something AWS documents, so this
/// is the emulator's own figure. It is what BOUNDS the work a search can ask for:
/// a window is at most [`MAX_SEARCH_WINDOW_DAYS`] days, so one search ranges over at
/// most `MAX_SEARCH_WINDOW_DAYS * MAX_FIXTURES_PER_DAY` fixtures whatever the request
/// says.
pub const MAX_FIXTURES_PER_DAY: usize = 4;

/// A day's fixtures are two hours apart from 17:00 UTC ([`build_fixture`]), so the
/// last of them must still start inside the day it is scheduled on.
const _: () = assert!(
    17 + 2 * (MAX_FIXTURES_PER_DAY - 1) <= 23,
    "every fixture of a day must start inside that UTC day"
);

/// How long a fixture counts as under way, i.e. the span after its scheduled start
/// within which its status is [`FIXTURE_STATUS_IN_PROGRESS`] rather than
/// [`FIXTURE_STATUS_COMPLETED`].
///
/// AWS documents no duration for this, so the span is the emulator's own choice: it
/// is long enough to cover a game of either modelled sport.
pub const FIXTURE_IN_PROGRESS_SECS: i64 = 3 * 60 * 60;

/// A fixture whose scheduled start is still ahead. `GetFixtureResponse$status` is the
/// lifecycle status "as provided by the data source. For example, Scheduled or
/// Completed", so the value is a free-form string and these three are the catalog's.
pub const FIXTURE_STATUS_SCHEDULED: &str = "Scheduled";

/// A fixture that has started and is within [`FIXTURE_IN_PROGRESS_SECS`] of its start.
pub const FIXTURE_STATUS_IN_PROGRESS: &str = "InProgress";

/// A fixture whose scheduled start is more than [`FIXTURE_IN_PROGRESS_SECS`] ago.
pub const FIXTURE_STATUS_COMPLETED: &str = "Completed";

/// The sports the data source covers: exactly the `DataSourceSport` enum
/// (`smithy_query(query="shape",shape="DataSourceSport")` — `basketball`,
/// `american-football`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataSourceSport {
    /// `basketball`.
    Basketball,
    /// `american-football`.
    AmericanFootball,
}

impl DataSourceSport {
    /// The sport a `DataSourceSport` value names, or `None` for a value that enum
    /// does not declare.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "basketball" => Some(Self::Basketball),
            "american-football" => Some(Self::AmericanFootball),
            _ => None,
        }
    }

    /// The value this sport takes on the wire, as the `DataSourceSport` enum spells it.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Basketball => "basketball",
            Self::AmericanFootball => "american-football",
        }
    }

    /// The competitors the catalog draws this sport's fixtures from.
    ///
    /// The real data source's teams are third-party data this emulator has no access
    /// to, so these names are deliberately invented rather than borrowed from any
    /// real competition: nobody can mistake them for data-source truth.
    fn roster(self) -> &'static [&'static str] {
        match self {
            Self::Basketball => &[
                "Alder Arrows",
                "Birchwood Bolts",
                "Cedar Cyclones",
                "Dunmore Dynamos",
                "Elmridge Eagles",
                "Foxglove Flyers",
                "Granite Giants",
                "Harborview Hawks",
                "Ironwood Icons",
                "Juniper Jets",
                "Kestrel Kings",
                "Larchmont Lions",
            ],
            Self::AmericanFootball => &[
                "Maplefield Mariners",
                "Northgate Nomads",
                "Oakhurst Otters",
                "Pinecrest Pilots",
                "Quarryside Quakers",
                "Ridgeway Rangers",
                "Stonebrook Stags",
                "Thornhill Titans",
                "Umberland Union",
                "Valewood Vipers",
                "Westmoor Wolves",
                "Yarrowdale Yaks",
            ],
        }
    }

    /// The competition this sport's fixtures belong to, as `fixtureGroup` reports it
    /// for a fixture the data source names a group for.
    fn group(self) -> &'static str {
        match self {
            Self::Basketball => "Emulator Basketball League",
            Self::AmericanFootball => "Emulator Football Conference",
        }
    }
}

/// One fixture in the catalog: a sports event the data source knows about.
///
/// Every field other than the status is fixed by the fixture's identity, so the
/// `FixtureSummary` `SearchFixtures` lists and the `GetFixture` response for the same
/// `fixtureId` agree member for member. The status is the one time-dependent member:
/// it follows the fixture's own scheduled start, which is what the data source
/// reports a lifecycle status from.
#[derive(Debug, Clone)]
pub struct Fixture {
    /// The `FixtureId`, as [`fixture_id`] builds it.
    pub id: String,
    /// The fixture's name: the two competing teams.
    pub name: String,
    /// The competition the fixture belongs to; `None` for a fixture the data source
    /// gives no group for ("The data source doesn't provide this information for
    /// every fixture", `GetFixtureResponse$fixtureGroup`).
    pub group: Option<String>,
    /// The scheduled start, in seconds since the Unix epoch.
    pub scheduled_start_secs: i64,
    /// The competitors, home side first.
    pub competitors: Vec<Competitor>,
}

impl Fixture {
    /// This fixture's lifecycle status at `now_secs` (seconds since the epoch).
    pub fn status_at(&self, now_secs: i64) -> &'static str {
        if now_secs < self.scheduled_start_secs {
            FIXTURE_STATUS_SCHEDULED
        } else if now_secs < self.scheduled_start_secs + FIXTURE_IN_PROGRESS_SECS {
            FIXTURE_STATUS_IN_PROGRESS
        } else {
            FIXTURE_STATUS_COMPLETED
        }
    }

    /// The scheduled start as the modelled `Timestamp`.
    pub fn scheduled_start(&self) -> ::aws_smithy_types::DateTime {
        ::aws_smithy_types::DateTime::from_secs(self.scheduled_start_secs)
    }

    /// The `FixtureSummary` projection `SearchFixtures` reports.
    pub fn summary(&self, now_secs: i64) -> FixtureSummary {
        FixtureSummary {
            fixture_id: self.id.clone(),
            name: self.name.clone(),
            fixture_group: self.group.clone(),
            scheduled_start: Some(self.scheduled_start()),
            status: self.status_at(now_secs).to_string(),
            competitors: self.competitors.clone(),
        }
    }

    /// Whether this fixture satisfies a `COMPETITOR` filter over `values`: true when
    /// one of its competitors' names contains one of the values, compared without
    /// regard to case. An empty value list matches nothing, since a fixture has to
    /// "match at least one of the values" (`SearchFixturesRequest$filters`).
    ///
    /// Containment rather than equality is a DELIBERATE choice, not an oversight: the
    /// model says only that a fixture must match one of the values, and a caller that
    /// knows a competitor by part of its name ("Hawks") is the case the filter exists
    /// for. Equality would be the stricter reading; nothing in the model picks one.
    pub fn matches_competitor(&self, values: &[String]) -> bool {
        self.competitors.iter().any(|competitor| {
            let name = competitor.name.as_deref().unwrap_or_default().to_lowercase();
            values
                .iter()
                .any(|value| name.contains(&value.to_lowercase()))
        })
    }
}

/// Parse a `FixtureDate` (`^\d{4}-\d{2}-\d{2}$`) into a day number since the Unix
/// epoch, or `None` when the text is not a real UTC calendar date.
///
/// The modelled pattern admits text that is not a date at all (`2026-02-31`), so the
/// date is parsed as an RFC-3339 instant by `aws-smithy-types` — the same crate the
/// framework parses modelled timestamps with — and then rendered back and compared,
/// which rejects anything the parser normalised instead of accepting.
pub fn parse_fixture_date(date: &str) -> Option<i64> {
    let parsed = ::aws_smithy_types::DateTime::from_str(
        &format!("{date}T00:00:00Z"),
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    let epoch_day = parsed.secs().div_euclid(SECONDS_PER_DAY);
    (fixture_date_string(epoch_day)?.as_str() == date).then_some(epoch_day)
}

/// The `FixtureDate` text (`YYYY-MM-DD`) of a day number since the Unix epoch, or
/// `None` for a day number no calendar date corresponds to.
pub fn fixture_date_string(epoch_day: i64) -> Option<String> {
    let rendered = ::aws_smithy_types::DateTime::from_secs(epoch_day.checked_mul(SECONDS_PER_DAY)?)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()?;
    // `Format::DateTime` renders `YYYY-MM-DDThh:mm:ssZ`, so the date is its first ten
    // characters — and a rendering that is shorter than that is not one.
    rendered.get(..10).map(str::to_string)
}

/// The `FixtureId` of the `index`-th fixture of `sport` on `epoch_day`.
///
/// The id is `<sport>-<YYYYMMDD>-<NN>`: it names the catalog entry, so `GetFixture`
/// can answer for any id `SearchFixtures` has reported without the two operations
/// having to share anything but this catalog. Ids of one sport sort in the same order
/// as the fixtures themselves, which is what lets a search page over them.
fn fixture_id(sport: DataSourceSport, epoch_day: i64, index: usize) -> Option<String> {
    let date = fixture_date_string(epoch_day)?;
    Some(format!(
        "{}-{}-{index:02}",
        sport.as_wire(),
        date.replace('-', "")
    ))
}

/// The seed the catalog derives one sport's day from: FNV-1a over the sport's name
/// with the day number mixed in, so no two sports and no two days share a seed.
fn day_seed(sport: DataSourceSport, epoch_day: i64) -> u64 {
    // FNV-1a's 64-bit offset basis and prime.
    let mut hash: u64 = 0xCBF2_9CE4_8422_2325;
    for byte in sport.as_wire().as_bytes() {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01B3);
    }
    let mut seed = hash ^ (epoch_day as u64);
    splitmix64(&mut seed)
}

/// Every fixture the catalog holds for `sport` on `epoch_day`, in scheduled order.
///
/// Deterministic: the same sport and day always yield the same fixtures, with the
/// same ids, names, competitors and scheduled starts. At most
/// [`MAX_FIXTURES_PER_DAY`] of them, and never zero — the data source schedules this
/// sport every day — so an empty search result means the filters matched nothing.
/// Scheduled order is also ascending id order, which is what a `nextToken` names a
/// position in.
pub fn fixtures_on_day(sport: DataSourceSport, epoch_day: i64) -> Vec<Fixture> {
    let mut seed = day_seed(sport, epoch_day);
    let count = 1 + (splitmix64(&mut seed) % MAX_FIXTURES_PER_DAY as u64) as usize;
    (1..=count)
        .filter_map(|index| build_fixture(sport, epoch_day, index))
        .collect()
}

/// The one fixture an id names, or `None` when no catalog entry has that id.
///
/// The id is parsed back into the sport, day and index it was built from, and the
/// day is then derived exactly as a search derives it, so a fixture retrieved by id
/// is the very record a search of its day reports.
pub fn fixture_by_id(id: &str) -> Option<Fixture> {
    // The sport itself contains `-` (`american-football`), so the id is split from
    // the RIGHT: index, day, then whatever is left is the sport.
    let mut parts = id.rsplitn(3, '-');
    let index_text = parts.next()?;
    let day_text = parts.next()?;
    let sport = DataSourceSport::from_wire(parts.next()?)?;
    if index_text.len() != 2 || !index_text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if day_text.len() != 8 || !day_text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let date = format!(
        "{}-{}-{}",
        day_text.get(..4)?,
        day_text.get(4..6)?,
        day_text.get(6..)?
    );
    let epoch_day = parse_fixture_date(&date)?;
    // Comparing the whole id is what refuses an index the day does not have: the
    // day's fixtures carry the only ids it has.
    fixtures_on_day(sport, epoch_day)
        .into_iter()
        .find(|fixture| fixture.id == id)
}

/// Derive one fixture of `sport` on `epoch_day` from its own seed.
fn build_fixture(sport: DataSourceSport, epoch_day: i64, index: usize) -> Option<Fixture> {
    let id = fixture_id(sport, epoch_day, index)?;
    let roster = sport.roster();
    // Each fixture is derived from a seed of its own, so a single fixture can be
    // rebuilt from its id alone without rebuilding the ones before it.
    let mut seed = day_seed(sport, epoch_day) ^ (index as u64).wrapping_mul(0x9E37_79B9);
    let home = (splitmix64(&mut seed) % roster.len() as u64) as usize;
    // The away side is drawn from the rest of the roster, so no team plays itself.
    let away =
        (home + 1 + (splitmix64(&mut seed) % (roster.len() as u64 - 1)) as usize) % roster.len();
    // A day's fixtures are two hours apart from 17:00 UTC, on the half hour: the
    // day's `index`-th fixture is its `index`-th in time too, and the last of at most
    // [`MAX_FIXTURES_PER_DAY`] still starts inside the day the id names.
    let hour = 17 + 2 * (index as i64 - 1);
    let minute = (splitmix64(&mut seed) % 2) as i64 * 30;
    // The data source does not name a group for every fixture, so one in four has none.
    let group = (!splitmix64(&mut seed).is_multiple_of(4)).then(|| sport.group().to_string());

    Some(Fixture {
        id,
        name: format!("{} at {}", roster[away], roster[home]),
        group,
        scheduled_start_secs: epoch_day.checked_mul(SECONDS_PER_DAY)? + hour * 60 * 60 + minute * 60,
        competitors: vec![
            Competitor {
                name: Some(roster[home].to_string()),
                is_home: Some(true),
            },
            Competitor {
                name: Some(roster[away].to_string()),
                is_home: Some(false),
            },
        ],
    })
}

/// Seconds since the Unix epoch now, as the fixture statuses are measured against.
pub fn now_secs() -> i64 {
    ::aws_smithy_types::DateTime::from(::std::time::SystemTime::now()).secs()
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
