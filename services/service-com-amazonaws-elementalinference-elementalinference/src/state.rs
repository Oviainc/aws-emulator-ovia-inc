// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-elementalinference-elementalinference service.
//!
//! The feed is this service's primary resource: `CreateFeed` mints one, `GetFeed` /
//! `ListFeeds` report it, `UpdateFeed` mutates it and `DeleteFeed` retires it. The
//! record below is the single copy every one of those handlers reads and writes, so
//! the shapes the handlers hand back are all built from it. A feed's resource-based
//! policy is part of that record ([`Feed::policy`]) rather than a store of its own:
//! `PutFeedPolicy` attaches or replaces it, `GetFeedPolicy` reports it and
//! `DeleteFeedPolicy` detaches it, so the document a client reads back is the one the
//! last put stored, and a feed that leaves the store takes its policy with it.
//!
//! A deletion that has COMPLETED leaves NOTHING behind: a delete is ACCEPTED (HTTP 202)
//! and the record is REMOVED from the store the instant the deletion falls due (see
//! `settle_due_transitions`). From that instant the id names no resource of this
//! service at all — for every operation, the delete itself included — so a repeat of a
//! delete whose deletion has completed is refused with `ResourceNotFoundException`
//! exactly as an id this store never held is, and no record outlives the resource it
//! described. That also keeps the store bounded by the live quotas below alone.
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
//! Both of those resources are REGIONAL: this service is reached through a Region's
//! endpoint and each endpoint owns the resources created through it. So every record
//! carries the Region it was created in ([`Feed::region`], [`Dictionary::region`]), and
//! every operation that reaches one resolves it WITHIN the Region the request was signed
//! for ([`ComAmazonawsElementalinferenceElementalinferenceState::feed`],
//! [`ComAmazonawsElementalinferenceElementalinferenceState::dictionary`],
//! [`ComAmazonawsElementalinferenceElementalinferenceState::feeds_in`],
//! [`ComAmazonawsElementalinferenceElementalinferenceState::dictionaries_in`]): a
//! resource is listed, read and mutated through the endpoint that created it alone, an
//! id presented to any other Region names nothing there, and the quotas below are each
//! Region's own. It is the same whole-ARN rule
//! [`ComAmazonawsElementalinferenceElementalinferenceState::tagged_resource_mut`] holds
//! the tagging path to, so the id-based and the ARN-based paths cannot disagree about
//! which resources a Region holds.
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
//! That store is FILLED when it is constructed, with the schedule this emulator's own
//! data source holds ([`default_fixture_catalog`]): a client can only learn a
//! `fixtureId` from a search, so a catalog holding nothing would make the whole
//! fixtures pair unusable — every search empty whatever it asked for, `GetFixture`
//! unreachable, and a clipping output with no event to name. The schedule covers the
//! [`CATALOG_SPAN_DAYS`] days either side of the day the store is built, for each sport
//! the `DataSourceSport` enum declares. A window OUTSIDE that span holds no event and
//! so still reports an empty list ("The array is empty if no fixtures match",
//! `SearchFixturesResponse$fixtures`), the in-process seam
//! ([`ComAmazonawsElementalinferenceElementalinferenceState::seed_fixtures`] and
//! [`ComAmazonawsElementalinferenceElementalinferenceState::clear_fixtures`]) still
//! installs or drops records, and no request mutates what a later caller sees.

use std::collections::{BTreeMap, HashMap};
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

/// How long a clock-owed transition takes: a feed's `DELETING` -> gone and its
/// `UPDATING` -> the status it was reconfigured from, and a dictionary's `CREATING` ->
/// `AVAILABLE` and `DELETING` -> gone.
///
/// AWS documents no duration for these, so this span is chosen rather than
/// specified: it is non-zero, so a caller that reads a resource straight after
/// starting the transition still observes the transient status the real service
/// reports, and it is short enough that no caller has to wait long for the settled
/// state. The due instant is stored on the record and materialised on the next
/// call — nothing ever sleeps for it.
///
/// A CREATED FEED owes no such transition: the recorded service answers `CreateFeed`
/// with a feed that is already usable — a `GetFeed` taken immediately after reports
/// `AVAILABLE`, and the association the caller makes in its very next request
/// succeeds — so a feed is stored settled and this span never applies to it.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(400);

/// The most feeds this emulator keeps in ONE Region at once.
///
/// Elemental Inference limits how many feeds an account may hold (the model declares
/// `ServiceQuotaExceededException` for `CreateFeed` precisely for that), but it does
/// not state the figure, so this one is the emulator's own. A service quota is a
/// per-Region figure, so this is what one Region's endpoint holds: past it `CreateFeed`
/// refuses with the modelled `ServiceQuotaExceededException` (HTTP 402), which is the
/// answer the real service gives when the quota is exhausted, and together with
/// [`MAX_REGIONS`] it is what bounds a store that would otherwise grow by one record
/// per request forever, and caps the work every settle pass and every page can do.
pub const MAX_FEEDS: usize = 100;

/// The most dictionaries this emulator keeps in ONE Region at once.
///
/// As with [`MAX_FEEDS`], Elemental Inference limits how many dictionaries an account
/// may hold (the model declares `ServiceQuotaExceededException` for
/// `CreateDictionary`) without stating the figure, so this one is the emulator's own,
/// and it is per Region because a service quota is. Past it `CreateDictionary` refuses
/// with the modelled `ServiceQuotaExceededException` (HTTP 402), which is also the
/// answer the real service gives once the quota is exhausted.
pub const MAX_DICTIONARIES: usize = 100;

/// The most Regions this store keeps resources of one kind in at once.
///
/// The Region a record belongs to comes from the request's own SigV4 credential scope
/// ([`request_region`]), so a caller chooses it: without a bound on how many distinct
/// Regions the store partitions across, a per-Region quota would bound nothing and a
/// run of requests could grow the store without limit. With it the store holds at most
/// [`MAX_REGIONS`] * [`MAX_FEEDS`] feeds and as many dictionaries, and a create signed
/// for one Region past that earns the same modelled `ServiceQuotaExceededException`
/// (HTTP 402) a create past a Region's own quota does. The figure is comfortably more
/// than the Regions this service is offered in, so no caller using it as a client of
/// the real service's Regions meets it.
pub const MAX_REGIONS: usize = 32;

/// The `FeedStatus` values this slice of the service produces.
///
/// The model's enum also carries `CREATING`, and it is never reported: the recorded
/// service hands back a feed that is immediately usable, so `CreateFeed` stores it
/// `AVAILABLE` and there is no window in which a caller is told to wait ("After creation
/// of the feed has succeeded, the status will be AVAILABLE",
/// `CreateFeedResponse$status`). Nor is `DELETED` a value a feed record holds: a
/// deletion that completes REMOVES the feed, so there is no feed left to report it (see
/// the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedStatus {
    /// The feed is settled and serviceable, with no resource associated.
    Available,
    /// A resource (the source media) is associated with the feed: `AssociateFeed`
    /// claims the feed for one resource, and that claim is what `ACTIVE` reports.
    Active,
    /// The feed is being reconfigured in place: `UpdateFeed` has replaced its payload
    /// and the reconfiguration is in flight. Transient — the clock moves the feed back
    /// into the status it was reconfigured from ([`Feed::settles_into`]).
    Updating,
    /// A `DeleteFeed` has been accepted; the deletion completes when it falls due.
    Deleting,
    /// The feed's resource has been released by `DisassociateFeed`. Terminal: a
    /// released feed takes no further association.
    Archived,
}

impl FeedStatus {
    /// The value this status takes on the wire, as spelled by the `FeedStatus` enum.
    pub fn as_wire(self) -> &'static str {
        match self {
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
/// The EXCEPTION is a refusal whose point is to report the state it was decided from:
/// where the service's refusal states the status it observed or how many records held
/// the request back, that figure is the information the refusal exists to give, so the
/// message is BUILT from the store and tracks it ([`feed_not_associated_message`],
/// [`dictionary_referenced_message`] — functions rather than constants for exactly that
/// reason). A fixed sentence in their place tells a caller nothing it did not already
/// know, and leaves tooling that reads the status or the count out of the refusal with
/// nothing to read.
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

/// `DisassociateFeed` on a feed that holds no resource to release: the refusal REPORTS
/// the status the feed was observed in, which is what decided it — recorded example,
/// for a feed that was never associated: "You can't disassociate this feed. It has
/// status AVAILABLE which indicate that it's not currently associated with a resource."
///
/// So the status is read from the stored feed on every call: a feed that is settled
/// without a resource, one whose deletion has been accepted and one that has been
/// released each report their own status, and a client diagnosing the refusal can tell
/// which of them blocked it.
pub fn feed_not_associated_message(status: FeedStatus) -> String {
    format!(
        "You can't disassociate this feed. It has status {} which indicate that it's not \
         currently associated with a resource.",
        status.as_wire()
    )
}

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
/// feed's subtitling configuration."): the refusal REPORTS how many feeds reference the
/// dictionary, which is what decided it — recorded example, for a single referencing
/// feed: "Cannot delete dictionary because it is referenced by 1 feed(s). Remove all
/// references by delete or disable related feed output(s) before deleting dictionary."
///
/// So the count is derived from the feed store on every call
/// ([`ComAmazonawsElementalinferenceElementalinferenceState::feeds_blocking_delete`]):
/// a caller tearing a dictionary down learns how much is left to remove, while the
/// refusal still names no feed, so it discloses no other caller's resource.
pub fn dictionary_referenced_message(feed_count: usize) -> String {
    format!(
        "Cannot delete dictionary because it is referenced by {feed_count} feed(s). Remove \
         all references by delete or disable related feed output(s) before deleting \
         dictionary."
    )
}

/// `PutFeedPolicy` with a `policy` that is not a policy document.
///
/// A resource-based policy is a JSON policy document (`PutFeedPolicyRequest$policy`:
/// "The resource-based policy document to attach to the feed"), so a body that does not
/// parse as a single JSON object is not one. The rule is about the member's own value,
/// so the message names the member and the form it must take and echoes nothing the
/// request supplied.
pub const INVALID_POLICY_DOCUMENT_MESSAGE: &str =
    "The policy member must be a resource-based policy document: supply one JSON object.";

/// Whether `policy` is a policy document, i.e. parses as a single JSON object.
///
/// One scan shared by every operation that is handed a policy document, so no two of
/// them can disagree about what the service accepts as one.
pub fn is_policy_document(policy: &str) -> bool {
    ::serde_json::from_str::<::serde_json::Value>(policy).is_ok_and(|parsed| parsed.is_object())
}

/// `SearchFixtures` with `endDate` before `startDate`: the rule is the ORDER of the two
/// members, so the recorded refusal names the members and not the dates they carried.
pub const END_DATE_BEFORE_START_DATE_MESSAGE: &str = "endDate must not be earlier than startDate.";

/// The first name `names` yields twice, if any.
///
/// An output's `name` IDENTIFIES it within its feed: it is how `AssociateFeed` decides
/// that an appended output would collide with one the feed holds, and how a caller
/// reading a feed tells one output from another. So a request that lists one name twice
/// describes a feed in which an output is not addressable at all, and every operation
/// that supplies a feed's output list goes through this one scan — `CreateFeed` and
/// `UpdateFeed`, which REPLACE the list, and `AssociateFeed`, which appends to it — so
/// none of them can disagree about what a feed's outputs may look like.
pub fn first_duplicate_output_name<'a>(
    mut names: impl Iterator<Item = &'a str>,
) -> Option<&'a str> {
    let mut seen = std::collections::BTreeSet::new();
    names.find(|name| !seen.insert(*name))
}

/// A request that lists more than one output under `name`: the rule is about ONE value's
/// own validity (the name that is not unique), so the message names the value the
/// REQUEST supplied, and the same sentence answers whichever operation carried the list.
pub fn duplicate_output_name_message(name: &str) -> String {
    format!(
        "The request lists more than one output named {name}. \
         Output names must be unique within a feed."
    )
}

/// An `UpdateFeed` output whose `fromAssociation` disagrees with the value the service
/// holds for the output of that name.
///
/// `UpdateOutput$fromAssociation` is set by the service — true for an output
/// `AssociateFeed` appended, false for one a create or an update introduced — and the
/// member exists so a caller can echo the value it read back; the request may not change
/// it. The violated rule is about ONE value's own validity, so the message names the
/// value the request supplied and the member it sat on, with the list position (1-based,
/// as a validation message indexes a list) rather than the stored value, which is not
/// the request's to know from the refusal.
pub fn from_association_not_changeable_message(position: usize, supplied: bool) -> String {
    format!(
        "1 validation error detected: Value '{supplied}' at \
         'outputs.{position}.member.fromAssociation' failed to satisfy constraint: Member is \
         set by Elemental Inference and must not be changed. Use GetFeed to read its current \
         value."
    )
}

/// The `OutputStatus` value a disabled output reports — one of the two values that
/// enum admits (`OUTPUT_STATUS_VALUES`: `DISABLED`, `ENABLED`).
pub const OUTPUT_STATUS_DISABLED: &str = "DISABLED";

/// The most outputs one feed may hold.
///
/// `AssociateFeed` APPENDS outputs ("New outputs will be appended"), so without a
/// bound a caller could grow one feed's record by another output on every call. AWS
/// does not publish the figure, so this one is the emulator's own: it is the BOUND on
/// a feed's output list, and past it a request refuses with the modelled
/// `ServiceQuotaExceededException` (HTTP 402) — the answer the real service gives
/// when a request would exceed a service quota.
///
/// It binds EVERY operation that sets a feed's output list, not only the append:
/// `CreateFeed` and `UpdateFeed` each REPLACE the list, so one request of theirs would
/// otherwise put a stored feed arbitrarily far past the bound in a single call. Holding
/// it on all three is what makes the stored list short — and the stored list is what a
/// later call reads, compares against and reports, so the bound on it bounds that work
/// too.
pub const MAX_OUTPUTS_PER_FEED: usize = 20;

/// A request whose output list is longer than [`MAX_OUTPUTS_PER_FEED`], for the
/// operations that SET the list (`CreateFeed`, `UpdateFeed`).
///
/// The quota is the rule, so the message states the quota and nothing about the request
/// or the stored feed — exactly as the append's own refusal does.
pub fn too_many_outputs_message() -> String {
    format!(
        "A feed holds at most {MAX_OUTPUTS_PER_FEED} outputs. Specify fewer outputs or \
         request a quota increase."
    )
}

/// The `DictionaryStatus` values this slice of the service produces.
///
/// These are the LIFECYCLE statuses a dictionary record holds. The model's enum also
/// carries `REFERENCED`, which is not a stored value: it is DERIVED from the feeds whose
/// subtitling outputs name the dictionary (see [`Dictionary::reported_status`]). `DELETED`
/// is not one either: a deletion that completes REMOVES the dictionary, so no record is
/// left to report it (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DictionaryStatus {
    /// The dictionary has been accepted and is not usable yet.
    Creating,
    /// The dictionary is settled and can be referenced by an output.
    Available,
    /// A `DeleteDictionary` has been accepted; the deletion completes when it falls
    /// due.
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
    /// The Region whose endpoint this dictionary was created through, and so the only
    /// one it exists for: the Region of the request that minted it
    /// ([`request_region`]), which is also the Region field of [`Self::arn`].
    pub region: String,
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

    /// Whether this dictionary is still EXACTLY what a `CreateDictionary` left behind,
    /// in the one respect a create does not state — its lifecycle: a dictionary is
    /// minted transient and settles on the clock, so either of those statuses is what a
    /// create produces, while one whose deletion has been accepted is on its way out.
    ///
    /// The dictionary's counterpart of [`Feed::is_as_created`], read by
    /// [`ComAmazonawsElementalinferenceElementalinferenceState::dictionary_created_from`]
    /// for the same reason: every other member a create of a dictionary states is a
    /// stored one, so comparing those members with the request covers the rest of the
    /// record. What it reads is the record's CURRENT content, not its history: a
    /// dictionary a feed's subtitling output has since named is one of these — the
    /// stored members are untouched, only the status it REPORTS is derived
    /// ([`Self::reported_status`]) — so a replay answers with it and reports
    /// `REFERENCED`.
    pub fn is_as_created(&self) -> bool {
        matches!(
            self.status,
            DictionaryStatus::Creating | DictionaryStatus::Available
        )
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
///
/// Two outputs are EQUAL when every member of the stored record matches, which is what
/// tells an output a request would really add from one the feed already holds exactly as
/// asked (`Feed::outputs_to_append`): the comparison is made on the stored form of both,
/// so a request that leaves an optional member to the service's default
/// ([`output_config_with_defaults`]) still matches the output that default produced. It
/// is the same comparison that tells a create this Region has already served from a new
/// one ([`ComAmazonawsElementalinferenceElementalinferenceState::feed_created_from`]).
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// The Region whose endpoint this feed was created through, and so the only one it
    /// exists for: the Region of the request that minted it ([`request_region`]), which
    /// is also the Region field of [`Self::arn`].
    pub region: String,
    pub data_endpoints: Vec<String>,
    pub outputs: Vec<Output>,
    pub access_role_arn: Option<String>,
    pub status: FeedStatus,
    /// The resource associated with the feed; `None` until `AssociateFeed` runs.
    pub association: Option<FeedAssociation>,
    /// The name of the resource whose RELEASE this feed has already served, and `None`
    /// while no release of it has been served.
    ///
    /// `DisassociateFeed` is `@idempotent` and its `associatedResourceName` is the
    /// operation's `@idempotencyToken`, so a retry of the same release — what an SDK
    /// sends when it never saw the first response — has to answer the first call's
    /// success again instead of being judged against the state that first call left
    /// behind. This member is what tells a retry from a new request: it records the name
    /// the served release carried, so a request naming it is that same release once more
    /// and any other name is a request of its own.
    ///
    /// It lives on the feed record rather than in a store of its own, so it is bounded by
    /// the feeds that exist and leaves the store with the feed it belongs to — a released
    /// feed that is then deleted is simply not found, as any absent feed is.
    pub released_resource_name: Option<String>,
    /// The resource-based policy document attached to the feed, or `None` while the
    /// feed carries no policy.
    ///
    /// A feed holds AT MOST ONE policy: `PutFeedPolicy` "attaches or replaces" it, so
    /// a second put overwrites this member rather than adding beside it,
    /// `GetFeedPolicy` reports exactly the document the last put stored and
    /// `DeleteFeedPolicy` clears it. It lives on
    /// the feed record, so a policy is reachable only for as long as the feed it is
    /// attached to is, and the feed's removal takes its policy with it — there is no
    /// second map that could outlive the resource or disagree with it.
    pub policy: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// When the clock owes this feed's transient status its next move, if any.
    pub transition_due_at: Option<Instant>,
    /// The status this feed goes back into when the in-flight reconfiguration it is
    /// carrying falls due, and `None` while no reconfiguration is in flight.
    ///
    /// A reconfiguration does not CHANGE which lifecycle status a feed is in — it
    /// suspends it for the span of the work — so the status the feed was reconfigured
    /// from has to be remembered somewhere for the settle pass to put it back. It is
    /// kept on the feed record beside [`Self::transition_due_at`], so the instant the
    /// move falls due and the status it moves to are one fact about one feed and cannot
    /// drift apart.
    pub settles_into: Option<FeedStatus>,
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
        // Whatever this feed was going to settle back into, it is this transition's
        // outcome that decides where the feed goes now.
        self.settles_into = None;
        due
    }

    /// Put this feed into `UPDATING` for the span of a reconfiguration of its payload,
    /// remembering the status the clock settles it back into, and return the instant
    /// that move falls due (handed to
    /// [`ComAmazonawsElementalinferenceElementalinferenceState::note_due`] exactly as
    /// [`Self::enter_transient`]'s is).
    ///
    /// A reconfiguration that arrives while another is still in flight restarts the
    /// span and keeps the SETTLED status already remembered: the status to go back to is
    /// the one the feed held before any of the work started, never the transient one it
    /// is carrying now.
    #[must_use]
    pub fn begin_reconfiguration(&mut self, now: Instant) -> Instant {
        let settled = self.settles_into.unwrap_or(self.status);
        let due = self.enter_transient(FeedStatus::Updating, now);
        self.settles_into = Some(settled);
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

    /// Which of `requested` — the outputs a request asks this feed to hold, already in
    /// their STORED form ([`Output::new`]) — an append would really add.
    ///
    /// An output the feed ALREADY holds exactly as `requested` describes it is in place
    /// already, so it is neither appended a second time nor refused: `AssociateFeed` is
    /// `@idempotent` and its `associatedResourceName` is the operation's
    /// `@idempotencyToken`, so the request an SDK repeats when it never saw the first
    /// response is that one request once more, and its effect has to be applied ONCE. A
    /// repeat whose outputs are all in place therefore adds nothing, and one that also
    /// carries an output the feed does not hold yet adds exactly that one.
    ///
    /// An output whose NAME the feed holds under a DIFFERENT definition is not in place
    /// and is not appendable either: appending it would MODIFY the stored output, which
    /// this operation cannot do ("You can't modify an existing output (for example to
    /// change its name). Instead, use UpdateFeed"). That one is refused, and the `Err`
    /// carries its name for the refusal to report.
    pub fn outputs_to_append(&self, requested: Vec<Output>) -> Result<Vec<Output>, String> {
        let mut to_append = Vec::with_capacity(requested.len());
        for candidate in requested {
            match self.outputs.iter().find(|held| held.name == candidate.name) {
                // Already in place, exactly as this request describes it.
                Some(held) if *held == candidate => {}
                // The same name carrying something else: a modification.
                Some(_) => return Err(candidate.name),
                None => to_append.push(candidate),
            }
        }
        Ok(to_append)
    }

    /// The name of the resource currently associated with this feed, if any.
    pub fn associated_resource_name(&self) -> Option<&str> {
        self.association
            .as_ref()
            .map(|association| association.associated_resource_name.as_str())
    }

    /// Whether a release naming `resource_name` has ALREADY been served for this feed,
    /// i.e. whether a `DisassociateFeed` carrying that name is the same release once more
    /// rather than a new request ([`Self::released_resource_name`]).
    pub fn has_served_release_of(&self, resource_name: &str) -> bool {
        self.released_resource_name.as_deref() == Some(resource_name)
    }

    /// The tags of this feed as the `TagMap` every feed response carries.
    ///
    /// As with [`Dictionary::tag_map`], an untagged feed reports an EMPTY map rather
    /// than an absent member.
    pub fn tag_map(&self) -> std::collections::HashMap<String, String> {
        self.tags.clone().into_iter().collect()
    }

    /// Whether this feed is still EXACTLY what a `CreateFeed` left behind, in every
    /// respect a create does not state: settled in the status a create produces, with
    /// no resource associated, no release served, no policy attached, nothing owed to
    /// the clock and no deletion accepted.
    ///
    /// A feed some later call has moved on is not a record a repeated create can answer
    /// with ([`ComAmazonawsElementalinferenceElementalinferenceState::feed_created_from`]):
    /// the caller of that first create saw its response and acted on it, so a request
    /// arriving now is a create of its own and not that one retried. What it reads is
    /// the record's CURRENT content, not its history, so a feed an update changed and
    /// then changed back is once more a feed that create would have produced.
    pub fn is_as_created(&self) -> bool {
        self.status == FeedStatus::Available
            && self.association.is_none()
            && self.released_resource_name.is_none()
            && self.policy.is_none()
            && self.transition_due_at.is_none()
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
#[derive(Debug)]
pub struct ComAmazonawsElementalinferenceElementalinferenceState {
    /// Every live feed, keyed by its `FeedId`. `BTreeMap` so `ListFeeds` has one
    /// stable order to paginate over. PARTITIONED BY REGION: each record carries the
    /// Region it was created in ([`Feed::region`]) and is reachable only through that
    /// Region ([`Self::feed`], [`Self::feeds_in`]), so this one map holds every
    /// Region's feeds without any of them seeing another's. BOUNDED at [`MAX_FEEDS`]
    /// entries per Region across at most [`MAX_REGIONS`] Regions: `CreateFeed` refuses
    /// past either with the modelled `ServiceQuotaExceededException`, so this map
    /// cannot grow without limit however many requests arrive.
    pub feeds: BTreeMap<String, Feed>,
    /// Monotonic source for minted feed ids: one more per feed this emulator mints.
    pub next_feed_seq: u64,
    /// Every live dictionary, keyed by its `DictionaryId`. `BTreeMap` so
    /// `ListDictionaries` has one stable order to paginate over. Partitioned by Region
    /// exactly as [`Self::feeds`] is ([`Dictionary::region`], [`Self::dictionary`],
    /// [`Self::dictionaries_in`]), and BOUNDED at [`MAX_DICTIONARIES`] entries per
    /// Region across at most [`MAX_REGIONS`] Regions: `CreateDictionary` refuses past
    /// either with the modelled `ServiceQuotaExceededException`.
    pub dictionaries: BTreeMap<String, Dictionary>,
    /// Monotonic source for minted dictionary ids: one more per dictionary this
    /// emulator mints.
    pub next_dictionary_seq: u64,
    /// Every fixture the external sports data source holds, keyed by its `FixtureId`.
    /// `BTreeMap` so a search has ONE stable order to page over, which is what a
    /// `nextToken` names a position in.
    ///
    /// Filled from [`default_fixture_catalog`] when the store is constructed (see the
    /// module docs) and afterwards only through the in-process seam
    /// ([`Self::seed_fixtures`], [`Self::clear_fixtures`]): `SearchFixtures` and
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

/// A fresh store: no feeds, no dictionaries — and the data-source catalog already
/// holding this emulator's schedule ([`default_fixture_catalog`]).
///
/// `engine::InMemoryEngine::state` builds a service's state with `Default` the first
/// time a handler reaches it, so this is the ONE construction every store goes through,
/// and it is why the fixture catalog is filled here: no modelled operation creates a
/// fixture (AWS relays them from an external sports data source), so a store that came
/// up holding none could never be given one over the wire, and both `SearchFixtures`
/// and `GetFixture` would answer nothing for the life of the process.
impl Default for ComAmazonawsElementalinferenceElementalinferenceState {
    fn default() -> Self {
        let mut state = Self {
            feeds: BTreeMap::new(),
            next_feed_seq: 0,
            dictionaries: BTreeMap::new(),
            next_dictionary_seq: 0,
            fixtures: BTreeMap::new(),
            earliest_due: None,
            run_seed: None,
        };
        state.seed_fixtures(default_fixture_catalog(current_epoch_secs()));
        state
    }
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

    /// The one feed `id` names IN `region`, or `None` when that Region holds no such
    /// feed.
    ///
    /// This service is regional: a feed exists for the Region whose endpoint created it
    /// and for no other, so a lookup is a (Region, id) lookup and an id that names a
    /// feed of a different Region names nothing. Every reporting operation that reaches
    /// a feed by its `FeedId` goes through this (and every mutating one through
    /// [`Self::feed_mut`]), so none of them can disagree about which feeds a Region
    /// holds, or answer for another Region's.
    ///
    /// A feed whose deletion has COMPLETED is not one of them, and needs no exclusion
    /// here: completing the deletion removed the record, so the id resolves to nothing
    /// for EVERY operation — `DeleteFeed` included — exactly as an id this store never
    /// held does.
    pub fn feed(&self, region: &str, id: &str) -> Option<&Feed> {
        self.feeds.get(id).filter(|feed| feed.region == region)
    }

    /// The one feed `id` names in `region`, for a caller that writes it — the mutating
    /// half of [`Self::feed`], with the same Region rule.
    pub fn feed_mut(&mut self, region: &str, id: &str) -> Option<&mut Feed> {
        self.feeds.get_mut(id).filter(|feed| feed.region == region)
    }

    /// Every feed of `region`, in `FeedId` order — what `ListFeeds` pages over, since a
    /// list reports the feeds of the endpoint it was sent to and nothing else.
    pub fn feeds_in<'a>(&'a self, region: &'a str) -> impl Iterator<Item = &'a Feed> + 'a {
        self.feeds
            .values()
            .filter(move |feed| feed.region == region)
    }

    /// The one dictionary `id` names in `region`, or `None` when that Region holds no
    /// such dictionary — the dictionary's counterpart of [`Self::feed`], with the same
    /// Region rule and the same rule about a completed deletion.
    pub fn dictionary(&self, region: &str, id: &str) -> Option<&Dictionary> {
        self.dictionaries
            .get(id)
            .filter(|dictionary| dictionary.region == region)
    }

    /// The one dictionary `id` names in `region`, for a caller that writes it.
    pub fn dictionary_mut(&mut self, region: &str, id: &str) -> Option<&mut Dictionary> {
        self.dictionaries
            .get_mut(id)
            .filter(|dictionary| dictionary.region == region)
    }

    /// Every dictionary of `region`, in `DictionaryId` order — what `ListDictionaries`
    /// pages over.
    pub fn dictionaries_in<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = &'a Dictionary> + 'a {
        self.dictionaries
            .values()
            .filter(move |dictionary| dictionary.region == region)
    }

    /// The refusal `CreateFeed` answers in `region`, or `None` while that Region may
    /// take one more feed.
    ///
    /// A service quota is a PER-REGION figure, so what the Region already holds is what
    /// decides it: another Region's feeds neither consume it nor are consumed by it, and
    /// a feed whose deletion has completed holds nothing, since the record went with the
    /// resource ([`Self::feeds_in`]).
    /// The second clause is the STORE's own bound rather than a quota of the service
    /// (see [`MAX_REGIONS`]): a Region this store holds no record of is a new partition,
    /// and there are at most that many. Each refusal states the bound it was decided
    /// from and reports nothing else about what the account holds.
    pub fn feed_quota_refusal(&self, region: &str) -> Option<String> {
        let held = self.feeds_in(region).count();
        if held >= MAX_FEEDS {
            return Some(format!(
                "This account already holds the maximum of {MAX_FEEDS} feeds in this Region. \
                 Delete unused feeds or request a quota increase."
            ));
        }
        let new_partition = !self.feeds.values().any(|feed| feed.region == region);
        if new_partition && self.feed_region_count() >= MAX_REGIONS {
            return Some(format!(
                "This account already holds feeds in the maximum of {MAX_REGIONS} Regions. \
                 Delete the feeds of a Region you no longer use or request a quota increase."
            ));
        }
        None
    }

    /// The refusal `CreateDictionary` answers in `region`, or `None` while that Region
    /// may take one more dictionary — the dictionary's counterpart of
    /// [`Self::feed_quota_refusal`], against [`MAX_DICTIONARIES`].
    pub fn dictionary_quota_refusal(&self, region: &str) -> Option<String> {
        let held = self.dictionaries_in(region).count();
        if held >= MAX_DICTIONARIES {
            return Some(format!(
                "This account already holds the maximum of {MAX_DICTIONARIES} dictionaries in \
                 this Region. Delete unused dictionaries or request a quota increase."
            ));
        }
        let new_partition = !self
            .dictionaries
            .values()
            .any(|dictionary| dictionary.region == region);
        if new_partition && self.dictionary_region_count() >= MAX_REGIONS {
            return Some(format!(
                "This account already holds dictionaries in the maximum of {MAX_REGIONS} \
                 Regions. Delete the dictionaries of a Region you no longer use or request a \
                 quota increase."
            ));
        }
        None
    }

    /// The feed a `CreateFeed` request has ALREADY minted in `region`, if this store
    /// holds it: the one whose stored name, access role, output list and tags are
    /// exactly what the request asks for, and which is otherwise still as that create
    /// left it ([`Feed::is_as_created`]).
    ///
    /// A create of this service carries no `@idempotencyToken`, so the request's own
    /// members are its key. The operation is `@idempotent`, which means the effect of
    /// several identical requests is the effect of ONE: the second and later ones
    /// answer with the resource the first minted instead of minting another, so an SDK
    /// retry of a create whose response was lost cannot double what the Region holds
    /// (nor what it has spent of its quota). Only a difference in the request itself
    /// makes it a different create.
    ///
    /// A record the Region no longer holds, or one a later call has moved on, is not
    /// one a replay can answer with: a feed whose deletion has been accepted or which
    /// has since been associated, released or given a policy is passed over, and a
    /// replay after the removal fell due mints afresh — the recognition is against what
    /// the Region holds NOW, with no index to keep in step with the store.
    ///
    /// Scans at most [`MAX_FEEDS`] feeds of the Region, each with at most
    /// [`MAX_OUTPUTS_PER_FEED`] outputs.
    pub fn feed_created_from<'a>(
        &'a self,
        region: &'a str,
        name: &str,
        access_role_arn: Option<&str>,
        outputs: &[Output],
        tags: &BTreeMap<String, String>,
    ) -> Option<&'a Feed> {
        self.feeds_in(region).find(|feed| {
            feed.is_as_created()
                && feed.name == name
                && feed.access_role_arn.as_deref() == access_role_arn
                && feed.outputs == outputs
                && &feed.tags == tags
        })
    }

    /// The dictionary a `CreateDictionary` request has ALREADY minted in `region`, if
    /// this store holds it — the dictionary's counterpart of [`Self::feed_created_from`],
    /// over the members a dictionary create sets.
    ///
    /// Every member a create of a dictionary states is a stored one, so the comparison
    /// below plus [`Dictionary::is_as_created`] covers the whole record: a dictionary
    /// `UpdateDictionary` or a tag write has moved on no longer matches the request that
    /// minted it, and one whose deletion has been accepted is passed over as a feed's
    /// is. The status the clock itself moves (`CREATING` to `AVAILABLE`) is
    /// not a change to the record, so a replay is recognised on either side of it.
    pub fn dictionary_created_from<'a>(
        &'a self,
        region: &'a str,
        name: &str,
        language: &str,
        entries: &str,
        tags: &BTreeMap<String, String>,
    ) -> Option<&'a Dictionary> {
        self.dictionaries_in(region).find(|dictionary| {
            dictionary.is_as_created()
                && dictionary.name == name
                && dictionary.language == language
                && dictionary.entries == entries
                && &dictionary.tags == tags
        })
    }

    /// How many distinct Regions this store holds a feed record in, since what this
    /// bounds is how many PARTITIONS the store holds (see [`MAX_REGIONS`]).
    fn feed_region_count(&self) -> usize {
        self.feeds
            .values()
            .map(|feed| feed.region.as_str())
            .collect::<std::collections::BTreeSet<&str>>()
            .len()
    }

    /// How many distinct Regions this store holds a dictionary record in, on the same
    /// rule as [`Self::feed_region_count`].
    fn dictionary_region_count(&self) -> usize {
        self.dictionaries
            .values()
            .map(|dictionary| dictionary.region.as_str())
            .collect::<std::collections::BTreeSet<&str>>()
            .len()
    }

    /// The resource a `resourceArn` names, if this store holds it.
    ///
    /// An ARN identifies a resource WHOLE: the kind of resource (`feed` or
    /// `dictionary`), its id, and the region and account it was minted in. So an ARN
    /// resolves only when it is exactly the ARN the stored record carries, and every
    /// other well-formed value — another kind, another id, another region, another
    /// account — names no resource of this store and so resolves to nothing. That is
    /// what makes the resource "the resource identified by the specified resourceArn in
    /// the current region" (`TagResource`) rather than whichever record happens to share
    /// its id.
    ///
    /// One resolution shared by every operation that reaches a resource by its
    /// `resourceArn`, so none of them can disagree about what an ARN names.
    ///
    /// A record whose deletion has COMPLETED is not a resource this store holds at all:
    /// the ARN of a deleted feed or dictionary resolves here no more than one of a
    /// resource that never existed, so a tagging call about it is refused exactly as
    /// that one is.
    pub fn tagged_resource_mut(&mut self, resource_arn: &str) -> Option<TaggedResourceMut<'_>> {
        // `arn:<partition>:<service>:<region>:<account>:<kind>/<id>` — field 5 is the
        // resource segment, which the modelled `resourceArn` pattern spells
        // `(feed|dictionary)/[a-zA-Z0-9]{1,19}`.
        let (kind, id) = resource_arn.split(':').nth(5)?.split_once('/')?;
        match kind {
            "feed" => self
                .feeds
                .get_mut(id)
                .filter(|feed| feed.arn == resource_arn)
                .map(TaggedResourceMut::Feed),
            "dictionary" => self
                .dictionaries
                .get_mut(id)
                .filter(|dictionary| dictionary.arn == resource_arn)
                .map(TaggedResourceMut::Dictionary),
            _ => None,
        }
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
    ///
    /// Scoped to `region`: an output names a dictionary by its bare `DictionaryId`, and
    /// an id resolves within the Region that holds it, so only the feeds of a
    /// dictionary's OWN Region can reference it.
    pub fn feeds_referencing(&self, region: &str, dictionary_id: &str) -> Vec<String> {
        self.feeds_in(region)
            .filter(|feed| feed.references_dictionary(dictionary_id))
            .map(|feed| feed.id.clone())
            .collect()
    }

    /// The ids of the feeds that are not already retiring and reference
    /// `dictionary_id`, in feed-id order: the records that make "you cannot delete a
    /// dictionary that is referenced by a feed" bite, and so both WHETHER
    /// `DeleteDictionary` is refused and the count its refusal reports
    /// ([`dictionary_referenced_message`]) — one derivation, so the figure a refusal
    /// states and the fact that refused it cannot disagree.
    ///
    /// A feed whose deletion has been ACCEPTED (`DELETING`) is on its way out and so
    /// holds nothing back: the dictionary it names may go as soon as the feed's own
    /// deletion is accepted, which is what lets a caller tear down a feed and then its
    /// dictionary without waiting for the feed's removal to fall due. It is still a
    /// live record until then, so it stays in [`Self::feeds_referencing`].
    pub fn feeds_blocking_delete(&self, region: &str, dictionary_id: &str) -> Vec<String> {
        self.feeds_in(region)
            .filter(|feed| {
                feed.status != FeedStatus::Deleting && feed.references_dictionary(dictionary_id)
            })
            .map(|feed| feed.id.clone())
            .collect()
    }

    /// Materialise every transition the clock already owes at `now`, for feeds and
    /// for dictionaries alike.
    ///
    /// Called at the top of every handler, including the reporting ones: these moves
    /// were due at this instant whether or not anybody called, so applying them on a
    /// read changes nothing a later caller would not have seen anyway.
    ///
    /// A deletion that falls due COMPLETES, and completing it REMOVES the record: the
    /// resource is gone, so from that instant the id names nothing for every operation
    /// of this service, the delete that accepted the deletion included.
    ///
    /// Returns immediately while no move is owed; the scan it then does is over at
    /// most [`MAX_FEEDS`] + [`MAX_DICTIONARIES`] entries.
    pub fn settle_due_transitions(&mut self, now: Instant) {
        if self.earliest_due.is_none_or(|due| now < due) {
            return;
        }
        self.feeds.retain(|_, feed| {
            if feed.transition_due_at.is_none_or(|due| now < due) {
                return true;
            }
            match feed.status {
                // The accepted deletion has now completed, so the feed is no longer a
                // resource this store holds: nothing `GetFeed` reports, `ListFeeds`
                // lists, a tagging call reaches or a repeated `DeleteFeed` resolves.
                FeedStatus::Deleting => return false,
                // A feed whose reconfiguration has completed goes back into the status
                // it was reconfigured from ([`Feed::settles_into`]): the work suspended
                // that status, it did not replace it.
                FeedStatus::Updating => {
                    if let Some(settled) = feed.settles_into.take() {
                        feed.status = settled;
                    }
                    feed.transition_due_at = None;
                }
                // A settled status owes the clock nothing; clear the stale instant.
                FeedStatus::Available | FeedStatus::Active | FeedStatus::Archived => {
                    feed.transition_due_at = None;
                }
            }
            true
        });
        self.dictionaries.retain(|_, dictionary| {
            if dictionary.transition_due_at.is_none_or(|due| now < due) {
                return true;
            }
            match dictionary.status {
                // The accepted deletion has completed, on the same rule the feed's is:
                // the dictionary leaves the store outright.
                DictionaryStatus::Deleting => return false,
                // "After creation succeeds, the status will be AVAILABLE."
                DictionaryStatus::Creating => {
                    dictionary.status = DictionaryStatus::Available;
                    dictionary.transition_due_at = None;
                }
                // A settled status owes the clock nothing; clear the stale instant.
                DictionaryStatus::Available => {
                    dictionary.transition_due_at = None;
                }
            }
            true
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

/// A resource one of this service's tagging operations has reached by its
/// `resourceArn`, borrowed mutably out of the store it lives in
/// ([`ComAmazonawsElementalinferenceElementalinferenceState::tagged_resource_mut`]).
///
/// Tags are a property of the RESOURCE, stored on the one record every other handler
/// reads ([`Feed::tags`], [`Dictionary::tags`]) — there is no second tag store — so a
/// tag written through `TagResource` is the tag `GetFeed`/`GetDictionary` reports. This
/// enum is what lets the tagging operations work on either kind of resource without
/// either of them holding its own copy of which kinds exist or where a kind keeps its
/// tags.
#[derive(Debug)]
pub enum TaggedResourceMut<'a> {
    Feed(&'a mut Feed),
    Dictionary(&'a mut Dictionary),
}

impl TaggedResourceMut<'_> {
    /// Whether this resource's deletion has already been accepted, so it is on its way
    /// out of the store.
    pub fn is_being_deleted(&self) -> bool {
        match self {
            Self::Feed(feed) => feed.status == FeedStatus::Deleting,
            Self::Dictionary(dictionary) => dictionary.status == DictionaryStatus::Deleting,
        }
    }

    /// The sentence a mutation of this resource earns while its deletion is pending —
    /// the same rule, and the same wording, that `UpdateFeed` and `UpdateDictionary`
    /// already refuse with ([`FEED_BEING_DELETED_MESSAGE`],
    /// [`DICTIONARY_BEING_DELETED_MESSAGE`]).
    pub fn being_deleted_message(&self) -> &'static str {
        match self {
            Self::Feed(_) => FEED_BEING_DELETED_MESSAGE,
            Self::Dictionary(_) => DICTIONARY_BEING_DELETED_MESSAGE,
        }
    }

    /// The tags stored on this resource, for a caller that writes them.
    pub fn tags_mut(&mut self) -> &mut BTreeMap<String, String> {
        match self {
            Self::Feed(feed) => &mut feed.tags,
            Self::Dictionary(dictionary) => &mut dictionary.tags,
        }
    }
}

/// The `@length` bounds `TagMap`'s KEY carries: a `TagKey` of length 1..128
/// (`smithy_query(query="shape",shape="TagMap")`).
///
/// A map's key is the one constrained value the framework's boundary walk does not
/// reach — it captures an entry's key as the path segment the entry's VALUE is reported
/// under rather than checking it ("the key itself is not a constrained leaf",
/// `ValidatingSerializer::write_leaf`) — so an operation that takes a `TagMap` enforces
/// the key's own bounds itself, with the modelled `ValidationException` the boundary
/// would have answered with. The value's bounds need no such copy: the boundary already
/// refuses a `TagValue` past its maximum.
pub const TAG_KEY_MIN_LENGTH: usize = 1;

/// The longest `TagKey` the model admits; see [`TAG_KEY_MIN_LENGTH`].
pub const TAG_KEY_MAX_LENGTH: usize = 128;

/// The refusal a tag key outside [`TAG_KEY_MIN_LENGTH`]..[`TAG_KEY_MAX_LENGTH`] earns.
///
/// The rule is about ONE value's own validity, so the message states the constraint the
/// value failed and the length the request's key actually had — built from the request,
/// as a constraint refusal is. The key itself is NOT echoed: its length is what is at
/// fault and a caller controls it, so echoing it would turn an arbitrarily long key into
/// an arbitrarily long message.
pub fn invalid_tag_key_message(length: usize) -> String {
    format!(
        "A tag key must have length greater than or equal to {TAG_KEY_MIN_LENGTH} and less \
         than or equal to {TAG_KEY_MAX_LENGTH}; this request carries a key of length {length}."
    )
}

/// The most tags one resource may carry.
///
/// AWS's tag restrictions, which apply to every taggable resource, cap a resource at
/// 50 tags ("Maximum number of tags per resource: 50", *Tagging AWS resources*), and
/// this service's model states no figure of its own, so that published cap is the
/// bound here. It is also what BOUNDS the tag map on a stored record, which
/// `TagResource` would otherwise grow by one entry per request forever: past it the
/// request is refused with the modelled `ValidationException` (HTTP 400), the answer a
/// request that "fails to satisfy the constraints specified by the service" earns.
///
/// It bounds the tag set a request ASKS a resource to carry, whichever operation
/// carries it: `TagResource` accumulates onto the stored set, while a create's tag map
/// is the whole set the new resource would carry, so each is measured against the same
/// cap ([`invalid_tags_message`]).
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The refusal a `TagResource` whose tags would take a resource past
/// [`MAX_TAGS_PER_RESOURCE`] earns.
///
/// The rule is about ONE value's own validity — how many tags the request asks the
/// resource to carry — so the message names that count and the cap it exceeds, which is
/// what a caller has to know to fix the request. `resulting` is the size of the tag set
/// the request would leave behind, so a request that only re-sets keys the resource
/// already holds is never reported as adding to it.
pub fn too_many_tags_message(resulting: usize) -> String {
    format!(
        "These tags would leave the resource with {resulting} tags, more than the \
         {MAX_TAGS_PER_RESOURCE} tags a resource can have. Remove tags before adding more."
    )
}

/// The tag-key prefix AWS reserves for its own tags.
///
/// AWS's tag restrictions reserve this prefix on every taggable resource: the keys under
/// it name AWS-generated tags, so a user request may not supply one and a request that
/// does is refused rather than stored. The restriction covers editing and DELETING such
/// a tag as well as creating one, so a key in this namespace is refused wherever a
/// request names it — in a supplied tag map ([`invalid_tags_message`]) and in the keys an
/// untag request asks the service to remove alike. The model constrains a `TagKey`'s length and
/// nothing else, so the reserved namespace is the service's own rule to enforce, with
/// the `ValidationException` (HTTP 400) a request that "fails to satisfy the constraints
/// specified by the service" earns.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` lies in the namespace [`RESERVED_TAG_KEY_PREFIX`] reserves.
///
/// The prefix is reserved at the START of a key, and a tag key is case-sensitive, so a
/// key that merely contains the prefix further along, or carries it in another case, is
/// a key of the caller's own namespace and is stored like any other.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.starts_with(RESERVED_TAG_KEY_PREFIX)
}

/// The refusal a tag key in the reserved namespace earns.
///
/// The rule is about ONE value's own validity, so the message is built from the request:
/// it names the key at fault and the prefix that is reserved, which is what a caller has
/// to know to fix the request. Naming the key is bounded because the key's own
/// `@length` is decided FIRST ([`invalid_tags_message`]), so by here it is at most
/// [`TAG_KEY_MAX_LENGTH`] characters.
pub fn reserved_tag_key_message(key: &str) -> String {
    format!(
        "The tag key '{key}' begins with '{RESERVED_TAG_KEY_PREFIX}', a prefix reserved for \
         AWS-generated tags, so it cannot be specified in a request."
    )
}

/// The refusal the `tags` of a request earn, or `None` when the request asks for a tag
/// set this service stores.
///
/// ONE copy of every rule a supplied tag map has to satisfy, so `CreateFeed`,
/// `CreateDictionary` and `TagResource` cannot drift in which tags they accept: a key
/// outside the modelled `TagKey` bounds ([`TAG_KEY_MIN_LENGTH`]), a key in the reserved
/// namespace ([`RESERVED_TAG_KEY_PREFIX`]), and more tags than a resource may carry
/// ([`MAX_TAGS_PER_RESOURCE`]). The request's own validity is decided before any
/// resource is looked up, so a malformed tag map is refused whatever resource the rest
/// of the request names.
///
/// `TagResource` ACCUMULATES onto a stored set, so it checks the resulting size against
/// the cap as well ([`too_many_tags_message`]); the size checked here is the one the
/// request itself asks for, which is the whole set of a resource a create mints.
pub fn invalid_tags_message(tags: &HashMap<String, String>) -> Option<String> {
    // Several keys can be at fault at once and one request earns one answer, so the key
    // reported is chosen in key order rather than by the map's iteration order.
    let mut keys: Vec<&str> = tags.keys().map(String::as_str).collect();
    keys.sort_unstable();

    // The modelled bounds of the key first: `@length` counts characters, not bytes,
    // exactly as the boundary walk does for the values it reaches.
    if let Some(length) = keys
        .iter()
        .map(|key| key.chars().count())
        .find(|length| !(TAG_KEY_MIN_LENGTH..=TAG_KEY_MAX_LENGTH).contains(length))
    {
        return Some(invalid_tag_key_message(length));
    }

    if let Some(key) = keys.iter().find(|key| is_reserved_tag_key(key)) {
        return Some(reserved_tag_key_message(key));
    }

    if tags.len() > MAX_TAGS_PER_RESOURCE {
        return Some(too_many_tags_message(tags.len()));
    }

    None
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

/// The summary-generation mode a contextual-metadata output is configured with when
/// the request carries none.
///
/// `ContextualMetadataConfig$summaryGeneration` documents its own default — "ENABLED
/// (default) - Elemental Inference populates the summary, objects, and actions fields"
/// (`smithy_query(query="shape",shape="ContextualMetadataConfig")`) — so an output
/// that states nothing is configured with summary generation on.
pub const DEFAULT_SUMMARY_GENERATION: &str = "ENABLED";

/// The extended-analysis mode a contextual-metadata output is configured with when the
/// request carries none.
///
/// `ContextualMetadataConfig$extendedAnalysis` documents "ENABLED (default) - Elemental
/// Inference populates the people, environments, brands, and on-screen text fields"
/// (`smithy_query(query="shape",shape="ContextualMetadataConfig")`), and the member is
/// independent of [`DEFAULT_SUMMARY_GENERATION`]'s, so each resolves on its own.
pub const DEFAULT_EXTENDED_ANALYSIS: &str = "ENABLED";

/// The profanity handling a subtitling output is configured with when the request
/// carries none.
///
/// `SubtitlingConfig$profanityFilter` documents "DISABLED (no filtering, default)"
/// among its valid values (`smithy_query(query="shape",shape="SubtitlingConfig")`), so
/// subtitles are generated unfiltered unless the request asks for CENSOR or DROP.
pub const DEFAULT_PROFANITY_FILTER: &str = "DISABLED";

/// An output config as the service STORES it: every optional member the service fills
/// in for itself is materialised, so a read-modify-write round trip through
/// `GetFeed` -> `UpdateFeed` (a full-replacement PUT) carries the service's own
/// configuration back instead of dropping it.
///
/// Only members this service's model declares can be reported, so the defaults are
/// exactly the modelled optional members the service resolves itself: the ones whose
/// documentation names the value the service applies when the request omits them —
/// [`DEFAULT_SUMMARY_GENERATION`], [`DEFAULT_EXTENDED_ANALYSIS`] and
/// [`DEFAULT_PROFANITY_FILTER`] — plus a subtitling output's
/// [`DEFAULT_SUBTITLING_ASPECT_RATIO`]. The cropping and clipping configs declare no
/// member the service resolves
/// (`smithy_query(query="shape",shape="CroppingConfig,ClippingConfig")`), so they are
/// stored as the request carried them. A member the request DID carry is never
/// rewritten: the stored config is the request's, widened with what it left unsaid.
pub fn output_config_with_defaults(mut config: OutputConfig) -> OutputConfig {
    match &mut config {
        OutputConfig::Subtitling(subtitling) => {
            subtitling
                .aspect_ratio
                .get_or_insert(DEFAULT_SUBTITLING_ASPECT_RATIO);
            subtitling
                .profanity_filter
                .get_or_insert_with(|| DEFAULT_PROFANITY_FILTER.to_string());
        }
        OutputConfig::contextualMetadata(contextual_metadata) => {
            contextual_metadata
                .summary_generation
                .get_or_insert_with(|| DEFAULT_SUMMARY_GENERATION.to_string());
            contextual_metadata
                .extended_analysis
                .get_or_insert_with(|| DEFAULT_EXTENDED_ANALYSIS.to_string());
        }
        OutputConfig::Cropping(_) | OutputConfig::Clipping(_) => {}
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

/// How many days either side of the day the catalog is built its schedule covers.
///
/// A search window is at most [`MAX_SEARCH_WINDOW_DAYS`] days wide, so a span of two
/// weeks each way means every window a caller can ask for around the present day holds
/// events, which is what the recorded service answers for a recent window. It also
/// BOUNDS the catalog: the schedule is `2 * CATALOG_SPAN_DAYS + 1` days of
/// [`FIXTURES_PER_SPORT_PER_DAY`] fixtures for each sport the `DataSourceSport` enum
/// declares, and nothing on the wire can add to it.
pub const CATALOG_SPAN_DAYS: i64 = 14;

/// How many fixtures of one sport the schedule holds on one day: a double-header, so a
/// single day's search reports more than one row and paging over a window has something
/// to page.
pub const FIXTURES_PER_SPORT_PER_DAY: usize = 2;

/// The UTC hour the first fixture of a day starts at, and the gap to the next one.
///
/// The data source's own figures; they only have to put every fixture of a day ON that
/// day, since the day is what a search window selects on.
const FIRST_KICK_OFF_HOUR: i64 = 18;
const KICK_OFF_SPACING_HOURS: i64 = 3;

/// Now, in seconds since the Unix epoch — the instant the catalog is built around and
/// the one a held fixture's status is decided against.
pub fn current_epoch_secs() -> i64 {
    ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_secs() as i64)
        .unwrap_or_default()
}

/// The teams of `sport`, as this emulator's data source holds them.
///
/// They are the EMULATOR's own clubs, not any real competition's: the modelled members
/// they fill (`FixtureSummary$name` "as provided by the data source", `Competitor$name`)
/// are free-form strings whose values belong to the data source, and this emulator
/// reaches no sports data source to relay another's. Each sport has its own list, so no
/// fixture pairs a basketball side against a football one, and both lists are long
/// enough that the pairing below never has a team play itself.
fn sport_teams(sport: DataSourceSport) -> &'static [&'static str] {
    match sport {
        DataSourceSport::Basketball => &[
            "Harbor Hawks",
            "Summit Suns",
            "Delta Dynamos",
            "Granite Giants",
            "Cobalt Comets",
            "Prairie Pumas",
            "Ember Eagles",
            "Lakeside Lynx",
        ],
        DataSourceSport::AmericanFootball => &[
            "Ironside Bisons",
            "Copper Cardinals",
            "Northgate Nomads",
            "Sandstone Stallions",
            "Riverbend Rangers",
            "Vantage Vipers",
            "Quarry Quakes",
            "Beacon Bulldogs",
        ],
    }
}

/// The competition `sport`'s fixtures belong to, for the fixtures the data source gives
/// a group for ("The group that the fixture belongs to, such as the competition, league,
/// or tournament", `FixtureSummary$fixtureGroup`).
fn sport_competition(sport: DataSourceSport) -> &'static str {
    match sport {
        DataSourceSport::Basketball => "Continental Basketball League",
        DataSourceSport::AmericanFootball => "Continental Gridiron League",
    }
}

/// The (home, away) pairing the schedule gives slot `slot` of day `day`.
///
/// A rotation over the sport's teams, so the same day always yields the same pairing,
/// two slots of one day are two different fixtures, and the away side is never the home
/// side (the offset is drawn from `1..=len-1`, which cannot be a multiple of the team
/// count).
fn scheduled_pairing(
    teams: &'static [&'static str],
    day: i64,
    slot: usize,
) -> (&'static str, &'static str) {
    let count = teams.len() as i64;
    let index = day * FIXTURES_PER_SPORT_PER_DAY as i64 + slot as i64;
    let home = index.rem_euclid(count);
    let offset = index.div_euclid(count).rem_euclid(count - 1) + 1;
    let away = (home + offset).rem_euclid(count);
    (teams[home as usize], teams[away as usize])
}

/// The `FixtureId` the schedule gives slot `slot` of day `day` for `sport`.
///
/// Opaque, as a data source's id is, and derived from the three figures that identify
/// the fixture (the same hash the data endpoints' label uses), so one fixture keeps one
/// id for the life of the store while no two fixtures share one, and a client cannot
/// read a date or a sport out of an id it was handed.
fn catalog_fixture_id(sport: DataSourceSport, day: i64, slot: usize) -> String {
    let mut seed = fnv1a64(format!("{}|{day}|{slot}", sport.as_wire()).as_bytes());
    format!("{:016x}", splitmix64(&mut seed))
}

/// The schedule the data source holds at `now_secs`: for each sport the
/// `DataSourceSport` enum declares, [`FIXTURES_PER_SPORT_PER_DAY`] fixtures on each UTC
/// day within [`CATALOG_SPAN_DAYS`] of that instant's day.
///
/// This is the catalog a store comes up with (see [`Default`] on the state struct), and
/// the reason a search of a recent window reports events rather than nothing: a client
/// obtains a `fixtureId` only from a search, so a `GetFixture` and a clipping output's
/// `dataSourceConfiguration` have nothing to name until a search can report one. A
/// fixture that has already started carries [`FIXTURE_STATUS_COMPLETED`] and one still
/// ahead [`FIXTURE_STATUS_SCHEDULED`] — the two statuses the model's own documentation
/// names — so the catalog is the snapshot of the schedule as of the instant it was taken.
pub fn default_fixture_catalog(now_secs: i64) -> Vec<Fixture> {
    let center_day = now_secs.div_euclid(SECONDS_PER_DAY);
    let mut catalog = Vec::new();
    for sport in DataSourceSport::ALL.iter().copied() {
        let teams = sport_teams(sport);
        for day in (center_day - CATALOG_SPAN_DAYS)..=(center_day + CATALOG_SPAN_DAYS) {
            for slot in 0..FIXTURES_PER_SPORT_PER_DAY {
                let hour = FIRST_KICK_OFF_HOUR + slot as i64 * KICK_OFF_SPACING_HOURS;
                // A day number the calendar does not have, or a kick-off past the end
                // of the day, is no fixture at all rather than a panic.
                let Some(scheduled_start_secs) = fixture_start_secs(day, hour, 0) else {
                    continue;
                };
                let (home, away) = scheduled_pairing(teams, day, slot);
                catalog.push(Fixture {
                    id: catalog_fixture_id(sport, day, slot),
                    sport,
                    // "For example, the names of the two competing teams"
                    // (`FixtureSummary$name`).
                    name: format!("{away} at {home}"),
                    // "The data source doesn't provide this information for every
                    // fixture": the second fixture of a day carries no group.
                    group: (slot == 0).then(|| sport_competition(sport).to_string()),
                    scheduled_start_secs,
                    status: if scheduled_start_secs <= now_secs {
                        FIXTURE_STATUS_COMPLETED.to_string()
                    } else {
                        FIXTURE_STATUS_SCHEDULED.to_string()
                    },
                    competitors: vec![
                        Competitor {
                            name: Some(home.to_string()),
                            is_home: Some(true),
                        },
                        Competitor {
                            name: Some(away.to_string()),
                            is_home: Some(false),
                        },
                    ],
                });
            }
        }
    }
    catalog
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
