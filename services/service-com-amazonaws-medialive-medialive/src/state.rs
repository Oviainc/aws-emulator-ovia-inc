// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-medialive-medialive service.
//!
//! The channel lifecycle lives here: one [`ChannelRecord`] per created channel,
//! the monotonic id counter that mints its `Id`, and the idempotency-token map
//! that makes a retried `CreateChannel` return the channel it already created.
//!
//! Every value a channel-shaped response carries is either stored verbatim from
//! the request that created the channel or derived from the stored state
//! ([`ComAmazonawsMedialiveMedialiveState::channel_description`]) — no operation
//! builds a response out of its own request.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AnywhereSettings, CdiInputSpecification, Channel, ChannelSummary,
    DescribeAnywhereSettings, DescribeFollowerChannelSettings, DescribeInferenceSettings,
    DescribeLinkedChannelSettings, DescribePrimaryChannelSettings, EncoderSettings,
    InferenceSettings, InputAttachment, InputSpecification, LinkedChannelSettings,
    MaintenanceStatus, OutputDestination, PipelineDetail, VpcOutputSettings,
    VpcOutputSettingsDescription,
};

/// The `ChannelState` enum values this slice uses, spelled exactly as the model
/// declares them (`smithy_query(query="shape", shape="ChannelState")`:
/// CREATING | CREATE_FAILED | IDLE | STARTING | RUNNING | RECOVERING | STOPPING |
/// DELETING | DELETED | UPDATING | UPDATE_FAILED).
pub mod channel_state {
    pub const CREATING: &str = "CREATING";
    pub const CREATE_FAILED: &str = "CREATE_FAILED";
    pub const IDLE: &str = "IDLE";
    pub const STARTING: &str = "STARTING";
    pub const RUNNING: &str = "RUNNING";
    pub const STOPPING: &str = "STOPPING";
    pub const DELETING: &str = "DELETING";
    pub const UPDATE_FAILED: &str = "UPDATE_FAILED";
}

/// The `ChannelClass` enum values (`smithy_query(query="shape",
/// shape="ChannelClass")`: STANDARD | SINGLE_PIPELINE).
pub mod channel_class {
    pub const STANDARD: &str = "STANDARD";
    pub const SINGLE_PIPELINE: &str = "SINGLE_PIPELINE";
}

/// How long a channel stays in a transient state (`CREATING`, `STARTING`,
/// `STOPPING`, `DELETING`) before the clock makes the next state due.
///
/// CHOSEN, not documented by AWS: the real service takes minutes, and no doc
/// sentence pins a number, so this is the emulator's own span. It is deliberately
/// non-zero so a caller that acts immediately after `CreateChannel` /
/// `StartChannel` still observes the transient state and the `ConflictException`
/// it produces, exactly as it would against the real service.
pub const TRANSIENT_STATE_SPAN: Duration = Duration::from_millis(500);

/// The id the first channel of a process gets. AWS channel ids are numeric
/// strings (the issue's example is `1234567`); minting them from a counter at a
/// fixed seven-digit width keeps the `BTreeMap` key order equal to the mint
/// order, which is what makes `ListChannels` pagination deterministic.
const FIRST_CHANNEL_ID: u64 = 1_000_000;

/// The most channels this emulator stores for one account.
///
/// A ceiling, not a guess at AWS's published quota: it exists so a create loop
/// cannot grow the emulator's heap without bound. `CreateChannel` models
/// `TooManyRequestsException`, which is the refusal once the store is full.
pub const MAX_CHANNELS: usize = 100;

/// The most `RequestId` entries kept for `CreateChannel` idempotency.
///
/// At most one token is recorded per stored channel and a token is dropped with
/// the channel it minted, so this cap is never the binding one in practice; it is
/// enforced anyway, by evicting the oldest token, so the map cannot accumulate
/// caller-chosen strings independently of the channels they name.
pub const MAX_IDEMPOTENCY_TOKENS: usize = MAX_CHANNELS;

/// The page size `ListChannels` uses when the request names no `MaxResults`, and
/// the ceiling it clamps a larger `MaxResults` to. Finite, so one request cannot
/// ask the service to project an unbounded number of channels in one response.
pub const MAX_LIST_PAGE_SIZE: usize = 50;

/// The state a channel's in-flight transition settles into once it comes due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Settled {
    /// `CREATING` and `STOPPING` settle into `IDLE`.
    Idle,
    /// `STARTING` settles into `RUNNING`.
    Running,
    /// `DELETING` settles into the channel no longer existing.
    Gone,
}

/// A transition the clock owes: the instant it becomes due and what it settles
/// into. Stored, never slept on.
#[derive(Debug, Clone, Copy)]
struct Transition {
    due: SystemTime,
    to: Settled,
}

/// One created channel, as `CreateChannel` stored it plus its lifecycle state.
///
/// The members mirror the model's `Channel` shape. Members the real service
/// computes from infrastructure it owns (a pipeline's egress IP, a VPC network
/// interface id, an engine-version expiry) are NOT invented here: they stay
/// absent, because this emulator has no such infrastructure to report.
#[derive(Debug, Clone)]
pub struct ChannelRecord {
    pub id: String,
    pub arn: String,
    pub state: String,
    pub channel_class: String,
    pub name: Option<String>,
    pub cdi_input_specification: Option<CdiInputSpecification>,
    pub destinations: Vec<OutputDestination>,
    pub encoder_settings: Option<EncoderSettings>,
    pub input_attachments: Vec<InputAttachment>,
    pub input_specification: Option<InputSpecification>,
    pub log_level: Option<String>,
    pub maintenance: Option<MaintenanceStatus>,
    pub role_arn: Option<String>,
    pub tags: HashMap<String, String>,
    pub channel_security_groups: Vec<String>,
    pub anywhere_settings: Option<AnywhereSettings>,
    pub linked_channel_settings: Option<LinkedChannelSettings>,
    pub inference_settings: Option<InferenceSettings>,
    pub vpc: Option<VpcOutputSettings>,
    pub channel_engine_version: Option<String>,
    /// The transition the clock owes this channel, if any.
    transition: Option<Transition>,
}

impl ChannelRecord {
    /// The number of encoding pipelines this channel's class gives it: two for
    /// `STANDARD`, one for `SINGLE_PIPELINE` (the `ChannelClass` doc: "A standard
    /// channel has two encoding pipelines and a single pipeline channel only has
    /// one").
    pub fn pipeline_count(&self) -> i32 {
        if self.channel_class == channel_class::SINGLE_PIPELINE {
            1
        } else {
            2
        }
    }

    /// How many of this channel's pipelines are running: all of them exactly
    /// while the channel is `RUNNING`, none in any other state.
    pub fn pipelines_running_count(&self) -> i32 {
        if self.state == channel_state::RUNNING {
            self.pipeline_count()
        } else {
            0
        }
    }

    /// `PipelineDetail` is documented as "Runtime details of a pipeline when a
    /// channel is running", so there is one entry per running pipeline and none
    /// at all when the channel is not running. Only `PipelineId` is reported: the
    /// other members describe media actually being ingested, which this emulator
    /// does not run and therefore does not claim.
    pub fn pipeline_details(&self) -> Vec<PipelineDetail> {
        (0..self.pipelines_running_count())
            .map(|pipeline| PipelineDetail {
                pipeline_id: Some(pipeline.to_string()),
                ..PipelineDetail::default()
            })
            .collect()
    }

    /// Whether this channel may be started, stopped or deleted right now.
    pub fn is_idle(&self) -> bool {
        self.state == channel_state::IDLE
    }

    /// Whether this channel is in a state `DeleteChannel` accepts: a channel is
    /// deletable once it has stopped (`IDLE`) or has failed, never while it is
    /// creating, starting, running, stopping or already deleting.
    pub fn is_deletable(&self) -> bool {
        self.is_idle()
            || self.state == channel_state::CREATE_FAILED
            || self.state == channel_state::UPDATE_FAILED
    }

    /// Put the channel into `state` as of `now` and owe `to` once
    /// [`TRANSIENT_STATE_SPAN`] has passed.
    fn begin_transition(&mut self, state: &str, to: Settled, now: SystemTime) {
        self.state = state.to_string();
        self.transition = Some(Transition {
            due: now + TRANSIENT_STATE_SPAN,
            to,
        });
    }

    /// Materialise the transition the clock has already made due. Returns false
    /// when the channel is now gone (a settled `DELETING`), so the caller drops
    /// the record.
    fn settle(&mut self, now: SystemTime) -> bool {
        match self.transition {
            Some(transition) if now >= transition.due => match transition.to {
                Settled::Idle => {
                    self.state = channel_state::IDLE.to_string();
                    self.transition = None;
                    true
                }
                Settled::Running => {
                    self.state = channel_state::RUNNING.to_string();
                    self.transition = None;
                    true
                }
                Settled::Gone => false,
            },
            _ => true,
        }
    }
}

/// Per-service state for com-amazonaws-medialive-medialive. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMedialiveMedialiveState {
    /// Every channel that exists, keyed by its minted `Id`. A `BTreeMap` so
    /// `ListChannels` walks it in id order on every call and its `NextToken`
    /// means the same thing on the next one.
    pub channels: BTreeMap<String, ChannelRecord>,
    /// Mint counter behind [`Self::mint_channel_id`].
    pub next_channel_id: u64,
    /// `CreateChannel`'s `RequestId` (`@idempotencyToken`) -> the channel it
    /// created, so a retry returns that channel instead of a second one.
    ///
    /// PRIVATE, so the cap and the eviction order are invariants the type
    /// enforces: it is written only by [`Self::record_idempotency_token`] and
    /// read only through [`Self::channel_for_token`].
    idempotency_tokens: HashMap<String, String>,
    /// The order [`Self::idempotency_tokens`] was written in, so the cap evicts
    /// the oldest token rather than an arbitrary one.
    idempotency_token_order: VecDeque<String>,
}

impl ComAmazonawsMedialiveMedialiveState {
    /// Materialise every transition the clock had already made due as of `now`,
    /// then answer from the result.
    ///
    /// Call this at the top of every channel operation, reporting ones included:
    /// it only applies transitions that were owed at that instant whether or not
    /// anybody called, so it never changes what a later caller would otherwise
    /// have seen.
    pub fn settle(&mut self, now: SystemTime) {
        let mut removed: Vec<String> = Vec::new();
        self.channels.retain(|id, channel| {
            let survives = channel.settle(now);
            if !survives {
                removed.push(id.clone());
            }
            survives
        });
        // A token never outlives the channel it minted.
        for id in removed {
            self.forget_idempotency_tokens_for(&id);
        }
    }

    /// Whether the store is full, so `CreateChannel` must refuse rather than mint
    /// another channel.
    pub fn is_full(&self) -> bool {
        self.channels.len() >= MAX_CHANNELS
    }

    /// Record `token` as having created `channel_id`, evicting the oldest token
    /// first if that would push the map past [`MAX_IDEMPOTENCY_TOKENS`].
    pub fn record_idempotency_token(&mut self, token: String, channel_id: &str) {
        while self.idempotency_tokens.len() >= MAX_IDEMPOTENCY_TOKENS {
            match self.idempotency_token_order.pop_front() {
                Some(oldest) => {
                    self.idempotency_tokens.remove(&oldest);
                }
                // The order queue and the map are written together, so this is
                // unreachable; clearing rather than looping forever keeps the cap
                // an invariant even if they ever drift.
                None => {
                    self.idempotency_tokens.clear();
                    break;
                }
            }
        }
        if self
            .idempotency_tokens
            .insert(token.clone(), channel_id.to_string())
            .is_none()
        {
            self.idempotency_token_order.push_back(token);
        }
    }

    /// Drop every idempotency token that created `channel_id`.
    fn forget_idempotency_tokens_for(&mut self, channel_id: &str) {
        self.idempotency_tokens
            .retain(|_, created| created != channel_id);
        let live = &self.idempotency_tokens;
        self.idempotency_token_order
            .retain(|token| live.contains_key(token));
    }

    /// The next channel id, as AWS shapes them: a numeric string.
    pub fn mint_channel_id(&mut self) -> String {
        let id = FIRST_CHANNEL_ID + self.next_channel_id;
        self.next_channel_id += 1;
        id.to_string()
    }

    /// The channel a stored `RequestId` created, if that channel still exists.
    pub fn channel_for_token(&self, request_id: &str) -> Option<&str> {
        let id = self.idempotency_tokens.get(request_id)?;
        self.channels.get(id).map(|channel| channel.id.as_str())
    }

    /// The full `Channel` description of one stored channel, or `None` when no
    /// channel has that id.
    pub fn channel_description(&self, id: &str) -> Option<Channel> {
        let channel = self.channels.get(id)?;
        Some(Channel {
            arn: Some(channel.arn.clone()),
            cdi_input_specification: channel.cdi_input_specification.clone(),
            channel_class: Some(channel.channel_class.clone()),
            destinations: Some(channel.destinations.clone()),
            // ABSENT, not empty: `EgressEndpoints` reports the public IP each
            // pipeline's output comes from, which the real service allocates and
            // this emulator does not. An empty list would claim the channel
            // egresses from nowhere; absent says it is not reported.
            egress_endpoints: None,
            encoder_settings: channel.encoder_settings.clone(),
            id: Some(channel.id.clone()),
            input_attachments: Some(channel.input_attachments.clone()),
            input_specification: channel.input_specification.clone(),
            log_level: channel.log_level.clone(),
            maintenance: channel.maintenance.clone(),
            name: channel.name.clone(),
            pipeline_details: Some(channel.pipeline_details()),
            pipelines_running_count: Some(channel.pipelines_running_count()),
            role_arn: channel.role_arn.clone(),
            state: Some(channel.state.clone()),
            tags: Some(channel.tags.clone()),
            vpc: channel.vpc.as_ref().map(described_vpc),
            anywhere_settings: channel.anywhere_settings.as_ref().map(described_anywhere),
            channel_engine_version: channel
                .channel_engine_version
                .as_deref()
                .map(described_engine_version),
            linked_channel_settings: channel
                .linked_channel_settings
                .as_ref()
                .map(|linked| self.described_linked(&channel.arn, linked)),
            channel_security_groups: Some(channel.channel_security_groups.clone()),
            inference_settings: channel.inference_settings.as_ref().map(described_inference),
        })
    }

    /// The `ChannelSummary` projection of one stored channel — the same stored
    /// record as [`Self::channel_description`], minus the members the summary
    /// shape does not carry (`EncoderSettings`, `PipelineDetails`).
    pub fn channel_summary(&self, channel: &ChannelRecord) -> ChannelSummary {
        ChannelSummary {
            arn: Some(channel.arn.clone()),
            cdi_input_specification: channel.cdi_input_specification.clone(),
            channel_class: Some(channel.channel_class.clone()),
            destinations: Some(channel.destinations.clone()),
            // ABSENT for the same reason as in `channel_description`.
            egress_endpoints: None,
            id: Some(channel.id.clone()),
            input_attachments: Some(channel.input_attachments.clone()),
            input_specification: channel.input_specification.clone(),
            log_level: channel.log_level.clone(),
            maintenance: channel.maintenance.clone(),
            name: channel.name.clone(),
            pipelines_running_count: Some(channel.pipelines_running_count()),
            role_arn: channel.role_arn.clone(),
            state: Some(channel.state.clone()),
            tags: Some(channel.tags.clone()),
            vpc: channel.vpc.as_ref().map(described_vpc),
            anywhere_settings: channel.anywhere_settings.as_ref().map(described_anywhere),
            channel_engine_version: channel
                .channel_engine_version
                .as_deref()
                .map(described_engine_version),
            used_channel_engine_versions: None,
            linked_channel_settings: channel
                .linked_channel_settings
                .as_ref()
                .map(|linked| self.described_linked(&channel.arn, linked)),
            channel_security_groups: Some(channel.channel_security_groups.clone()),
            inference_settings: channel.inference_settings.as_ref().map(described_inference),
        }
    }

    /// A primary channel's `FollowingChannelArns`: the ARNs of the channels
    /// whose own follower settings name this channel as their primary. Derived
    /// from state, so it tracks every later create and delete.
    fn following_channel_arns(&self, primary_arn: &str) -> Vec<String> {
        self.channels
            .values()
            .filter(|channel| {
                channel
                    .linked_channel_settings
                    .as_ref()
                    .and_then(|linked| linked.follower_channel_settings.as_ref())
                    .and_then(|follower| follower.primary_channel_arn.as_deref())
                    == Some(primary_arn)
            })
            .map(|channel| channel.arn.clone())
            .collect()
    }

    fn described_linked(
        &self,
        arn: &str,
        linked: &LinkedChannelSettings,
    ) -> DescribeLinkedChannelSettings {
        DescribeLinkedChannelSettings {
            follower_channel_settings: linked.follower_channel_settings.as_ref().map(|follower| {
                DescribeFollowerChannelSettings {
                    linked_channel_type: follower.linked_channel_type.clone(),
                    primary_channel_arn: follower.primary_channel_arn.clone(),
                }
            }),
            primary_channel_settings: linked.primary_channel_settings.as_ref().map(|primary| {
                DescribePrimaryChannelSettings {
                    following_channel_arns: Some(self.following_channel_arns(arn)),
                    linked_channel_type: primary.linked_channel_type.clone(),
                }
            }),
        }
    }
}

/// `VpcOutputSettings` (request) -> `VpcOutputSettingsDescription` (response).
///
/// `AvailabilityZones` and `NetworkInterfaceIds` are left absent: the real
/// service fills them from the ENIs it allocates, which this emulator does not
/// create, so reporting values for them would be fabrication.
fn described_vpc(vpc: &VpcOutputSettings) -> VpcOutputSettingsDescription {
    VpcOutputSettingsDescription {
        availability_zones: None,
        network_interface_ids: None,
        security_group_ids: vpc.security_group_ids.clone(),
        subnet_ids: Some(vpc.subnet_ids.clone()),
    }
}

fn described_anywhere(anywhere: &AnywhereSettings) -> DescribeAnywhereSettings {
    DescribeAnywhereSettings {
        channel_placement_group_id: anywhere.channel_placement_group_id.clone(),
        cluster_id: anywhere.cluster_id.clone(),
    }
}

fn described_inference(inference: &InferenceSettings) -> DescribeInferenceSettings {
    DescribeInferenceSettings {
        feed_arn: inference.feed_arn.clone(),
        audio_feed_inputs: inference.audio_feed_inputs.clone(),
        enrichment_methods: inference.enrichment_methods.clone(),
    }
}

/// `ChannelEngineVersionRequest` (just a `Version`) -> the response shape.
/// `ExpirationDate` is "The UTC time when the version expires" — an AWS-side
/// release fact, not something this emulator knows, so it stays absent.
fn described_engine_version(version: &str) -> crate::ops::types::ChannelEngineVersionResponse {
    crate::ops::types::ChannelEngineVersionResponse {
        expiration_date: None,
        version: Some(version.to_string()),
    }
}

/// Build a new channel record from a `CreateChannel` request's values.
///
/// Every member is the value the request supplied; the lifecycle members are the
/// only computed ones. The channel starts in `CREATING` and owes `IDLE`, which is
/// what the real service does.
pub fn new_channel(
    id: String,
    arn: String,
    channel_class: String,
    now: SystemTime,
) -> ChannelRecord {
    let mut channel = ChannelRecord {
        id,
        arn,
        state: String::new(),
        channel_class,
        name: None,
        cdi_input_specification: None,
        destinations: Vec::new(),
        encoder_settings: None,
        input_attachments: Vec::new(),
        input_specification: None,
        log_level: None,
        maintenance: None,
        role_arn: None,
        tags: HashMap::new(),
        channel_security_groups: Vec::new(),
        anywhere_settings: None,
        linked_channel_settings: None,
        inference_settings: None,
        vpc: None,
        channel_engine_version: None,
        transition: None,
    };
    channel.begin_transition(channel_state::CREATING, Settled::Idle, now);
    channel
}

/// Move a channel from `IDLE` into `STARTING`, owing `RUNNING`.
pub fn begin_start(channel: &mut ChannelRecord, now: SystemTime) {
    channel.begin_transition(channel_state::STARTING, Settled::Running, now);
}

/// Move a running channel into `STOPPING`, owing `IDLE`.
pub fn begin_stop(channel: &mut ChannelRecord, now: SystemTime) {
    channel.begin_transition(channel_state::STOPPING, Settled::Idle, now);
}

/// Move a channel into `DELETING`, owing its removal from state.
pub fn begin_delete(channel: &mut ChannelRecord, now: SystemTime) {
    channel.begin_transition(channel_state::DELETING, Settled::Gone, now);
}

/// The region this request was signed for, which is the region every ARN this
/// request mints names. Falls back to the emulator's default region when the
/// request carries no SigV4 credential scope, or an empty region in it.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The region a request that names none is treated as being in (`emulator_arn`'s
/// documented default).
const DEFAULT_REGION: &str = "us-east-1";

/// The message for the one failure that is not about the request: this service
/// was mounted on an engine that holds no store for it. One constant, so the six
/// channel operations cannot word it six ways.
pub const NO_STORE: &str = "medialive channel state is unavailable";

/// Project a `Channel` onto one of the structurally identical per-operation
/// channel-description output shapes (`DescribeChannelOutput`,
/// `StartChannelOutput`, `StopChannelOutput`, `DeleteChannelOutput`).
///
/// One macro rather than four hand-written copies of the same 23-member move, so
/// the four operations cannot drift from each other.
macro_rules! channel_description_output {
    ($out:ident, $channel:expr) => {{
        let channel: $crate::ops::types::Channel = $channel;
        $out {
            arn: channel.arn,
            cdi_input_specification: channel.cdi_input_specification,
            channel_class: channel.channel_class,
            destinations: channel.destinations,
            egress_endpoints: channel.egress_endpoints,
            encoder_settings: channel.encoder_settings,
            id: channel.id,
            input_attachments: channel.input_attachments,
            input_specification: channel.input_specification,
            log_level: channel.log_level,
            maintenance: channel.maintenance,
            name: channel.name,
            pipeline_details: channel.pipeline_details,
            pipelines_running_count: channel.pipelines_running_count,
            role_arn: channel.role_arn,
            state: channel.state,
            tags: channel.tags,
            vpc: channel.vpc,
            anywhere_settings: channel.anywhere_settings,
            channel_engine_version: channel.channel_engine_version,
            linked_channel_settings: channel.linked_channel_settings,
            channel_security_groups: channel.channel_security_groups,
            inference_settings: channel.inference_settings,
        }
    }};
}

pub(crate) use channel_description_output;

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMedialiveMedialiveState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMedialiveMedialiveState>)
}
