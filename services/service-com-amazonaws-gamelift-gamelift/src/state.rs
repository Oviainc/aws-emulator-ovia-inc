// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-gamelift-gamelift service.
//!
//! The hosting resources this emulator persists — fleets, the aliases that route at
//! them, the game sessions placed on them, and the player slots reserved in those
//! sessions — plus the behaviour every handler shares over them: resolving a fleet, an
//! alias or a game session by id or ARN, materialising the status changes the clock has
//! already made due, minting ids and ARNs in the formats the model documents, and the
//! opaque pagination token the paginated describes share.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    Alias, AnywhereConfiguration, CertificateConfiguration, FilterConfiguration, FleetAttributes,
    GameProperty, GameSession, GameSessionQueue, GameSessionQueueDestination, IpPermission,
    LocationState, PlayerGatewayConfiguration, PlayerLatencyPolicy, PlayerSession,
    PriorityConfiguration, ResourceCreationLimitPolicy, RoutingStrategy, RuntimeConfiguration, Tag,
};

/// `ComputeType` values the model declares (`smithy_query shape=ComputeType`).
pub const COMPUTE_TYPE_EC2: &str = "EC2";
/// See [`COMPUTE_TYPE_EC2`].
pub const COMPUTE_TYPE_ANYWHERE: &str = "ANYWHERE";

/// `RoutingStrategyType` values the model declares (`smithy_query
/// shape=RoutingStrategyType`), i.e. the two routings an alias can carry.
pub const ROUTING_STRATEGY_TYPE_SIMPLE: &str = "SIMPLE";
/// See [`ROUTING_STRATEGY_TYPE_SIMPLE`].
pub const ROUTING_STRATEGY_TYPE_TERMINAL: &str = "TERMINAL";

/// `FleetStatus` values this slice uses (`smithy_query shape=FleetStatus`).
pub const FLEET_STATUS_NEW: &str = "NEW";
/// See [`FLEET_STATUS_NEW`].
pub const FLEET_STATUS_ACTIVE: &str = "ACTIVE";
/// See [`FLEET_STATUS_NEW`].
pub const FLEET_STATUS_DELETING: &str = "DELETING";

/// `GameSessionStatus` values this slice uses (`smithy_query shape=GameSessionStatus`).
pub const GAME_SESSION_STATUS_ACTIVATING: &str = "ACTIVATING";
/// See [`GAME_SESSION_STATUS_ACTIVATING`].
pub const GAME_SESSION_STATUS_ACTIVE: &str = "ACTIVE";

/// `PlayerSessionStatus` value a freshly reserved slot carries
/// (`smithy_query shape=PlayerSessionStatus`).
pub const PLAYER_SESSION_STATUS_RESERVED: &str = "RESERVED";

/// `PlayerSessionCreationPolicy` a new game session carries: CreateGameSession's own
/// doc — "By default, newly created game sessions are open to new players."
pub const PLAYER_SESSION_CREATION_POLICY_ACCEPT_ALL: &str = "ACCEPT_ALL";

/// `ProtectionPolicy` default: CreateFleet's `NewGameSessionProtectionPolicy` doc —
/// "By default, this property is set to NoProtection."
pub const PROTECTION_POLICY_NO_PROTECTION: &str = "NoProtection";

/// `FleetType` default: CreateFleet's `FleetType` doc — "By default, this property is
/// set to ON_DEMAND."
pub const FLEET_TYPE_ON_DEMAND: &str = "ON_DEMAND";

/// `CertificateConfiguration.CertificateType` that puts a fleet behind TLS, which
/// decides which of the two documented `DnsName` forms a game session reports.
pub const CERTIFICATE_TYPE_GENERATED: &str = "GENERATED";

/// `CertificateConfiguration.CertificateType` default: that member's own doc —
/// "DISABLED - (default) Do not generate a TLS/SSL certificate for this fleet." A fleet
/// created without a `CertificateConfiguration` carries this one, so every fleet reports
/// the member the service decided for it.
pub const CERTIFICATE_TYPE_DISABLED: &str = "DISABLED";

/// `PlayerGatewayMode` default: CreateFleet's `PlayerGatewayMode` doc — "`DISABLED`
/// (default) -- Game clients connect to the game server endpoint."
pub const PLAYER_GATEWAY_MODE_DISABLED: &str = "DISABLED";

/// The CloudWatch metric group a fleet created without one is added to.
///
/// `FleetAttributes$MetricGroups`' doc: "Name of a metric group that metrics for this
/// fleet are added to. [...] A fleet can be included in only one metric group at a
/// time" — so a fleet is always in exactly one group, and a `CreateFleet` that names
/// none leaves the fleet in the service's own group. The model does not name that group;
/// the recorded wire responses of this service report it as `default`.
pub const DEFAULT_METRIC_GROUP: &str = "default";

/// The upper bound on `MaximumPlayerSessionCount`. CreatePlayerSession's doc: "The
/// maximum number of players per game session is 200. It is not adjustable." The model
/// leaves the member unbounded above, so the service rule is enforced here.
pub const MAX_PLAYERS_PER_GAME_SESSION: i32 = 200;

/// How many server processes one fleet compute may run at once, and so how many game
/// sessions it can host: `RuntimeConfiguration`'s doc — "An Amazon GameLift Servers
/// instance is limited to 50 processes running concurrently. To calculate the total
/// number of processes defined in a runtime configuration, add the values of the
/// `ConcurrentExecutions` parameter for each server process" — and each running server
/// process hosts one game session at a time.
pub const MAX_SERVER_PROCESSES_PER_INSTANCE: i32 = 50;

/// How many fleets this emulator holds before `CreateFleet` answers the modelled
/// `LimitExceededException` ("The requested operation would cause the resource to exceed
/// the allowed service limit").
///
/// Amazon GameLift Servers caps active fleets per account per Region; this emulator
/// keeps ONE store for the account rather than one per Region — a fleet is reported by
/// every describe whatever Region the request is signed for, and carries the Region of
/// the request that created it in its ARN and home location — so this cap, like the
/// store, is account-wide. The model carries no quota, so the count is CHOSEN: far above
/// any realistic script, and the bound that keeps the fleet store (and, through each
/// fleet's capacity, the game-session and player-session stores) from growing without
/// limit.
pub const MAX_FLEETS_PER_ACCOUNT: usize = 100;

/// How many fleet identifiers one `DescribeFleetAttributes` request may name.
///
/// That operation's own doc: "Some API operations limit the number of fleet IDs that are
/// allowed in one request. If a request exceeds this limit, the request fails and the
/// error message contains the maximum allowed number." The model bounds
/// `FleetIdOrArnList` only below (`len 1..`) and the number itself is not documented, so
/// this count is CHOSEN: it is [`MAX_FLEETS_PER_ACCOUNT`], the most distinct fleets that
/// can exist, so no legitimate request is refused — and naming more than that can only
/// be repetition, which would otherwise let one request drive an unbounded response.
pub const MAX_FLEET_IDS_PER_REQUEST: usize = MAX_FLEETS_PER_ACCOUNT;

/// How many aliases this emulator holds before `CreateAlias` answers the modelled
/// `LimitExceededException` ("The requested operation would cause the resource to exceed
/// the allowed service limit").
///
/// Like [`MAX_FLEETS_PER_ACCOUNT`], Amazon GameLift Servers caps aliases per account per
/// Region while this emulator keeps ONE store for the account — an alias is reported
/// whatever Region the request is signed for, and carries the Region of the request that
/// created it in its ARN — so the cap is account-wide. The model carries no quota, so the
/// count is CHOSEN: far above any realistic script ("a fleet can have multiple aliases"),
/// and the bound that keeps the alias store from growing without limit.
pub const MAX_ALIASES_PER_ACCOUNT: usize = 100;

/// How many game session queues this emulator holds before `CreateGameSessionQueue`
/// answers the modelled `LimitExceededException` ("The requested operation would cause
/// the resource to exceed the allowed service limit").
///
/// Like [`MAX_FLEETS_PER_ACCOUNT`], Amazon GameLift Servers caps queues per account per
/// Region while this emulator keeps ONE store for the account, so the cap is
/// account-wide. The model carries no quota, so the count is CHOSEN: far above any
/// realistic script, and the bound that keeps the queue store from growing without limit.
pub const MAX_GAME_SESSION_QUEUES_PER_ACCOUNT: usize = 50;

/// How many queue identifiers one `DescribeGameSessionQueues` request may name.
///
/// The model bounds `GameSessionQueueNameOrArnList` neither above nor below, and each
/// entry that is an ARN is resolved by a scan of the queue store, so the work one request
/// can ask for is the PRODUCT of the list length and the stored queue count. This count
/// is CHOSEN, exactly as [`MAX_FLEET_IDS_PER_REQUEST`] is: it is
/// [`MAX_GAME_SESSION_QUEUES_PER_ACCOUNT`], the most distinct queues that can exist, so
/// no legitimate request is refused — and naming more than that can only be repetition.
pub const MAX_QUEUE_NAMES_PER_REQUEST: usize = MAX_GAME_SESSION_QUEUES_PER_ACCOUNT;

/// How many destinations one game session queue may list.
///
/// `GameSessionQueueDestinationList` carries no modelled length, and a stored queue holds
/// the list verbatim for as long as the queue exists, so this count is CHOSEN: far above
/// any realistic queue — a destination is a fleet or alias of the account, which
/// [`MAX_FLEETS_PER_ACCOUNT`] already bounds — and the bound that keeps one request from
/// deciding how much a stored record occupies.
pub const MAX_QUEUE_DESTINATIONS: usize = MAX_FLEETS_PER_ACCOUNT;

/// How many player latency policies one game session queue may list.
///
/// `PlayerLatencyPolicyList` carries no modelled length either, and the policies are
/// "evaluated in order starting with the lowest maximum latency value" to "gradually
/// relax the cap over time", so a queue needs a handful. This count is CHOSEN for the
/// same reason as [`MAX_QUEUE_DESTINATIONS`]: it bounds what one request can make a
/// stored record occupy.
pub const MAX_QUEUE_PLAYER_LATENCY_POLICIES: usize = 10;

/// The bounds a game session queue's `TimeoutInSeconds` must lie within.
///
/// `CreateGameSessionQueue$TimeoutInSeconds` and `GameSessionQueue$TimeoutInSeconds`
/// carry the same note: "The minimum value is 10 and the maximum value is 600." The
/// modelled `WholeNumber` range bounds the member only below (`0..`), so the documented
/// bounds are enforced here.
pub const QUEUE_TIMEOUT_RANGE_SECONDS: (i32, i32) = (10, 600);

/// The `TimeoutInSeconds` a queue created without one carries.
///
/// `CreateGameSessionQueue$TimeoutInSeconds`' doc: "If you don't specify a request
/// timeout, the queue uses a default value." Neither the model nor the docs name that
/// value, so it is CHOSEN: the documented maximum
/// ([`QUEUE_TIMEOUT_RANGE_SECONDS`]`.1`), the most permissive setting, which keeps a
/// placement request queued for as long as the service allows before it times out.
pub const DEFAULT_QUEUE_TIMEOUT_SECONDS: i32 = QUEUE_TIMEOUT_RANGE_SECONDS.1;

/// The port range a fleet that opens no inbound permissions draws game-session ports
/// from, and the range an opened one must lie within. `IpPermission$FromPort`'s doc:
/// "For fleets using Windows builds, only ports `1026-60000` are valid."
pub const DEFAULT_PORT_RANGE: (i32, i32) = (1026, 60000);

/// The one port outside [`DEFAULT_PORT_RANGE`] a fleet may open. Same doc: "For fleets
/// using Linux builds, only ports `22` and `1026-60000` are valid."
pub const REMOTE_ACCESS_PORT: i32 = 22;

/// Whether `from`-`to` is a port range a fleet may open: it must run forwards and lie
/// within [`DEFAULT_PORT_RANGE`], or be [`REMOTE_ACCESS_PORT`] alone.
pub fn valid_port_range(from: i32, to: i32) -> bool {
    from <= to
        && ((from == REMOTE_ACCESS_PORT && to == REMOTE_ACCESS_PORT)
            || (from >= DEFAULT_PORT_RANGE.0 && to <= DEFAULT_PORT_RANGE.1))
}

/// The `GameSessionStatus` values `DescribeGameSessions`'s `StatusFilter` accepts. Its
/// doc: "You can filter on the following states: `ACTIVE`, `TERMINATED`, `ACTIVATING`,
/// and `TERMINATING`."
pub const STATUS_FILTER_VALUES: [&str; 4] = ["ACTIVE", "TERMINATED", "ACTIVATING", "TERMINATING"];

/// The region an unsigned request (no SigV4 credential scope to read a region from) is
/// served in.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long after the call that starts it a status change this service owes comes due:
/// `CreateFleet` places a managed EC2 fleet in `NEW` and it becomes `ACTIVE` when the
/// fleet-creation workflow finishes; `CreateGameSession` returns a session in
/// `ACTIVATING` and "when the game session status is `ACTIVE`, it is updated with
/// connection information"; `DeleteFleet` moves a fleet to `DELETING` until the
/// deletion completes.
///
/// Amazon GameLift Servers takes minutes over each of those and documents no duration,
/// so this span is CHOSEN: short enough for a caller to wait for, and long enough that
/// a caller acting straight after the call still observes the transient status and the
/// state errors it causes.
pub const WORKFLOW_DELAY: Duration = Duration::from_millis(400);

/// A fresh opaque id of the shape AWS mints for a GameLift resource: a UUID, which the
/// modelled patterns accept (`FleetId` `^[a-z]*fleet-[a-zA-Z0-9\-]+$`, `PlayerSessionId`
/// `^psess-\S+$`, `IdStringModel` `^[a-zA-Z0-9-]+$`) and the docs show
/// (`arn:aws:gamelift:<region>::fleet/fleet-a1234567-...`).
///
/// The UUID is a digest of this store's id counter — unique within the process — folded
/// with a per-process seed so two emulator processes do not mint the same ids, hashed so
/// the id discloses neither. It takes the counter rather than `&mut self` so a handler
/// holding a stored record borrowed can still mint, which is what lets it mint only
/// AFTER every refusal has been answered. This mirrors `mint_workspace_id` in the
/// Amazon Managed Prometheus slice rather than adding a dependency for it.
pub fn mint_uuid(counter: &mut u64, kind: &str) -> String {
    *counter += 1;
    let mut hasher = Sha256::new();
    hasher.update(kind.as_bytes());
    hasher.update(counter.to_be_bytes());
    hasher.update(process_seed().to_be_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    let hex = hex::encode(digest);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// A seed drawn once per process, so ids minted by two emulator processes from the same
/// counter still differ.
fn process_seed() -> u128 {
    static SEED: OnceLock<u128> = OnceLock::new();
    *SEED.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or_default()
    })
}

/// An ARN for a GameLift resource: `arn:aws:gamelift:<region>:<account>:<resource>`.
///
/// Every segment the model documents for `FleetAttributes$FleetArn` ("Format is
/// `arn:aws:gamelift:<region>::fleet/fleet-a1234567-...`") and for
/// `GameSession$GameSessionId` ("`arn:aws:gamelift:<home_region>::gamesession/<fleet
/// ID>/<ID string>`"), with the account segment POPULATED: those prose examples render
/// the account empty, but the service's own responses carry the account id of the
/// calling account, and an ARN is only usable as an IAM resource — or splittable back
/// into its owning account — when that segment is there. So this delegates to
/// [`::engine::emulator_arn`], the single source of the one account every emulated
/// response presents.
pub fn gamelift_arn(region: &str, resource: &str) -> String {
    ::engine::emulator_arn("gamelift", region, resource)
}

/// The region the caller addressed, read from the request's SigV4 credential scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// A modelled timestamp for a wall-clock instant.
///
/// `aws_smithy_types` owns the epoch handling: this is its own `From<SystemTime>`
/// conversion, named here only so every record renders its instants the one way.
pub fn to_date_time(instant: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(instant)
}

/// Split a `<kind>IdOrArn` member into its id and its ARN, whichever of the two the
/// caller supplied: the model's ARN docs say "the resource ID matches the `<kind>Id`
/// value", so each is derivable from the other.
pub fn id_and_arn(value: &str, region: &str, kind: &str) -> (String, String) {
    if value.starts_with("arn:") {
        let id = value.rsplit('/').next().unwrap_or(value).to_string();
        (id, value.to_string())
    } else {
        let arn = gamelift_arn(region, &format!("{kind}/{value}"));
        (value.to_string(), arn)
    }
}

/// Which collection a page token walks. Mixed into the token so one paginated describe
/// refuses a token another minted, instead of reading it as an offset into its own set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageKind {
    /// `DescribeFleetAttributes`.
    Fleets,
    /// `DescribeGameSessions`.
    GameSessions,
    /// `DescribeGameSessionQueues`.
    GameSessionQueues,
}

impl PageKind {
    /// The discriminator this collection's tokens carry.
    const fn tag(self) -> char {
        match self {
            Self::Fleets => 'f',
            Self::GameSessions => 'g',
            Self::GameSessionQueues => 'q',
        }
    }
}

/// Encode a page boundary as the opaque `NextToken` the paginated describes hand out.
///
/// A collection tag plus fixed-width hex, so a token carries no readable offset and
/// every token has the same shape; [`decode_page_token`] is the only reader.
pub fn encode_page_token(kind: PageKind, offset: usize) -> String {
    format!("{}{offset:015x}", kind.tag())
}

/// Decode a token [`encode_page_token`] produced for `kind`. `None` for anything else —
/// including a token of another collection — which a handler answers with
/// `InvalidRequestException`.
pub fn decode_page_token(kind: PageKind, token: &str) -> Option<usize> {
    let offset = token.strip_prefix(kind.tag())?;
    if offset.len() != 15 {
        return None;
    }
    usize::from_str_radix(offset, 16).ok()
}

/// One page of `items`, starting at `next_token`'s boundary and at most `limit` long,
/// plus the token for the page after it.
///
/// `None` means the token was not one this service minted for `kind`.
pub fn paginate<T: Clone>(
    kind: PageKind,
    items: &[T],
    limit: Option<i32>,
    next_token: Option<&str>,
) -> Option<(Vec<T>, Option<String>)> {
    let start = match next_token {
        Some(token) => decode_page_token(kind, token)?.min(items.len()),
        None => 0,
    };
    // `Limit` is modelled `PositiveInteger` with range min 1, so the boundary walk has
    // already refused 0 and below; absent means "no page size", i.e. the whole tail.
    let page_size = limit.map_or(items.len(), |limit| limit as usize);
    let end = start.saturating_add(page_size).min(items.len());
    let page = items[start..end].to_vec();
    let next = if end < items.len() {
        Some(encode_page_token(kind, end))
    } else {
        None
    };
    Some((page, next))
}

/// A fleet of compute resources, as `CreateFleet` persisted it.
#[derive(Debug, Clone)]
pub struct Fleet {
    pub fleet_id: String,
    pub fleet_arn: String,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    /// When `NEW` becomes `ACTIVE`; `None` once the fleet is no longer provisioning.
    pub active_at: Option<SystemTime>,
    /// When a `DELETING` fleet and everything placed on it leave state.
    pub purge_at: Option<SystemTime>,
    pub fleet_type: String,
    pub instance_type: Option<String>,
    pub compute_type: String,
    pub build_id: Option<String>,
    pub build_arn: Option<String>,
    pub script_id: Option<String>,
    pub script_arn: Option<String>,
    pub server_launch_path: Option<String>,
    pub server_launch_parameters: Option<String>,
    pub log_paths: Option<Vec<String>>,
    pub runtime_configuration: Option<RuntimeConfiguration>,
    pub resource_creation_limit_policy: Option<ResourceCreationLimitPolicy>,
    pub certificate_configuration: Option<CertificateConfiguration>,
    pub anywhere_configuration: Option<AnywhereConfiguration>,
    pub player_gateway_mode: Option<String>,
    pub player_gateway_configuration: Option<PlayerGatewayConfiguration>,
    pub instance_role_arn: Option<String>,
    pub instance_role_credentials_provider: Option<String>,
    pub ec2_inbound_permissions: Vec<IpPermission>,
    pub new_game_session_protection_policy: String,
    pub metric_groups: Vec<String>,
    /// The fleet's home location followed by every remote location it was created with.
    pub locations: Vec<FleetLocation>,
    pub home_location: String,
    pub region: String,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub termination_time: Option<SystemTime>,
    /// The address of the instance hosting this fleet's game servers.
    pub ip_address: String,
    /// Where the next port handout starts: a rotating cursor over the ports this fleet
    /// opens, advanced past every port it hands out and never rewound, so consecutive
    /// sessions get consecutive ports (see [`Fleet::next_game_session_port`]).
    pub port_cursor: u32,
}

/// One location a fleet deploys to, and that location's own life-cycle status.
#[derive(Debug, Clone)]
pub struct FleetLocation {
    pub location: String,
    pub status: String,
    /// How many server processes are running at this location and available to host a
    /// game session, i.e. this location's game-session capacity. See
    /// [`server_processes_per_location`] for where the number comes from.
    pub server_processes: usize,
}

/// How many server processes a fleet location runs, and so how many game sessions that
/// location can host at once.
///
/// A managed (`EC2`) fleet is hosted on instances Amazon GameLift Servers provisions and
/// launches the fleet's server processes on itself, so its locations run the processes
/// the fleet's `RuntimeConfiguration` asks for — `RuntimeConfiguration`'s doc: "To
/// calculate the total number of processes defined in a runtime configuration, add the
/// values of the `ConcurrentExecutions` parameter for each server process" — capped at
/// the [`MAX_SERVER_PROCESSES_PER_INSTANCE`] one instance may run, and given that same
/// per-instance maximum when the fleet carries no runtime configuration, which
/// `CreateFleet`'s member doc allows. `ComputeType` declares exactly `EC2` and `ANYWHERE`
/// (`smithy_query shape=ComputeType`), and `CreateFleet` applies the managed-fleet rules
/// to everything that is not `ANYWHERE`, so this does too: the two stay in step.
///
/// An `ANYWHERE` fleet runs NO server process of its own. Its game servers run on compute
/// resources the customer registers (`RegisterCompute`'s doc: "Registers a compute
/// resource in an Amazon GameLift Servers Anywhere fleet"), and each server process on
/// such a compute becomes active and available only by announcing itself through the
/// server SDK ("Each server process includes the SDK endpoint in its call to the Amazon
/// GameLift Servers server SDK action `InitSDK()`") — a game-server-side protocol, not an
/// API this emulator serves. So no process on an Anywhere fleet location is ever active
/// and available here, its capacity is zero, and `CreateGameSession` against it answers
/// the modelled `FleetCapacityExceededException`, as the real service does for an
/// Anywhere fleet with no active, available server process.
pub fn server_processes_per_location(compute_type: &str, configured_processes: i32) -> usize {
    if compute_type == COMPUTE_TYPE_ANYWHERE {
        return 0;
    }
    let processes = if configured_processes > 0 {
        configured_processes.min(MAX_SERVER_PROCESSES_PER_INSTANCE)
    } else {
        MAX_SERVER_PROCESSES_PER_INSTANCE
    };
    usize::try_from(processes).unwrap_or(0)
}

/// The total `ConcurrentExecutions` a runtime configuration asks for, saturating rather
/// than overflowing on a sum no instance could run (the caller refuses it).
pub fn configured_server_processes(configuration: Option<&RuntimeConfiguration>) -> i32 {
    configuration
        .and_then(|configuration| configuration.server_processes.as_ref())
        .map_or(0, |processes| {
            processes.iter().fold(0_i32, |total, process| {
                total.saturating_add(process.concurrent_executions)
            })
        })
}

impl Fleet {
    /// This fleet as the modelled `FleetAttributes`.
    pub fn attributes(&self) -> FleetAttributes {
        FleetAttributes {
            fleet_id: Some(self.fleet_id.clone()),
            fleet_arn: Some(self.fleet_arn.clone()),
            fleet_type: Some(self.fleet_type.clone()),
            instance_type: self.instance_type.clone(),
            description: self.description.clone(),
            name: Some(self.name.clone()),
            creation_time: Some(to_date_time(self.creation_time)),
            termination_time: self.termination_time.map(to_date_time),
            status: Some(self.status.clone()),
            build_id: self.build_id.clone(),
            build_arn: self.build_arn.clone(),
            script_id: self.script_id.clone(),
            script_arn: self.script_arn.clone(),
            server_launch_path: self.server_launch_path.clone(),
            server_launch_parameters: self.server_launch_parameters.clone(),
            log_paths: self.log_paths.clone(),
            new_game_session_protection_policy: Some(
                self.new_game_session_protection_policy.clone(),
            ),
            // `OperatingSystem` reports the build's OS, and builds are a separate slice
            // that still answers 501, so there is nothing stored to report here.
            operating_system: None,
            resource_creation_limit_policy: self.resource_creation_limit_policy.clone(),
            metric_groups: Some(self.metric_groups.clone()),
            // Fleet actions are started and stopped by StartFleetActions /
            // StopFleetActions, which are a separate slice: nothing has stopped.
            stopped_actions: None,
            instance_role_arn: self.instance_role_arn.clone(),
            certificate_configuration: self.certificate_configuration.clone(),
            compute_type: Some(self.compute_type.clone()),
            anywhere_configuration: self.anywhere_configuration.clone(),
            instance_role_credentials_provider: self.instance_role_credentials_provider.clone(),
            player_gateway_mode: self.player_gateway_mode.clone(),
            player_gateway_configuration: self.player_gateway_configuration.clone(),
        }
    }

    /// This fleet's locations and the life-cycle status of each.
    pub fn location_states(&self) -> Vec<LocationState> {
        self.locations
            .iter()
            .map(|location| LocationState {
                location: Some(location.location.clone()),
                status: Some(location.status.clone()),
                player_gateway_status: None,
            })
            .collect()
    }

    /// Whether Amazon GameLift Servers owns and runs this fleet's compute.
    ///
    /// `ComputeType`'s doc distinguishes the two: an `EC2` fleet's instances are managed
    /// by the service, while an `ANYWHERE` fleet "represents compute resources that are
    /// not owned or managed by Amazon GameLift Servers". Deleting a fleet "shuts down
    /// any currently running fleet instances", so only a fleet WITH managed instances
    /// has anything to shut down — which is what makes its deletion take time.
    pub fn hosts_managed_instances(&self) -> bool {
        self.compute_type != COMPUTE_TYPE_ANYWHERE
    }

    /// Whether this fleet deploys to `location`.
    pub fn has_location(&self, location: &str) -> bool {
        self.locations
            .iter()
            .any(|candidate| candidate.location == location)
    }

    /// How many game sessions this fleet can host at once in `location`.
    ///
    /// Each active, available server process hosts one game session at a time, so the
    /// capacity of a fleet location is the number of such processes running there (see
    /// [`server_processes_per_location`]). A location this fleet does not deploy to, and
    /// one whose own life cycle has not reached `ACTIVE` — so its processes are not
    /// running yet — has none.
    ///
    /// The ports the fleet opens bound its live sessions too, but they are opened for the
    /// WHOLE fleet rather than per location, so that bound is [`open_port_count`] against
    /// the fleet's live sessions and is applied beside this one.
    ///
    /// [`open_port_count`]: Fleet::open_port_count
    pub fn game_session_capacity(&self, location: &str) -> usize {
        self.locations
            .iter()
            .find(|known| known.location == location)
            .filter(|known| known.status == FLEET_STATUS_ACTIVE)
            .map_or(0, |known| known.server_processes)
    }

    /// The inbound port ranges this fleet opens, lowest first and with overlaps merged,
    /// or [`DEFAULT_PORT_RANGE`] when it opens none.
    ///
    /// `CreateFleet` refuses a range outside [`DEFAULT_PORT_RANGE`] (see
    /// [`valid_port_range`]), so a stored range always runs forwards; a record that
    /// somehow held one that does not is dropped here rather than trusted.
    pub fn open_port_ranges(&self) -> Vec<(i32, i32)> {
        let mut ranges: Vec<(i32, i32)> = self
            .ec2_inbound_permissions
            .iter()
            .map(|permission| (permission.from_port, permission.to_port))
            .filter(|(from, to)| from <= to)
            .collect();
        if ranges.is_empty() {
            return vec![DEFAULT_PORT_RANGE];
        }
        ranges.sort_unstable();
        let mut merged: Vec<(i32, i32)> = Vec::with_capacity(ranges.len());
        for (from, to) in ranges {
            match merged.last_mut() {
                // `from - 1` cannot underflow: a stored port is positive.
                Some(last) if from <= last.1.saturating_add(1) => last.1 = last.1.max(to),
                _ => merged.push((from, to)),
            }
        }
        merged
    }

    /// How many distinct ports this fleet opens, i.e. how many game sessions can listen
    /// on it at once.
    pub fn open_port_count(&self) -> usize {
        self.open_port_ranges().iter().map(port_span).sum()
    }

    /// The next port a game session on this fleet listens on: the first port the fleet
    /// opens that no live session already holds, starting the walk where the last
    /// handout left off so consecutive sessions get consecutive ports.
    ///
    /// `None` only when every opened port is taken, which `CreateGameSession`'s port
    /// bound — the fleet's live sessions against its [`open_port_count`] — keeps
    /// unreachable for a session the fleet admitted.
    ///
    /// [`open_port_count`]: Fleet::open_port_count
    pub fn next_game_session_port(
        &mut self,
        taken: &HashSet<i32>,
    ) -> Option<i32> {
        let ranges = self.open_port_ranges();
        let total: usize = ranges.iter().map(port_span).sum();
        if total == 0 {
            return None;
        }
        let start = usize::try_from(self.port_cursor).unwrap_or(0) % total;
        let ports = || ranges.iter().flat_map(|&(from, to)| from..=to);
        // The two passes chained, skipped to `start` and cut at `total`, are the opened
        // ports in order starting from where the last handout left off.
        for (step, port) in ports().chain(ports()).skip(start).take(total).enumerate() {
            if !taken.contains(&port) {
                let advance = u32::try_from(step).unwrap_or(0).saturating_add(1);
                self.port_cursor = self.port_cursor.wrapping_add(advance);
                return Some(port);
            }
        }
        None
    }

    /// The DNS name of the instance hosting this fleet's game servers, in whichever of
    /// the two forms `GameSession$DnsName` documents applies: a TLS-enabled fleet
    /// reports `<unique identifier>.<region identifier>.amazongamelift.com`, a
    /// non-TLS-enabled one `ec2-<unique identifier>.compute.amazonaws.com`, where the
    /// unique identifier of an EC2 instance is its address.
    pub fn dns_name(&self) -> String {
        let tls_enabled = self
            .certificate_configuration
            .as_ref()
            .is_some_and(|config| config.certificate_type == CERTIFICATE_TYPE_GENERATED);
        if tls_enabled {
            format!("{}.{}.amazongamelift.com", self.fleet_id, self.region)
        } else {
            format!(
                "ec2-{}.compute.amazonaws.com",
                self.ip_address.replace('.', "-")
            )
        }
    }
}

/// How many ports an inbound range covers, saturating rather than overflowing on bounds
/// a stored record should never hold.
fn port_span(range: &(i32, i32)) -> usize {
    range
        .1
        .checked_sub(range.0)
        .and_then(|width| width.checked_add(1))
        .and_then(|width| usize::try_from(width).ok())
        .unwrap_or(1)
}

/// A game session placed on a fleet, as `CreateGameSession` persisted it.
#[derive(Debug, Clone)]
pub struct GameSessionRecord {
    pub game_session_id: String,
    pub name: Option<String>,
    pub fleet_id: String,
    pub fleet_arn: String,
    pub status: String,
    /// When `ACTIVATING` becomes `ACTIVE`; `None` once the session is running.
    pub active_at: Option<SystemTime>,
    pub creation_time: SystemTime,
    pub termination_time: Option<SystemTime>,
    pub current_player_session_count: i32,
    pub maximum_player_session_count: i32,
    pub game_properties: Vec<GameProperty>,
    pub game_session_data: Option<String>,
    pub creator_id: Option<String>,
    pub location: String,
    pub player_session_creation_policy: String,
    /// Connection information, which a session carries only once it is `ACTIVE`.
    pub ip_address: Option<String>,
    pub port: Option<i32>,
    pub dns_name: Option<String>,
}

impl GameSessionRecord {
    /// This game session as the modelled `GameSession`.
    pub fn to_modelled(&self) -> GameSession {
        GameSession {
            game_session_id: Some(self.game_session_id.clone()),
            name: self.name.clone(),
            fleet_id: Some(self.fleet_id.clone()),
            fleet_arn: Some(self.fleet_arn.clone()),
            creation_time: Some(to_date_time(self.creation_time)),
            termination_time: self.termination_time.map(to_date_time),
            current_player_session_count: Some(self.current_player_session_count),
            maximum_player_session_count: Some(self.maximum_player_session_count),
            status: Some(self.status.clone()),
            status_reason: None,
            game_properties: Some(self.game_properties.clone()),
            ip_address: self.ip_address.clone(),
            dns_name: self.dns_name.clone(),
            port: self.port,
            player_session_creation_policy: Some(self.player_session_creation_policy.clone()),
            creator_id: self.creator_id.clone(),
            game_session_data: self.game_session_data.clone(),
            // FlexMatch fills this in, and matchmaking is a separate slice.
            matchmaker_data: None,
            location: Some(self.location.clone()),
            // The compute a session runs on is reported by the compute slice.
            compute_name: None,
            player_gateway_status: None,
        }
    }
}

/// A player slot reserved in a game session, as `CreatePlayerSession` persisted it.
#[derive(Debug, Clone)]
pub struct PlayerSessionRecord {
    pub player_session_id: String,
    pub player_id: String,
    pub game_session_id: String,
    pub fleet_id: String,
    pub fleet_arn: String,
    pub status: String,
    pub creation_time: SystemTime,
    pub termination_time: Option<SystemTime>,
    pub ip_address: Option<String>,
    pub port: Option<i32>,
    pub dns_name: Option<String>,
    pub player_data: Option<String>,
}

impl PlayerSessionRecord {
    /// This player session as the modelled `PlayerSession`.
    pub fn to_modelled(&self) -> PlayerSession {
        PlayerSession {
            player_session_id: Some(self.player_session_id.clone()),
            player_id: Some(self.player_id.clone()),
            game_session_id: Some(self.game_session_id.clone()),
            fleet_id: Some(self.fleet_id.clone()),
            fleet_arn: Some(self.fleet_arn.clone()),
            creation_time: Some(to_date_time(self.creation_time)),
            termination_time: self.termination_time.map(to_date_time),
            status: Some(self.status.clone()),
            ip_address: self.ip_address.clone(),
            dns_name: self.dns_name.clone(),
            port: self.port,
            player_data: self.player_data.clone(),
        }
    }
}

/// A game session placement queue, as `CreateGameSessionQueue` persisted it.
///
/// `GameSessionQueue` is "configuration for a game session placement mechanism": every
/// member of it is a setting the request supplied or the service decided, so the record
/// holds them all and [`GameSessionQueueRecord::to_modelled`] is the one projection every
/// read of the queue is served from.
#[derive(Debug, Clone)]
pub struct GameSessionQueueRecord {
    pub name: String,
    pub queue_arn: String,
    pub timeout_in_seconds: i32,
    pub player_latency_policies: Vec<PlayerLatencyPolicy>,
    pub destinations: Vec<GameSessionQueueDestination>,
    pub filter_configuration: Option<FilterConfiguration>,
    pub priority_configuration: Option<PriorityConfiguration>,
    pub custom_event_data: Option<String>,
    pub notification_target: Option<String>,
    /// The labels assigned to the queue resource. `GameSessionQueue` has no `Tags`
    /// member — tags are read back through `ListTagsForResource` — so they are held on
    /// the record and reported by no projection of it yet.
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    /// The Region of the request that created the queue, which its ARN names.
    pub region: String,
}

impl GameSessionQueueRecord {
    /// This queue as the modelled `GameSessionQueue`.
    ///
    /// The two list members are always reported — empty for a queue configured with
    /// none, since "a queue can't make placements without at least one destination" is a
    /// property of an empty list, not of an absent one — while the two optional
    /// configuration structures are reported only when the queue carries them.
    pub fn to_modelled(&self) -> GameSessionQueue {
        GameSessionQueue {
            name: Some(self.name.clone()),
            game_session_queue_arn: Some(self.queue_arn.clone()),
            timeout_in_seconds: Some(self.timeout_in_seconds),
            player_latency_policies: Some(self.player_latency_policies.clone()),
            destinations: Some(self.destinations.clone()),
            filter_configuration: self.filter_configuration.clone(),
            priority_configuration: self.priority_configuration.clone(),
            custom_event_data: self.custom_event_data.clone(),
            notification_target: self.notification_target.clone(),
        }
    }
}

/// An alias for a fleet, as `CreateAlias` persisted it.
///
/// Every member of the modelled `Alias` is either a setting the request supplied or one
/// the service decided, so the record holds them all and [`AliasRecord::to_modelled`] is
/// the one projection every read of the alias is served from.
#[derive(Debug, Clone)]
pub struct AliasRecord {
    pub alias_id: String,
    pub alias_arn: String,
    pub name: String,
    pub description: Option<String>,
    /// The routing configuration as the request configured it: what every read of the
    /// alias reports, and what [`AliasRecord::routing`] routes a request through.
    pub routing_strategy: RoutingStrategy,
    /// The labels assigned to the alias resource. `Alias` has no `Tags` member — tags are
    /// read back through `ListTagsForResource` — so they are held on the record and
    /// reported by no projection of it yet.
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    /// When the alias was last modified, which `UpdateAlias` moves forward.
    pub last_updated_time: SystemTime,
    /// The Region of the request that created the alias, which its ARN names.
    pub region: String,
}

/// Where a request that names an alias is routed, which is what the alias's
/// `RoutingStrategy$Type` decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AliasRouting<'a> {
    /// `SIMPLE` — "The alias resolves to one specific fleet": the `FleetId` it names.
    Fleet(&'a str),
    /// `TERMINAL` — "The alias does not resolve to a fleet but instead can be used to
    /// display a message to the user. A terminal alias throws a
    /// TerminalRoutingStrategyException with the message embedded": that message.
    Terminal(Option<&'a str>),
}

impl AliasRecord {
    /// This alias as the modelled `Alias`.
    pub fn to_modelled(&self) -> Alias {
        Alias {
            alias_id: Some(self.alias_id.clone()),
            alias_arn: Some(self.alias_arn.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            routing_strategy: Some(self.routing_strategy.clone()),
            creation_time: Some(to_date_time(self.creation_time)),
            last_updated_time: Some(to_date_time(self.last_updated_time)),
        }
    }

    /// Where this alias routes a request that names it.
    ///
    /// Only the two forms [`validate_routing_strategy`] accepts are ever stored, so a
    /// strategy that is not a `SIMPLE` one naming a fleet is the `TERMINAL` one, which
    /// resolves to no fleet at all.
    pub fn routing(&self) -> AliasRouting<'_> {
        match (
            self.routing_strategy.r#type.as_deref(),
            self.routing_strategy.fleet_id.as_deref(),
        ) {
            (Some(ROUTING_STRATEGY_TYPE_SIMPLE), Some(fleet_id)) => AliasRouting::Fleet(fleet_id),
            _ => AliasRouting::Terminal(self.routing_strategy.message.as_deref()),
        }
    }
}

/// Whether `strategy` is one of the two routings an alias can carry, or the reason it is
/// not — which a handler answers with the modelled `InvalidRequestException`.
///
/// `RoutingStrategy$Type`'s doc names both forms and what each one needs: "SIMPLE - The
/// alias resolves to one specific fleet", so a simple strategy names the fleet it points
/// at; "TERMINAL - The alias does not resolve to a fleet but instead can be used to
/// display a message to the user. A terminal alias throws a
/// TerminalRoutingStrategyException with the message embedded", so a terminal strategy
/// carries that message. The modelled `RoutingStrategyType` enum is refused by the
/// boundary walk before a handler runs, so an unknown type cannot reach here; which
/// member each type requires is the service's own rule, enforced here.
///
/// Whether the named fleet EXISTS is deliberately not checked: an alias outlives the
/// fleets it points at ("You can reassign an alias to another fleet by calling
/// UpdateAlias"), so resolution is what discovers that a target has gone.
pub fn validate_routing_strategy(strategy: &RoutingStrategy) -> Result<(), String> {
    match strategy.r#type.as_deref() {
        Some(ROUTING_STRATEGY_TYPE_SIMPLE) => {
            if strategy.fleet_id.is_none() {
                return Err(format!(
                    "RoutingStrategy of type {ROUTING_STRATEGY_TYPE_SIMPLE} must name the fleet \
                     it resolves to in RoutingStrategy.FleetId."
                ));
            }
            Ok(())
        }
        Some(ROUTING_STRATEGY_TYPE_TERMINAL) => {
            if strategy.message.is_none() {
                return Err(format!(
                    "RoutingStrategy of type {ROUTING_STRATEGY_TYPE_TERMINAL} must carry the \
                     message to display in RoutingStrategy.Message."
                ));
            }
            Ok(())
        }
        _ => Err(format!(
            "RoutingStrategy.Type is required, and must be {ROUTING_STRATEGY_TYPE_SIMPLE} or \
             {ROUTING_STRATEGY_TYPE_TERMINAL}."
        )),
    }
}

/// Per-service state for com-amazonaws-gamelift-gamelift. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsGameliftGameliftState {
    /// Fleets by `FleetId`.
    pub fleets: HashMap<String, Fleet>,
    /// Aliases by `AliasId`: "Alias IDs are unique within a Region", while "alias names
    /// do not need to be unique", so the id is the alias's key.
    pub aliases: HashMap<String, AliasRecord>,
    /// Game session queues by `Name`: "Queue names must be unique within each Region",
    /// and this store — like the fleet store — is the account's one store, so the name
    /// is the queue's key.
    pub game_session_queues: HashMap<String, GameSessionQueueRecord>,
    /// Game sessions by their full-ARN `GameSessionId`.
    pub game_sessions: HashMap<String, GameSessionRecord>,
    /// Player sessions by `PlayerSessionId`.
    pub player_sessions: HashMap<String, PlayerSessionRecord>,
    /// `IdempotencyToken` (or caller-supplied `GameSessionId`) to the game session the
    /// request that first carried it created, keyed by the fleet it was created on:
    /// `IdempotentParameterMismatchException`'s doc scopes the custom string to one
    /// fleet ("A game session with this custom ID string already exists in this fleet"),
    /// so the same string on another fleet is a fresh session, not a mismatch.
    pub idempotency_tokens: HashMap<(String, String), String>,
    /// How many fleet instances have been provisioned, for minting their addresses.
    pub provisioned_instances: u32,
    /// How many resource ids this store has minted, which is what makes the next one
    /// unique (see [`mint_uuid`]).
    pub minted_ids: u64,
}

impl ComAmazonawsGameliftGameliftState {
    /// Apply every status change the clock has already made due, so that each handler
    /// reads and reports the state the service would be in at `now` whether or not
    /// anybody called in between.
    pub fn tick(&mut self, now: SystemTime) {
        // A deletion that has completed takes the fleet and everything placed on it.
        let purged: Vec<String> = self
            .fleets
            .iter()
            .filter(|(_, fleet)| fleet.purge_at.is_some_and(|due| now >= due))
            .map(|(fleet_id, _)| fleet_id.clone())
            .collect();
        for fleet_id in purged {
            self.remove_fleet(&fleet_id);
        }

        // A finished fleet-creation workflow leaves the fleet, and each of its
        // locations, ACTIVE.
        for fleet in self.fleets.values_mut() {
            if fleet.status == FLEET_STATUS_NEW && fleet.active_at.is_some_and(|due| now >= due) {
                fleet.status = FLEET_STATUS_ACTIVE.to_string();
                fleet.active_at = None;
                for location in &mut fleet.locations {
                    location.status = FLEET_STATUS_ACTIVE.to_string();
                }
            }
        }

        // A started game session becomes ACTIVE and only then carries connection
        // information, which comes from the fleet instance hosting it.
        let starting: Vec<String> = self
            .game_sessions
            .iter()
            .filter(|(_, session)| {
                session.status == GAME_SESSION_STATUS_ACTIVATING
                    && session.active_at.is_some_and(|due| now >= due)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for game_session_id in starting {
            let Some(fleet_id) = self
                .game_sessions
                .get(&game_session_id)
                .map(|session| session.fleet_id.clone())
            else {
                continue;
            };
            // The ports the fleet's other live sessions hold: no two of them may listen
            // on the same one.
            let held: HashSet<i32> = self
                .game_sessions
                .values()
                .filter(|session| session.fleet_id == fleet_id)
                .filter_map(|session| session.port)
                .collect();
            let Some(fleet) = self.fleets.get_mut(&fleet_id) else {
                continue;
            };
            let ip_address = fleet.ip_address.clone();
            let dns_name = fleet.dns_name();
            // `None` would mean the fleet opened no port this session could listen on,
            // which its capacity refuses at CreateGameSession; such a session stays
            // ACTIVATING rather than reporting a port another one holds.
            let Some(port) = fleet.next_game_session_port(&held) else {
                continue;
            };
            if let Some(session) = self.game_sessions.get_mut(&game_session_id) {
                session.status = GAME_SESSION_STATUS_ACTIVE.to_string();
                session.active_at = None;
                session.ip_address = Some(ip_address);
                session.dns_name = Some(dns_name);
                session.port = Some(port);
            }
        }
    }

    /// Drop a fleet, the game sessions placed on it, the player slots reserved in those
    /// sessions, and the idempotency tokens that named them.
    pub fn remove_fleet(&mut self, fleet_id: &str) {
        self.fleets.remove(fleet_id);
        let sessions: HashSet<String> = self
            .game_sessions
            .iter()
            .filter(|(_, session)| session.fleet_id == fleet_id)
            .map(|(id, _)| id.clone())
            .collect();
        for game_session_id in &sessions {
            self.game_sessions.remove(game_session_id);
            self.player_sessions
                .retain(|_, player| &player.game_session_id != game_session_id);
        }
        self.idempotency_tokens
            .retain(|_, game_session_id| !sessions.contains(game_session_id));
    }

    /// How many game sessions are placed on `fleet_id` in `location` and have not reached
    /// a terminal status, i.e. how much of that fleet location's capacity is in use.
    pub fn live_game_sessions_at(&self, fleet_id: &str, location: &str) -> usize {
        self.live_game_sessions(fleet_id, Some(location))
    }

    /// How many game sessions are placed on `fleet_id`, in any of its locations, and have
    /// not reached a terminal status — so how many of the ports the fleet opens are held.
    pub fn live_game_sessions_on(&self, fleet_id: &str) -> usize {
        self.live_game_sessions(fleet_id, None)
    }

    /// The live game sessions of a fleet, optionally narrowed to one of its locations.
    fn live_game_sessions(&self, fleet_id: &str, location: Option<&str>) -> usize {
        self.game_sessions
            .values()
            .filter(|session| {
                session.fleet_id == fleet_id
                    && location.is_none_or(|location| session.location == location)
                    && (session.status == GAME_SESSION_STATUS_ACTIVATING
                        || session.status == GAME_SESSION_STATUS_ACTIVE)
            })
            .count()
    }

    /// The `FleetId` of the fleet a `FleetIdOrArn` names, or `None` when no stored
    /// fleet matches it.
    pub fn resolve_fleet_id(&self, id_or_arn: &str) -> Option<String> {
        if id_or_arn.starts_with("arn:") {
            self.fleets
                .values()
                .find(|fleet| fleet.fleet_arn == id_or_arn)
                .map(|fleet| fleet.fleet_id.clone())
        } else {
            self.fleets
                .get(id_or_arn)
                .map(|fleet| fleet.fleet_id.clone())
        }
    }

    /// Every stored fleet, oldest first, so a paginated walk is stable across calls.
    pub fn fleets_in_order(&self) -> Vec<&Fleet> {
        let mut fleets: Vec<&Fleet> = self.fleets.values().collect();
        fleets.sort_by(|left, right| {
            left.creation_time
                .cmp(&right.creation_time)
                .then_with(|| left.fleet_id.cmp(&right.fleet_id))
        });
        fleets
    }

    /// Every stored game session, oldest first, so a paginated walk is stable.
    pub fn game_sessions_in_order(&self) -> Vec<&GameSessionRecord> {
        let mut sessions: Vec<&GameSessionRecord> = self.game_sessions.values().collect();
        sessions.sort_by(|left, right| {
            left.creation_time
                .cmp(&right.creation_time)
                .then_with(|| left.game_session_id.cmp(&right.game_session_id))
        });
        sessions
    }

    /// The alias an `AliasIdOrArn` names, or `None` when no stored alias matches it.
    ///
    /// `AliasArn`'s doc: "In a GameLift alias ARN, the resource ID matches the alias ID
    /// value", so either form selects the same alias.
    pub fn resolve_alias(&self, id_or_arn: &str) -> Option<&AliasRecord> {
        if id_or_arn.starts_with("arn:") {
            self.aliases
                .values()
                .find(|alias| alias.alias_arn == id_or_arn)
        } else {
            self.aliases.get(id_or_arn)
        }
    }

    /// The `Name` of the queue a `GameSessionQueueNameOrArn` names, or `None` when no
    /// stored queue matches it.
    ///
    /// `GameSessionQueueArn`'s doc: "In a Amazon GameLift Servers game session queue ARN,
    /// the resource ID matches the `Name` value", so either form selects the same queue.
    pub fn resolve_queue_name(&self, name_or_arn: &str) -> Option<String> {
        if name_or_arn.starts_with("arn:") {
            self.game_session_queues
                .values()
                .find(|queue| queue.queue_arn == name_or_arn)
                .map(|queue| queue.name.clone())
        } else {
            self.game_session_queues
                .get(name_or_arn)
                .map(|queue| queue.name.clone())
        }
    }

    /// Every stored game session queue, oldest first, so a paginated walk is stable
    /// across calls.
    pub fn game_session_queues_in_order(&self) -> Vec<&GameSessionQueueRecord> {
        let mut queues: Vec<&GameSessionQueueRecord> = self.game_session_queues.values().collect();
        queues.sort_by(|left, right| {
            left.creation_time
                .cmp(&right.creation_time)
                .then_with(|| left.name.cmp(&right.name))
        });
        queues
    }

    /// An address for a newly provisioned fleet instance, held by no stored fleet.
    ///
    /// Drawn from 203.0.113.0/24, which RFC 5737 reserves for documentation, so an
    /// address this emulator reports can never name a real host. Two live fleets must
    /// never report the same connection information — a non-TLS fleet's `DnsName` is
    /// derived from this address — so the counter walks past an address a stored fleet
    /// already holds. [`MAX_FLEETS_PER_ACCOUNT`] is below the block's host count, so the
    /// walk always finds a free one.
    pub fn next_instance_address(&mut self) -> String {
        /// Host addresses in a /24 that this emulator hands out (`.1`-`.254`).
        const HOSTS: u32 = 254;
        const _: () = assert!(
            MAX_FLEETS_PER_ACCOUNT < HOSTS as usize,
            "the fleet quota must stay below the host count of the address block, or two \
             live fleets could be given the same address"
        );

        let mut candidate = String::new();
        for _ in 0..HOSTS {
            self.provisioned_instances = self.provisioned_instances.wrapping_add(1);
            candidate = format!("203.0.113.{}", self.provisioned_instances % HOSTS + 1);
            if !self
                .fleets
                .values()
                .any(|fleet| fleet.ip_address == candidate)
            {
                break;
            }
        }
        candidate
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsGameliftGameliftState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsGameliftGameliftState>)
}
