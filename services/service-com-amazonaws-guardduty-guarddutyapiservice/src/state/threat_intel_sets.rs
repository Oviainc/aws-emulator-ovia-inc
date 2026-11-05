// SPDX-License-Identifier: CC0-1.0

//! Threat intel sets: the threat-list a detector holds, its statuses and the
//! spans the clock owes between them.

use super::*;

/// One ThreatIntelSet: a list of "known malicious IP addresses" GuardDuty generates
/// findings from (`smithy_query(query="doc",shape="CreateThreatIntelSet")`).
///
/// The list is owned by the detector it was created under, so `DeleteDetector`
/// drops it with the detector.
#[derive(Debug, Clone)]
pub struct ThreatIntelSetRecord {
    /// The minted `ThreatIntelSetId`, the id a read resolves it by.
    pub id: String,
    /// "A user-friendly ThreatIntelSet name"
    /// (`smithy_query(operation="CreateThreatIntelSet")`).
    pub name: String,
    /// `ThreatIntelSetFormat`: the format of the file that contains the list.
    pub format: String,
    /// "The URI of the file that contains the ThreatIntelSet", stored verbatim: it
    /// is caller text, so a read hands back exactly what the create wrote.
    pub location: String,
    /// The account that owns the bucket the location names, when the create said.
    pub expected_bucket_owner: Option<String>,
    /// "The tags to be added to a new threat list resource".
    pub tags: BTreeMap<String, String>,
    /// The `@idempotencyToken` the create carried, so a replayed create returns the
    /// list the first call minted instead of minting a second one.
    pub client_token: Option<String>,
    pub created_at_millis: i64,
    /// `ThreatIntelSetStatus`, as a read reports it.
    pub status: String,
    /// The status the CLOCK owes this list and the instant it comes due, or `None`
    /// when nothing is outstanding. See [`Self::settle`].
    pub pending_status: Option<(String, i64)>,
    /// The instant a delete was ACCEPTED for this list, or `None` while no delete
    /// has been. It is the key [`Detector::delete_threat_intel_set`] evicts the
    /// retained deleted records by, oldest deletion first, so it records the
    /// acceptance and nothing else moves it — not the clock settling
    /// [`THREAT_INTEL_SET_DELETE_PENDING`] to [`THREAT_INTEL_SET_DELETED`], which
    /// would otherwise reorder two deletions accepted within one span.
    pub deleted_at_millis: Option<i64>,
}

impl ThreatIntelSetRecord {
    /// Materialise the status change the clock has ALREADY made due.
    ///
    /// The change is owed at its due instant whether or not anybody called, so a
    /// reporting operation may apply it: it shows no caller anything it would not
    /// have seen anyway, and it never moves a status the clock does not owe.
    ///
    /// The status it settles to was fixed when the transition started, not when
    /// somebody looked, so two reads of a settled list agree.
    pub fn settle(&mut self, now_millis: i64) {
        let Some((next, due)) = self.pending_status.clone() else {
            return;
        };
        if now_millis >= due {
            self.status = next;
            self.pending_status = None;
        }
    }

    /// Has a delete already been accepted for this threat list — is it
    /// [`THREAT_INTEL_SET_DELETE_PENDING`] or [`THREAT_INTEL_SET_DELETED`]?
    ///
    /// Such a record is no longer a live threat list: it is still REPORTED, so a
    /// read can answer with the status the delete put it in, but GuardDuty is not
    /// using it, so it occupies none of the detector's
    /// [`THREAT_INTEL_SETS_PER_DETECTOR`] slots and no list names it.
    pub fn is_deleting(&self) -> bool {
        self.deleted_at_millis.is_some()
    }

    /// Accept this list's deletion: [`THREAT_INTEL_SET_DELETE_PENDING`] now, with
    /// the clock owing [`THREAT_INTEL_SET_DELETED`] at
    /// [`THREAT_INTEL_SET_DELETION_SPAN_MILLIS`]. Reports whether this call is the
    /// one that accepted it.
    ///
    /// A record already being deleted is left exactly where it is — neither the
    /// status nor the instant the clock owes it moves — so a repeated delete
    /// neither restarts the transition nor walks a `DELETED` record backwards, and
    /// it answers `false` so a caller writes nothing else either. Whatever status
    /// the list was in when the delete arrived, including a transition the clock
    /// had not settled yet, the deletion replaces what was owed: nothing activates
    /// a list on its way out.
    pub fn begin_delete(&mut self, now: i64) -> bool {
        if self.is_deleting() {
            return false;
        }
        self.status = THREAT_INTEL_SET_DELETE_PENDING.to_string();
        self.pending_status = Some((
            THREAT_INTEL_SET_DELETED.to_string(),
            now.saturating_add(THREAT_INTEL_SET_DELETION_SPAN_MILLIS),
        ));
        self.deleted_at_millis = Some(now);
        true
    }

    /// Start the transition `Activate` asks for, from whatever status this list
    /// currently has: [`THREAT_INTEL_SET_ACTIVATING`] then
    /// [`THREAT_INTEL_SET_ACTIVE`] when it asks for the list to be used,
    /// [`THREAT_INTEL_SET_DEACTIVATING`] then [`THREAT_INTEL_SET_INACTIVE`] when it
    /// asks for it to stop.
    ///
    /// Asking for the status the list has already settled on, or is already
    /// transitioning to, owes nothing, so a repeated request leaves the status and
    /// the instant the clock owes it exactly where they are rather than re-opening
    /// a transition. Asking for the other one replaces whatever was owed, so a
    /// request that reverses a transition still running turns it around instead of
    /// queueing behind it.
    ///
    /// The transition is never immediate: the clock owes the settled status
    /// [`THREAT_INTEL_SET_ACTIVATION_SPAN_MILLIS`] later, and only
    /// [`Self::settle`] materialises it.
    pub fn set_activation(&mut self, activate: bool, now: i64) {
        if !self.activation_changes(activate) {
            return;
        }
        let (settled, transitional) = if activate {
            (THREAT_INTEL_SET_ACTIVE, THREAT_INTEL_SET_ACTIVATING)
        } else {
            (THREAT_INTEL_SET_INACTIVE, THREAT_INTEL_SET_DEACTIVATING)
        };
        self.status = transitional.to_string();
        self.pending_status = Some((
            settled.to_string(),
            now.saturating_add(THREAT_INTEL_SET_ACTIVATION_SPAN_MILLIS),
        ));
    }

    /// Would [`Self::set_activation`] move anything — does `activate` ask for a
    /// status this list has neither settled on nor already started towards?
    ///
    /// Answered WITHOUT writing, so a caller can decide whether its request
    /// changes this record at all BEFORE it reads the store's clock. The answer
    /// does not depend on whether [`Self::settle`] has run yet: settling only ever
    /// moves a status to the other member of the same pair.
    pub fn activation_changes(&self, activate: bool) -> bool {
        let (settled, transitional) = if activate {
            (THREAT_INTEL_SET_ACTIVE, THREAT_INTEL_SET_ACTIVATING)
        } else {
            (THREAT_INTEL_SET_INACTIVE, THREAT_INTEL_SET_DEACTIVATING)
        };
        self.status != settled && self.status != transitional
    }
}

/// How many ThreatIntelSets one detector may hold.
///
/// AWS publishes a quota of 6 threat lists per account per Region for GuardDuty,
/// and a detector is the account's single resource in a Region ("you can have only
/// one detector per account per Region" —
/// `smithy_query(query="doc",shape="CreateDetector")`), so the account's quota is
/// the detector's.
///
/// The quota is also what BOUNDS this store: without it a caller varying or
/// omitting `ClientToken` would grow [`Detector::threat_intel_sets`] by one entry
/// per request, and the `@idempotencyToken` scan over it
/// ([`Detector::threat_intel_set_for_token`]) would grow with it.
pub const THREAT_INTEL_SETS_PER_DETECTOR: usize = 6;

/// The message a `CreateThreatIntelSet` past [`THREAT_INTEL_SETS_PER_DETECTOR`] is
/// refused with. It names the limit, never the caller's values.
pub fn threat_intel_set_limit_message() -> String {
    format!(
        "The request is rejected because the account has reached the limit of {THREAT_INTEL_SETS_PER_DETECTOR} threat lists."
    )
}

/// `ThreatIntelSetStatus`: the status a list that is not in use reports
/// (`smithy_query(shape="ThreatIntelSetStatus")`).
pub const THREAT_INTEL_SET_INACTIVE: &str = "INACTIVE";
/// The TRANSIENT status a list the create asked to activate reports until the
/// activation that create started comes due. See [`ThreatIntelSetRecord::settle`].
pub const THREAT_INTEL_SET_ACTIVATING: &str = "ACTIVATING";
/// The status an activated list reports once its activation is due.
pub const THREAT_INTEL_SET_ACTIVE: &str = "ACTIVE";
/// The TRANSIENT status a list asked to stop being used reports until that change
/// comes due — the counterpart of [`THREAT_INTEL_SET_ACTIVATING`]
/// (`smithy_query(shape="ThreatIntelSetStatus")` carries both). See
/// [`ThreatIntelSetRecord::set_activation`].
pub const THREAT_INTEL_SET_DEACTIVATING: &str = "DEACTIVATING";

/// How long a list asked to activate spends in [`THREAT_INTEL_SET_ACTIVATING`]
/// before the clock owes it [`THREAT_INTEL_SET_ACTIVE`], and — the change being
/// the same one in reverse — how long a list asked to stop being used spends in
/// [`THREAT_INTEL_SET_DEACTIVATING`] before the clock owes it
/// [`THREAT_INTEL_SET_INACTIVE`].
///
/// AWS documents no duration for ingesting and activating a threat list, so this
/// span is CHOSEN: non-zero, so a caller reading straight after the create still
/// observes the transient status the real service reports, and short enough that
/// the activation lands without anybody sleeping on it.
pub const THREAT_INTEL_SET_ACTIVATION_SPAN_MILLIS: i64 = 500;

/// `ThreatIntelSetStatus`: a delete has been accepted and the list is on its way
/// out of use. TRANSIENT — the clock owes it [`THREAT_INTEL_SET_DELETED`]
/// (`smithy_query(shape="ThreatIntelSetStatus")` carries both).
pub const THREAT_INTEL_SET_DELETE_PENDING: &str = "DELETE_PENDING";

/// `ThreatIntelSetStatus`: the list has been deleted. Terminal — nothing moves a
/// deleted list anywhere else.
pub const THREAT_INTEL_SET_DELETED: &str = "DELETED";

/// How long a list a delete has accepted spends in
/// [`THREAT_INTEL_SET_DELETE_PENDING`] before the clock owes it
/// [`THREAT_INTEL_SET_DELETED`].
///
/// AWS documents no duration for removing a threat list, so this span is CHOSEN,
/// matching the activation span this record already uses: non-zero, so a caller
/// reading straight after the delete still observes the transient
/// `DELETE_PENDING` the real service reports, and short enough that the deletion
/// lands without anybody sleeping on it.
pub const THREAT_INTEL_SET_DELETION_SPAN_MILLIS: i64 = THREAT_INTEL_SET_ACTIVATION_SPAN_MILLIS;

/// How many DELETED threat-list records one detector retains, so a read can still
/// report the [`THREAT_INTEL_SET_DELETE_PENDING`] / [`THREAT_INTEL_SET_DELETED`]
/// status a delete moved a list to.
///
/// Retention needs its own ceiling because [`THREAT_INTEL_SETS_PER_DETECTOR`]
/// bounds only the LIVE lists: a caller creating and deleting in a loop would
/// otherwise make the detector keep a record per request. The ceiling is the live
/// quota, the way [`DELETED_IP_SETS_RETAINED`] is the IPSet one.
pub const DELETED_THREAT_INTEL_SETS_RETAINED: usize = THREAT_INTEL_SETS_PER_DETECTOR;
