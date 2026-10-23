// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-backupgateway-backuponpremises-v20210101 service.
//!
//! The resource this store owns is the **gateway**: `CreateGateway` mints one and
//! persists it here, and every other gateway operation resolves it out of this map by
//! ARN. Nothing is kept per request and nothing is recomputed from the request body —
//! the ARN a create returns is read back out of [`StoredGateway`] after the insert, so
//! a response can only describe a record that is really in the store.
//!
//! Beside the gateways the store keeps the ACTIVATION KEYS this service has issued.
//! `CreateGateway` resolves its `ActivationKey` there and refuses a key the registry
//! does not hold, so a gateway is only ever minted for an appliance the service really
//! activated — the read side (`ListGateways`, `GetGateway`) then cannot report a
//! gateway that a refused create left behind.
//!
//! The record carries the members of the later slices too (hypervisor association,
//! maintenance window, bandwidth schedule), so those operations extend this one struct
//! instead of opening a second map beside it and splitting one gateway across two
//! copies. They reuse the MODELLED shapes
//! ([`MaintenanceStartTime`], [`BandwidthRateLimitInterval`]) rather than mirroring
//! them, so a member cannot drift between what is stored and what is serialised.
//!
//! The second resource this store owns is the **hypervisor** ([`StoredHypervisor`]),
//! keyed by its own ARN: `GetHypervisor` resolves it out of that map and reports the
//! record, `ListHypervisors` pages over that map in ARN order, and the state either one
//! reports is derived from the record and the clock
//! ([`StoredHypervisor::state_as_of`]) so a read never moves it.
//!
//! Both listings page through one helper ([`page_of`], handing back a [`Page`]), so a
//! page boundary and the token that encodes it mean the same thing whichever resource
//! is being listed.
//!
//! A hypervisor also carries its PROPERTY MAPPINGS ([`StoredPropertyMappings`]) — the
//! VMware-to-Amazon tag mappings and the IAM role they are applied with. They live on
//! the hypervisor record rather than in a map beside it, because that is the resource
//! they belong to: `GetHypervisorPropertyMappings` reports the mappings of the record its
//! `ServerArn` resolves to, and a hypervisor that carries none has no mappings resource
//! to report ([`MissingResource::PropertyMappings`]).

use std::collections::BTreeMap;
use std::ops::Bound;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    BandwidthRateLimitInterval, Gateway, GatewayDetails, Hypervisor, HypervisorDetails,
    MaintenanceStartTime, Tag,
};

/// The modelled mapping shape a stored property mapping is kept as, re-exported so the
/// record, the handlers and a test all name ONE type
/// (`smithy_query(query="shape",shape="VmwareToAwsTagMapping")`: `VmwareCategory`,
/// `VmwareTagName`, `AwsTagKey`, `AwsTagValue`, all required) instead of mirroring it.
pub use crate::ops::types::VmwareToAwsTagMapping;

/// This service's ARN namespace (`smithy_query(query="service_info")`:
/// `awsService.arnNamespace`), and the second-to-last ARN segment every gateway ARN
/// carries.
pub const ARN_SERVICE: &str = "backup-gateway";

/// The ARN resource type of a gateway: `gateway/<id>`.
pub const GATEWAY_RESOURCE_TYPE: &str = "gateway";

/// The ARN resource type of a hypervisor: `hypervisor/<id>`.
///
/// The hypervisor is the second resource this service's ARNs name
/// (`smithy_query(query="shape",shape="GetHypervisorInput")`: `HypervisorArn` targets
/// `ServerArn`), and the tagging operations take either kind in the one `ResourceARN`
/// member — so they have to tell them apart. See [`MissingResource`].
pub const HYPERVISOR_RESOURCE_TYPE: &str = "hypervisor";

/// `ValidationException$ErrorCode` for a `CreateGateway` whose activation key this
/// service never issued, and the sentence that states it.
///
/// `ValidationException$ErrorCode` is documented as "A description of what caused the
/// validation error" (`smithy_query(query="shape",shape="ValidationException")`), and
/// the code AWS answers an unissued activation key with is `ActivationKeyNotFound` —
/// the same name the sibling gateway service spells in its own `ErrorCode` vocabulary.
pub const ACTIVATION_KEY_NOT_FOUND: &str = "ActivationKeyNotFound";

/// See [`ACTIVATION_KEY_NOT_FOUND`].
pub const ACTIVATION_KEY_NOT_FOUND_MESSAGE: &str = "The specified activation key was not found.";

/// `ResourceNotFoundException$ErrorCode` for a well-formed hypervisor ARN that names no
/// hypervisor, and the sentence that states it. See [`MissingResource`].
pub const HYPERVISOR_NOT_FOUND: &str = "HypervisorNotFound";

/// See [`HYPERVISOR_NOT_FOUND`].
pub const HYPERVISOR_NOT_FOUND_MESSAGE: &str = "The hypervisor is not found.";

/// `ResourceNotFoundException$ErrorCode` for a hypervisor that really exists but carries
/// no property mappings, so there is no mappings resource to report. See
/// [`MissingResource::PropertyMappings`].
///
/// `ErrorCode` is documented as "A description of which resource wasn't found"
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`), and the resource
/// that is missing here is the MAPPINGS, not the hypervisor — so the code names them,
/// the way [`HYPERVISOR_NOT_FOUND`] names the hypervisor.
pub const MAPPINGS_NOT_FOUND: &str = "MappingsNotFound";

/// See [`MAPPINGS_NOT_FOUND`]. Phrased like [`HYPERVISOR_NOT_FOUND_MESSAGE`], the
/// sentence this service is recorded stating for a resource it holds nothing for; the
/// requirement is the error shape and the `ErrorCode`, and this wording is an example.
pub const MAPPINGS_NOT_FOUND_MESSAGE: &str = "The mappings are not found.";

/// The region a minted ARN falls back to when the request carries no SigV4 credential
/// scope to read one from (an unsigned request on an emulator with authentication off).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The version the emulated gateway appliance reports as `SoftwareVersion`.
///
/// `GatewayDetails$SoftwareVersion` targets the `Name` shape, whose modelled pattern is
/// `^[a-zA-Z0-9-]*$` (`smithy_query(query="shape",shape="GatewayDetails")`), so a dotted
/// version is not a legal value for this member — the appliance version is rendered with
/// hyphens.
pub const GATEWAY_SOFTWARE_VERSION: &str = "2-0-0";

/// The only `GatewayType` the model declares (`GATEWAY_TYPE_VALUES`).
pub const BACKUP_VM: &str = "BACKUP_VM";

/// `HypervisorState` right after a hypervisor configuration is imported, before the
/// service has reached the host.
///
/// One of the four values the modelled enum declares — `PENDING`, `ONLINE`, `OFFLINE`,
/// `ERROR` (`smithy_query(query="shape",shape="HypervisorState")`), whose documented
/// order the `HypervisorDetails$State` doc restates.
pub const HYPERVISOR_PENDING: &str = "PENDING";

/// `HypervisorState` once the service has reached the hypervisor host. See
/// [`HYPERVISOR_PENDING`].
pub const HYPERVISOR_ONLINE: &str = "ONLINE";

/// How long a freshly imported hypervisor stays [`HYPERVISOR_PENDING`] before it is
/// [`HYPERVISOR_ONLINE`].
///
/// A CHOSEN span, not a documented one: AWS states no duration for the activation an
/// import starts, only that a hypervisor's state passes through `PENDING`. It is
/// non-zero so a caller that reads straight after an import still observes `PENDING`,
/// and short enough that a caller which waits sees `ONLINE`. Nothing sleeps for it — the
/// due instant is stored and every read compares the clock against it
/// ([`StoredHypervisor::state_as_of`]).
pub const HYPERVISOR_PENDING_FOR: Duration = Duration::from_millis(500);

/// `ConflictException$ErrorCode` for an import naming a host this service has already
/// imported a hypervisor for, and the sentence that states it.
///
/// A hypervisor IS its host: the record stands for the machine reachable at that address,
/// so importing the same host twice cannot mint a second one — `ConflictException` is the
/// modelled "the operation cannot proceed because it is not supported"
/// (`smithy_query(query="shape",shape="ConflictException")`), whose `ErrorCode` member is
/// REQUIRED and documented as "a description of why the operation is not supported". AWS
/// publishes no vocabulary for that member, so this value names the condition.
pub const HYPERVISOR_ALREADY_EXISTS: &str = "HypervisorAlreadyExists";

/// See [`HYPERVISOR_ALREADY_EXISTS`].
pub const HYPERVISOR_ALREADY_EXISTS_MESSAGE: &str =
    "A hypervisor has already been imported for this host.";

/// The largest number of gateways this emulator keeps.
///
/// A ceiling, not an AWS quota: `CreateGateway` is the one operation that grows resident
/// memory, one record per call, for the life of the process, so the store is bounded and
/// a create past the ceiling is refused rather than served. The same bound the other
/// services in this workspace use for a caller-driven store.
pub const MAX_STORED_GATEWAYS: usize = 1_000;

/// The largest number of hypervisors this emulator keeps, for the same reason as
/// [`MAX_STORED_GATEWAYS`]: `ImportHypervisorConfiguration` grows resident memory one
/// record per call, so the store is bounded and an import past the ceiling is refused
/// rather than served.
pub const MAX_STORED_HYPERVISORS: usize = 1_000;

/// The page size a list operation of this service uses when a request names no
/// `MaxResults`, and the largest page it will return whatever `MaxResults` asks for.
///
/// The model declares neither (`MaxResults` has a minimum of 1 and no maximum), so these
/// are the emulator's own server-side bounds: without them one tiny request would clone
/// and serialise the whole store, so the response a caller can ask for is capped and the
/// rest is reached through `NextToken`.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// See [`DEFAULT_PAGE_SIZE`].
pub const MAX_PAGE_SIZE: usize = 100;

/// How many tags one resource can carry.
///
/// From the modelled doc of `CreateGatewayInput$Tags`: "A list of up to 50 tags to
/// assign to the gateway. Each tag is a key-value pair." The `Tags` list itself carries
/// no `@length` trait, so this limit is not refused at the boundary and the handlers
/// enforce it.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// One persisted gateway.
///
/// Keyed in the store by [`StoredGateway::arn`]; `gateway_id` is the ARN's resource
/// suffix, kept beside it so a later slice can resolve a gateway by id without
/// re-parsing the ARN.
#[derive(Debug, Clone)]
pub struct StoredGateway {
    /// The gateway's ARN — the store's key and the identity every operation takes.
    pub arn: String,
    /// The generated id in the ARN's `gateway/<id>` suffix.
    pub gateway_id: String,
    /// `GatewayDisplayName`, as last set by create or `UpdateGatewayInformation`.
    pub display_name: String,
    /// `GatewayType`; the model declares only [`BACKUP_VM`].
    pub gateway_type: String,
    /// The activation key the gateway was created with. Not part of any response
    /// shape — kept because it identifies the appliance this record stands for.
    pub activation_key: String,
    /// The hypervisor this gateway is associated with, once `AssociateGatewayToServer`
    /// is implemented. `None` means "not associated".
    pub hypervisor_arn: Option<String>,
    /// `HypervisorId`, absent until the gateway is associated with a hypervisor.
    pub hypervisor_id: Option<String>,
    /// `LastSeenTime` — when the appliance last communicated with the cloud. Set when
    /// the record is minted and moved only by a call that really contacts the gateway;
    /// a read never advances it.
    pub last_seen_time: SystemTime,
    /// When this record was created.
    pub created_time: SystemTime,
    /// `SoftwareVersion` of the appliance.
    pub software_version: String,
    /// `VpcEndpoint`, set only for a gateway activated through a VPC endpoint.
    pub vpc_endpoint: Option<String>,
    /// `MaintenanceStartTime`, absent until `PutMaintenanceStartTime` sets one.
    pub maintenance_start_time: Option<MaintenanceStartTime>,
    /// The bandwidth rate-limit schedule, empty until
    /// `PutBandwidthRateLimitSchedule` sets one.
    pub bandwidth_rate_limit_intervals: Vec<BandwidthRateLimitInterval>,
    /// The resource's tags, by key. A map, not a list: `TagResource` merges by key and
    /// the same key set twice is one tag.
    pub tags: BTreeMap<String, String>,
}

impl StoredGateway {
    /// This record as the `GatewayDetails` shape `GetGateway` returns.
    ///
    /// `NextUpdateAvailabilityTime` and `DeprecationDate` stay absent: no appliance
    /// update is scheduled and no deprecation date is set for an emulated gateway.
    pub fn details(&self) -> GatewayDetails {
        GatewayDetails {
            gateway_arn: Some(self.arn.clone()),
            gateway_display_name: Some(self.display_name.clone()),
            gateway_type: Some(self.gateway_type.clone()),
            hypervisor_id: self.hypervisor_id.clone(),
            last_seen_time: Some(date_time(self.last_seen_time)),
            maintenance_start_time: self.maintenance_start_time.clone(),
            next_update_availability_time: None,
            vpc_endpoint: self.vpc_endpoint.clone(),
            deprecation_date: None,
            software_version: Some(self.software_version.clone()),
        }
    }

    /// This record as the `Gateway` summary shape `ListGateways` returns.
    pub fn summary(&self) -> Gateway {
        Gateway {
            gateway_arn: Some(self.arn.clone()),
            gateway_display_name: Some(self.display_name.clone()),
            gateway_type: Some(self.gateway_type.clone()),
            hypervisor_id: self.hypervisor_id.clone(),
            last_seen_time: Some(date_time(self.last_seen_time)),
        }
    }

    /// The persisted tags as the modelled `Tags` list, in key order.
    pub fn tag_list(&self) -> Vec<Tag> {
        tag_list(&self.tags)
    }
}

/// A stored tag map as the modelled `Tags` list, in key order. One rendering for every
/// resource this service tags.
fn tag_list(tags: &BTreeMap<String, String>) -> Vec<Tag> {
    tags.iter()
        .map(|(key, value)| Tag {
            key: key.clone(),
            value: value.clone(),
        })
        .collect()
}

/// One persisted hypervisor.
///
/// Keyed in the store by [`StoredHypervisor::arn`], the identity every hypervisor
/// operation takes (`GetHypervisor`, `ListHypervisors`, `UpdateHypervisor`,
/// `DeleteHypervisor` all name or return a `ServerArn`). The members are exactly the
/// ones `HypervisorDetails` reports
/// (`smithy_query(query="shape",shape="HypervisorDetails")`), so a read serialises the
/// record rather than recomputing anything from the request — the hypervisor's
/// credentials are NOT among them and are not kept here.
#[derive(Debug, Clone)]
pub struct StoredHypervisor {
    /// The hypervisor's ARN — the store's key. The generated id is its
    /// `hypervisor/<id>` suffix and is not kept beside it: one source of truth for the
    /// identity.
    pub arn: String,
    /// `HypervisorDetails$Name`.
    pub name: String,
    /// `HypervisorDetails$Host` — an IP address or an FQDN.
    pub host: String,
    /// `HypervisorDetails$KmsKeyArn`, absent unless the import named one.
    pub kms_key_arn: Option<String>,
    /// `HypervisorDetails$LogGroupArn`, absent until `UpdateHypervisor` sets one.
    pub log_group_arn: Option<String>,
    /// `HypervisorDetails$State` as last PERSISTED; what a read reports is
    /// [`StoredHypervisor::state_as_of`], which also accounts for the activation the
    /// clock has already made due.
    pub state: String,
    /// When this record's [`HYPERVISOR_PENDING`] state becomes [`HYPERVISOR_ONLINE`],
    /// or `None` for a record whose state no longer moves on its own.
    pub online_due: Option<SystemTime>,
    /// `LastSuccessfulMetadataSyncTime`, absent until a metadata sync has succeeded.
    pub last_successful_metadata_sync_time: Option<SystemTime>,
    /// `LatestMetadataSyncStatus` — a `SyncMetadataStatus` value — absent until a
    /// metadata sync has been started.
    pub latest_metadata_sync_status: Option<String>,
    /// `LatestMetadataSyncStatusMessage`.
    pub latest_metadata_sync_status_message: Option<String>,
    /// When this record was created.
    pub created_time: SystemTime,
    /// The resource's tags, by key — the same representation [`StoredGateway::tags`]
    /// uses, so the tagging operations treat both resources alike.
    pub tags: BTreeMap<String, String>,
    /// The hypervisor's property mappings, absent until they have been SET for it.
    ///
    /// An import never mints mappings: `PutHypervisorPropertyMappings` is the operation
    /// that writes them (still a scaffold answering 501 under its own stub issue), and
    /// `GetHypervisorPropertyMappings` reports whatever this field holds. Absent is a
    /// meaningful value and not an empty mapping set: a hypervisor with no mappings has
    /// no mappings resource to report ([`MissingResource::PropertyMappings`]), where one
    /// whose mappings are an empty list has.
    pub property_mappings: Option<StoredPropertyMappings>,
}

impl StoredHypervisor {
    /// The `HypervisorState` this hypervisor is in at `now`.
    ///
    /// An import leaves the hypervisor [`HYPERVISOR_PENDING`] with the instant its
    /// activation falls due; the clock alone moves it to [`HYPERVISOR_ONLINE`], so the
    /// state a read reports is a function of the record and the current time and is the
    /// same for every reader — no call has to poll for the transition to happen, and a
    /// read does not cause it.
    pub fn state_as_of(&self, now: SystemTime) -> &str {
        if self.state == HYPERVISOR_PENDING && self.online_due.is_some_and(|due| now >= due) {
            HYPERVISOR_ONLINE
        } else {
            &self.state
        }
    }

    /// This record as the `HypervisorDetails` shape `GetHypervisor` returns, as of `now`.
    pub fn details_as_of(&self, now: SystemTime) -> HypervisorDetails {
        HypervisorDetails {
            host: Some(self.host.clone()),
            hypervisor_arn: Some(self.arn.clone()),
            kms_key_arn: self.kms_key_arn.clone(),
            name: Some(self.name.clone()),
            log_group_arn: self.log_group_arn.clone(),
            state: Some(self.state_as_of(now).to_string()),
            last_successful_metadata_sync_time: self
                .last_successful_metadata_sync_time
                .map(date_time),
            latest_metadata_sync_status_message: self.latest_metadata_sync_status_message.clone(),
            latest_metadata_sync_status: self.latest_metadata_sync_status.clone(),
        }
    }

    /// This record as the `Hypervisor` summary shape `ListHypervisors` returns, as of
    /// `now`.
    ///
    /// The summary is the subset of [`Self::details_as_of`] the `Hypervisor` shape
    /// declares (`smithy_query(query="shape",shape="Hypervisor")`: `Host`,
    /// `HypervisorArn`, `KmsKeyArn`, `Name`, `State`) — read off the same record and the
    /// same clock, so a listing and a get cannot disagree about one hypervisor.
    pub fn summary_as_of(&self, now: SystemTime) -> Hypervisor {
        Hypervisor {
            host: Some(self.host.clone()),
            hypervisor_arn: Some(self.arn.clone()),
            kms_key_arn: self.kms_key_arn.clone(),
            name: Some(self.name.clone()),
            state: Some(self.state_as_of(now).to_string()),
        }
    }

    /// The persisted tags as the modelled `Tags` list, in key order.
    pub fn tag_list(&self) -> Vec<Tag> {
        tag_list(&self.tags)
    }
}

/// The property mappings SET for one hypervisor.
///
/// The members are exactly the ones `GetHypervisorPropertyMappings` reports beside the
/// `HypervisorArn` (`smithy_query(query="operation",
/// operation="GetHypervisorPropertyMappings")`: `IamRoleArn`, `VmwareToAwsTagMappings`),
/// kept as the MODELLED [`VmwareToAwsTagMapping`] shape so a member cannot drift between
/// what is stored and what is serialised, and in the order they were set — a list, not a
/// set, is what the model declares.
#[derive(Debug, Clone)]
pub struct StoredPropertyMappings {
    /// `IamRoleArn` — the role the mappings are applied with.
    pub iam_role_arn: String,
    /// `VmwareToAwsTagMappings`, in the order they were set.
    pub vmware_to_aws_tag_mappings: Vec<VmwareToAwsTagMapping>,
}

/// What an import asks for a hypervisor to be created with: the members
/// `ImportHypervisorConfiguration` takes that a later read reports
/// (`smithy_query(query="operation",operation="ImportHypervisorConfiguration")`). Its
/// `Username` / `Password` are `@sensitive` credentials no response member carries, so
/// they are deliberately absent here and never persisted.
#[derive(Debug, Clone, Default)]
pub struct HypervisorConfiguration {
    /// `Name` — required by the import's input.
    pub name: String,
    /// `Host` — required by the import's input.
    pub host: String,
    /// `KmsKeyArn`, optional.
    pub kms_key_arn: Option<String>,
    /// `Tags` — "the tags of the hypervisor configuration to import" — by key, as
    /// [`tag_map`] validated them.
    pub tags: BTreeMap<String, String>,
}

/// One page of a list operation: the records it returns and the token that resumes
/// after them, present only while more records remain.
///
/// One shape for every paginated listing of this service, so a page boundary means the
/// same thing whichever resource is being listed.
#[derive(Debug, Default)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_token: Option<String>,
}

/// Per-service state for com-amazonaws-backupgateway-backuponpremises-v20210101. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsBackupgatewayBackuponpremisesV20210101State {
    /// Gateways by ARN. A `BTreeMap` because `ListGateways` paginates over it: the key
    /// order is total and stable, so a page boundary means the same thing on every call
    /// and a token can name the key it resumes after.
    gateways: BTreeMap<String, StoredGateway>,
    /// Monotonic counter behind the `gateway/<id>` ARN suffix, so every minted ARN is
    /// unique within a run and the sequence is reproducible.
    next_gateway_id: u64,
    /// The activation keys this service has ISSUED, each with the instant it was issued
    /// at — the registry `CreateGateway` resolves its `ActivationKey` in.
    ///
    /// A gateway's activation key is not minted by this API: it reaches the service out
    /// of band, when the gateway appliance a customer deployed completes its activation
    /// handshake, which is not an operation of the SDK surface (no operation in this
    /// model issues one). So on AWS, as here, `CreateGateway` can only ever find a key
    /// something OUTSIDE the API put in this registry, and a key nothing issued is
    /// refused — see [`ComAmazonawsBackupgatewayBackuponpremisesV20210101State::activation_key_issued`].
    activation_keys: BTreeMap<String, SystemTime>,
    /// Hypervisors by ARN. A `BTreeMap` for the same reason as `gateways`: the key order
    /// is total and stable, and it is the order `ListHypervisors` reports them in —
    /// "ordered by their Amazon Resource Names (ARNs)"
    /// (`smithy_query(query="operation",operation="ListHypervisors")`) — so a page
    /// boundary means the same thing on every call and a token can name the key it
    /// resumes after.
    hypervisors: BTreeMap<String, StoredHypervisor>,
    /// Monotonic counter behind the `hypervisor/<id>` ARN suffix.
    next_hypervisor_id: u64,
}

impl ComAmazonawsBackupgatewayBackuponpremisesV20210101State {
    /// Record `key` as an activation key this service has issued, and hand back the
    /// instant it is now registered under — read back out of the map after the insert.
    ///
    /// This is the out-of-band channel described on `activation_keys`: the appliance
    /// handshake that issues a key on AWS has no operation in this model, so nothing
    /// a client can send over the wire reaches here. Re-issuing a key that is already
    /// registered keeps the instant it was first issued at: one key, one issuance.
    pub fn issue_activation_key(&mut self, key: &str, now: SystemTime) -> SystemTime {
        *self.activation_keys.entry(key.to_string()).or_insert(now)
    }

    /// Whether this service issued `key`.
    ///
    /// `CreateGateway` registers a gateway only for a key this answers `true` for; for
    /// anything else it raises the modelled `ValidationException` carrying
    /// [`ACTIVATION_KEY_NOT_FOUND`], rather than minting a gateway for an appliance the
    /// service never activated. The answer comes from the registry alone — never from
    /// the key's length, spelling or shape, none of which AWS discloses.
    pub fn activation_key_issued(&self, key: &str) -> bool {
        self.activation_keys.contains_key(key)
    }
    /// Mint and persist a gateway, and hand back the record as it is now STORED —
    /// read out of the map after the insert, so the ARN a caller gets is the key the
    /// store holds rather than anything derived from the request.
    pub fn create_gateway(
        &mut self,
        region: &str,
        activation_key: &str,
        display_name: &str,
        gateway_type: &str,
        tags: BTreeMap<String, String>,
        now: SystemTime,
    ) -> StoredGateway {
        let gateway_id = format!("bgw-{:016x}", self.next_gateway_id);
        self.next_gateway_id += 1;
        let arn = ::engine::emulator_arn(
            ARN_SERVICE,
            &arn_region(region),
            &format!("{GATEWAY_RESOURCE_TYPE}/{gateway_id}"),
        );
        let record = StoredGateway {
            arn: arn.clone(),
            gateway_id,
            display_name: display_name.to_string(),
            gateway_type: gateway_type.to_string(),
            activation_key: activation_key.to_string(),
            hypervisor_arn: None,
            hypervisor_id: None,
            last_seen_time: now,
            created_time: now,
            software_version: GATEWAY_SOFTWARE_VERSION.to_string(),
            vpc_endpoint: None,
            maintenance_start_time: None,
            bandwidth_rate_limit_intervals: Vec::new(),
            tags,
        };
        // `or_insert` hands back a reference INTO the map, so the record returned is
        // the one the store now holds; the id counter makes the key fresh, so this
        // always inserts.
        self.gateways.entry(arn).or_insert(record).clone()
    }

    /// The gateway stored under `arn`, if any.
    pub fn gateway(&self, arn: &str) -> Option<&StoredGateway> {
        self.gateways.get(arn)
    }

    /// The gateway stored under `arn`, for in-place mutation.
    pub fn gateway_mut(&mut self, arn: &str) -> Option<&mut StoredGateway> {
        self.gateways.get_mut(arn)
    }

    /// Remove the gateway stored under `arn`, returning the record that was removed.
    pub fn remove_gateway(&mut self, arn: &str) -> Option<StoredGateway> {
        self.gateways.remove(arn)
    }

    /// How many gateways are stored.
    pub fn gateway_count(&self) -> usize {
        self.gateways.len()
    }

    /// One page of gateways in key order.
    ///
    /// `after` is the ARN the previous page ended on (resolved from a `NextToken`), and
    /// the page starts at the next key AFTER it — so a record created, or deleted,
    /// between two pages changes what the second page holds without ever repeating or
    /// skipping a record that stayed put. `limit` is the caller's `MaxResults` already
    /// clamped to [`MAX_PAGE_SIZE`] by the handler, so no request can make one response
    /// carry the whole store.
    pub fn page_of_gateways(&self, after: Option<&str>, limit: usize) -> Page<StoredGateway> {
        page_of(&self.gateways, after, limit)
    }

    /// Persist a hypervisor for `configuration` and hand back the record as it is now
    /// STORED — read out of the map after the insert, so the ARN that comes back is the
    /// key the store holds rather than a string built from the request.
    ///
    /// The record is minted [`HYPERVISOR_PENDING`] with the instant its activation falls
    /// due ([`HYPERVISOR_PENDING_FOR`] after `now`): on AWS an imported hypervisor is not
    /// reachable the moment the import returns, and the state a read reports is derived
    /// from that stored instant rather than advanced by anything a caller does.
    ///
    /// `ImportHypervisorConfiguration` is the operation that performs this: its handler
    /// resolves the host, bounds the store and then calls here, so the record a caller
    /// gets an ARN for is the one this map holds.
    pub fn import_hypervisor(
        &mut self,
        region: &str,
        configuration: HypervisorConfiguration,
        now: SystemTime,
    ) -> StoredHypervisor {
        let hypervisor_id = format!("hv-{:016x}", self.next_hypervisor_id);
        self.next_hypervisor_id += 1;
        let arn = ::engine::emulator_arn(
            ARN_SERVICE,
            &arn_region(region),
            &format!("{HYPERVISOR_RESOURCE_TYPE}/{hypervisor_id}"),
        );
        let record = StoredHypervisor {
            arn: arn.clone(),
            name: configuration.name,
            host: configuration.host,
            kms_key_arn: configuration.kms_key_arn,
            log_group_arn: None,
            state: HYPERVISOR_PENDING.to_string(),
            online_due: Some(now + HYPERVISOR_PENDING_FOR),
            last_successful_metadata_sync_time: None,
            latest_metadata_sync_status: None,
            latest_metadata_sync_status_message: None,
            created_time: now,
            tags: configuration.tags,
            // An import sets no property mappings: they are SET for a hypervisor that
            // already exists, so a freshly imported record carries none.
            property_mappings: None,
        };
        // `or_insert` hands back a reference INTO the map, so the record returned is the
        // one the store now holds; the id counter makes the key fresh, so this always
        // inserts.
        self.hypervisors.entry(arn).or_insert(record).clone()
    }

    /// The hypervisor stored under `arn`, if any.
    ///
    /// The ONE lookup every hypervisor operation resolves its `ServerArn` through: an ARN
    /// with no record behind it is absent here, whether it was never imported or has been
    /// deleted, which is the same answer the real service gives for both.
    pub fn hypervisor(&self, arn: &str) -> Option<&StoredHypervisor> {
        self.hypervisors.get(arn)
    }

    /// The hypervisor stored under `arn`, for in-place mutation.
    pub fn hypervisor_mut(&mut self, arn: &str) -> Option<&mut StoredHypervisor> {
        self.hypervisors.get_mut(arn)
    }

    /// Remove the hypervisor stored under `arn`, returning the record that was removed.
    pub fn remove_hypervisor(&mut self, arn: &str) -> Option<StoredHypervisor> {
        self.hypervisors.remove(arn)
    }

    /// How many hypervisors are stored.
    pub fn hypervisor_count(&self) -> usize {
        self.hypervisors.len()
    }

    /// One page of hypervisors in ARN order — the order `ListHypervisors` reports them
    /// in. Same contract as [`Self::page_of_gateways`], whose implementation it shares.
    pub fn page_of_hypervisors(&self, after: Option<&str>, limit: usize) -> Page<StoredHypervisor> {
        page_of(&self.hypervisors, after, limit)
    }

    /// The hypervisor stored for `host`, if any.
    ///
    /// A hypervisor record stands for the machine reachable at its host, so the host is
    /// what an import resolves against before minting a second record for it.
    pub fn hypervisor_by_host(&self, host: &str) -> Option<&StoredHypervisor> {
        self.hypervisors
            .values()
            .find(|hypervisor| hypervisor.host == host)
    }

    /// The ARN `arn` is stored under and the tags the resource it names carries, or
    /// `None` for an ARN this store holds no record for.
    ///
    /// The tagging operations take any of this service's resources in one `ResourceARN`
    /// member, so they resolve it through this one lookup rather than knowing which map
    /// holds which kind: the ARN comes back out of the store, so what a response echoes
    /// is the key the store holds.
    pub fn resource_tags(&self, arn: &str) -> Option<(String, Vec<Tag>)> {
        if let Some(gateway) = self.gateways.get(arn) {
            return Some((gateway.arn.clone(), gateway.tag_list()));
        }
        let hypervisor = self.hypervisors.get(arn)?;
        Some((hypervisor.arn.clone(), hypervisor.tag_list()))
    }

    /// Merge `requested` into the tags of the resource `arn` names, and hand back the ARN
    /// the store holds it under.
    ///
    /// `None` for an ARN this store holds no record for; `Err` for tags the model's own
    /// traits cannot refuse ([`merged_tags`]), in which case NOTHING is written. Spans
    /// both resource types for the same reason as [`resource_tags`].
    pub fn merge_resource_tags(
        &mut self,
        arn: &str,
        requested: &[Tag],
    ) -> Option<Result<String, String>> {
        if let Some(gateway) = self.gateways.get_mut(arn) {
            return Some(merged_tags(&gateway.tags, requested).map(|merged| {
                gateway.tags = merged;
                gateway.arn.clone()
            }));
        }
        let hypervisor = self.hypervisors.get_mut(arn)?;
        Some(merged_tags(&hypervisor.tags, requested).map(|merged| {
            hypervisor.tags = merged;
            hypervisor.arn.clone()
        }))
    }
}

/// One page of an ARN-keyed resource map in key order — the paging every list
/// operation of this service does, written once.
///
/// `after` is the key the previous page ended on (resolved from a `NextToken`), and the
/// page starts at the next key AFTER it — so a record created, or deleted, between two
/// pages changes what the second page holds without ever repeating or skipping a record
/// that stayed put. `limit` is the caller's `MaxResults` already clamped to
/// [`MAX_PAGE_SIZE`] by the handler, so no request can make one response carry a whole
/// store.
fn page_of<T: Clone>(records: &BTreeMap<String, T>, after: Option<&str>, limit: usize) -> Page<T> {
    let lower = match after {
        Some(key) => Bound::Excluded(key.to_string()),
        None => Bound::Unbounded,
    };
    let mut remaining = records.range((lower, Bound::Unbounded));
    let page: Vec<(&String, &T)> = remaining.by_ref().take(limit).collect();
    // A token only when a record really remains past this page: the final page must
    // not carry one, or a paginating client never stops.
    let next_token = remaining
        .next()
        .and_then(|_| page.last().map(|(key, _)| encode_page_token(key)));
    Page {
        items: page.into_iter().map(|(_, record)| record.clone()).collect(),
        next_token,
    }
}

/// Merge a request's `Tags` into the tags a resource already carries, refusing what the
/// model's own traits cannot express.
///
/// Two rules, both from the model's docs rather than its traits — so the boundary
/// validator cannot have refused them already:
///
/// * `Tag$Key` "can't start with `aws:`" (that prefix is reserved for AWS-generated
///   tags), which the member's `@pattern` allows;
/// * a resource carries at most [`MAX_TAGS_PER_RESOURCE`] tags.
///
/// `Err` carries the sentence the refusal states. Merging is by key, so a key already
/// present is overwritten rather than duplicated.
pub fn merged_tags(
    existing: &BTreeMap<String, String>,
    requested: &[Tag],
) -> Result<BTreeMap<String, String>, String> {
    let mut merged = existing.clone();
    for tag in requested {
        if tag.key.to_ascii_lowercase().starts_with("aws:") {
            return Err(format!(
                "Value '{}' at 'Tags' failed to satisfy constraint: a tag key must not \
                 start with the reserved prefix 'aws:'",
                tag.key
            ));
        }
        merged.insert(tag.key.clone(), tag.value.clone());
    }
    if merged.len() > MAX_TAGS_PER_RESOURCE {
        return Err(format!(
            "Value '{}' at 'Tags' failed to satisfy constraint: a resource must carry at \
             most {MAX_TAGS_PER_RESOURCE} tags",
            merged.len()
        ));
    }
    Ok(merged)
}

/// A request's `Tags` as the stored tag map, for a resource that carries none yet.
/// Same rules as [`merged_tags`], which this delegates to.
pub fn tag_map(requested: &[Tag]) -> Result<BTreeMap<String, String>, String> {
    merged_tags(&BTreeMap::new(), requested)
}

/// The opaque pagination token for a page that ended on `arn`.
///
/// The token is the page boundary itself (the last ARN returned), hex-encoded so a
/// client has nothing to read in it and treats it as opaque, as the modelled `NextToken`
/// requires. Encoding the boundary rather than storing a cursor server-side is what lets
/// a token stay resolvable for as long as the client holds it, with no per-caller state.
pub fn encode_page_token(arn: &str) -> String {
    hex::encode(arn)
}

/// Resolve a client-supplied `NextToken` back to the ARN its page ended on, for a
/// listing of `resource_type`.
///
/// `None` for anything this service cannot have issued for THIS listing: not hex, not
/// UTF-8, not an ARN of this service, or an ARN naming another resource type — a token
/// one listing handed out does not resume another, because the page boundary it encodes
/// is a key of a different map. The caller answers `None` with the modelled
/// `ValidationException`, never by silently restarting the listing from the top: a
/// client that mangled a token must learn that, not receive page one again.
pub fn decode_page_token(token: &str, resource_type: &str) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    is_resource_arn(&decoded, resource_type).then_some(decoded)
}

/// Whether `candidate` is an ARN of this service naming `resource_type`, in the form
/// the modelled `GatewayArn` and `ServerArn` patterns accept:
/// `arn:(aws|aws-cn|aws-us-gov):backup-gateway:<region>:<account>:<type>/<id>`, every
/// segment non-empty and drawn from `[a-zA-Z-0-9]`.
pub fn is_resource_arn(candidate: &str, resource_type: &str) -> bool {
    let fields: Vec<&str> = candidate.split(':').collect();
    let [arn, partition, service, region, account, resource] = fields.as_slice() else {
        return false;
    };
    if *arn != "arn" || !matches!(*partition, "aws" | "aws-cn" | "aws-us-gov") {
        return false;
    }
    if *service != ARN_SERVICE {
        return false;
    }
    let Some((kind, id)) = resource.split_once('/') else {
        return false;
    };
    kind == resource_type
        && [*region, *account, kind, id]
            .into_iter()
            .all(is_arn_segment)
}

/// The resource-type segment of a backup-gateway ARN — the `gateway` of
/// `arn:aws:backup-gateway:<region>:<account>:gateway/<id>` — or `None` for anything
/// that is not an ARN of this service at all.
///
/// The one place an ARN is classified: the tagging operations take any of this
/// service's resources in a single `ResourceARN` member, so the resource they look the
/// ARN up as is read off the ARN rather than guessed from which store happens to hold
/// a record.
pub fn arn_resource_type(arn: &str) -> Option<&str> {
    let fields: Vec<&str> = arn.split(':').collect();
    let [literal, partition, service, _region, _account, resource] = fields.as_slice() else {
        return None;
    };
    if *literal != "arn" || !matches!(*partition, "aws" | "aws-cn" | "aws-us-gov") {
        return None;
    }
    if *service != ARN_SERVICE {
        return None;
    }
    resource.split_once('/').map(|(kind, _id)| kind)
}

/// How this service refuses a well-formed resource ARN that names no resource it holds.
///
/// Every such refusal is the modelled `ResourceNotFoundException`, but this service does
/// not answer alike for all resource types: a missing HYPERVISOR is answered at HTTP 400
/// with `ErrorCode` [`HYPERVISOR_NOT_FOUND`], where every other resource type takes the
/// 404 that shape's `@httpError` declares and states no `ErrorCode`. Both halves of a
/// refusal — the status and the code — are read off this one value, so they cannot be
/// chosen apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingResource {
    /// A `hypervisor/<id>` ARN with no hypervisor behind it.
    ///
    /// The store holds hypervisors ([`StoredHypervisor`]), so this arm is reached for a
    /// hypervisor ARN the store has no record under — never for one it does.
    Hypervisor,
    /// A hypervisor the store DOES hold, which carries no property mappings
    /// ([`StoredHypervisor::property_mappings`]) — so the resource that is missing is the
    /// mappings and not the hypervisor.
    ///
    /// Chosen by the operation that resolved the mappings, never by [`Self::for_arn`]:
    /// the ARN names a hypervisor in both cases, and only the record says which of the
    /// two is missing.
    PropertyMappings,
    /// Every other resource type, including a `gateway/<id>` ARN with no gateway
    /// behind it.
    Other,
}

impl MissingResource {
    /// Which refusal `arn` takes, read off the resource type the ARN names.
    pub fn for_arn(arn: &str) -> Self {
        match arn_resource_type(arn) {
            Some(HYPERVISOR_RESOURCE_TYPE) => Self::Hypervisor,
            _ => Self::Other,
        }
    }

    /// `ResourceNotFoundException$ErrorCode` — "A description of which resource wasn't
    /// found" (`smithy_query(query="shape",shape="ResourceNotFoundException")`) — or
    /// `None` for a resource type this service states no code for.
    pub fn error_code(self) -> Option<&'static str> {
        match self {
            Self::Hypervisor => Some(HYPERVISOR_NOT_FOUND),
            Self::PropertyMappings => Some(MAPPINGS_NOT_FOUND),
            Self::Other => None,
        }
    }

    /// The sentence the refusal states.
    pub fn message(self, arn: &str) -> String {
        match self {
            Self::Hypervisor => HYPERVISOR_NOT_FOUND_MESSAGE.to_string(),
            Self::PropertyMappings => MAPPINGS_NOT_FOUND_MESSAGE.to_string(),
            Self::Other => resource_not_found_message(arn),
        }
    }

    /// The status this service answers the refusal with where it is NOT the one
    /// `ResourceNotFoundException`'s `@httpError` declares (404), and `None` where it
    /// is: a missing hypervisor is answered 400 with the same shape.
    pub fn status_override(self) -> Option<u16> {
        match self {
            Self::Hypervisor => Some(400),
            // No recording shows this service answering a missing mappings resource with
            // anything but the status `ResourceNotFoundException`'s `@httpError` declares,
            // so that case keeps the model's 404 like every other resource type.
            Self::PropertyMappings | Self::Other => None,
        }
    }
}

/// The sentence `ResourceNotFoundException` states for an ARN this service holds no
/// record for. One wording for every operation that resolves an ARN.
pub fn resource_not_found_message(arn: &str) -> String {
    format!("The resource '{arn}' was not found.")
}

/// One `[a-zA-Z-0-9]+` ARN segment, as the modelled `GatewayArn` pattern spells it.
fn is_arn_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// The region to mint an ARN in, falling back to [`DEFAULT_REGION`] for anything that
/// would not satisfy the modelled ARN pattern's `[a-zA-Z-0-9]+` region segment.
pub fn arn_region(region: &str) -> String {
    if is_arn_segment(region) {
        region.to_string()
    } else {
        DEFAULT_REGION.to_string()
    }
}

/// The region this request was signed for, read from its SigV4 credential scope.
///
/// A resource is regional, and the region a client's ARN must carry is the one it
/// addressed the endpoint with — which on a signed request is the scope's third field.
/// An unsigned request (authentication off) carries none, so [`DEFAULT_REGION`] stands
/// in.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| arn_region(&scope.region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A stored instant as the modelled `Time` timestamp.
pub fn date_time(time: SystemTime) -> ::aws_smithy_types::DateTime {
    match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(since_epoch) => ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64()),
        Err(before_epoch) => {
            ::aws_smithy_types::DateTime::from_secs_f64(-before_epoch.duration().as_secs_f64())
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBackupgatewayBackuponpremisesV20210101State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBackupgatewayBackuponpremisesV20210101State>)
}
