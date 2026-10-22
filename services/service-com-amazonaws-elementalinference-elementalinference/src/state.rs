// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-elementalinference-elementalinference service.
//!
//! The feed is this service's primary resource: `CreateFeed` mints one, `GetFeed` /
//! `ListFeeds` report it, `UpdateFeed` mutates it and `DeleteFeed` retires it. The
//! record below is the single copy every one of those handlers reads and writes, so
//! the shapes the handlers hand back are all built from it.
//!
//! The dictionary is this service's other resource — a named, language-tagged payload
//! of custom words a subtitling output can reference. `CreateDictionary` mints one,
//! `GetDictionary` reports it, `UpdateDictionary` mutates it and `DeleteDictionary`
//! retires it; all four work on the single [`Dictionary`] record stored here, exactly
//! as the feed handlers work on [`Feed`]. WHICH feeds reference a dictionary is not
//! stored on the dictionary: it is derived from the feed store
//! ([`ComAmazonawsElementalinferenceElementalinferenceState::feeds_referencing`]), so
//! one copy of that fact backs the dictionary's `references` list, its `REFERENCED`
//! status and `DeleteDictionary`'s refusal alike.
//!
//! A FIXTURE is not one of those resources: no operation of this service creates,
//! updates or deletes one. A fixture is a sports event "as provided by the data
//! source" (`GetFixtureResponse$name`/`$status`), an external feed of sports data that
//! both `SearchFixtures` and `GetFixture` only report — the model marks each of them
//! `@readonly`, so neither may write. The fixture catalog at the bottom of this module
//! is therefore a STORE of the events that data source holds
//! ([`ComAmazonawsElementalinferenceElementalinferenceState::fixtures`]), and both
//! operations answer out of it: a search reports the held fixtures its criteria select
//! and nothing else, and `GetFixture` answers for an id the store holds.
//!
//! That store starts EMPTY and is filled only through the in-process seeding seam
//! ([`ComAmazonawsElementalinferenceElementalinferenceState::seed_fixtures`]): this
//! emulator reaches no sports data source, and no modelled operation admits an event
//! into one, so there is nothing for it to hold until something hands it a record. A
//! window the store holds no event in therefore reports an empty list ("The array is
//! empty if no fixtures match", `SearchFixturesResponse$fixtures`) rather than a card
//! of invented games, and no request mutates what a later caller sees.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::ops::types::{
    AspectRatio, Competitor, DictionarySummary, FeedAssociation, FeedSummary, FixtureSummary,
    GetOutput, OutputConfig,
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

/// How long a clock-owed transition takes: a feed's `CREATING` -> `AVAILABLE` and
/// `DELETING` -> gone, and a dictionary's `CREATING` -> `AVAILABLE` and `DELETING` ->
/// gone.
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
/// The model's enum also carries `DELETED` and `UPDATING`, and neither value is ever
/// reported. A feed whose deletion has completed is removed from the store instead
/// (`GetFeed` then answers `ResourceNotFoundException`). `UpdateFeed` answers HTTP
/// 200 rather than 202 — "UpdateFeed is a PUT operation, which means that the payload
/// that you specify completely overwrites the existing payload" and that overwrite is
/// done when the call returns — so the update reports the feed's SETTLED status, as
/// `AssociateFeed` (also 200) already does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedStatus {
    /// The feed has been accepted and is not serviceable yet.
    Creating,
    /// The feed is settled and serviceable, with no resource associated.
    Available,
    /// A resource (the source media) is associated with the feed: `AssociateFeed`
    /// claims the feed for one resource, and that claim is what `ACTIVE` reports.
    Active,
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

/// The messages this service renders for the refusals its handlers decide from STORED
/// STATE or from a relation between request members.
///
/// A refusal's message follows the RULE that refused the request, not the request's or
/// the record's values: where the violated rule is a property of the stored resource,
/// or a relation between two members, no single value is what is at fault, so the
/// message is the rule's invariant sentence — the same text whatever the request named
/// and whatever the record holds. That is also what makes a message matchable by a
/// client, and it is what keeps one caller's refusal from disclosing another caller's
/// association or a resource's internal status. Only a rule about ONE value's own
/// validity (a member whose value violates its constraint, an identifier that resolved
/// to nothing) names a value, and then it names the value the REQUEST supplied.
///
/// `AssociateFeed` on a feed that already holds an association: the recorded refusal is
/// this sentence, naming neither the resource that holds the feed nor the feed's status.
pub const FEED_ALREADY_ASSOCIATED_MESSAGE: &str = "The feed is already associated with a resource.";

/// `AssociateFeed` on a feed whose status admits no association at all (every status
/// other than `AVAILABLE`, and `ACTIVE`, which [`FEED_ALREADY_ASSOCIATED_MESSAGE`]
/// covers): the rule is a property of the stored feed, so the sentence carries no
/// status.
pub const FEED_NOT_ASSOCIABLE_MESSAGE: &str =
    "You can't associate this feed because its current status doesn't allow an association.";

/// `DisassociateFeed` naming a resource that is not the one associated with the feed:
/// the recorded refusal is this sentence, naming neither the rejected name nor the
/// resource that does hold the feed.
pub const WRONG_ASSOCIATED_RESOURCE_MESSAGE: &str =
    "You can't disassociate this feed because it isn't associated with the resource you \
     specified. Specify the correct resource.";

/// `DisassociateFeed` on a feed that holds no resource to release: again a property of
/// the stored feed, so the sentence carries no status.
pub const FEED_NOT_ASSOCIATED_MESSAGE: &str =
    "You can't disassociate this feed because it isn't associated with a resource.";

/// `UpdateFeed` on a feed whose deletion has already been accepted: a property of the
/// stored feed, so the sentence names neither the feed nor its status.
pub const FEED_BEING_DELETED_MESSAGE: &str =
    "You can't update this feed because it is being deleted.";

/// `UpdateDictionary` on a dictionary whose deletion has already been accepted: as with
/// [`FEED_BEING_DELETED_MESSAGE`], a property of the stored dictionary, so the sentence
/// names neither the dictionary nor its status.
pub const DICTIONARY_BEING_DELETED_MESSAGE: &str =
    "You can't update this dictionary because it is being deleted.";

/// `DeleteDictionary` on a dictionary a feed references ("You cannot delete a dictionary
/// that is referenced by a feed. You must first remove the dictionary reference from the
/// feed's subtitling configuration."): the rule is a property of the stored resources,
/// so the sentence names neither the dictionary nor the feeds that hold it — which also
/// keeps one caller's refusal from disclosing another caller's feed.
pub const DICTIONARY_REFERENCED_MESSAGE: &str =
    "You can't delete this dictionary because it is referenced by a feed. Remove the \
     dictionary from the feed's subtitling configuration and try again.";

/// `SearchFixtures` with `endDate` before `startDate`: the rule is the ORDER of the two
/// members, so the recorded refusal names the members and not the dates they carried.
pub const END_DATE_BEFORE_START_DATE_MESSAGE: &str = "endDate must not be earlier than startDate.";

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
/// These are the LIFECYCLE statuses a dictionary record holds. The model's enum also
/// carries `REFERENCED` and `DELETED`, and neither is a stored value: `REFERENCED` is
/// DERIVED from the feeds whose subtitling outputs name the dictionary (see
/// [`Dictionary::reported_status`]), and a dictionary whose deletion has completed is
/// removed from the store instead, so `GetDictionary` then answers
/// `ResourceNotFoundException`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DictionaryStatus {
    /// The dictionary has been accepted and is not usable yet.
    Creating,
    /// The dictionary is settled and can be referenced by an output.
    Available,
    /// A `DeleteDictionary` has been accepted; the dictionary disappears when the
    /// removal falls due.
    Deleting,
}

impl DictionaryStatus {
    /// The value this status takes on the wire, as spelled by the `DictionaryStatus`
    /// enum.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Available => "AVAILABLE",
            Self::Deleting => "DELETING",
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
    pub tags: BTreeMap<String, String>,
    /// When the clock owes this dictionary's transient status its next move, if any.
    pub transition_due_at: Option<Instant>,
}

impl Dictionary {
    /// Put this dictionary into a transient status the clock will move on by itself,
    /// and return the instant that move falls due — the dictionary's counterpart of
    /// [`Feed::enter_transient`]. The caller hands that instant to
    /// [`ComAmazonawsElementalinferenceElementalinferenceState::note_due`].
    #[must_use]
    pub fn enter_transient(&mut self, status: DictionaryStatus, now: Instant) -> Instant {
        let due = now + TRANSITION_SPAN;
        self.status = status;
        self.transition_due_at = Some(due);
        due
    }

    /// The `DictionaryStatus` this dictionary reports given `references`, the feeds
    /// that currently reference it.
    ///
    /// A dictionary in a transient lifecycle status reports that status; a settled one
    /// reports `REFERENCED` while any feed's subtitling output names it (which is also
    /// why `DeleteDictionary` refuses it) and `AVAILABLE` while none does.
    pub fn reported_status(&self, references: &[String]) -> &'static str {
        match self.status {
            DictionaryStatus::Available if !references.is_empty() => "REFERENCED",
            status => status.as_wire(),
        }
    }

    /// The tags of this dictionary as the `TagMap` every dictionary response carries.
    ///
    /// A dictionary that carries no tags reports an EMPTY map, not an absent member:
    /// the tag set is a property every dictionary HAS, so a client reading it gets an
    /// empty collection rather than null, and the member does not start existing the
    /// moment the first tag is added.
    pub fn tag_map(&self) -> std::collections::HashMap<String, String> {
        self.tags.clone().into_iter().collect()
    }

    /// The `DictionarySummary` projection `ListDictionaries` reports — the dictionary's
    /// counterpart of [`Feed::summary`].
    ///
    /// `references` is the feeds that currently reference this dictionary
    /// ([`ComAmazonawsElementalinferenceElementalinferenceState::feeds_referencing`]),
    /// so the summary's `status` is the SAME reported status `GetDictionary` answers
    /// with ([`Self::reported_status`]) and a list and a get cannot disagree about one
    /// stored dictionary.
    pub fn summary(&self, references: &[String]) -> DictionarySummary {
        DictionarySummary {
            arn: self.arn.clone(),
            id: self.id.clone(),
            name: self.name.clone(),
            language: self.language.clone(),
            status: self.reported_status(references).to_string(),
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
    /// The id of the dictionary this output references, if it references one.
    ///
    /// Only a subtitling output can: `SubtitlingConfig$dictionary` is "the ID of a
    /// custom dictionary to improve transcription accuracy", and no other member of
    /// the `OutputConfig` union names a dictionary. An output whose subtitling config
    /// carries no dictionary references none.
    pub fn referenced_dictionary(&self) -> Option<&str> {
        match &self.output_config {
            OutputConfig::Subtitling(subtitling) => subtitling.dictionary.as_deref(),
            OutputConfig::Cropping(_)
            | OutputConfig::Clipping(_)
            | OutputConfig::contextualMetadata(_) => None,
        }
    }

    /// One output of a feed as it is STORED, from the output a create or update
    /// request carried.
    ///
    /// The config is stored with the service's own defaults already materialised
    /// ([`output_config_with_defaults`]), so every later read of the feed reports the
    /// configuration the service actually applied rather than only the members the
    /// request happened to carry.
    pub fn new(
        name: String,
        output_config: OutputConfig,
        status: String,
        description: Option<String>,
        from_association: bool,
    ) -> Self {
        Self {
            name,
            output_config: output_config_with_defaults(output_config),
            status,
            description,
            from_association,
        }
    }

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

    /// Whether any of this feed's outputs references `dictionary_id`, i.e. names it in
    /// its subtitling config ([`Output::referenced_dictionary`]).
    pub fn references_dictionary(&self, dictionary_id: &str) -> bool {
        self.outputs
            .iter()
            .any(|output| output.referenced_dictionary() == Some(dictionary_id))
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

    /// The tags of this feed as the `TagMap` every feed response carries.
    ///
    /// As with [`Dictionary::tag_map`], an untagged feed reports an EMPTY map rather
    /// than an absent member.
    pub fn tag_map(&self) -> std::collections::HashMap<String, String> {
        self.tags.clone().into_iter().collect()
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
    /// Monotonic source for minted feed ids: one more per feed this emulator mints.
    pub next_feed_seq: u64,
    /// Every live dictionary, keyed by its `DictionaryId`. `BTreeMap` so
    /// `ListDictionaries` has one stable order to paginate over. BOUNDED at
    /// [`MAX_DICTIONARIES`] entries: `CreateDictionary` refuses past that with the
    /// modelled `ServiceQuotaExceededException`.
    pub dictionaries: BTreeMap<String, Dictionary>,
    /// Monotonic source for minted dictionary ids: one more per dictionary this
    /// emulator mints.
    pub next_dictionary_seq: u64,
    /// Every fixture the external sports data source holds, keyed by its `FixtureId`.
    /// `BTreeMap` so a search has ONE stable order to page over, which is what a
    /// `nextToken` names a position in.
    ///
    /// EMPTY until something seeds it ([`Self::seed_fixtures`]): `SearchFixtures` and
    /// `GetFixture` are the only operations that touch a fixture and both are
    /// `@readonly`, so no request adds, changes or removes an entry — which is also
    /// what BOUNDS this map, since nothing on the wire can grow it.
    pub fixtures: BTreeMap<String, Fixture>,
    /// The earliest instant at which some record owes the clock a move, when any does.
    /// [`Self::settle_due_transitions`] reads it first and returns without touching
    /// the store while nothing is owed, so the common request path is O(1).
    pub earliest_due: Option<Instant>,
    /// The entropy that distinguishes THIS emulator's ids from another's, drawn once
    /// on the first mint ([`Self::run_seed`]).
    ///
    /// An id identifies exactly one resource, so no two resources may ever carry the
    /// same one. The per-resource sequence alone cannot promise that: it starts over
    /// at the same value in a fresh store, so two stores would hand out the same
    /// first id. Mixing this seed in is what makes a minted id unique across stores
    /// as well as within one.
    pub run_seed: Option<u64>,
}

impl ComAmazonawsElementalinferenceElementalinferenceState {
    /// This store's id entropy, drawn on first use and then held for its life, so
    /// every id it mints is unique across stores while an id, once minted, never
    /// changes.
    fn run_seed(&mut self) -> u64 {
        *self.run_seed.get_or_insert_with(fresh_run_seed)
    }

    /// Mint a fresh `FeedId`: 19 characters of `[a-z0-9]`, as the model requires.
    ///
    /// Derived from this store's [`Self::run_seed`] plus the per-resource sequence
    /// through a bit mixer, so no two feeds — of this store or of any other — share
    /// an id, and consecutive ids share no prefix. The loop is the collision guard: a
    /// mixed value that happens to land on a live feed's id takes the next sequence
    /// number instead.
    pub fn mint_feed_id(&mut self) -> String {
        let run_seed = self.run_seed();
        loop {
            self.next_feed_seq += 1;
            let id = feed_id_from_seq(run_seed.wrapping_add(self.next_feed_seq));
            if !self.feeds.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint a fresh `DictionaryId`: up to [`DICTIONARY_ID_LEN`] characters of
    /// `[a-zA-Z0-9]`, as the model requires.
    ///
    /// Same construction as [`Self::mint_feed_id`] — this store's seed plus the
    /// per-resource sequence through a bit mixer, with the loop as the collision
    /// guard — over the wider alphabet the `DictionaryId` pattern admits.
    pub fn mint_dictionary_id(&mut self) -> String {
        let run_seed = self.run_seed();
        loop {
            self.next_dictionary_seq += 1;
            let id = id_from_seq(
                run_seed.wrapping_add(self.next_dictionary_seq),
                DICTIONARY_ID_LEN,
                DICTIONARY_ID_ALPHABET,
            );
            if !self.dictionaries.contains_key(&id) {
                return id;
            }
        }
    }

    /// Install fixtures into the data-source catalog, keyed by their `FixtureId`.
    ///
    /// THE SEEDING SEAM, and the only way an entry reaches [`Self::fixtures`]: this
    /// service models no operation that creates a fixture, because AWS relays
    /// fixtures from an external sports data source, so an emulator that is to report
    /// any has to be handed them in process. A fixture whose id is already held is
    /// REPLACED, so re-seeding an updated record neither duplicates it nor moves it in
    /// the page order.
    pub fn seed_fixtures<I: IntoIterator<Item = Fixture>>(&mut self, fixtures: I) {
        for fixture in fixtures {
            self.fixtures.insert(fixture.id.clone(), fixture);
        }
    }

    /// Drop every held fixture — the seam's other half, for a data source that holds
    /// nothing.
    pub fn clear_fixtures(&mut self) {
        self.fixtures.clear();
    }

    /// The one fixture a `FixtureId` names, or `None` when the catalog holds no such
    /// entry.
    pub fn fixture(&self, fixture_id: &str) -> Option<&Fixture> {
        self.fixtures.get(fixture_id)
    }

    /// Every held fixture of `sport` scheduled on a UTC day from `start_day` through
    /// `end_day` (both ends included, as a search window is), in `FixtureId` order.
    ///
    /// This is the SELECTION a search reports from: a fixture of another sport, or one
    /// scheduled outside the window, is not in it, so a window the catalog holds no
    /// event for selects nothing at all.
    pub fn fixtures_in_window(
        &self,
        sport: DataSourceSport,
        start_day: i64,
        end_day: i64,
    ) -> impl Iterator<Item = &Fixture> {
        self.fixtures.values().filter(move |fixture| {
            fixture.sport == sport && (start_day..=end_day).contains(&fixture.scheduled_day())
        })
    }

    /// Record that the clock owes a move at `due`, so the next settle pass runs.
    pub fn note_due(&mut self, due: Instant) {
        self.earliest_due = Some(self.earliest_due.map_or(due, |earliest| earliest.min(due)));
    }

    /// The ids of the feeds that reference `dictionary_id`, in feed-id order — the
    /// `FeedReferences` list every dictionary response carries.
    ///
    /// DERIVED from the feed store, which is the only place a reference lives: a feed
    /// references a dictionary exactly while one of its outputs' subtitling configs
    /// names that dictionary ([`Feed::references_dictionary`]). So a feed that is
    /// created, reconfigured, associated or deleted changes the dictionaries it
    /// references by that one write, with no second copy to keep in step, and this list
    /// is a faithful view of the feeds `GetFeed`/`ListFeeds` report rather than a
    /// separate opinion about them.
    ///
    /// Scans at most [`MAX_FEEDS`] feeds, each with at most [`MAX_OUTPUTS_PER_FEED`]
    /// outputs.
    pub fn feeds_referencing(&self, dictionary_id: &str) -> Vec<String> {
        self.feeds
            .values()
            .filter(|feed| feed.references_dictionary(dictionary_id))
            .map(|feed| feed.id.clone())
            .collect()
    }

    /// Whether a feed that is not already retiring references `dictionary_id`, i.e.
    /// whether "you cannot delete a dictionary that is referenced by a feed" bites.
    ///
    /// A feed whose deletion has been ACCEPTED (`DELETING`) is on its way out and so
    /// holds nothing back: the dictionary it names may go as soon as the feed's own
    /// deletion is accepted, which is what lets a caller tear down a feed and then its
    /// dictionary without waiting for the feed's removal to fall due. It is still a
    /// live record until then, so it stays in [`Self::feeds_referencing`].
    pub fn delete_blocked_by_feed(&self, dictionary_id: &str) -> bool {
        self.feeds.values().any(|feed| {
            feed.status != FeedStatus::Deleting && feed.references_dictionary(dictionary_id)
        })
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
                FeedStatus::Creating => {
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
        self.dictionaries.retain(|_, dictionary| {
            if dictionary.transition_due_at.is_none_or(|due| now < due) {
                return true;
            }
            match dictionary.status {
                // A dictionary whose deletion has completed is gone, so a later
                // GetDictionary answers ResourceNotFoundException.
                DictionaryStatus::Deleting => false,
                // "After creation succeeds, the status will be AVAILABLE."
                DictionaryStatus::Creating => {
                    dictionary.status = DictionaryStatus::Available;
                    dictionary.transition_due_at = None;
                    true
                }
                // A settled status owes the clock nothing; clear the stale instant.
                DictionaryStatus::Available => {
                    dictionary.transition_due_at = None;
                    true
                }
            }
        });
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

/// The refusal a `language` outside the modelled `DictionaryLanguage` enum earns, or
/// `None` when the value is one the enum lists.
///
/// `language` is an enum member, so only the values the model lists are a dictionary
/// language. The boundary validator refuses the rest, and this is the same refusal for
/// a request that reaches a handler regardless — ONE copy, so `CreateDictionary` and
/// `UpdateDictionary` cannot drift in which values they accept or in what they say
/// about a value they do not.
pub fn invalid_language_message(language: &str) -> Option<String> {
    if crate::ops::types::DICTIONARY_LANGUAGE_VALUES.contains(&language) {
        return None;
    }
    Some(format!(
        "'{language}' is not a valid value for 'language'; it must be one of {}.",
        crate::ops::types::DICTIONARY_LANGUAGE_VALUES.join(", ")
    ))
}

/// The ARN of a dictionary in `region`:
/// `arn:aws:elemental-inference:<region>:<account>:dictionary/<id>`, the form the
/// modelled `resourceArn` pattern `…:(feed|dictionary)/[a-zA-Z0-9]{1,19}$` admits.
pub fn dictionary_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(SERVICE_NAMESPACE, region, &format!("dictionary/{id}"))
}

/// The top-level domain every data endpoint of this service sits under.
///
/// The media data plane is reached on a DEDICATED per-region domain,
/// `<signing-name>-data.<region>.api.aws`, not on the control-plane service domain
/// the control API itself answers on: a client routes media by the endpoint's domain
/// suffix, so that suffix is part of the contract even though the host label is not.
/// This constant is its last two labels; the rest are built from the signing name and
/// the request's region in [`feed_data_endpoints`].
const DATA_ENDPOINT_TLD: &str = "api.aws";

/// How many hex characters the opaque host label of a data endpoint carries.
///
/// The label itself is environment-specific — it is assigned by the data plane and is
/// deliberately NOT the feed id, so a client cannot read a feed id out of an endpoint
/// it was handed.
const DATA_ENDPOINT_LABEL_LEN: usize = 10;

/// The data endpoints of a feed in `region`.
///
/// One endpoint per feed ("Typically, there is only one endpoint"), as a data-plane
/// URL: `https://<opaque-label>.<signing-name>-data.<region>.api.aws/`, with the
/// trailing slash a client concatenates a path onto. The label is derived from the
/// feed id, so it is stable for the life of the feed and distinct per feed, while
/// being opaque rather than the id itself. The media data plane those endpoints serve
/// (`PutMedia` / `GetMetadata`) is not part of this service's Smithy model, so no
/// listener backs them — the field reports where the feed would be reached, which is
/// all the control plane models.
pub fn feed_data_endpoints(region: &str, id: &str) -> Vec<String> {
    let label = data_endpoint_label(id);
    vec![format!(
        "https://{label}.{SERVICE_NAMESPACE}-data.{region}.{DATA_ENDPOINT_TLD}/"
    )]
}

/// The opaque host label a feed's data endpoint carries: [`DATA_ENDPOINT_LABEL_LEN`]
/// hex characters derived from the feed id (FNV-1a, then mixed), so two feeds get two
/// labels and one feed keeps its own for life without the label spelling the id.
fn data_endpoint_label(id: &str) -> String {
    let mut hash = fnv1a64(id.as_bytes());
    let mut label = format!("{:016x}", splitmix64(&mut hash));
    label.truncate(DATA_ENDPOINT_LABEL_LEN);
    label
}

/// The aspect ratio a subtitling output is configured with when the request that
/// created it carried none.
///
/// `SubtitlingConfig$aspectRatio` is optional ("Elemental Inference uses the aspect
/// ratio to determine subtitle layout and line lengths") and the model declares no
/// default, so this value is OBSERVED behaviour rather than documented: every created
/// subtitling output comes back on the standard broadcast frame, 16:9. It is applied
/// to every subtitling output alike, never keyed on a particular request; should the
/// model ever declare the default, that declaration replaces this constant.
pub const DEFAULT_SUBTITLING_ASPECT_RATIO: AspectRatio = AspectRatio {
    width: 16,
    height: 9,
};

/// An output config as the service STORES it: every optional member the service fills
/// in for itself is materialised, so a read-modify-write round trip through
/// `GetFeed` -> `UpdateFeed` (a full-replacement PUT) carries the service's own
/// configuration back instead of dropping it.
///
/// Only members this service's model declares can be reported, so the defaults are
/// exactly the modelled optional members the service resolves itself: a subtitling
/// output's [`DEFAULT_SUBTITLING_ASPECT_RATIO`]. The cropping, clipping and
/// contextual-metadata configs declare no member the service resolves
/// (`smithy_query(query="shape",shape="CroppingConfig,ClippingConfig,ContextualMetadataConfig")`),
/// so they are stored as the request carried them.
pub fn output_config_with_defaults(mut config: OutputConfig) -> OutputConfig {
    if let OutputConfig::Subtitling(subtitling) = &mut config {
        subtitling
            .aspect_ratio
            .get_or_insert(DEFAULT_SUBTITLING_ASPECT_RATIO);
    }
    config
}

/// The region this request was signed for, or [`DEFAULT_REGION`] when it carries no
/// SigV4 credential scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Draw one store's id entropy.
///
/// Two sources that differ between two stores: the wall clock at the moment the
/// first id is minted and this process's id. Mixed together, so the seed is spread
/// over the whole 64-bit range rather than tracking the clock.
fn fresh_run_seed() -> u64 {
    let nanos = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_nanos() as u64)
        .unwrap_or_default();
    let mut seed = nanos ^ (u64::from(::std::process::id()) << 32);
    splitmix64(&mut seed)
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

/// FNV-1a over `bytes`: the one place this service hashes a byte string into a seed,
/// so the data-endpoint label's derivation is stated once.
fn fnv1a64(bytes: &[u8]) -> u64 {
    // FNV-1a's 64-bit offset basis and prime.
    let mut hash: u64 = 0xCBF2_9CE4_8422_2325;
    for byte in bytes {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

/// SplitMix64: a well-known 64-bit mixer, used here only to spread a counter or a
/// hashed byte string over the whole 64-bit range. Deterministic, so everything
/// derived from it (an id this service mints, a feed's endpoint label) is identical
/// on every call with the same input.
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
// The fixture catalog: the events the external sports data source holds, as this
// emulator holds them, and the window arithmetic a search selects them with
// (see the module docs). The records themselves live in the state struct above.
// ---------------------------------------------------------------------------

/// Seconds in a UTC day. A `FixtureDate` names a UTC day ("in UTC",
/// `SearchFixturesRequest$startDate`), and UTC days have no offset changes, so a day
/// is exactly this many seconds and an epoch-second count divides into day numbers.
const SECONDS_PER_DAY: i64 = 86_400;

/// The widest search window `SearchFixtures` accepts, in days, counting both ends:
/// "The window from startDate through endDate must not exceed seven days"
/// (`SearchFixturesRequest$endDate`).
pub const MAX_SEARCH_WINDOW_DAYS: i64 = 7;

/// A fixture whose scheduled start is still ahead.
///
/// `GetFixtureResponse$status` is the lifecycle status "as provided by the data
/// source. For example, Scheduled or Completed", so the member is a free-form string
/// whose values belong to the data source: this constant and
/// [`FIXTURE_STATUS_COMPLETED`] are the two the model's own documentation names, and a
/// held fixture carries whatever status it was given.
pub const FIXTURE_STATUS_SCHEDULED: &str = "Scheduled";

/// A fixture that has been played — the other status the model's documentation names
/// (see [`FIXTURE_STATUS_SCHEDULED`]).
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

    /// Every sport the enum declares, in model order.
    pub const ALL: &'static [Self] = &[Self::Basketball, Self::AmericanFootball];
}

/// One fixture the data source holds: a sports event, as the store keeps it.
///
/// Every member is the data source's own — the emulator derives none of them — so the
/// `FixtureSummary` `SearchFixtures` lists and the `GetFixture` response for the same
/// `fixtureId` are two projections of this ONE record and agree member for member.
#[derive(Debug, Clone)]
pub struct Fixture {
    /// The `FixtureId` (modelled length 1..128): the key this fixture is held under
    /// and the id `GetFixture` answers for.
    pub id: String,
    /// The sport this fixture belongs to, which is what a search's required `sport`
    /// selects on.
    pub sport: DataSourceSport,
    /// The fixture's name, "as provided by the data source. For example, the names of
    /// the two competing teams" (`FixtureSummary$name`).
    pub name: String,
    /// The competition the fixture belongs to; `None` for a fixture the data source
    /// gives no group for ("The data source doesn't provide this information for
    /// every fixture", `GetFixtureResponse$fixtureGroup`).
    pub group: Option<String>,
    /// The scheduled start, in seconds since the Unix epoch. Its UTC day is what a
    /// search's date window selects on ("The search includes fixtures that are
    /// scheduled on this day", `SearchFixturesRequest$startDate`), so a held fixture
    /// always has one.
    pub scheduled_start_secs: i64,
    /// The lifecycle status "as provided by the data source"
    /// (`FixtureSummary$status`) — see [`FIXTURE_STATUS_SCHEDULED`].
    pub status: String,
    /// The competitors (`CompetitorList`, modelled length ..30).
    pub competitors: Vec<Competitor>,
}

impl Fixture {
    /// The UTC day this fixture is scheduled on, as a day number since the Unix
    /// epoch — the figure a search's window is compared against.
    pub fn scheduled_day(&self) -> i64 {
        self.scheduled_start_secs.div_euclid(SECONDS_PER_DAY)
    }

    /// The scheduled start as the modelled `Timestamp`.
    pub fn scheduled_start(&self) -> ::aws_smithy_types::DateTime {
        ::aws_smithy_types::DateTime::from_secs(self.scheduled_start_secs)
    }

    /// The `FixtureSummary` projection `SearchFixtures` reports.
    pub fn summary(&self) -> FixtureSummary {
        FixtureSummary {
            fixture_id: self.id.clone(),
            name: self.name.clone(),
            fixture_group: self.group.clone(),
            scheduled_start: Some(self.scheduled_start()),
            status: self.status.clone(),
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

/// The epoch-second instant of `hour`:`minute` UTC on `epoch_day`, or `None` when
/// that day number is not a day the calendar has, or the time of day is not a time a
/// UTC day has (`hour` outside 0..=23, `minute` outside 0..=59).
///
/// The seam's companion to [`parse_fixture_date`]: a fixture is held with the instant
/// it starts at, and this is how a caller that knows the day and the kick-off time
/// builds that instant without reimplementing the UTC day arithmetic. Refusing a time
/// of day outside the day is what keeps the instant ON the day the caller named, since
/// that day is what a search window selects on; every step is checked, so no input
/// wraps or panics.
pub fn fixture_start_secs(epoch_day: i64, hour: i64, minute: i64) -> Option<i64> {
    fixture_date_string(epoch_day)?;
    if !(0..24).contains(&hour) || !(0..60).contains(&minute) {
        return None;
    }
    epoch_day
        .checked_mul(SECONDS_PER_DAY)?
        .checked_add(hour.checked_mul(3_600)?)?
        .checked_add(minute.checked_mul(60)?)
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
