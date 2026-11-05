// SPDX-License-Identifier: CC0-1.0

//! The states a stored resource moves through and what the clock owes it: the
//! lifecycle of an instance, a disk, a snapshot and a managed database, and the
//! instant each owed transition falls due at.

use super::*;

/// One `InstanceState`: the numeric code and the name Lightsail reports together.
///
/// A Lightsail instance is an EC2 instance, and `code` carries EC2's
/// `InstanceState.code` values verbatim — 0 `pending`, 16 `running`, 64
/// `stopping`, 80 `stopped` — which is also the pairing the modelled
/// `InstanceState` documentation shows (`code` 16 beside the name `running`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lifecycle {
    pub code: i32,
    pub name: &'static str,
}

/// An instance that has been created or started but is not serving yet.
pub const PENDING: Lifecycle = Lifecycle {
    code: 0,
    name: "pending",
};
/// A serving instance.
pub const RUNNING: Lifecycle = Lifecycle {
    code: 16,
    name: "running",
};
/// An instance on its way down.
pub const STOPPING: Lifecycle = Lifecycle {
    code: 64,
    name: "stopping",
};
/// A stopped instance.
pub const STOPPED: Lifecycle = Lifecycle {
    code: 80,
    name: "stopped",
};

impl Lifecycle {
    /// Whether an instance in this state is one of the two an operation
    /// documented for "a running or stopped instance" acts on.
    ///
    /// The crate's ONE owner of that pair, asked by every operation whose own
    /// documented scope is those two states, so a settled instance and one the
    /// clock is still moving ([`PENDING`], [`STOPPING`]) are told apart in a
    /// single place.
    pub fn is_running_or_stopped(self) -> bool {
        self == RUNNING || self == STOPPED
    }
}

/// How long after the call that starts it the clock owes a resource its next
/// settled state: an instance's (`pending` -> `running`, `stopping` ->
/// `stopped`), a block storage disk's (`pending` -> `available`, and the
/// `in-use` -> `available` a `DetachDisk` starts) and a container
/// service's (`PENDING` -> `READY`, and each step of `PENDING` -> `DEPLOYING` ->
/// `RUNNING`).
///
/// CHOSEN, not an AWS duration: AWS takes tens of seconds to minutes, which no
/// caller can be made to wait for here. The span is deliberately NON-ZERO, so a
/// caller that reads straight after `CreateInstances`, `StartInstance`,
/// `StopInstance`, `CreateDisk`, `DetachDisk` or `CreateContainerService` still
/// observes the transient state exactly as it does against AWS.
pub const STATE_TRANSITION_MILLIS: i64 = 500;

/// How long after the `AutoSnapshot` add-on is enabled on a resource this store
/// takes the FIRST automatic snapshot of it.
///
/// CHOSEN, not an AWS duration: the add-on takes a copy once a day at the hour
/// it carries, so against AWS the first copy lands up to a day after the call
/// that enabled the add-on — a wait no caller can be made to serve here, and one
/// that would leave the copies the add-on exists to take unobservable. The span
/// is deliberately NON-ZERO, so a caller that reads straight after enabling the
/// add-on still sees a resource no copy has been taken of yet. Every later copy
/// follows the add-on's own daily hour ([`next_auto_snapshot_due_ms`]).
pub const AUTO_SNAPSHOT_FIRST_MILLIS: i64 = STATE_TRANSITION_MILLIS;

/// A state change the clock already owes an instance.
#[derive(Debug, Clone, Copy)]
pub struct DueTransition {
    /// Epoch milliseconds at which this change STARTED being owed: the instant
    /// of the call that scheduled it. It is what a progress report measures
    /// from, so the progress follows the transition that was actually scheduled
    /// rather than assuming every one spans the same time.
    pub started_at_ms: i64,
    /// Epoch milliseconds at which the change became (or becomes) due.
    pub at_ms: i64,
    /// The state the instance settles into.
    pub state: Lifecycle,
}

impl DueTransition {
    /// The transition `state`, due [`STATE_TRANSITION_MILLIS`] after `now_ms`.
    pub fn after(now_ms: i64, state: Lifecycle) -> Self {
        Self {
            started_at_ms: now_ms,
            at_ms: now_ms.saturating_add(STATE_TRANSITION_MILLIS),
            state,
        }
    }

    /// How far through this transition the instant `now_ms` is, as a whole
    /// percentage — what an operation reporting the progress of the work that
    /// scheduled it answers with.
    pub fn percentage_complete_at(&self, now_ms: i64) -> i32 {
        elapsed_percentage(self.started_at_ms, self.at_ms, now_ms)
    }
}

/// `DiskState` values this slice uses, from the modelled `DiskState` enum and the
/// `Disk$state` documentation: a disk Lightsail is still provisioning is
/// `pending`, one that exists and is attached to nothing is `available`, and one
/// an instance is using is `in-use`.
pub const DISK_STATE_PENDING: &str = "pending";
/// See [`DISK_STATE_PENDING`].
pub const DISK_STATE_AVAILABLE: &str = "available";
/// See [`DISK_STATE_PENDING`].
pub const DISK_STATE_IN_USE: &str = "in-use";

/// The verbs the disk calls name themselves by in a refusal their disk's state
/// blocks, one per operation that has such a guard, so the sentence
/// [`disk_blocked_by_state_message`] builds carries the verb of the call that
/// was refused.
pub const DISK_VERB_ATTACH: &str = "attach";
/// See [`DISK_VERB_ATTACH`].
pub const DISK_VERB_DETACH: &str = "detach";
/// See [`DISK_VERB_ATTACH`].
pub const DISK_VERB_DELETE: &str = "delete";

/// `DiskSnapshotState` values this slice uses, from the modelled
/// `DiskSnapshotState` enum and the `DiskSnapshot$state` documentation ("The
/// status of the disk snapshot operation"): a snapshot Lightsail is still taking
/// is `pending` and one whose data is all captured is `completed`. The enum's
/// `error` and `unknown` members describe a snapshot that failed or whose status
/// Lightsail cannot report, neither of which this store can produce — nothing
/// here fails a copy.
pub const DISK_SNAPSHOT_STATE_PENDING: &str = "pending";
/// See [`DISK_SNAPSHOT_STATE_PENDING`].
pub const DISK_SNAPSHOT_STATE_COMPLETED: &str = "completed";

/// `InstanceSnapshotState` values this slice uses, from the modelled
/// `InstanceSnapshotState` enum and the `InstanceSnapshot$state` documentation
/// ("The state the snapshot is in"): a snapshot Lightsail is still taking is
/// `pending` and one that can be restored is `available`. The enum's third
/// member, `error`, describes a snapshot that failed, which this store cannot
/// produce — nothing here fails a copy.
pub const INSTANCE_SNAPSHOT_STATE_PENDING: &str = "pending";
/// See [`INSTANCE_SNAPSHOT_STATE_PENDING`].
pub const INSTANCE_SNAPSHOT_STATE_AVAILABLE: &str = "available";

/// `AutoSnapshotStatus` values this slice uses, from the modelled
/// `AutoSnapshotStatus` enum and the `AutoSnapshotDetails$status` documentation
/// ("The status of the automatic snapshot"): a copy the service is still taking
/// reports `InProgress`, and one whose data is all captured reports `Success`.
/// The enum's `Failed` member describes a copy that failed, which this store
/// cannot produce — nothing here fails a copy — and its `NotFound` member a copy
/// the service has no record of, which is not reported at all.
pub const AUTO_SNAPSHOT_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`AUTO_SNAPSHOT_STATUS_IN_PROGRESS`].
pub const AUTO_SNAPSHOT_STATUS_SUCCESS: &str = "Success";

/// `RecordState` values this slice uses, from the modelled `RecordState` enum
/// and the `ExportSnapshotRecord$state` documentation ("The state of the export
/// snapshot record"): an export is answered `Started` and settles into
/// `Succeeded` once the resource it creates at the destination exists. The
/// enum's `Failed` member describes an export that failed, which this store
/// cannot produce — nothing here fails an export.
pub const RECORD_STATE_STARTED: &str = "Started";
/// See [`RECORD_STATE_STARTED`].
pub const RECORD_STATE_SUCCEEDED: &str = "Succeeded";

/// A state the clock already owes a resource whose state is a plain NAME: a
/// block storage disk's `DiskState` value, a disk snapshot's
/// `DiskSnapshotState` value, a managed relational database's `state`, or one of
/// that database's snapshots' `state`.
///
/// Separate from [`DueTransition`] because those states are names with no
/// numeric code, where an instance's is an EC2 `InstanceState` (a code and a
/// name together).
#[derive(Debug, Clone, Copy)]
pub struct DueNamedState {
    /// Epoch milliseconds at which the change became (or becomes) due.
    pub at_ms: i64,
    /// The state the resource settles into.
    pub state: &'static str,
}

impl DueNamedState {
    /// The transition to `state`, due [`STATE_TRANSITION_MILLIS`] after `now_ms`.
    pub fn after(now_ms: i64, state: &'static str) -> Self {
        Self {
            at_ms: now_ms.saturating_add(STATE_TRANSITION_MILLIS),
            state,
        }
    }
}

/// `RelationalDatabase$state` values this slice uses — "Describes the current
/// state of the database", a plain string in the model with no enum behind it.
///
/// A Lightsail managed database is an Amazon RDS DB instance, and the member
/// carries RDS's own DB instance status values: `creating` while the instance is
/// being provisioned, `available` once it accepts connections, `rebooting`
/// while its engine is being restarted, `stopping` while it is being shut down,
/// `stopped` once it is shut down and accepts no connection, and `starting`
/// while it is being brought back up (Amazon RDS User Guide, "Viewing DB
/// instance status").
pub const RELATIONAL_DATABASE_STATE_CREATING: &str = "creating";
/// See [`RELATIONAL_DATABASE_STATE_CREATING`].
pub const RELATIONAL_DATABASE_STATE_AVAILABLE: &str = "available";
/// See [`RELATIONAL_DATABASE_STATE_CREATING`].
pub const RELATIONAL_DATABASE_STATE_REBOOTING: &str = "rebooting";
/// See [`RELATIONAL_DATABASE_STATE_CREATING`].
pub const RELATIONAL_DATABASE_STATE_STOPPING: &str = "stopping";
/// See [`RELATIONAL_DATABASE_STATE_CREATING`].
pub const RELATIONAL_DATABASE_STATE_STOPPED: &str = "stopped";
/// See [`RELATIONAL_DATABASE_STATE_CREATING`].
pub const RELATIONAL_DATABASE_STATE_STARTING: &str = "starting";

/// How long a managed relational database may stay stopped before the service
/// starts it again on its own.
///
/// An AWS DURATION, kept at the published value rather than chosen: "If you
/// don't manually start your database instance after it has been stopped for
/// seven consecutive days, Amazon Lightsail automatically starts it for you"
/// (the modelled `StopRelationalDatabase` documentation).
pub const RELATIONAL_DATABASE_AUTO_START_AFTER_MILLIS: i64 = 7 * 24 * 60 * 60 * 1_000;

/// `RelationalDatabaseSnapshot$state` values this slice uses — "The state of the
/// database snapshot", a plain string in the model with no enum behind it.
///
/// A Lightsail database snapshot is an Amazon RDS DB snapshot, and the member
/// carries RDS's own DB snapshot status values: `creating` while the snapshot is
/// being taken and `available` once it can be restored from (Amazon RDS API
/// Reference, `DBSnapshot$Status`). RDS's third value, `failed`, describes a
/// snapshot whose copy failed, which this store cannot produce — nothing here
/// fails a copy.
pub const RELATIONAL_DATABASE_SNAPSHOT_STATE_CREATING: &str = "creating";
/// See [`RELATIONAL_DATABASE_SNAPSHOT_STATE_CREATING`].
pub const RELATIONAL_DATABASE_SNAPSHOT_STATE_AVAILABLE: &str = "available";

/// `RelationalDatabaseEvent$eventCategories` values this slice uses — "The
/// category that the database event belongs to", a plain string in the model with
/// no enum behind it.
///
/// A Lightsail managed database is an Amazon RDS DB instance, so what happens to
/// it is an RDS DB instance event and carries RDS's own event categories (Amazon
/// RDS User Guide, "Amazon RDS event categories and event messages"):
/// `creation` while the database is being provisioned, `availability` for its
/// engine going down and coming back up, `backup` for a snapshot of it being
/// taken, and `configuration change` for a modification reaching it.
pub const RELATIONAL_DATABASE_EVENT_CATEGORY_CREATION: &str = "creation";
/// See [`RELATIONAL_DATABASE_EVENT_CATEGORY_CREATION`].
pub const RELATIONAL_DATABASE_EVENT_CATEGORY_AVAILABILITY: &str = "availability";
/// See [`RELATIONAL_DATABASE_EVENT_CATEGORY_CREATION`].
pub const RELATIONAL_DATABASE_EVENT_CATEGORY_BACKUP: &str = "backup";
/// See [`RELATIONAL_DATABASE_EVENT_CATEGORY_CREATION`].
pub const RELATIONAL_DATABASE_EVENT_CATEGORY_CONFIGURATION_CHANGE: &str = "configuration change";

/// The event a database logs once it has finished being provisioned and accepts
/// connections.
///
/// CHOSEN wording: the model publishes no event text, only that
/// `RelationalDatabaseEvent$message` is "the message of the database event". What
/// the service DOES guarantee, and what this slice keeps, is that one entry is
/// logged per change the service makes to the database, at the instant it
/// happened, in the categories that change falls in.
pub const RELATIONAL_DATABASE_EVENT_CREATED: &str = "Database created";
/// See [`RELATIONAL_DATABASE_EVENT_CREATED`]. Logged when the engine is asked to
/// go down — a restart or a stop.
pub const RELATIONAL_DATABASE_EVENT_SHUTDOWN: &str = "Database shutdown";
/// See [`RELATIONAL_DATABASE_EVENT_CREATED`]. Logged when a restart has brought
/// the engine back up.
pub const RELATIONAL_DATABASE_EVENT_RESTARTED: &str = "Database restarted";
/// See [`RELATIONAL_DATABASE_EVENT_CREATED`]. Logged when the engine has finished
/// shutting down.
pub const RELATIONAL_DATABASE_EVENT_STOPPED: &str = "Database stopped";
/// See [`RELATIONAL_DATABASE_EVENT_CREATED`]. Logged when a stopped database's
/// engine is serving again.
pub const RELATIONAL_DATABASE_EVENT_STARTED: &str = "Database started";
/// See [`RELATIONAL_DATABASE_EVENT_CREATED`]. Logged when a snapshot of the
/// database starts being taken.
pub const RELATIONAL_DATABASE_EVENT_BACKUP_STARTED: &str = "Backing up database";
/// See [`RELATIONAL_DATABASE_EVENT_CREATED`]. Logged when that snapshot can be
/// restored from.
pub const RELATIONAL_DATABASE_EVENT_BACKUP_FINISHED: &str = "Finished database backup";
/// See [`RELATIONAL_DATABASE_EVENT_CREATED`]. Logged when a value reaches the
/// database's parameter group.
pub const RELATIONAL_DATABASE_EVENT_PARAMETER_GROUP_CHANGED: &str =
    "Applied change to database parameter group";
/// See [`RELATIONAL_DATABASE_EVENT_CREATED`]. Logged when a modification reaches
/// the database, followed by the members it changed.
pub const RELATIONAL_DATABASE_EVENT_MODIFICATION_APPLIED: &str = "Applied database modification";

/// The window `GetRelationalDatabaseEvents` reports when the request names none.
///
/// An AWS-published value: "The number of minutes in the past from which to
/// retrieve events. … Default: 60" (the modelled `durationInMinutes`
/// documentation).
pub const RELATIONAL_DATABASE_EVENT_DEFAULT_DURATION_MINUTES: i32 = 60;

/// The shortest window `GetRelationalDatabaseEvents` accepts: "The minimum is 1"
/// (the modelled `durationInMinutes` documentation).
pub const RELATIONAL_DATABASE_EVENT_MIN_DURATION_MINUTES: i32 = 1;

/// The longest window `GetRelationalDatabaseEvents` accepts: "the maximum is 14
/// days (20160 minutes)" (the modelled `durationInMinutes` documentation).
pub const RELATIONAL_DATABASE_EVENT_MAX_DURATION_MINUTES: i32 = 20_160;

/// How long an entry stays in a database's event log.
///
/// The longest window a caller may ask for, so an entry older than this can
/// never be reported again and is dropped rather than kept for ever.
pub const RELATIONAL_DATABASE_EVENT_RETENTION_MILLIS: i64 =
    RELATIONAL_DATABASE_EVENT_MAX_DURATION_MINUTES as i64 * 60_000;

/// The most events one database's log keeps, oldest dropped first.
///
/// Emulator resource guard, like [`MAX_OPERATION_HISTORY`]: the log is
/// append-only, so without a cap a long-lived emulator would grow without bound.
pub const MAX_RELATIONAL_DATABASE_EVENTS: usize = 500;
