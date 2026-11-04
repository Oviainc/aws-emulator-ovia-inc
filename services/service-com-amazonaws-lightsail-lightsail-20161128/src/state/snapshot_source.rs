// SPDX-License-Identifier: CC0-1.0

//! The one rule every operation of this service that can address an AUTOMATIC
//! snapshot answers its source members with, in a module of its own.
//! Re-exported from [`super`], so every caller reaches it as `crate::state::…`
//! exactly like the rest of that module.
//!
//! Three operations take such a source — `CopySnapshot`,
//! `CreateDiskFromSnapshot` and `CreateInstancesFromSnapshot` — and the modelled
//! documentation states the same three constraints on all three inputs: the two
//! sources (a MANUAL snapshot named directly, or the instance or disk an
//! automatic snapshot was taken of) "are mutually exclusive"; `restoreDate` and
//! `useLatestRestorableAutoSnapshot`, the two members that say WHICH automatic
//! snapshot, "are mutually exclusive"; and both of those are to be defined "only
//! when" the request names the source of an automatic snapshot, which the second
//! source member is what names. A restore date also "Must be specified in
//! `YYYY-MM-DD` format".
//!
//! Those constraints are one rule, so they have one owner here rather than a
//! copy per door: a copy per door is what lets two doors answer one request
//! shape with different refusals. Only the member names and the operation's own
//! verb differ between the doors, and those are what
//! [`SnapshotSourceMembers`] carries.

/// Which snapshot a request addressed, once its combination of source members is
/// known to address exactly one.
#[derive(Debug, Clone)]
pub enum RequestedSnapshotSource {
    /// The MANUAL snapshot the request's snapshot-name member named.
    Snapshot(String),
    /// An AUTOMATIC snapshot of the instance or disk the source member named,
    /// addressed by the `date` the request carried — `None` when it asked for
    /// the latest restorable copy instead of naming a date.
    AutoSnapshot {
        resource_name: String,
        date: Option<String>,
    },
}

/// What one door's wording of [`requested_snapshot_source`]'s refusals needs
/// from that door: the names of the two source members the CALLER sent, and how
/// this operation's own documentation words what it does with the snapshot.
///
/// Every refusal below names the member that is wrong, so the member paths are a
/// parameter of the rule rather than a sentence one caller rewrites after the
/// fact: [`COPY_SNAPSHOT_SOURCE_MEMBERS`], [`DISK_RESTORE_SOURCE_MEMBERS`] and
/// [`INSTANCE_RESTORE_SOURCE_MEMBERS`] are the three the model gives.
#[derive(Debug, Clone, Copy)]
pub struct SnapshotSourceMembers {
    /// The member naming the MANUAL snapshot.
    pub snapshot_member: &'static str,
    /// The member naming the instance or disk an AUTOMATIC snapshot was taken
    /// of.
    pub source_member: &'static str,
    /// The clause the modelled documentation of the automatic-snapshot members
    /// states their applicability with ("Define this parameter only when …").
    pub only_when: &'static str,
    /// What this operation does with the snapshot it is given, as a bare verb.
    pub verb: &'static str,
    /// How a refusal describes the manual snapshot [`Self::snapshot_member`]
    /// names.
    pub snapshot_description: &'static str,
    /// How a refusal describes the resource [`Self::source_member`] names.
    pub source_description: &'static str,
    /// What naming one source or the other does, for the refusal a request
    /// naming neither earns.
    pub either_source: &'static str,
}

/// The two sources `CopySnapshot` copies, as its own documentation words them.
pub const COPY_SNAPSHOT_SOURCE_MEMBERS: SnapshotSourceMembers = SnapshotSourceMembers {
    snapshot_member: "sourceSnapshotName",
    source_member: "sourceResourceName",
    only_when: "copying an automatic snapshot as a manual snapshot",
    verb: "copy",
    snapshot_description: "the manual snapshot",
    source_description: "the source instance or disk",
    either_source: "a manual snapshot is copied as another manual snapshot, or an automatic \
                    snapshot of an instance or disk is copied as a manual one",
};

/// The two sources `CreateDiskFromSnapshot` restores, as its own documentation
/// words them.
pub const DISK_RESTORE_SOURCE_MEMBERS: SnapshotSourceMembers = SnapshotSourceMembers {
    snapshot_member: "diskSnapshotName",
    source_member: "sourceDiskName",
    only_when: "creating a new disk from an automatic snapshot",
    verb: "restore",
    snapshot_description: "the disk snapshot",
    source_description: "the source disk",
    either_source: "a disk is created from a manual snapshot of a disk, or from an automatic \
                    snapshot of one",
};

/// The two sources `CreateInstancesFromSnapshot` restores, as its own
/// documentation words them.
pub const INSTANCE_RESTORE_SOURCE_MEMBERS: SnapshotSourceMembers = SnapshotSourceMembers {
    snapshot_member: "instanceSnapshotName",
    source_member: "sourceInstanceName",
    only_when: "creating a new instance from an automatic snapshot",
    verb: "restore",
    snapshot_description: "the instance snapshot",
    source_description: "the source instance",
    either_source: "an instance is created from a manual snapshot of an instance, or from an \
                    automatic snapshot of one",
};

/// How a refusal names the automatic snapshot a request addressed and this
/// account holds no copy for: the resource it would have been taken of, and the
/// date it was asked for by — or, when the request asked for the latest
/// restorable copy instead of naming a date, that the resource has none.
///
/// One sentence per door would be one wording per door for one rule, so the two
/// operations that resolve such a copy build it here, each in its own verb
/// ([`SnapshotSourceMembers::verb`]).
pub fn no_such_auto_snapshot(
    members: SnapshotSourceMembers,
    resource_type: &str,
    resource_name: &str,
    date: Option<&str>,
) -> String {
    let verb = members.verb;
    match date {
        Some(date) => format!(
            "The automatic snapshot of the {resource_type} {resource_name} does not exist: {date}"
        ),
        None => format!(
            "The {resource_type} {resource_name} has no restorable automatic snapshot to {verb}"
        ),
    }
}

/// The snapshot the request's source members address, or the refusal their
/// combination earns.
///
/// WHICH snapshot the members address is settled before the FORM of any value
/// they carry: applicability and mutual exclusion say whether a member belongs
/// in the request at all, so a request carrying an automatic-snapshot member
/// with a manual source — or carrying both automatic-snapshot members at once —
/// is told that, rather than being refused for the form of a date it should not
/// have sent, or should not have sent alongside the other member, at all. The
/// date's form is then read in the one branch where the date is a member of the
/// request the service would act on.
pub fn requested_snapshot_source(
    members: SnapshotSourceMembers,
    snapshot_name: Option<&str>,
    source_name: Option<&str>,
    restore_date: Option<&str>,
    use_latest_restorable_auto_snapshot: Option<bool>,
) -> Result<RequestedSnapshotSource, String> {
    let SnapshotSourceMembers {
        snapshot_member,
        source_member,
        only_when,
        verb,
        snapshot_description,
        source_description,
        either_source,
    } = members;
    let wants_latest = use_latest_restorable_auto_snapshot == Some(true);
    match (snapshot_name, source_name) {
        (Some(snapshot_name), None) => {
            if restore_date.is_some() || wants_latest {
                return Err(format!(
                    "restoreDate and useLatestRestorableAutoSnapshot are to be defined only \
                     when {only_when}, which {source_member} names the source of, not together \
                     with {snapshot_member}"
                ));
            }
            Ok(RequestedSnapshotSource::Snapshot(snapshot_name.to_string()))
        }
        (None, Some(source_name)) => match (restore_date, wants_latest) {
            (Some(restore_date), false) => {
                if !super::is_auto_snapshot_date(restore_date) {
                    return Err(format!(
                        "restoreDate '{restore_date}' must be specified in YYYY-MM-DD format"
                    ));
                }
                Ok(RequestedSnapshotSource::AutoSnapshot {
                    resource_name: source_name.to_string(),
                    date: Some(restore_date.to_string()),
                })
            }
            (None, true) => Ok(RequestedSnapshotSource::AutoSnapshot {
                resource_name: source_name.to_string(),
                date: None,
            }),
            (Some(_), true) => Err(format!(
                "restoreDate and useLatestRestorableAutoSnapshot are mutually exclusive: name \
                 the date of the automatic snapshot to {verb}, or ask for the latest restorable \
                 one, but not both"
            )),
            (None, false) => Err(format!(
                "{source_member} names {source_description} of an automatic snapshot, so one of \
                 restoreDate or useLatestRestorableAutoSnapshot must be defined with it"
            )),
        },
        (Some(_), Some(_)) => Err(format!(
            "{snapshot_member} and {source_member} are mutually exclusive: name \
             {snapshot_description} to {verb}, or {source_description} of the automatic snapshot \
             to {verb}, but not both"
        )),
        (None, None) => Err(format!(
            "one of {snapshot_member} or {source_member} must be defined: {either_source}"
        )),
    }
}
