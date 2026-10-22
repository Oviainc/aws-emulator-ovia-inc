// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-gameliftstreams-gameliftstreams service.
//!
//! This module owns the three persisted resources of Amazon GameLift Streams —
//! applications, stream groups and the stream sessions a stream group hosts —
//! plus the helpers every handler in `src/ops/` shares:
//!
//! * [`ComAmazonawsGameliftstreamsGameliftstreamsState::materialize`] advances every
//!   transition the CLOCK already owed at a given instant (a create finishing, a
//!   delete completing, a placement resolving). Handlers call it first, so a
//!   reporting operation reports what the clock already made due and nothing else;
//!   the advanced record's `LastUpdatedAt` is the instant the transition was DUE,
//!   never the instant it was observed, so two reads of the same resource agree.
//! * [`identifier_id`] resolves the model's `Identifier` shape, which accepts either
//!   a bare resource id or a full ARN for the same resource.
//! * [`application_arn`] / [`stream_group_arn`] / [`stream_session_arn`] build ARNs
//!   through `engine::emulator_arn`, so the account segment is the canonical one.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    DefaultApplication, DisplayConfiguration, LocationConfiguration, LocationState,
    PerformanceStatsConfiguration, ReplicationStatus, RuntimeEnvironment,
};

/// This service's SigV4 signing name, and the `service` segment of its ARNs.
pub const SIGNING_NAME: &str = "gameliftstreams";

/// The region a request that carries no SigV4 credential scope is answered in.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long a transition the clock owes takes to come due: a create reaching its
/// ready status, a delete completing, a stream placement resolving.
///
/// An emulator-chosen span (the real service takes minutes), deliberately
/// non-zero so a caller that polls immediately after the call that started the
/// transition still observes the transient status and the state errors that go
/// with it.
pub const TRANSITION: Duration = Duration::from_millis(500);

/// A stream group's lifetime. `CreateStreamGroup`'s documentation: "Stream groups
/// expire when they are 365 days old, at which point they can no longer stream
/// sessions. The exact expiration date is indicated by the date value in the
/// `ExpiresAt` field."
pub const STREAM_GROUP_LIFETIME: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// How many applications the store holds before `CreateApplication` refuses with
/// the modelled `ServiceQuotaExceededException`.
///
/// Amazon GameLift Streams publishes an adjustable per-account, per-Region quota on
/// applications, and this emulator models neither Service Quotas nor an account's
/// adjustments — so this is the emulator's OWN bound on the store rather than a
/// claim about any account's limit. It is far above any workflow an emulator
/// serves, and it is what stops the store growing without end: every record it
/// holds is released by `DeleteApplication`, which also forgets that application's
/// client tokens, so the token map cannot outgrow this bound either.
pub const APPLICATION_QUOTA: usize = 1_000;

/// How many stream groups the store holds before `CreateStreamGroup` refuses with
/// the modelled `ServiceQuotaExceededException`. The emulator's own bound, for the
/// same reason as [`APPLICATION_QUOTA`]; `DeleteStreamGroup` releases each record
/// and its client tokens.
pub const STREAM_GROUP_QUOTA: usize = 1_000;

/// How many tags ONE resource holds.
///
/// The Amazon Web Services General Reference's tag restrictions — which
/// `TagResource` points at ("Tagging Amazon Web Services Resources in the Amazon
/// Web Services General Reference") — give a maximum of 50 tags per resource, and
/// the model mirrors that bound on every tagging operation's `Tags` map
/// (`length 1..50`). A request that would take a resource past it is refused, so
/// the stored tag map cannot grow without end.
pub const TAGS_PER_RESOURCE: usize = 50;

/// How many stream sessions ONE stream group holds at a time.
///
/// A stream session occupies a compute resource the stream group allocated, so the
/// real service bounds concurrent sessions by the group's capacity. This emulator
/// does not allocate compute, so it bounds the per-group session store itself:
/// sessions that have reached a terminal status are released first (oldest first,
/// see [`StreamGroup::make_room_for_a_session`]), and a group whose live sessions
/// fill the bound refuses the start with the modelled `ConflictException` —
/// `StartStreamSession` does not model `ServiceQuotaExceededException`.
pub const SESSIONS_PER_STREAM_GROUP: usize = 100;

/// How many locations ONE stream group holds, after which
/// `AddStreamGroupLocations` refuses with the modelled
/// `ServiceQuotaExceededException`.
///
/// The model bounds one request's `LocationConfigurations` list at `length 1..100`
/// (`CreateStreamGroup` and `AddStreamGroupLocations` share the shape), so a
/// single stream group can never be configured with more than 100 locations in one
/// call; this is that same bound applied to the group's accumulated set, which is
/// what stops repeated adds growing one group without end.
pub const LOCATIONS_PER_STREAM_GROUP: usize = 100;

/// How long a stream session that has reached a terminal status (`ERROR`,
/// `TERMINATED`) is kept before the clock releases it.
///
/// Emulator-chosen: AWS documents no retention for a stream session record. It is
/// long enough that a caller which terminates a session and then reads it back
/// still sees the terminal status and its `StatusReason`, and short enough that a
/// start/terminate loop cannot grow the store without end. The instant is derived
/// from the record's own `LastUpdatedAt`; nothing sleeps.
///
/// The per-group BOUND wins over this span: a group already at
/// [`SESSIONS_PER_STREAM_GROUP`] releases its oldest terminal session to admit a
/// new one even if that session's retention is not up yet (see
/// [`StreamGroup::make_room_for_a_session`]), because a bound that a caller can
/// defer is not a bound.
pub const TERMINAL_SESSION_RETENTION: Duration = Duration::from_secs(60);

// The enum values this slice stores, from the model's `ApplicationStatus`,
// `StreamGroupStatus`, `StreamGroupLocationStatus`, `StreamSessionStatus`,
// `StreamSessionStatusReason` and `ReplicationStatusType` (see
// `crate::ops::types::shared_types`, which lists every value each accepts).
pub const APPLICATION_INITIALIZED: &str = "INITIALIZED";
pub const APPLICATION_PROCESSING: &str = "PROCESSING";
pub const APPLICATION_READY: &str = "READY";
pub const APPLICATION_DELETING: &str = "DELETING";
pub const STREAM_GROUP_ACTIVATING: &str = "ACTIVATING";
pub const STREAM_GROUP_ACTIVE: &str = "ACTIVE";
/// "UPDATING_LOCATIONS: One or more locations in the stream group are in the
/// process of updating (either activating or deleting)" (`StreamGroupStatus`
/// documentation).
pub const STREAM_GROUP_UPDATING_LOCATIONS: &str = "UPDATING_LOCATIONS";
pub const STREAM_GROUP_DELETING: &str = "DELETING";
pub const STREAM_GROUP_EXPIRED: &str = "EXPIRED";
pub const LOCATION_ACTIVATING: &str = "ACTIVATING";
pub const LOCATION_ACTIVE: &str = "ACTIVE";
pub const LOCATION_REMOVING: &str = "REMOVING";
pub const LOCATION_ERROR: &str = "ERROR";
pub const SESSION_ACTIVATING: &str = "ACTIVATING";
pub const SESSION_ERROR: &str = "ERROR";
pub const SESSION_TERMINATING: &str = "TERMINATING";
pub const SESSION_TERMINATED: &str = "TERMINATED";
pub const SESSION_REASON_INTERNAL_ERROR: &str = "internalError";
pub const SESSION_REASON_API_TERMINATED: &str = "apiTerminated";
pub const REPLICATION_REPLICATING: &str = "REPLICATING";
pub const REPLICATION_COMPLETED: &str = "COMPLETED";

/// The ARN resource-type segment of each resource this service names.
pub const APPLICATION_RESOURCE: &str = "application";
pub const STREAM_GROUP_RESOURCE: &str = "streamgroup";
pub const STREAM_SESSION_RESOURCE: &str = "streamsession";

/// The characters a minted `Id` is built from. The model's `Id` shape is
/// `length 1..32, pattern ^[a-zA-Z0-9-]+$`.
const ID_ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// One location's share of an application's content replication, as
/// `CreateApplication` reports it in `ReplicationStatuses`.
///
/// Keyed by the stream group that asked for the location, so dropping that group
/// drops exactly its replication rows and leaves the ones another group needs.
#[derive(Debug, Clone)]
pub struct Replication {
    /// ARN of the stream group whose location configuration asked for this.
    pub stream_group_arn: String,
    pub location: String,
    /// When replication to `location` is due to finish.
    pub completes_at: SystemTime,
}

/// A persisted application resource.
#[derive(Debug, Clone)]
pub struct Application {
    pub id: String,
    pub arn: String,
    pub description: String,
    pub runtime_environment: RuntimeEnvironment,
    pub executable_path: String,
    pub application_source_uri: String,
    pub application_log_paths: Vec<String>,
    pub application_log_output_uri: Option<String>,
    /// An `ApplicationStatus` value.
    pub status: String,
    /// An `ApplicationStatusReason` value, set only in `ERROR` status.
    pub status_reason: Option<String>,
    pub replications: Vec<Replication>,
    /// ARNs of the stream groups this application is associated with.
    pub associated_stream_groups: Vec<String>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub last_updated_at: SystemTime,
    /// When the clock owes this application's next status transition, if any.
    pub status_due_at: Option<SystemTime>,
}

impl Application {
    /// `ReplicationStatuses` for this application: one row per location, reported
    /// `COMPLETED` once every group that asked for that location has finished
    /// replicating to it. Empty for an application no stream group references.
    ///
    /// Collapsed through an ordered map keyed by location, so the cost is
    /// `n log n` in the stored rows rather than quadratic, and the result is in a
    /// stable order.
    pub fn replication_statuses(&self, now: SystemTime) -> Vec<ReplicationStatus> {
        let mut by_location: ::std::collections::BTreeMap<&str, bool> =
            ::std::collections::BTreeMap::new();
        for replication in &self.replications {
            let completed = now >= replication.completes_at;
            by_location
                .entry(replication.location.as_str())
                .and_modify(|all_completed| *all_completed &= completed)
                .or_insert(completed);
        }
        by_location
            .into_iter()
            .map(|(location, completed)| ReplicationStatus {
                location: Some(location.to_string()),
                status: Some(
                    if completed {
                        REPLICATION_COMPLETED
                    } else {
                        REPLICATION_REPLICATING
                    }
                    .to_string(),
                ),
            })
            .collect()
    }

    /// Advance the transition the clock owed at `now`. Answers `true` when the
    /// record is due to be gone (a `DELETING` application whose deletion completed).
    fn advance(&mut self, now: SystemTime) -> bool {
        let Some(due) = self.status_due_at else {
            return false;
        };
        if now < due {
            return false;
        }
        self.status_due_at = None;
        // `LastUpdatedAt` moves only when the status actually does, so a due
        // instant on a status with no transition cannot rewrite it.
        match self.status.as_str() {
            APPLICATION_INITIALIZED => {
                self.status = APPLICATION_READY.to_string();
                self.last_updated_at = due;
                false
            }
            APPLICATION_DELETING => true,
            _ => false,
        }
    }
}

/// One location of a stream group, as the request configured it.
#[derive(Debug, Clone)]
pub struct LocationSlot {
    pub name: String,
    pub always_on_capacity: Option<i32>,
    pub on_demand_capacity: Option<i32>,
    pub target_idle_capacity: Option<i32>,
    pub maximum_capacity: Option<i32>,
    /// A `StreamGroupLocationStatus` value. Each location carries its OWN status,
    /// because a location added to a group that is already serving is
    /// `ACTIVATING` while the locations beside it stay `ACTIVE`.
    pub status: String,
    /// When the clock owes this location's next status transition, if any.
    pub status_due_at: Option<SystemTime>,
}

impl LocationSlot {
    /// A location as `CreateStreamGroup` or `AddStreamGroupLocations` configured
    /// it, in `ACTIVATING`: "ACTIVATING: Amazon GameLift Streams is preparing the
    /// location. You cannot stream from, scale the capacity of, or remove this
    /// location yet." The clock owes its move to `ACTIVE` one [`TRANSITION`] later,
    /// once the capacity it provisions is in place.
    pub fn activating(configuration: LocationConfiguration, now: SystemTime) -> Self {
        Self {
            name: configuration.location_name,
            always_on_capacity: configuration.always_on_capacity,
            on_demand_capacity: configuration.on_demand_capacity,
            target_idle_capacity: configuration.target_idle_capacity,
            maximum_capacity: configuration.maximum_capacity,
            status: LOCATION_ACTIVATING.to_string(),
            status_due_at: Some(now + TRANSITION),
        }
    }

    /// Is this location still working towards a status of its own?
    ///
    /// `REMOVING` counts as well as `ACTIVATING` — "REMOVING: Amazon GameLift
    /// Streams is working to remove this location" — so a location on its way out
    /// keeps the group in `UPDATING_LOCATIONS` too. Nothing stores `REMOVING` yet:
    /// `RemoveStreamGroupLocations` is still a scaffold stub, and whoever
    /// implements it owns that slot status and the transition that ends it.
    fn is_settling(&self) -> bool {
        matches!(self.status.as_str(), LOCATION_ACTIVATING | LOCATION_REMOVING)
    }

    /// Advance the transition the clock owed this location at `now`.
    fn advance(&mut self, now: SystemTime) {
        let Some(due) = self.status_due_at else {
            return;
        };
        if now < due {
            return;
        }
        self.status_due_at = None;
        // "ACTIVE: The location is provisioned with initial capacity."
        if self.status == LOCATION_ACTIVATING {
            self.status = LOCATION_ACTIVE.to_string();
        }
    }
}

/// The message refusing a `LocationConfiguration` this service cannot store, or
/// `None` when it can.
///
/// Every capacity member of `LocationConfiguration` is modelled `@range(min: 0)`.
/// The capacities are stored, echoed and used in this service's capacity
/// arithmetic, so a negative one is refused by the operation that carries it
/// rather than carried into state. Shared by `CreateStreamGroup` and
/// `AddStreamGroupLocations`, which configure a location through the same shape.
pub fn unstorable_location_configuration(configuration: &LocationConfiguration) -> Option<String> {
    for (member, value) in [
        ("AlwaysOnCapacity", configuration.always_on_capacity),
        ("OnDemandCapacity", configuration.on_demand_capacity),
        ("TargetIdleCapacity", configuration.target_idle_capacity),
        ("MaximumCapacity", configuration.maximum_capacity),
    ] {
        if let Some(capacity) = value {
            if capacity < 0 {
                return Some(format!(
                    "LocationConfigurations[{}].{member} must not be negative, got {capacity}",
                    configuration.location_name
                ));
            }
        }
    }
    None
}

/// A persisted stream group, together with the stream sessions it hosts.
#[derive(Debug, Clone)]
pub struct StreamGroup {
    pub id: String,
    pub arn: String,
    pub description: String,
    /// A `StreamClass` value.
    pub stream_class: String,
    pub default_application: Option<DefaultApplication>,
    pub locations: Vec<LocationSlot>,
    /// A `StreamGroupStatus` value.
    pub status: String,
    /// A `StreamGroupStatusReason` value, set only in `ERROR` status.
    pub status_reason: Option<String>,
    /// ARNs of the applications associated with this stream group.
    pub associated_applications: Vec<String>,
    pub sessions: HashMap<String, StreamSession>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub last_updated_at: SystemTime,
    pub expires_at: SystemTime,
    /// When the clock owes this stream group's next status transition, if any.
    pub status_due_at: Option<SystemTime>,
}

impl StreamGroup {
    /// Does this group hold `location`?
    pub fn has_location(&self, location: &str) -> bool {
        self.locations.iter().any(|slot| slot.name == location)
    }

    /// Can this group host a stream session right now?
    ///
    /// "Prerequisites: You must have a stream group in ACTIVE status"
    /// (`StartStreamSession` documentation). A group that is updating its
    /// locations is still serving the capacity its provisioned locations hold —
    /// `UPDATING_LOCATIONS` says "one or more locations ... are in the process of
    /// updating", not that the group stopped streaming — so it hosts sessions too,
    /// on the locations [`StreamGroup::can_stream_from`] admits.
    pub fn can_host_sessions(&self) -> bool {
        matches!(
            self.status.as_str(),
            STREAM_GROUP_ACTIVE | STREAM_GROUP_UPDATING_LOCATIONS
        )
    }

    /// Does this group hold `location` in a status that can host a stream?
    ///
    /// Only an `ACTIVE` location can: "ACTIVATING: Amazon GameLift Streams is
    /// preparing the location. You cannot stream from, scale the capacity of, or
    /// remove this location yet."
    pub fn can_stream_from(&self, location: &str) -> bool {
        self.locations
            .iter()
            .any(|slot| slot.name == location && slot.status == LOCATION_ACTIVE)
    }

    /// Make room for one more stream session, releasing the sessions that have
    /// reached a terminal status (oldest first) when the store is at
    /// [`SESSIONS_PER_STREAM_GROUP`].
    ///
    /// Answers `false` when every session the bound allows is still live, which is
    /// the one case the caller must refuse; otherwise the group is below the bound
    /// on return.
    pub fn make_room_for_a_session(&mut self) -> bool {
        if self.sessions.len() < SESSIONS_PER_STREAM_GROUP {
            return true;
        }
        let mut terminal: Vec<(SystemTime, String)> = self
            .sessions
            .values()
            .filter(|session| session.is_terminal())
            .map(|session| (session.last_updated_at, session.id.clone()))
            .collect();
        terminal.sort();
        for (_, id) in terminal {
            self.sessions.remove(&id);
            if self.sessions.len() < SESSIONS_PER_STREAM_GROUP {
                return true;
            }
        }
        false
    }

    /// One location's `LocationState`: the capacity the request configured, the
    /// location's own status, and what is allocated to it and idle in it right now.
    ///
    /// `AllocatedCapacity` is the always-on capacity the location holds once it is
    /// `ACTIVE` (always-on capacity "is allocated to you and is never released back
    /// to the service" — `CreateStreamGroup` documentation), and `IdleCapacity` is
    /// that allocation less the stream sessions the location is currently running.
    ///
    /// The group's own status wins where it says what every location is doing: a
    /// `DELETING` group is releasing all of them, and an `EXPIRED` group's capacity
    /// is gone, so its locations are not provisioned any more and cannot host a
    /// stream.
    pub fn location_state(&self, slot: &LocationSlot) -> LocationState {
        let location_status = match self.status.as_str() {
            STREAM_GROUP_DELETING => LOCATION_REMOVING,
            STREAM_GROUP_EXPIRED => LOCATION_ERROR,
            _ => slot.status.as_str(),
        };
        let requested = slot.always_on_capacity.unwrap_or(0).max(0);
        let allocated = if location_status == LOCATION_ACTIVE {
            requested
        } else {
            0
        };
        let running = i32::try_from(
            self.sessions
                .values()
                .filter(|session| session.location == slot.name && session.is_running())
                .count(),
        )
        .unwrap_or(i32::MAX);
        LocationState {
            location_name: Some(slot.name.clone()),
            status: Some(location_status.to_string()),
            always_on_capacity: slot.always_on_capacity,
            on_demand_capacity: slot.on_demand_capacity,
            target_idle_capacity: slot.target_idle_capacity,
            maximum_capacity: slot.maximum_capacity,
            requested_capacity: Some(requested),
            allocated_capacity: Some(allocated),
            // Saturating, not checked: the capacities come from the request,
            // so the arithmetic must not be able to overflow whatever a
            // caller sent.
            idle_capacity: Some(allocated.saturating_sub(running).max(0)),
            internal_vpc_ipv4_cidr_block: None,
            vpc_transit_configuration: None,
        }
    }

    /// `LocationStates` for every location this group holds, in the order the
    /// requests that configured them named them.
    pub fn location_states(&self) -> Vec<LocationState> {
        self.locations
            .iter()
            .map(|slot| self.location_state(slot))
            .collect()
    }

    /// `LocationStates` for the locations `names` lists, skipping any this group
    /// does not hold. Used by `AddStreamGroupLocations`, which reports the
    /// locations THAT CALL configured rather than the group's whole set.
    pub fn location_states_of(&self, names: &[String]) -> Vec<LocationState> {
        names
            .iter()
            .filter_map(|name| self.locations.iter().find(|slot| &slot.name == name))
            .map(|slot| self.location_state(slot))
            .collect()
    }

    /// Is any location of this group still activating or being removed? While one
    /// is, the group reports `UPDATING_LOCATIONS`.
    pub fn has_settling_location(&self) -> bool {
        self.locations.iter().any(LocationSlot::is_settling)
    }

    /// Advance this group and its sessions. Answers `true` when the group is due
    /// to be gone (a `DELETING` group whose deletion completed).
    fn advance(&mut self, now: SystemTime) -> bool {
        for session in self.sessions.values_mut() {
            session.advance(now);
        }
        // A session the clock has retained long enough past its terminal status is
        // released, so `sessions` is bounded by the streams still worth reporting
        // rather than by every `StartStreamSession` ever served.
        self.sessions
            .retain(|_, session| !session.retention_elapsed(now));
        for slot in &mut self.locations {
            slot.advance(now);
        }
        if let Some(due) = self.status_due_at {
            if now >= due {
                // `LastUpdatedAt` moves only when the status actually does.
                match self.status.as_str() {
                    // A group whose locations have all settled is serving again:
                    // "ACTIVE: The stream group is ready to host streams." While
                    // one has not, the group is still updating its locations and
                    // the clock still owes the move, so the due instant stands.
                    STREAM_GROUP_ACTIVATING | STREAM_GROUP_UPDATING_LOCATIONS => {
                        if !self.has_settling_location() {
                            self.status_due_at = None;
                            self.status = STREAM_GROUP_ACTIVE.to_string();
                            self.last_updated_at = due;
                        }
                    }
                    STREAM_GROUP_DELETING => {
                        self.status_due_at = None;
                        return true;
                    }
                    _ => self.status_due_at = None,
                }
            }
        }
        if self.status == STREAM_GROUP_ACTIVE && now >= self.expires_at {
            self.status = STREAM_GROUP_EXPIRED.to_string();
            self.last_updated_at = self.expires_at;
        }
        false
    }
}

/// A persisted stream session, stored under the stream group that hosts it.
#[derive(Debug, Clone)]
pub struct StreamSession {
    pub id: String,
    pub arn: String,
    pub stream_group_id: String,
    pub application_arn: String,
    pub description: Option<String>,
    pub user_id: Option<String>,
    /// A `StreamSessionStatus` value.
    pub status: String,
    /// A `StreamSessionStatusReason` value.
    pub status_reason: Option<String>,
    /// A `Protocol` value.
    pub protocol: String,
    pub location: String,
    pub signal_request: String,
    /// The WebRTC answer "that the stream server generates in response to the
    /// `SignalRequest`" (`SignalResponse` documentation). This emulator runs no
    /// stream server, so nothing ever fills it in.
    pub signal_response: Option<String>,
    pub web_sdk_protocol_url: Option<String>,
    pub connection_timeout_seconds: Option<i32>,
    pub session_length_seconds: Option<i32>,
    pub additional_launch_args: Vec<String>,
    pub additional_environment_variables: HashMap<String, String>,
    /// Whether the session shares its performance statistics with the client, as
    /// the request configured it.
    pub performance_stats_configuration: Option<PerformanceStatsConfiguration>,
    /// The virtual monitor the session was started with (its resolution).
    pub display_configuration: Option<DisplayConfiguration>,
    /// The IAM role the session assumes, as the request named it.
    pub role_arn: Option<String>,
    pub log_file_location_uri: Option<String>,
    pub created_at: SystemTime,
    pub last_updated_at: SystemTime,
    /// When the clock owes this session's next status transition, if any.
    pub status_due_at: Option<SystemTime>,
}

impl StreamSession {
    /// Is this session occupying stream capacity?
    pub fn is_running(&self) -> bool {
        !matches!(
            self.status.as_str(),
            SESSION_ERROR | SESSION_TERMINATED | SESSION_TERMINATING
        )
    }

    /// Has this session reached a status it can never leave?
    pub fn is_terminal(&self) -> bool {
        matches!(self.status.as_str(), SESSION_ERROR | SESSION_TERMINATED)
    }

    /// Is this session's [`TERMINAL_SESSION_RETENTION`] up, so the clock owes its
    /// release? `false` for a session that is still live.
    pub fn retention_elapsed(&self, now: SystemTime) -> bool {
        self.is_terminal() && now >= self.last_updated_at + TERMINAL_SESSION_RETENTION
    }

    /// Advance the transition the clock owed at `now`.
    ///
    /// `ACTIVATING` resolves into `ERROR` with `StatusReason` `internalError`: a
    /// session only reaches `ACTIVE` when a stream server starts the content and
    /// answers the client's WebRTC offer, and this emulator has no stream server,
    /// so there is no placement to report as successful.
    fn advance(&mut self, now: SystemTime) {
        let Some(due) = self.status_due_at else {
            return;
        };
        if now < due {
            return;
        }
        self.status_due_at = None;
        // `LastUpdatedAt` moves only when the status actually does.
        match self.status.as_str() {
            SESSION_ACTIVATING => {
                self.status = SESSION_ERROR.to_string();
                self.status_reason = Some(SESSION_REASON_INTERNAL_ERROR.to_string());
                self.last_updated_at = due;
            }
            SESSION_TERMINATING => {
                self.status = SESSION_TERMINATED.to_string();
                self.last_updated_at = due;
            }
            _ => {}
        }
    }
}

/// Per-service state for com-amazonaws-gameliftstreams-gameliftstreams. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsGameliftstreamsGameliftstreamsState {
    /// Applications by application id.
    pub applications: HashMap<String, Application>,
    /// Stream groups by stream group id; each one owns its stream sessions.
    pub stream_groups: HashMap<String, StreamGroup>,
    /// `"<operation>:<ClientToken>"` -> the id of the resource that token created,
    /// so a replayed `@idempotencyToken` answers with the first result instead of
    /// creating a second resource. The operation name is part of the key because
    /// one client may reuse a token value across unrelated creates.
    pub idempotency: HashMap<String, String>,
    /// Monotonic counter mixed into every minted id, which is what makes two
    /// minted ids distinct by construction rather than by luck.
    mint_counter: u64,
}

impl ComAmazonawsGameliftstreamsGameliftstreamsState {
    /// Mint a fresh `Id` with `prefix`: five pseudo-random characters followed by
    /// this store's mint counter in base62, so the result is unique within the
    /// store and matches the model's `Id` shape.
    pub fn mint_id(&mut self, prefix: &str) -> String {
        self.mint_counter += 1;
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos() as u64);
        let mut hash = nanos ^ self.mint_counter.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let mut id = String::with_capacity(prefix.len() + 11);
        id.push_str(prefix);
        for _ in 0..5 {
            hash = hash
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .wrapping_add(0xBF58_476D_1CE4_E5B9);
            let mixed = (hash >> 29) ^ hash;
            id.push(char::from(
                ID_ALPHABET[(mixed % ID_ALPHABET.len() as u64) as usize],
            ));
        }
        let base = ID_ALPHABET.len() as u64;
        let mut counter = self.mint_counter;
        let mut digits = Vec::new();
        loop {
            digits.push(ID_ALPHABET[(counter % base) as usize]);
            counter /= base;
            if counter == 0 {
                break;
            }
        }
        digits.reverse();
        for digit in digits {
            id.push(char::from(digit));
        }
        id
    }

    /// Apply every transition the clock had already made due at `now`, and drop
    /// the resources whose deletion completed.
    ///
    /// Every handler calls this before it reads or writes, so a reporting
    /// operation reports the state the clock owed at the instant it ran — and
    /// nothing else.
    pub fn materialize(&mut self, now: SystemTime) {
        self.applications
            .retain(|_, application| !application.advance(now));
        self.stream_groups.retain(|_, group| !group.advance(now));
        self.forget_dead_idempotency_keys();
    }

    /// Drop the idempotency entries whose resource is gone.
    ///
    /// A `ClientToken` can only answer with the resource it created, so an entry
    /// whose resource has been deleted can never be used again. Releasing it here
    /// means the table is bounded by the LIVE resources rather than by the number
    /// of creates this emulator has ever served: it shrinks on exactly the
    /// caller-driven path (`Delete*`, then this) that releases the resources.
    fn forget_dead_idempotency_keys(&mut self) {
        let applications = &self.applications;
        let stream_groups = &self.stream_groups;
        self.idempotency
            .retain(|key, id| match key.split_once(':').map(|(name, _)| name) {
                Some("CreateApplication") => applications.contains_key(id),
                Some("CreateStreamGroup") => stream_groups.contains_key(id),
                Some("StartStreamSession") => stream_groups
                    .values()
                    .any(|group| group.sessions.contains_key(id)),
                // An operation this slice records nothing for: there is nothing to
                // judge the entry by, so leave it rather than drop it blindly.
                _ => true,
            });
    }

    /// The stored application an `Identifier` names, by bare id or by ARN.
    pub fn application(&self, identifier: &str) -> Option<&Application> {
        self.applications
            .get(&identifier_id(identifier, APPLICATION_RESOURCE)?)
    }

    /// The stored application an `Identifier` names, for mutation.
    pub fn application_mut(&mut self, identifier: &str) -> Option<&mut Application> {
        let id = identifier_id(identifier, APPLICATION_RESOURCE)?;
        self.applications.get_mut(&id)
    }

    /// The stored stream group an `Identifier` names, by bare id or by ARN.
    pub fn stream_group(&self, identifier: &str) -> Option<&StreamGroup> {
        self.stream_groups
            .get(&identifier_id(identifier, STREAM_GROUP_RESOURCE)?)
    }

    /// The stored stream group an `Identifier` names, for mutation.
    pub fn stream_group_mut(&mut self, identifier: &str) -> Option<&mut StreamGroup> {
        let id = identifier_id(identifier, STREAM_GROUP_RESOURCE)?;
        self.stream_groups.get_mut(&id)
    }

    /// The tags of the taggable resource an `Arn` names.
    ///
    /// "You can assign tags to the following Amazon GameLift Streams resource
    /// types: Application StreamGroup" (`TagResource`'s documentation), so the ARN
    /// is resolved against those two stores and nothing else: an ARN of any other
    /// resource type (a stream session, say) names nothing taggable and answers
    /// `None`, as does an ARN whose resource this store no longer holds.
    ///
    /// An `Arn` is always the full ARN form (`pattern
    /// ^arn:aws:gameliftstreams:([^: ]*):([0-9]{12}):([^: ]*)$`) — unlike an
    /// `Identifier`, a bare id is not one, so a non-ARN resolves nothing.
    pub fn tags_of(&self, resource_arn: &str) -> Option<&HashMap<String, String>> {
        if !resource_arn.starts_with("arn:") {
            return None;
        }
        if let Some(application) = self.application(resource_arn) {
            return Some(&application.tags);
        }
        self.stream_group(resource_arn).map(|group| &group.tags)
    }

    /// The tags of the taggable resource an `Arn` names, for mutation — the same
    /// resolution as [`Self::tags_of`], for the operations that assign or remove
    /// tags rather than report them.
    pub fn tags_of_mut(&mut self, resource_arn: &str) -> Option<&mut HashMap<String, String>> {
        if !resource_arn.starts_with("arn:") {
            return None;
        }
        // Which store holds it is decided by an immutable lookup first, so only one
        // mutable borrow of `self` is ever live.
        if self.application(resource_arn).is_some() {
            return self
                .application_mut(resource_arn)
                .map(|application| &mut application.tags);
        }
        self.stream_group_mut(resource_arn)
            .map(|group| &mut group.tags)
    }

    /// The id of the resource a replayed `ClientToken` already created, if the
    /// resource is still there.
    pub fn replayed(&self, operation: &str, client_token: Option<&str>) -> Option<String> {
        self.idempotency
            .get(&idempotency_key(operation, client_token?))
            .cloned()
    }

    /// Record which resource a `ClientToken` created.
    pub fn remember(&mut self, operation: &str, client_token: Option<&str>, id: &str) {
        if let Some(token) = client_token {
            self.idempotency
                .insert(idempotency_key(operation, token), id.to_string());
        }
    }
}

fn idempotency_key(operation: &str, client_token: &str) -> String {
    format!("{operation}:{client_token}")
}

/// One page of a `List*` operation over `ids`, and the `NextToken` that continues
/// it.
///
/// The ids are sorted, so `after` — the last id of the previous page — is all the
/// token has to carry: a page boundary stays correct when the store changes between
/// calls, which an offset could not promise. `None` for the token means this was
/// the last page. Shared by every paginated `List*` so the two pagers cannot drift.
pub fn page_of_ids(
    mut ids: Vec<String>,
    after: &str,
    page_size: usize,
) -> (Vec<String>, Option<String>) {
    ids.sort();
    let mut page: Vec<String> = ids
        .into_iter()
        .filter(|id| after.is_empty() || id.as_str() > after)
        .take(page_size + 1)
        .collect();
    if page.len() > page_size {
        page.truncate(page_size);
        let next = page.last().cloned();
        (page, next)
    } else {
        (page, None)
    }
}

/// The resource id an `Identifier` names.
///
/// The model's `Identifier` shape accepts either form for the same resource
/// (`pattern ^(^[a-zA-Z0-9-]+$)|(^arn:aws:gameliftstreams:([^: ]*):([0-9]{12}):([^: ]*)$)$`),
/// so a bare id is returned as given and an ARN is resolved to the last segment of
/// its resource path — but only when that path is of `resource_type`, so an
/// application ARN never resolves a stream group.
pub fn identifier_id(identifier: &str, resource_type: &str) -> Option<String> {
    if !identifier.starts_with("arn:") {
        return Some(identifier.to_string());
    }
    let mut fields = identifier.splitn(6, ':');
    if fields.next() != Some("arn") {
        return None;
    }
    let _partition = fields.next()?;
    if fields.next() != Some(SIGNING_NAME) {
        return None;
    }
    let _region = fields.next()?;
    let _account = fields.next()?;
    let path = fields
        .next()?
        .strip_prefix(resource_type)?
        .strip_prefix('/')?;
    path.split('/').next_back().map(str::to_string)
}

/// The ARN of an application: `arn:aws:gameliftstreams:<region>:<account>:application/<id>`.
pub fn application_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        SIGNING_NAME,
        region,
        &format!("{APPLICATION_RESOURCE}/{id}"),
    )
}

/// The ARN of a stream group: `arn:aws:gameliftstreams:<region>:<account>:streamgroup/<id>`.
pub fn stream_group_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(
        SIGNING_NAME,
        region,
        &format!("{STREAM_GROUP_RESOURCE}/{id}"),
    )
}

/// The ARN of a stream session, which carries the hosting stream group:
/// `arn:aws:gameliftstreams:<region>:<account>:streamsession/<group id>/<session id>`.
pub fn stream_session_arn(region: &str, group_id: &str, session_id: &str) -> String {
    ::engine::emulator_arn(
        SIGNING_NAME,
        region,
        &format!("{STREAM_SESSION_RESOURCE}/{group_id}/{session_id}"),
    )
}

/// The region a request is answered in: the region of its SigV4 credential scope,
/// or [`DEFAULT_REGION`] when it carries none.
pub fn region_of(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// A `SystemTime` as the `Timestamp` the wire carries.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsGameliftstreamsGameliftstreamsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsGameliftstreamsGameliftstreamsState>)
}
