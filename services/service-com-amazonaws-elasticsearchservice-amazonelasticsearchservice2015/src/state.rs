// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-elasticsearchservice-amazonelasticsearchservice2015 service.
//!
//! The service's primary resource is the Elasticsearch **domain**. One
//! [`Domain`] record is the single source of truth for every answer the domain
//! operations give: `CreateElasticsearchDomain` inserts it,
//! `UpdateElasticsearchDomainConfig` merges into it,
//! `DescribeElasticsearchDomain` / `DescribeElasticsearchDomainConfig` /
//! `ListDomainNames` read it, and `DeleteElasticsearchDomain` removes it.
//!
//! Every per-option `*Status` wrapper the model declares is BUILT FROM the
//! stored [`OptionRecord`] (its creation date, its update date, the config
//! version it last changed at, and the instant it finishes provisioning), never
//! fabricated per response — so two reads of an unchanged domain agree and a
//! read never has to write.
//!
//! The other resource this store holds is the outbound
//! [`CrossClusterSearchConnection`]: `CreateOutboundCrossClusterSearchConnection`
//! mints one and `DescribeOutboundCrossClusterSearchConnections` reports it. Its
//! status is COMPUTED from the stored validation outcome and the instant that
//! outcome falls due, for the same reason: reporting it must not write.

use std::collections::BTreeMap;
use std::time::SystemTime;

// The service's modelled shapes. They are the stored representation as well as
// the wire one: a second, parallel set of structs here would let a describe and
// an update drift apart.
#[allow(unused_imports)]
use crate::ops::types::*;

/// The region an ARN or endpoint uses when the request carries no SigV4
/// credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The config version `CreateElasticsearchDomain` stamps on every option it
/// stores; `UpdateElasticsearchDomainConfig` bumps it from there.
pub const FIRST_CONFIG_VERSION: i32 = 1;

/// How many domains one account may hold in one region before
/// `CreateElasticsearchDomain` answers the modelled `LimitExceededException`
/// ("an exception for trying to create more than allowed resources").
///
/// 100 is the service's documented default quota for domains per account per
/// Region. Enforcing it keeps the store's entry count bounded by a stated
/// service limit rather than by how many creates a caller issues, and makes the
/// modelled limit error reachable for callers that test their own
/// limit handling.
pub const MAX_DOMAINS_PER_REGION: usize = 100;

/// A hard ceiling on the WHOLE store, across every region.
///
/// [`MAX_DOMAINS_PER_REGION`] alone does not bound it: the region is read from
/// the request's own credential scope, so a caller that varies that field would
/// get a fresh per-region allowance each time. This cap dominates the single
/// growth site instead — at most `MAX_DOMAINS_TOTAL` records can exist however
/// many regions are addressed, and a region partition exists only while it holds
/// a record, so the number of partitions is bounded by the same number. It sits
/// well above the per-region quota, so no single region's allowance is clipped
/// by it.
pub const MAX_DOMAINS_TOTAL: usize = 1_000;

/// The largest access-policy document a domain may carry, in bytes.
///
/// AWS documents a maximum domain access policy size of 100 KB. It is also the
/// bound on the largest caller-supplied payload one stored record retains, since
/// `PolicyDocument` itself declares no length constraint for the boundary
/// validator to enforce.
pub const MAX_ACCESS_POLICY_BYTES: usize = 100 * 1024;

/// `Some(reason)` when an access-policy document is larger than the documented
/// maximum. The reason names the member and its constraint.
pub fn access_policy_refusal(policy: &str) -> Option<String> {
    (policy.len() > MAX_ACCESS_POLICY_BYTES).then(|| {
        format!(
            "AccessPolicies document of {len} bytes failed to satisfy constraint: \
             an access policy must not be larger than {max} bytes",
            len = policy.len(),
            max = MAX_ACCESS_POLICY_BYTES
        )
    })
}

/// The longest snapshot suspension `AutomatedSnapshotPauseOptions` accepts, in
/// seconds.
///
/// Straight from that member's own documentation: "Maximum suspension duration:
/// 3 days."
pub const MAX_SNAPSHOT_PAUSE_SECONDS: f64 = 3.0 * 24.0 * 60.0 * 60.0;

/// The key one domain is stored under: the region it was created in and its
/// name.
pub fn domain_key(region: &str, domain_name: &str) -> (String, String) {
    (region.to_string(), domain_name.to_string())
}

/// `Some(reason)` when a requested snapshot-suspension window cannot hold:
/// either it ends before it starts, or it is longer than the documented
/// three-day maximum. The reason names the member and its constraint, so the
/// caller can fix the request from the message alone.
///
/// A half-specified window (a start with no end, or an end with no start) is
/// stored as given: the model makes both members optional and documents no rule
/// for one without the other, so there is no bound to apply and inventing a
/// refusal here would refuse a request the service accepts.
pub fn snapshot_pause_refusal(options: &AutomatedSnapshotPauseRequestOptions) -> Option<String> {
    let start = options.start_time.as_ref()?.as_secs_f64();
    let end = options.end_time.as_ref()?.as_secs_f64();
    if end < start {
        return Some(
            "AutomatedSnapshotPauseOptions.EndTime must not be earlier than \
             AutomatedSnapshotPauseOptions.StartTime"
                .to_string(),
        );
    }
    let days = (end - start) / (24.0 * 60.0 * 60.0);
    (end - start > MAX_SNAPSHOT_PAUSE_SECONDS).then(|| {
        format!(
            "AutomatedSnapshotPauseOptions suspension window of {days:.2} days failed to satisfy \
             constraint: maximum suspension duration is 3 days"
        )
    })
}

/// How long a domain and the options a call just changed stay `Processing`
/// before the clock makes them `Active`.
///
/// The emulator CHOOSES this span: real provisioning takes minutes, which no
/// caller of an emulator can wait for. It is deliberately non-zero, so a client
/// that polls immediately after a create or an update still observes
/// `DomainStatus.Processing == true` and `OptionStatus.State == "Processing"`
/// (both modelled states) before the transition lands. Nothing sleeps for it:
/// the due instant is stored and compared against the clock on each read.
const PROVISIONING_SPAN: std::time::Duration = std::time::Duration::from_millis(500);

/// Convert a stored instant into the model's timestamp type.
fn datetime_of(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The region to build this request's ARNs and endpoints from: the SigV4
/// credential scope's region, else [`DEFAULT_REGION`].
pub fn request_region(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `DomainId` — the account the domain lives in and its name, the shape AWS
/// reports (`<account id>/<domain name>`).
pub fn domain_id(domain_name: &str) -> String {
    format!("{}/{}", ::engine::emulator_account_id(), domain_name)
}

/// The domain's ARN: `arn:aws:es:<region>:<account>:domain/<name>` (the model's
/// own `es` ARN namespace, and the `CreateVpcEndpoint` `DomainArn` pattern).
pub fn domain_arn(region: &str, domain_name: &str) -> String {
    ::engine::emulator_arn("es", region, &format!("domain/{domain_name}"))
}

/// The domain's search endpoint:
/// `search-<name>-<suffix>.<region>.es.amazonaws.com`.
///
/// AWS's suffix is an opaque, per-domain token. Here it is the leading 26 hex
/// digits of a SHA-256 over the domain name and the store's monotonic sequence
/// number, so it is a valid DNS label, unique per created domain, and stable for
/// the life of that domain (every read returns the stored string).
pub fn domain_endpoint(domain_name: &str, region: &str, seq: u64) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(format!("{domain_name}/{seq}").as_bytes());
    let hex = hex::encode(digest);
    format!(
        "search-{domain_name}-{suffix}.{region}.es.amazonaws.com",
        suffix = &hex[..26]
    )
}

/// The engine the domain runs, derived from `ElasticsearchVersion`.
///
/// `ElasticsearchVersionString`'s pattern admits three forms — `X.Y`,
/// `OpenSearch_X.Y` and `OS_X.Y` — and `EngineType` has exactly two values, so
/// the prefixed forms are OpenSearch and a bare `X.Y` is Elasticsearch. A domain
/// created without a version is an Elasticsearch domain, this service's own
/// engine.
pub fn engine_type_of(version: Option<&str>) -> &'static str {
    match version {
        Some(v) if v.starts_with("OpenSearch_") || v.starts_with("OS_") => "OpenSearch",
        _ => "Elasticsearch",
    }
}

/// One configuration option as stored: its value plus the provenance the
/// model's `OptionStatus` reports.
#[derive(Debug, Clone)]
pub struct OptionRecord<T> {
    /// The option's current value.
    pub value: T,
    /// When this option was first set on the domain.
    pub creation_date: SystemTime,
    /// When it last changed.
    pub update_date: SystemTime,
    /// The domain's config version at that change.
    pub update_version: i32,
    /// When the clock makes this option `Active`.
    pub active_at: SystemTime,
}

impl<T> OptionRecord<T> {
    /// Record an option set for the first time, at config version `version`.
    pub fn new(value: T, now: SystemTime, version: i32) -> Self {
        Self {
            value,
            creation_date: now,
            update_date: now,
            update_version: version,
            active_at: now + PROVISIONING_SPAN,
        }
    }

    /// Replace the option's value, which re-starts its provisioning span and
    /// stamps it with the new config version.
    pub fn replace(&mut self, value: T, now: SystemTime, version: i32) {
        self.value = value;
        self.update_date = now;
        self.update_version = version;
        self.active_at = now + PROVISIONING_SPAN;
    }

    /// Whether the clock has not yet made this option active.
    pub fn processing(&self, now: SystemTime) -> bool {
        now < self.active_at
    }

    /// The option's modelled state — one of `OPTION_STATE_VALUES`.
    pub fn state(&self, now: SystemTime) -> &'static str {
        if self.processing(now) {
            "Processing"
        } else {
            "Active"
        }
    }

    /// The `OptionStatus` wrapper for this option, built from what is stored.
    pub fn status(&self, now: SystemTime) -> OptionStatus {
        OptionStatus {
            creation_date: datetime_of(self.creation_date),
            update_date: datetime_of(self.update_date),
            update_version: self.update_version,
            state: self.state(now).to_string(),
            pending_deletion: Some(false),
        }
    }
}

/// Store `value` into `slot` when the request supplied one, leaving the stored
/// value untouched when it did not. Answers whether anything was written, which
/// is what decides that a config version bump is owed.
pub fn apply_option<T>(
    slot: &mut Option<OptionRecord<T>>,
    value: Option<T>,
    now: SystemTime,
    version: i32,
) -> bool {
    let Some(value) = value else {
        return false;
    };
    match slot {
        Some(record) => record.replace(value, now, version),
        None => *slot = Some(OptionRecord::new(value, now, version)),
    }
    true
}

/// The subnets and security groups a VPC domain was created with, as the
/// `VPCDerivedInfo` the read operations report.
///
/// `VpcId` and `AvailabilityZones` are left unset: both are derived from the
/// subnets by EC2, which this emulator holds no state for, and the model makes
/// them optional.
pub fn vpc_derived_info(options: VPCOptions) -> VPCDerivedInfo {
    VPCDerivedInfo {
        vpc_id: None,
        subnet_ids: options.subnet_ids,
        availability_zones: None,
        security_group_ids: options.security_group_ids,
    }
}

/// `AdvancedSecurityOptionsInput` as the `AdvancedSecurityOptions` the read
/// operations report.
///
/// `MasterUserOptions` is deliberately dropped rather than stored: it carries
/// `MasterUserPassword`, and the output shape has no member for it, so keeping
/// it would only risk disclosing a credential the caller supplied.
pub fn advanced_security_options(input: AdvancedSecurityOptionsInput) -> AdvancedSecurityOptions {
    AdvancedSecurityOptions {
        enabled: input.enabled,
        internal_user_database_enabled: input.internal_user_database_enabled,
        saml_options: input.saml_options.map(saml_options_output),
        anonymous_auth_disable_date: None,
        anonymous_auth_enabled: input.anonymous_auth_enabled,
    }
}

/// `SAMLOptionsInput` as the `SAMLOptionsOutput` the read operations report.
/// `MasterUserName` and `MasterBackendRole` have no output member, so they are
/// not carried over.
fn saml_options_output(input: SAMLOptionsInput) -> SAMLOptionsOutput {
    SAMLOptionsOutput {
        enabled: input.enabled,
        idp: input.idp,
        subject_key: input.subject_key,
        roles_key: input.roles_key,
        session_timeout_minutes: input.session_timeout_minutes,
    }
}

/// `AutoTuneOptionsInput` (what create takes) as the `AutoTuneOptions` both the
/// store and `UpdateElasticsearchDomainConfig` use. `RollbackOnDisable` has no
/// create-time input member, so it stays unset until an update sets it.
pub fn auto_tune_options(input: AutoTuneOptionsInput) -> AutoTuneOptions {
    AutoTuneOptions {
        desired_state: input.desired_state,
        rollback_on_disable: None,
        maintenance_schedules: input.maintenance_schedules,
    }
}

/// Every configuration option of one domain, each with its own provenance: the
/// merge target for `UpdateElasticsearchDomainConfig` and the source for both
/// `Describe*` operations.
#[derive(Debug, Clone)]
pub struct DomainConfigRecord {
    pub elasticsearch_version: Option<OptionRecord<String>>,
    /// Not optional: `ElasticsearchDomainStatus.ElasticsearchClusterConfig` is
    /// `@required`, so a domain always has one.
    pub cluster_config: OptionRecord<ElasticsearchClusterConfig>,
    pub ebs_options: Option<OptionRecord<EBSOptions>>,
    pub access_policies: Option<OptionRecord<String>>,
    pub snapshot_options: Option<OptionRecord<SnapshotOptions>>,
    pub vpc_options: Option<OptionRecord<VPCDerivedInfo>>,
    pub cognito_options: Option<OptionRecord<CognitoOptions>>,
    pub encryption_at_rest_options: Option<OptionRecord<EncryptionAtRestOptions>>,
    pub node_to_node_encryption_options: Option<OptionRecord<NodeToNodeEncryptionOptions>>,
    /// Ordered so a response's option map is deterministic across reads.
    pub advanced_options: Option<OptionRecord<BTreeMap<String, String>>>,
    pub log_publishing_options: Option<OptionRecord<BTreeMap<String, LogPublishingOption>>>,
    pub domain_endpoint_options: Option<OptionRecord<DomainEndpointOptions>>,
    pub advanced_security_options: Option<OptionRecord<AdvancedSecurityOptions>>,
    pub auto_tune_options: Option<OptionRecord<AutoTuneOptions>>,
    pub deployment_strategy_options: Option<OptionRecord<DeploymentStrategyOptions>>,
    pub automated_snapshot_pause_options:
        Option<OptionRecord<AutomatedSnapshotPauseRequestOptions>>,
    pub use_case: Option<OptionRecord<String>>,
    pub engine_mode: Option<OptionRecord<String>>,
}

impl DomainConfigRecord {
    /// A config record holding only the cluster config, which every domain has.
    /// The caller stores the options the create request supplied with
    /// [`apply_option`].
    pub fn new(cluster_config: ElasticsearchClusterConfig, now: SystemTime) -> Self {
        Self {
            elasticsearch_version: None,
            cluster_config: OptionRecord::new(cluster_config, now, FIRST_CONFIG_VERSION),
            ebs_options: None,
            access_policies: None,
            snapshot_options: None,
            vpc_options: None,
            cognito_options: None,
            encryption_at_rest_options: None,
            node_to_node_encryption_options: None,
            advanced_options: None,
            log_publishing_options: None,
            domain_endpoint_options: None,
            advanced_security_options: None,
            auto_tune_options: None,
            deployment_strategy_options: None,
            automated_snapshot_pause_options: None,
            use_case: None,
            engine_mode: None,
        }
    }
}

/// One Elasticsearch domain, as stored.
#[derive(Debug, Clone)]
pub struct Domain {
    pub domain_id: String,
    pub domain_name: String,
    pub arn: String,
    /// One of `ENGINE_TYPE_VALUES`, derived at create time from the version.
    pub engine_type: String,
    pub created: bool,
    pub deleted: bool,
    pub upgrade_processing: bool,
    pub endpoint: Option<String>,
    pub config: DomainConfigRecord,
    /// `CreateElasticsearchDomain.TagList`, as given.
    pub tags: Vec<Tag>,
    pub created_at: SystemTime,
    /// Bumped by every `UpdateElasticsearchDomainConfig` that changes an option.
    pub config_version: i32,
    pub last_updated: SystemTime,
    /// When the clock finishes the domain's current change.
    pub active_at: SystemTime,
}

/// When a change begun at `now` becomes `Active`. Stored, never slept on.
pub fn active_deadline(now: SystemTime) -> SystemTime {
    now + PROVISIONING_SPAN
}

impl Domain {
    /// A newly created domain. Its id, ARN and endpoint are derived ONCE, here,
    /// and stored: every later read answers with the stored strings.
    pub fn create(
        domain_name: String,
        region: &str,
        seq: u64,
        config: DomainConfigRecord,
        tags: Vec<Tag>,
        now: SystemTime,
    ) -> Self {
        let engine_type = engine_type_of(
            config
                .elasticsearch_version
                .as_ref()
                .map(|record| record.value.as_str()),
        );
        Self {
            domain_id: domain_id(&domain_name),
            arn: domain_arn(region, &domain_name),
            endpoint: Some(domain_endpoint(&domain_name, region, seq)),
            engine_type: engine_type.to_string(),
            domain_name,
            created: true,
            deleted: false,
            upgrade_processing: false,
            config,
            tags,
            created_at: now,
            config_version: FIRST_CONFIG_VERSION,
            last_updated: now,
            active_at: active_deadline(now),
        }
    }

    /// Record that a config change has begun: the domain moves to `version` and
    /// starts a fresh provisioning span.
    pub fn begin_change(&mut self, version: i32, now: SystemTime) {
        self.config_version = version;
        self.last_updated = now;
        self.active_at = active_deadline(now);
    }

    /// Whether the domain's last change is still provisioning.
    pub fn processing(&self, now: SystemTime) -> bool {
        now < self.active_at
    }

    /// The domain's modelled processing status — one of
    /// `DOMAIN_PROCESSING_STATUS_TYPE_VALUES`.
    pub fn processing_status(&self, now: SystemTime) -> &'static str {
        if self.deleted {
            "Deleting"
        } else if self.processing(now) {
            if self.config_version <= FIRST_CONFIG_VERSION {
                "Creating"
            } else {
                "Modifying"
            }
        } else {
            "Active"
        }
    }

    /// This domain's entry in `ListDomainNames`.
    pub fn domain_info(&self) -> DomainInfo {
        DomainInfo {
            domain_name: Some(self.domain_name.clone()),
            engine_type: Some(self.engine_type.clone()),
        }
    }

    /// The domain's `ElasticsearchDomainStatus`, computed from what is stored.
    pub fn domain_status(&self, now: SystemTime) -> ElasticsearchDomainStatus {
        let config = &self.config;
        ElasticsearchDomainStatus {
            domain_id: self.domain_id.clone(),
            domain_name: self.domain_name.clone(),
            arn: self.arn.clone(),
            created: Some(self.created),
            deleted: Some(self.deleted),
            endpoint: self.endpoint.clone(),
            // Only a VPC domain reports per-subnet endpoints, and those are EC2-derived.
            endpoints: None,
            processing: Some(self.processing(now)),
            upgrade_processing: Some(self.upgrade_processing),
            elasticsearch_version: config
                .elasticsearch_version
                .as_ref()
                .map(|record| record.value.clone()),
            elasticsearch_cluster_config: config.cluster_config.value.clone(),
            ebs_options: config.ebs_options.as_ref().map(|r| r.value.clone()),
            access_policies: config.access_policies.as_ref().map(|r| r.value.clone()),
            snapshot_options: config.snapshot_options.as_ref().map(|r| r.value.clone()),
            vpc_options: config.vpc_options.as_ref().map(|r| r.value.clone()),
            cognito_options: config.cognito_options.as_ref().map(|r| r.value.clone()),
            encryption_at_rest_options: config
                .encryption_at_rest_options
                .as_ref()
                .map(|r| r.value.clone()),
            node_to_node_encryption_options: config
                .node_to_node_encryption_options
                .as_ref()
                .map(|r| r.value.clone()),
            advanced_options: config
                .advanced_options
                .as_ref()
                .map(|r| r.value.clone().into_iter().collect()),
            log_publishing_options: config
                .log_publishing_options
                .as_ref()
                .map(|r| r.value.clone().into_iter().collect()),
            // Owned by Start/CancelElasticsearchServiceSoftwareUpdate, which are
            // still scaffolded: no software update has been requested on this domain.
            service_software_options: None,
            domain_endpoint_options: config
                .domain_endpoint_options
                .as_ref()
                .map(|r| r.value.clone()),
            advanced_security_options: config
                .advanced_security_options
                .as_ref()
                .map(|r| r.value.clone()),
            auto_tune_options: config.auto_tune_options.as_ref().map(|record| {
                AutoTuneOptionsOutput {
                    state: Some(
                        auto_tune_state(
                            record.value.desired_state.as_deref(),
                            record.processing(now),
                        )
                        .to_string(),
                    ),
                    error_message: None,
                }
            }),
            // Set by CancelDomainConfigChange / DescribeDomainChangeProgress, both
            // still scaffolded.
            change_progress_details: None,
            domain_processing_status: Some(self.processing_status(now).to_string()),
            modifying_properties: None,
            deployment_strategy_options: config
                .deployment_strategy_options
                .as_ref()
                .map(|r| r.value.clone()),
            automated_snapshot_pause_options: config
                .automated_snapshot_pause_options
                .as_ref()
                .map(|r| automated_snapshot_pause_options(r.value.clone(), now)),
            use_case: config.use_case.as_ref().map(|r| r.value.clone()),
            engine_mode: config.engine_mode.as_ref().map(|r| r.value.clone()),
        }
    }

    /// The domain's `ElasticsearchDomainConfig`: every stored option wrapped in
    /// its own status, built from that option's own provenance.
    pub fn domain_config(&self, now: SystemTime) -> ElasticsearchDomainConfig {
        let config = &self.config;
        ElasticsearchDomainConfig {
            elasticsearch_version: config.elasticsearch_version.as_ref().map(|r| {
                ElasticsearchVersionStatus {
                    options: r.value.clone(),
                    status: r.status(now),
                }
            }),
            elasticsearch_cluster_config: Some(ElasticsearchClusterConfigStatus {
                options: config.cluster_config.value.clone(),
                status: config.cluster_config.status(now),
            }),
            ebs_options: config.ebs_options.as_ref().map(|r| EBSOptionsStatus {
                options: r.value.clone(),
                status: r.status(now),
            }),
            access_policies: config.access_policies.as_ref().map(|r| AccessPoliciesStatus {
                options: r.value.clone(),
                status: r.status(now),
            }),
            snapshot_options: config.snapshot_options.as_ref().map(|r| SnapshotOptionsStatus {
                options: r.value.clone(),
                status: r.status(now),
            }),
            vpc_options: config.vpc_options.as_ref().map(|r| VPCDerivedInfoStatus {
                options: r.value.clone(),
                status: r.status(now),
            }),
            cognito_options: config.cognito_options.as_ref().map(|r| CognitoOptionsStatus {
                options: r.value.clone(),
                status: r.status(now),
            }),
            encryption_at_rest_options: config.encryption_at_rest_options.as_ref().map(|r| {
                EncryptionAtRestOptionsStatus {
                    options: r.value.clone(),
                    status: r.status(now),
                }
            }),
            node_to_node_encryption_options: config.node_to_node_encryption_options.as_ref().map(
                |r| NodeToNodeEncryptionOptionsStatus {
                    options: r.value.clone(),
                    status: r.status(now),
                },
            ),
            advanced_options: config.advanced_options.as_ref().map(|r| AdvancedOptionsStatus {
                options: r.value.clone().into_iter().collect(),
                status: r.status(now),
            }),
            log_publishing_options: config.log_publishing_options.as_ref().map(|r| {
                LogPublishingOptionsStatus {
                    options: Some(r.value.clone().into_iter().collect()),
                    status: Some(r.status(now)),
                }
            }),
            domain_endpoint_options: config.domain_endpoint_options.as_ref().map(|r| {
                DomainEndpointOptionsStatus {
                    options: r.value.clone(),
                    status: r.status(now),
                }
            }),
            advanced_security_options: config.advanced_security_options.as_ref().map(|r| {
                AdvancedSecurityOptionsStatus {
                    options: r.value.clone(),
                    status: r.status(now),
                }
            }),
            auto_tune_options: config.auto_tune_options.as_ref().map(|r| AutoTuneOptionsStatus {
                options: Some(r.value.clone()),
                status: Some(AutoTuneStatus {
                    creation_date: datetime_of(r.creation_date),
                    update_date: datetime_of(r.update_date),
                    update_version: r.update_version,
                    state: auto_tune_state(r.value.desired_state.as_deref(), r.processing(now))
                        .to_string(),
                    error_message: None,
                    pending_deletion: Some(false),
                }),
            }),
            change_progress_details: None,
            modifying_properties: None,
            deployment_strategy_options: config.deployment_strategy_options.as_ref().map(|r| {
                DeploymentStrategyOptionsStatus {
                    options: r.value.clone(),
                    status: r.status(now),
                }
            }),
            automated_snapshot_pause_options: config
                .automated_snapshot_pause_options
                .as_ref()
                .map(|r| AutomatedSnapshotPauseOptionsStatus {
                    options: automated_snapshot_pause_options(r.value.clone(), now),
                    status: r.status(now),
                }),
            use_case: config.use_case.as_ref().map(|r| UseCaseStatus {
                options: r.value.clone(),
                status: r.status(now),
            }),
            engine_mode: config.engine_mode.as_ref().map(|r| EngineModeStatus {
                options: r.value.clone(),
                status: r.status(now),
            }),
        }
    }
}

/// The Auto-Tune state to report — one of `AUTO_TUNE_STATE_VALUES`.
///
/// While the option is still provisioning the state is the matching
/// `*_IN_PROGRESS` value; once the clock has made it active it is the state the
/// caller asked for.
fn auto_tune_state(desired_state: Option<&str>, processing: bool) -> &'static str {
    match (desired_state, processing) {
        (Some("ENABLED"), true) => "ENABLE_IN_PROGRESS",
        (Some("ENABLED"), false) => "ENABLED",
        (Some("DISABLED"), true) => "DISABLE_IN_PROGRESS",
        _ => "DISABLED",
    }
}

/// The automated-snapshot pause options as reported, with the `PauseState` the
/// stored window and the clock imply — one of `PAUSE_STATE_VALUES`.
fn automated_snapshot_pause_options(
    stored: AutomatedSnapshotPauseRequestOptions,
    now: SystemTime,
) -> AutomatedSnapshotPauseOptions {
    let now_secs = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let started = stored
        .start_time
        .as_ref()
        .is_none_or(|start| now_secs >= start.as_secs_f64());
    let ended = stored
        .end_time
        .as_ref()
        .is_some_and(|end| now_secs >= end.as_secs_f64());
    let state = if !stored.enabled {
        "Disabled"
    } else if !started {
        "Scheduled"
    } else if ended {
        "Completed"
    } else {
        "Active"
    };
    AutomatedSnapshotPauseOptions {
        enabled: stored.enabled,
        start_time: stored.start_time,
        end_time: stored.end_time,
        state: Some(state.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Cross-cluster search connections
// ---------------------------------------------------------------------------

/// `OutboundCrossClusterSearchConnectionStatusCode`'s `VALIDATING`: "The outbound
/// connection request is being validated." The status a connection holds from the
/// instant the create returns until [`VALIDATION_SPAN`] has passed.
pub const CONNECTION_STATUS_VALIDATING: &str = "VALIDATING";

/// `OutboundCrossClusterSearchConnectionStatusCode`'s `PENDING_ACCEPTANCE`:
/// "Outbound connection request is validated and is not yet accepted by destination
/// domain owner" — where a request that passed validation waits for that decision.
pub const CONNECTION_STATUS_PENDING_ACCEPTANCE: &str = "PENDING_ACCEPTANCE";

/// `OutboundCrossClusterSearchConnectionStatusCode`'s `VALIDATION_FAILED`:
/// "Validation failed for the connection request" — where a request naming a domain
/// that does not resolve ends up, since no owner can decide on it.
pub const CONNECTION_STATUS_VALIDATION_FAILED: &str = "VALIDATION_FAILED";

/// How long a connection request stays [`CONNECTION_STATUS_VALIDATING`] before the
/// clock settles it on the outcome the create computed.
///
/// The emulator CHOOSES this span: AWS validates a connection request
/// asynchronously, so the create's answer is never the validation's outcome. It is
/// deliberately non-zero, so a caller that reads the connection straight after
/// creating it still observes the modelled `VALIDATING` status. Nothing sleeps for
/// it: the due instant is stored and compared against the clock on each read.
pub const VALIDATION_SPAN: std::time::Duration = std::time::Duration::from_secs(3);

/// How many outbound cross-cluster search connections ONE source domain may hold
/// before the create answers the modelled `LimitExceededException` ("an exception
/// for trying to create more than allowed resources or sub-resources").
///
/// 20 is the service's documented limit on the cross-cluster search connections a
/// domain takes part in. It bounds what one domain can add to the store.
pub const MAX_CONNECTIONS_PER_DOMAIN: usize = 20;

/// A hard ceiling on every stored connection, across every source domain and
/// region.
///
/// [`MAX_CONNECTIONS_PER_DOMAIN`] alone does not bound the store: the source domain
/// is named by the request, so a caller varying it would get a fresh allowance each
/// time. This cap dominates the single growth site instead, and sits at the product
/// of the two documented quotas, so it never clips a caller that stays inside both.
pub const MAX_CONNECTIONS_TOTAL: usize = MAX_CONNECTIONS_PER_DOMAIN * MAX_DOMAINS_TOTAL;

/// The odd multiplier that scrambles a connection counter into its id.
///
/// Multiplication by an odd constant modulo 2^64 is a BIJECTION on `u64`, so two
/// counter values can never render the same id.
const CONNECTION_ID_MULTIPLIER: u64 = 0x9e37_79b9_7f4a_7c15;

/// The id an outbound cross-cluster search connection is addressed by.
///
/// `CrossClusterSearchConnectionId` declares no constraint, so what matters is that
/// the id is opaque and that two connections never share one: it is the store's
/// monotonic connection counter scrambled through the bijection described on
/// [`CONNECTION_ID_MULTIPLIER`], behind the prefix naming the resource it
/// identifies.
pub fn cross_cluster_connection_id(seq: u64) -> String {
    format!(
        "ccs-{token:016x}",
        token = seq.wrapping_mul(CONNECTION_ID_MULTIPLIER)
    )
}

/// A `DomainInformation` reference with the owner and Region it is resolved against
/// filled in.
///
/// A reference naming no `OwnerId` is to a domain of the account making the call,
/// and one naming no `Region` is to a domain in the Region the call was signed for —
/// so the stored record, and every report of it, names the domain in full however
/// little the request said.
pub fn resolved_domain_information(info: DomainInformation, region: &str) -> DomainInformation {
    DomainInformation {
        owner_id: Some(
            info.owner_id
                .unwrap_or_else(|| ::engine::emulator_account_id().to_string()),
        ),
        domain_name: info.domain_name,
        region: Some(info.region.unwrap_or_else(|| region.to_string())),
    }
}

/// One outbound cross-cluster search connection, as stored.
///
/// The record is the single source of truth for every answer about the connection:
/// the create's response and every later read are both [`Self::to_outbound`] of it,
/// so the two cannot disagree. Where the connection is in its lifecycle is COMPUTED
/// from the stored validation outcome and its due instant rather than stored as a
/// mutable status, so no read has to write.
#[derive(Debug, Clone)]
pub struct CrossClusterSearchConnection {
    /// `CrossClusterSearchConnectionId`, unique per connection, so it IS the key the
    /// store holds the record under.
    pub connection_id: String,
    /// The Region the connection itself lives in: the one the create was signed for,
    /// which is the control plane that holds it.
    pub region: String,
    /// `ConnectionAlias`: the name the source domain's owner gave the connection.
    pub connection_alias: String,
    /// The source domain, as [`resolved_domain_information`] recorded it.
    pub source_domain: DomainInformation,
    /// The destination domain, as [`resolved_domain_information`] recorded it.
    pub destination_domain: DomainInformation,
    /// When the clock finishes validating the request.
    pub validated_at: SystemTime,
    /// The status the connection is in once [`Self::validated_at`] has passed: the
    /// outcome the create computed from the domains the request named.
    pub validated_status: String,
    /// `OutboundCrossClusterSearchConnectionStatus$Message` for that outcome, where
    /// there is something to say about it.
    pub validated_message: Option<String>,
}

impl CrossClusterSearchConnection {
    /// Where the connection is in its lifecycle at `now` — one of
    /// `OUTBOUND_CROSS_CLUSTER_SEARCH_CONNECTION_STATUS_CODE_VALUES`. Still being
    /// validated until the clock has made the outcome due.
    pub fn status_code(&self, now: SystemTime) -> &str {
        if now < self.validated_at {
            CONNECTION_STATUS_VALIDATING
        } else {
            &self.validated_status
        }
    }

    /// The verbose information for that status, which a request still being
    /// validated has none of: the outcome's message is only the settled status's.
    pub fn status_message(&self, now: SystemTime) -> Option<String> {
        if now < self.validated_at {
            None
        } else {
            self.validated_message.clone()
        }
    }

    /// This record as the `OutboundCrossClusterSearchConnection` the describe
    /// reports, member for member the projection the create answers with.
    pub fn to_outbound(&self, now: SystemTime) -> OutboundCrossClusterSearchConnection {
        OutboundCrossClusterSearchConnection {
            source_domain_info: Some(self.source_domain.clone()),
            destination_domain_info: Some(self.destination_domain.clone()),
            cross_cluster_search_connection_id: Some(self.connection_id.clone()),
            connection_alias: Some(self.connection_alias.clone()),
            connection_status: Some(OutboundCrossClusterSearchConnectionStatus {
                status_code: Some(self.status_code(now).to_string()),
                message: self.status_message(now),
            }),
        }
    }
}

/// Per-service state for com-amazonaws-elasticsearchservice-amazonelasticsearchservice2015. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsElasticsearchserviceAmazonelasticsearchservice2015State {
    /// Every live domain, keyed by `(region, domain name)` — the scope the name
    /// is unique in (`DomainName`'s own doc: "unique across the domains owned by
    /// an account within an AWS region"), so the same name in two regions is two
    /// domains. Ordered, so `ListDomainNames` is deterministic.
    pub domains: BTreeMap<(String, String), Domain>,
    /// Monotonic counter feeding generated endpoint suffixes, so each domain
    /// created in a run gets its own.
    pub next_id_seq: u64,
    /// Every outbound cross-cluster search connection, keyed by its
    /// `CrossClusterSearchConnectionId`. Ordered, so the describe reports the same
    /// order twice — which is what lets a pagination token name a place in the set.
    pub connections: BTreeMap<String, CrossClusterSearchConnection>,
    /// Monotonic counter feeding [`Self::mint_connection_id`].
    pub connection_seq: u64,
}

impl ComAmazonawsElasticsearchserviceAmazonelasticsearchservice2015State {
    /// Hand out the next sequence number.
    pub fn next_seq(&mut self) -> u64 {
        self.next_id_seq += 1;
        self.next_id_seq
    }

    /// How many domains the account already holds in `region` — the count the
    /// per-region quota is checked against.
    pub fn domain_count(&self, region: &str) -> usize {
        self.domains
            .keys()
            .filter(|(each, _)| each == region)
            .count()
    }

    /// The domains of one region, in name order.
    pub fn domains_in<'a>(&'a self, region: &'a str) -> impl Iterator<Item = &'a Domain> {
        self.domains
            .iter()
            .filter(move |((each, _), _)| each == region)
            .map(|(_, domain)| domain)
    }

    /// Hand out the id of the next connection created, so two connections never
    /// share one. See [`cross_cluster_connection_id`].
    pub fn mint_connection_id(&mut self) -> String {
        self.connection_seq = self.connection_seq.wrapping_add(1);
        cross_cluster_connection_id(self.connection_seq)
    }

    /// Whether the domain a `DomainInformation` reference names is one this store
    /// holds — what a connection request is validated against.
    ///
    /// A reference is resolved against the registry this emulator serves, which
    /// holds one account's domains keyed by Region and name, so it is the Region and
    /// the NAME that resolve; the `OwnerId` a request names is recorded with the
    /// reference rather than resolved, since no other account's domains are here to
    /// resolve it against.
    pub fn holds_referenced_domain(&self, reference: &DomainInformation) -> bool {
        reference.region.as_ref().is_some_and(|region| {
            self.domains
                .contains_key(&domain_key(region, &reference.domain_name))
        })
    }

    /// The connections of one Region, in connection-id order: a connection lives in
    /// the Region its create was signed for, so that is the scope a describe reports.
    pub fn connections_in<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = &'a CrossClusterSearchConnection> {
        self.connections
            .values()
            .filter(move |connection| connection.region == region)
    }

    /// How many connections one source domain already holds — the count the
    /// per-domain quota is checked against.
    pub fn connection_count_from(&self, region: &str, source_domain_name: &str) -> usize {
        self.connections_in(region)
            .filter(|connection| connection.source_domain.domain_name == source_domain_name)
            .count()
    }
}

/// What a handler answers when [`service_state`] cannot reach this service's store.
///
/// Only reachable when the service is mounted on an engine that is not the shared
/// [`::engine::InMemoryEngine`] — the router never does that. It is the scaffold's
/// 501 rather than a 5xx because there is nothing to retry: no engine of this
/// process will ever serve the call. An operation whose model declares a server
/// error answers with THAT instead.
pub fn state_unavailable<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::NotImplemented(crate::scaffolded::NotImplemented {
        message: "the Elasticsearch emulator's state store is not reachable from this engine"
            .to_string(),
    })
}

/// `InvalidPaginationTokenException` for a `NextToken` that names no place in the
/// set being paginated.
pub fn invalid_pagination_token(token: &str) -> InvalidPaginationTokenException {
    InvalidPaginationTokenException {
        message: Some(format!("Invalid pagination token: {token}")),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsElasticsearchserviceAmazonelasticsearchservice2015State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsElasticsearchserviceAmazonelasticsearchservice2015State>)
}
