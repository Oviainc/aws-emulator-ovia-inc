// SPDX-License-Identifier: CC0-1.0

//! IP sets: the trusted-IP list a detector holds, its statuses and the spans the
//! clock owes between them.

use super::*;

/// `IpSetStatus`: the list is being activated, so GuardDuty is not using it yet.
/// One of `INACTIVE | ACTIVATING | ACTIVE | DEACTIVATING | ERROR | DELETE_PENDING
/// | DELETED` (`smithy_query(shape="IpSetStatus")`).
pub const IP_SET_ACTIVATING: &str = "ACTIVATING";

/// `IpSetStatus`: the list is uploaded and in use. See [`IP_SET_ACTIVATING`] for
/// the enum.
pub const IP_SET_ACTIVE: &str = "ACTIVE";

/// `IpSetStatus`: the list is being taken out of use.
pub const IP_SET_DEACTIVATING: &str = "DEACTIVATING";

/// `IpSetStatus`: the list is stored but GuardDuty is not using it.
pub const IP_SET_INACTIVE: &str = "INACTIVE";

/// `IpSetStatus`: a delete has been accepted and the list is on its way out of
/// use. See [`IP_SET_ACTIVATING`] for the enum.
pub const IP_SET_DELETE_PENDING: &str = "DELETE_PENDING";

/// `IpSetStatus`: the list has been deleted. Terminal — nothing moves a deleted
/// IPSet to any other status.
pub const IP_SET_DELETED: &str = "DELETED";

/// How long an IPSet stays in a transitional [`IP_SET_ACTIVATING`] /
/// [`IP_SET_DEACTIVATING`] / [`IP_SET_DELETE_PENDING`] status before the CLOCK
/// owes it the settled status.
///
/// GuardDuty documents no duration for activating or deleting a trusted IP list,
/// so this emulator CHOOSES one. It is deliberately non-zero: a caller that reads
/// the IPSet back immediately after creating or deleting it still observes the
/// transient status.
pub const IP_SET_TRANSITION_MILLIS: i64 = 500;

/// How many DELETED IPSets one detector retains so a read can still report
/// [`IP_SET_DELETED`].
///
/// A deleted record is kept because `GetIPSet`'s `Status` is `@required` and the
/// enum carries `DELETE_PENDING` and `DELETED`
/// (`smithy_query(operation="GetIPSet")`), so the status after a delete is
/// observable rather than a 400. The retention is bounded for the same reason
/// [`IP_SETS_PER_DETECTOR`] bounds the live ones: a caller creating and deleting
/// in a loop must not be able to make the service retain records without limit.
pub const DELETED_IP_SETS_RETAINED: usize = IP_SETS_PER_DETECTOR;

/// How many LIVE IPSets one detector may hold ([`Detector::live_ip_set_count`]).
///
/// `ListIPSets` reports a detector's IPSets in a single page: `IpSetIds` is
/// `len 0..50` and `MaxResults` is `range 1..50`
/// (`smithy_query(operation="ListIPSets")`), so a detector's whole collection
/// fits in one page of at most 50 ids. The same number BOUNDS the store: an IPSet
/// is retained until its detector is deleted and has no expiry of its own, so
/// without this maximum a caller could make the service retain records without
/// limit.
pub const IP_SETS_PER_DETECTOR: usize = 50;

/// The message a create past [`IP_SETS_PER_DETECTOR`] is refused with. Like every
/// other rejection here it names the resource class and the limit, never the
/// caller's values.
pub fn ip_set_limit_message() -> String {
    format!(
        "The request is rejected because the detector has reached the limit of {IP_SETS_PER_DETECTOR} IPSets."
    )
}

/// How many trusted IP lists an account may have GuardDuty USE in one Region.
///
/// AWS publishes an allowance of one trusted IP list per account per Region. The
/// number of lists a detector may HOLD is the larger [`IP_SETS_PER_DETECTOR`]:
/// uploading a further list unused is accepted, and what the allowance governs is
/// putting a list INTO USE — so a request that asks GuardDuty to start using a
/// list is refused once the account's lists have taken the allowance up, while the
/// same request that does not ask for it is served
/// ([`Detector::live_ip_set_count_excluding`]).
///
/// A detector is the account's one resource in a Region ("you can have only one
/// detector per account per Region" —
/// `smithy_query(query="doc",shape="CreateDetector")`), so the account's allowance
/// in a Region is the detector's.
///
/// The threat-list family needs no allowance of its own: AWS publishes six threat
/// lists per account per Region and that is already the ceiling on the lists a
/// detector HOLDS ([`THREAT_INTEL_SETS_PER_DETECTOR`]), so no sequence of creates
/// or updates can put more than the allowance into use.
pub const ACTIVATED_IP_SETS_PER_ACCOUNT: usize = 1;

/// The message GuardDuty refuses a request that would take the account past one
/// of its resource allowances with.
///
/// A server-side allowance rather than a malformed request: the class is the
/// modelled `InternalServerErrorException` (`httpError` 500,
/// `smithy_query(shape="InternalServerErrorException")`), so a client reading the
/// class alone still tells it apart from the 400 a bad parameter gets. The
/// sentence names neither the resource nor the caller's values.
pub const ACCOUNT_LIMIT_MESSAGE: &str =
    "The request is rejected because it\u{2019}s an attempt to create resources beyond the current AWS account limits.";

/// The modelled server-side failure a request past an account allowance is
/// refused with, carrying [`ACCOUNT_LIMIT_MESSAGE`].
///
/// `Type` is "The error type" (`smithy_query(shape="InternalServerErrorException")`)
/// — the error class, the same value the body's `__type` discriminator carries,
/// which is how [`state_unavailable`] fills it too.
pub fn account_limit_exceeded() -> crate::ops::types::shared_types::InternalServerErrorException {
    crate::ops::types::shared_types::InternalServerErrorException {
        message: Some(ACCOUNT_LIMIT_MESSAGE.to_string()),
        r#type: Some("InternalServerErrorException".to_string()),
    }
}

/// The wire name of the `IpSetId` member, as the request carries it
/// (`smithy_query(operation="GetIPSet")` binds it as the URI label `IpSetId`,
/// rendered `ipSetId` in the JSON body of the sibling operations). The family's own
/// absence refusal names no member ([`SET_ID_NOT_FOUND_MESSAGE`]), so this is the
/// wire name itself and not a rejection's subject.
pub const IP_SET_ID_MEMBER: &str = "ipSetId";

/// The message the uploaded-list families — an IPSet and a threat list, read,
/// updated or deleted — refuse an id the detector does not hold with:
/// [`NO_SUCH_RESOURCE_MESSAGE`], one fixed sentence that names neither the member
/// nor the value, and the same one whatever the FORM of the id, since a list that
/// was never minted, one minted under another detector and a value that could never
/// have been minted at all are the same absence.
pub const SET_ID_NOT_FOUND_MESSAGE: &str = NO_SUCH_RESOURCE_MESSAGE;

/// The message GuardDuty refuses a change to an uploaded list whose own delete is
/// already running with.
///
/// One fixed sentence that names neither the operation, the member nor the
/// caller's value: what it reports is that the request collides with work already
/// in flight on that list, which is why it is answered under the service's server
/// error class ([`crate::scaffolded::Scaffolded::in_progress`]) rather than as the
/// absence an id the detector never held gets ([`SET_ID_NOT_FOUND_MESSAGE`]).
pub const OPERATION_IN_PROGRESS_MESSAGE: &str =
    "The request is rejected. This operation is already in progress.";

/// The wire name of the `Name` member an IPSet create or update carries: both
/// bind it in the JSON body under `jsonName` `name`
/// (`smithy_query(operation="CreateIPSet")`). It is what a rejection names,
/// AS THE REQUEST CARRIES IT.
pub const IP_SET_NAME_MEMBER: &str = "name";

/// The name GuardDuty gives the IPSet `Location` member in a rejection of it.
///
/// It is the ONE member name where the service does not use the request's own
/// spelling: the create and the update bind the member in the JSON body under
/// `jsonName` `location` (`smithy_query(operation="CreateIPSet")`), while the
/// service names the parameter it refuses by its resource-qualified name — the
/// same convention its other IPSet-qualified member names follow
/// ([`IP_SET_ID_MEMBER`]).
pub const IP_SET_LOCATION_MEMBER: &str = "ipSetLocation";

/// Does `name` hold only the characters an IPSet `Name` may contain?
///
/// "Allowed characters are alphanumeric, whitespace, dash (-), and underscores
/// (_)" (`smithy_query(query="doc",shape="CreateIPSetRequest",member="Name")`)
/// — a documented constraint the model's `len 1..300` does not express, so it is
/// enforced here. The whitespace a user-friendly name carries is the space: an
/// ASCII control character (a tab, a newline) is not a character such a name
/// displays.
pub fn is_ip_set_name(name: &str) -> bool {
    is_list_name(name, true)
}

/// Does `name` hold only characters from the alphabet an uploaded list's `Name`
/// is documented to allow — ASCII alphanumerics, dash (-) and underscore (_),
/// plus the space when `space_allowed`?
///
/// The list families document the same alphabet and differ only on whether the
/// space is in it, so the alphabet lives in ONE place and each family's
/// predicate ([`is_ip_set_name`], [`is_entity_set_name`]) says which of the two
/// it is. An empty name is in neither, though the modelled `len 1..300` already
/// refuses it before any handler runs.
pub(super) fn is_list_name(name: &str, space_allowed: bool) -> bool {
    !name.is_empty()
        && name.bytes().all(|b| {
            b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || (space_allowed && b == b' ')
        })
}

/// One stored IPSet (trusted IP list). Timestamps are epoch milliseconds, the
/// single source for the instants anything reports.
#[derive(Debug, Clone)]
pub struct IpSetRecord {
    /// The minted `IpSetId`.
    pub id: String,
    /// `Name`: "The user-friendly name for the IPSet"
    /// (`smithy_query(operation="GetIPSet")`), stored verbatim.
    pub name: String,
    /// `IpSetFormat`: the format of the file that contains the IPSet.
    pub format: String,
    /// `Location`: the URI of the file that contains the IPSet.
    pub location: String,
    /// `ExpectedBucketOwner`, which a read reports "only if it was provided
    /// during IPSet creation or update" (`smithy_query(operation="GetIPSet")`).
    pub expected_bucket_owner: Option<String>,
    /// The tags of the IPSet resource.
    pub tags: BTreeMap<String, String>,
    /// `IpSetStatus`, the status of the uploaded file.
    pub status: String,
    /// When the clock owes [`Self::status`] its settled value, while the current
    /// one is transitional. Absent once nothing is owed.
    pub status_due_millis: Option<i64>,
    /// The `ClientToken` the create carried, so a replay returns this same IPSet
    /// instead of minting a second one.
    pub client_token: Option<String>,
    pub created_at_millis: i64,
    pub updated_at_millis: i64,
}

impl IpSetRecord {
    /// An IPSet created at `now`.
    ///
    /// `Activate` is "a Boolean value that indicates whether GuardDuty is to start
    /// using the uploaded IPSet" (`smithy_query(operation="CreateIPSet")`): a list
    /// the caller asked to be used starts in the transitional
    /// [`IP_SET_ACTIVATING`] and the clock owes it [`IP_SET_ACTIVE`], while one the
    /// caller did not ask to be used is stored unused — [`IP_SET_INACTIVE`], which
    /// is settled, so nothing is owed on it.
    #[allow(clippy::too_many_arguments)]
    pub fn created(
        id: String,
        name: String,
        format: String,
        location: String,
        expected_bucket_owner: Option<String>,
        tags: BTreeMap<String, String>,
        activate: bool,
        client_token: Option<String>,
        now: i64,
    ) -> Self {
        let mut record = Self {
            id,
            name,
            format,
            location,
            expected_bucket_owner,
            tags,
            status: IP_SET_INACTIVE.to_string(),
            status_due_millis: None,
            client_token,
            created_at_millis: now,
            updated_at_millis: now,
        };
        record.set_activation(activate, now);
        record
    }

    /// Has a delete already been accepted for this IPSet — is it
    /// [`IP_SET_DELETE_PENDING`] or [`IP_SET_DELETED`]?
    ///
    /// Such a record is no longer a live trusted IP list: it is still REPORTED, so
    /// a read can answer with the status the delete put it in, but it occupies none
    /// of the detector's [`IP_SETS_PER_DETECTOR`] slots and no update may move it.
    pub fn is_deleting(&self) -> bool {
        self.status == IP_SET_DELETE_PENDING || self.status == IP_SET_DELETED
    }

    /// Accept this IPSet's deletion: [`IP_SET_DELETE_PENDING`] now, with the clock
    /// owing [`IP_SET_DELETED`] at [`IP_SET_TRANSITION_MILLIS`]. Reports whether
    /// this call is the one that accepted it.
    ///
    /// A record already being deleted is left exactly where it is — neither the
    /// status nor the instant the clock owes it moves — so a repeated delete
    /// neither restarts the transition nor walks a `DELETED` record backwards, and
    /// it answers `false` so a caller writes nothing else either.
    pub fn begin_delete(&mut self, now: i64) -> bool {
        if self.is_deleting() {
            return false;
        }
        self.status = IP_SET_DELETE_PENDING.to_string();
        self.status_due_millis = Some(now.saturating_add(IP_SET_TRANSITION_MILLIS));
        self.updated_at_millis = now;
        true
    }

    /// Would [`Self::set_activation`] move anything — does `activate` ask for a
    /// status this IPSet has neither settled on nor already started towards?
    ///
    /// Answered WITHOUT writing, so a caller can decide whether its request
    /// changes this record at all BEFORE it reads the store's clock. The answer
    /// does not depend on whether [`Self::settle`] has run yet: settling only ever
    /// moves a status to the other member of the same pair.
    pub fn activation_changes(&self, activate: bool) -> bool {
        let (settled, transitional) = if activate {
            (IP_SET_ACTIVE, IP_SET_ACTIVATING)
        } else {
            (IP_SET_INACTIVE, IP_SET_DEACTIVATING)
        };
        self.status != settled && self.status != transitional
    }

    /// Start the transition the caller's `Activate` asks for, from whatever status
    /// this IPSet currently has.
    ///
    /// Asking for the status it already has owes nothing, so a repeated request
    /// leaves the settled status alone instead of re-opening a transition — the
    /// same question [`Self::activation_changes`] answers, asked through it so the
    /// two cannot drift.
    pub fn set_activation(&mut self, activate: bool, now: i64) {
        if !self.activation_changes(activate) {
            return;
        }
        let transitional = if activate {
            IP_SET_ACTIVATING
        } else {
            IP_SET_DEACTIVATING
        };
        self.status = transitional.to_string();
        self.status_due_millis = Some(now.saturating_add(IP_SET_TRANSITION_MILLIS));
        self.updated_at_millis = now;
    }

    /// Apply the status change the CLOCK already made due, if any.
    ///
    /// The move out of a transitional status was owed at
    /// [`Self::status_due_millis`] whether or not anybody called, which is why a
    /// reporting operation may apply it. It is applied exactly once: a settled
    /// status never moves on its own again.
    pub fn settle(&mut self, now: i64) {
        let Some(due) = self.status_due_millis else {
            return;
        };
        if now < due {
            return;
        }
        self.status = match self.status.as_str() {
            IP_SET_ACTIVATING => IP_SET_ACTIVE.to_string(),
            IP_SET_DEACTIVATING => IP_SET_INACTIVE.to_string(),
            IP_SET_DELETE_PENDING => IP_SET_DELETED.to_string(),
            // Nothing else is transitional, so nothing else is owed a move.
            other => other.to_string(),
        };
        self.status_due_millis = None;
    }
}
