// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-kafkaconnect-kafkaconnect service.
//!
//! Three resources live here — custom plugins, connectors and worker
//! configurations — beside the conversions from the shapes a create request
//! carries to the `*Description` shapes the read operations answer with, and the
//! paging helper the list operations share. Every handler in `src/ops/` reaches
//! this ONE struct through [`service_state`], so no two operations can read two
//! copies of one resource.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

use sha2::Digest;

#[allow(unused_imports)]
use crate::ops::types::*;

/// The marker every `nextToken` this service issues carries, so a token it did
/// not issue is refused rather than honoured as a resume point.
const PAGE_TOKEN_MARKER: &str = "kafkaconnect:v1:";

/// The region an ARN carries when the caller signed with no credential scope the
/// service can read (an unsigned request in a test, say). Every emulator
/// defaults to `us-east-1`.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN service namespace (`arnNamespace` in the model's `aws.api#service`
/// trait) every ARN this service mints carries.
pub const ARN_SERVICE: &str = "kafkaconnect";

/// The ARN service namespace of the bucket a custom plugin's file lives in
/// (`S3Location$bucketArn`: "The Amazon Resource Name (ARN) of an S3 bucket").
pub const S3_ARN_SERVICE: &str = "s3";

/// The ARN service namespace of the role a connector runs under
/// (`CreateConnectorRequest$serviceExecutionRoleArn`: "The Amazon Resource Name
/// (ARN) of the IAM role used by the connector").
pub const IAM_ARN_SERVICE: &str = "iam";

/// How long after the call that starts it a resource's move out of its transient
/// state is due.
///
/// `CreateCustomPlugin` and `CreateConnector` both answer with a non-terminal
/// state (`CREATING`), and `DeleteConnector` / `DeleteCustomPlugin` both answer
/// with `DELETING` for a resource that is still there; the clock, not the next
/// call, is what finishes each of those. The span is deliberately non-zero, so a
/// caller that reads the resource straight after creating it still observes
/// `CREATING` — exactly as it would against the real service, where the
/// transition takes minutes. It is a chosen span (AWS documents no duration for
/// it), kept short enough that a test can wait it out.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// `CustomPluginState` while the plugin's file is still being taken on; one of
/// [`CUSTOM_PLUGIN_STATE_VALUES`].
pub const CUSTOM_PLUGIN_CREATING: &str = "CREATING";
/// `CustomPluginState` once the plugin can be referenced by a connector.
pub const CUSTOM_PLUGIN_ACTIVE: &str = "ACTIVE";
/// `CustomPluginState` between `DeleteCustomPlugin` and the plugin's removal.
pub const CUSTOM_PLUGIN_DELETING: &str = "DELETING";

/// `WorkerConfigurationState` once the configuration can be referenced by a
/// connector; one of [`WORKER_CONFIGURATION_STATE_VALUES`]. A worker
/// configuration is nothing but stored properties, so
/// `CreateWorkerConfiguration` answers with this state straight away — the
/// model gives the shape no creating state at all (its two values are `ACTIVE`
/// and `DELETING`), so there is no transition for the clock to owe.
pub const WORKER_CONFIGURATION_ACTIVE: &str = "ACTIVE";
/// `WorkerConfigurationState` between `DeleteWorkerConfiguration` and the
/// configuration's removal.
pub const WORKER_CONFIGURATION_DELETING: &str = "DELETING";

/// The revision a freshly created worker configuration is at.
/// `WorkerConfigurationRevisionSummary$revision` is modelled `__long`, and a
/// configuration's first revision is one.
pub const FIRST_WORKER_CONFIGURATION_REVISION: i64 = 1;

/// `ConnectorState` while the connector's workers are being provisioned; one of
/// [`CONNECTOR_STATE_VALUES`].
pub const CONNECTOR_CREATING: &str = "CREATING";
/// `ConnectorState` once the connector's workers run.
pub const CONNECTOR_RUNNING: &str = "RUNNING";
/// `ConnectorState` between `DeleteConnector` and the connector's removal.
pub const CONNECTOR_DELETING: &str = "DELETING";

/// The version a freshly created connector is at. `currentVersion` is an opaque
/// string to a client, which only ever echoes it back on an update or a delete.
pub const FIRST_CONNECTOR_VERSION: &str = "1";

/// The revision a freshly created custom plugin is at. `CustomPlugin$revision`
/// is modelled `__longMin1` (range 1..), so revisions start at one.
pub const FIRST_PLUGIN_REVISION: i64 = 1;

/// The `networkType` a connector that named none takes: "Defaults to IPV4",
/// per the `networkType` member's own documentation.
pub const DEFAULT_NETWORK_TYPE: &str = "IPV4";

/// The largest page either list operation returns, and the page it returns when
/// the caller names no `maxResults`: the `MaxResults` shape's own maximum
/// (`range 1..100`).
pub const MAX_PAGE_SIZE: i32 = 100;
/// The smallest page `maxResults` may ask for, from the same `range`.
pub const MIN_PAGE_SIZE: i32 = 1;

/// How many custom plugins, and how many connectors, this emulator holds before
/// it refuses the next create with the modelled `TooManyRequestsException`
/// (HTTP 429, "Limit exceeded. Resource limit reached.").
///
/// The real service meters both resources against a service quota and answers
/// that same error once it is reached, so a cap is the faithful behaviour as
/// well as the one that keeps a long-lived emulator's state from growing without
/// bound on repeated calls. The value is the quota AWS ships for MSK Connect
/// connectors and custom plugins.
///
/// The cap, like the name uniqueness both creates check, is counted over this
/// emulator's ONE store: the emulator serves a single account and does not
/// partition state by region, so a resource created under one signing region is
/// the same resource a call signed for another region reads. Only the ARN a
/// resource was minted with records the region of the call that created it.
pub const RESOURCE_QUOTA: usize = 100;

/// The most tags one resource's stored tag set may hold.
///
/// The `Tags` map is modelled `length 0..200`, so 200 entries is the largest
/// tag set the real service accepts for a resource, and a request that would
/// leave more than that is refused. The model bounds ONE request's map, which
/// the framework enforces before a handler runs; this bounds the set a
/// REPEATED `TagResource` accumulates, which is the one caller-driven
/// dimension of this store that would otherwise grow without limit.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// Why a tag write was refused, with nothing written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagWriteRefusal {
    /// This emulator holds no resource of that ARN.
    NoSuchResource,
    /// The write would leave the resource holding more than
    /// [`MAX_TAGS_PER_RESOURCE`] tags; `would_hold` is how many.
    CapExceeded { would_hold: usize },
}

/// What the clock still owes a resource, and when.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingChange {
    /// Which change is owed.
    pub kind: PendingKind,
    /// The instant it becomes due. Stored, never slept on.
    pub due: Instant,
}

impl PendingChange {
    /// The change `kind`, due [`TRANSITION_SPAN`] from now.
    pub fn in_a_moment(kind: PendingKind) -> Self {
        Self {
            kind,
            due: Instant::now() + TRANSITION_SPAN,
        }
    }

    /// Whether this change is owed at `now`.
    pub fn is_due(&self, now: Instant) -> bool {
        self.due <= now
    }
}

/// The two changes the clock owes a kafkaconnect resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingKind {
    /// Leave the transient creating state for the resource's running state.
    Settle,
    /// Finish a delete: drop the resource.
    Remove,
}

/// One stored custom plugin: the whole `CreateCustomPlugin` request plus the
/// identity and lifecycle the service assigned it.
#[derive(Debug, Clone)]
pub struct CustomPluginRecord {
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub content_type: String,
    pub location: CustomPluginLocation,
    pub state: String,
    pub revision: i64,
    pub creation_time: SystemTime,
    pub tags: HashMap<String, String>,
    pub pending: Option<PendingChange>,
}

impl CustomPluginRecord {
    /// This plugin's stored location, as the read operations report it.
    pub fn location_description(&self) -> CustomPluginLocationDescription {
        CustomPluginLocationDescription {
            s3_location: Some(S3LocationDescription {
                bucket_arn: Some(self.location.s3_location.bucket_arn.clone()),
                file_key: Some(self.location.s3_location.file_key.clone()),
                object_version: self.location.s3_location.object_version.clone(),
            }),
        }
    }

    /// The summary of this plugin's latest (only) revision.
    ///
    /// `fileDescription` is left unset: it reports the md5 and size of the
    /// plugin file in the caller's S3 bucket, which this emulator never reads.
    pub fn latest_revision(&self) -> CustomPluginRevisionSummary {
        CustomPluginRevisionSummary {
            content_type: Some(self.content_type.clone()),
            creation_time: Some(date_time(self.creation_time)),
            description: self.description.clone(),
            file_description: None,
            location: Some(self.location_description()),
            revision: self.revision,
        }
    }

    /// This plugin as `ListCustomPlugins` reports it.
    pub fn summary(&self) -> CustomPluginSummary {
        CustomPluginSummary {
            creation_time: Some(date_time(self.creation_time)),
            custom_plugin_arn: Some(self.arn.clone()),
            custom_plugin_state: Some(self.state.clone()),
            description: self.description.clone(),
            latest_revision: Some(self.latest_revision()),
            name: Some(self.name.clone()),
        }
    }
}

/// One stored connector: the whole `CreateConnector` request plus the identity,
/// version and lifecycle the service assigned it.
#[derive(Debug, Clone)]
pub struct ConnectorRecord {
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub configuration: HashMap<String, String>,
    pub capacity: Capacity,
    pub kafka_cluster: KafkaCluster,
    pub kafka_cluster_client_authentication: KafkaClusterClientAuthentication,
    pub kafka_cluster_encryption_in_transit: KafkaClusterEncryptionInTransit,
    pub kafka_connect_version: String,
    pub network_type: String,
    pub plugins: Vec<Plugin>,
    pub service_execution_role_arn: String,
    pub worker_configuration: Option<WorkerConfiguration>,
    pub log_delivery: Option<LogDelivery>,
    pub state: String,
    pub current_version: String,
    pub creation_time: SystemTime,
    pub tags: HashMap<String, String>,
    pub pending: Option<PendingChange>,
}

impl ConnectorRecord {
    /// This connector's stored capacity, as the read operations report it.
    pub fn capacity_description(&self) -> CapacityDescription {
        CapacityDescription {
            auto_scaling: self
                .capacity
                .auto_scaling
                .as_ref()
                .map(|scaling| AutoScalingDescription {
                    max_worker_count: scaling.max_worker_count,
                    mcu_count: scaling.mcu_count,
                    min_worker_count: scaling.min_worker_count,
                    scale_in_policy: scaling.scale_in_policy.as_ref().map(|policy| {
                        ScaleInPolicyDescription {
                            cpu_utilization_percentage: policy.cpu_utilization_percentage,
                        }
                    }),
                    scale_out_policy: scaling.scale_out_policy.as_ref().map(|policy| {
                        ScaleOutPolicyDescription {
                            cpu_utilization_percentage: policy.cpu_utilization_percentage,
                        }
                    }),
                    max_autoscaling_task_count: scaling.max_autoscaling_task_count,
                }),
            provisioned_capacity: self.capacity.provisioned_capacity.as_ref().map(|fixed| {
                ProvisionedCapacityDescription {
                    mcu_count: fixed.mcu_count,
                    worker_count: fixed.worker_count,
                }
            }),
        }
    }

    /// This connector's stored Kafka cluster, as the read operations report it.
    pub fn kafka_cluster_description(&self) -> KafkaClusterDescription {
        let cluster = &self.kafka_cluster.apache_kafka_cluster;
        KafkaClusterDescription {
            apache_kafka_cluster: Some(ApacheKafkaClusterDescription {
                bootstrap_servers: Some(cluster.bootstrap_servers.clone()),
                vpc: Some(VpcDescription {
                    security_groups: cluster.vpc.security_groups.clone(),
                    subnets: Some(cluster.vpc.subnets.clone()),
                }),
            }),
        }
    }

    /// This connector's stored client authentication, as reported.
    pub fn client_authentication_description(&self) -> KafkaClusterClientAuthenticationDescription {
        KafkaClusterClientAuthenticationDescription {
            authentication_type: Some(
                self.kafka_cluster_client_authentication
                    .authentication_type
                    .clone(),
            ),
        }
    }

    /// This connector's stored encryption in transit, as reported.
    pub fn encryption_in_transit_description(&self) -> KafkaClusterEncryptionInTransitDescription {
        KafkaClusterEncryptionInTransitDescription {
            encryption_type: Some(
                self.kafka_cluster_encryption_in_transit
                    .encryption_type
                    .clone(),
            ),
        }
    }

    /// This connector's stored log delivery, as reported; `None` when the
    /// request named none.
    pub fn log_delivery_description(&self) -> Option<LogDeliveryDescription> {
        let delivery = self.log_delivery.as_ref()?;
        let worker = &delivery.worker_log_delivery;
        Some(LogDeliveryDescription {
            worker_log_delivery: Some(WorkerLogDeliveryDescription {
                cloud_watch_logs: worker.cloud_watch_logs.as_ref().map(|logs| {
                    CloudWatchLogsLogDeliveryDescription {
                        enabled: logs.enabled,
                        log_group: logs.log_group.clone(),
                    }
                }),
                firehose: worker
                    .firehose
                    .as_ref()
                    .map(|firehose| FirehoseLogDeliveryDescription {
                        delivery_stream: firehose.delivery_stream.clone(),
                        enabled: firehose.enabled,
                    }),
                s3: worker.s3.as_ref().map(|s3| S3LogDeliveryDescription {
                    bucket: s3.bucket.clone(),
                    enabled: s3.enabled,
                    prefix: s3.prefix.clone(),
                }),
            }),
        })
    }

    /// This connector's stored worker configuration, as reported; `None` when
    /// the request named none.
    pub fn worker_configuration_description(&self) -> Option<WorkerConfigurationDescription> {
        self.worker_configuration
            .as_ref()
            .map(|worker| WorkerConfigurationDescription {
                revision: worker.revision,
                worker_configuration_arn: Some(worker.worker_configuration_arn.clone()),
            })
    }

    /// The plugins this connector was created with, as reported.
    pub fn plugin_descriptions(&self) -> Vec<PluginDescription> {
        self.plugins
            .iter()
            .map(|plugin| PluginDescription {
                custom_plugin: Some(CustomPluginDescription {
                    custom_plugin_arn: Some(plugin.custom_plugin.custom_plugin_arn.clone()),
                    revision: plugin.custom_plugin.revision,
                }),
            })
            .collect()
    }

    /// This connector as `ListConnectors` reports it.
    pub fn summary(&self) -> ConnectorSummary {
        ConnectorSummary {
            capacity: Some(self.capacity_description()),
            connector_arn: Some(self.arn.clone()),
            connector_description: self.description.clone(),
            connector_name: Some(self.name.clone()),
            connector_state: Some(self.state.clone()),
            creation_time: Some(date_time(self.creation_time)),
            current_version: Some(self.current_version.clone()),
            kafka_cluster: Some(self.kafka_cluster_description()),
            kafka_cluster_client_authentication: Some(self.client_authentication_description()),
            kafka_cluster_encryption_in_transit: Some(self.encryption_in_transit_description()),
            kafka_connect_version: Some(self.kafka_connect_version.clone()),
            log_delivery: self.log_delivery_description(),
            network_type: Some(self.network_type.clone()),
            plugins: Some(self.plugin_descriptions()),
            service_execution_role_arn: Some(self.service_execution_role_arn.clone()),
            worker_configuration: self.worker_configuration_description(),
        }
    }
}

/// One stored worker configuration: the whole `CreateWorkerConfiguration`
/// request plus the identity and lifecycle the service assigned it.
#[derive(Debug, Clone)]
pub struct WorkerConfigurationRecord {
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    /// The base64 `connect-distributed.properties` body the create request
    /// carried, stored verbatim: the read operations hand back exactly what was
    /// stored.
    pub properties_file_content: String,
    pub state: String,
    pub revision: i64,
    pub creation_time: SystemTime,
    pub tags: HashMap<String, String>,
    pub pending: Option<PendingChange>,
}

impl WorkerConfigurationRecord {
    /// The summary of this configuration's latest (only) revision, as the create
    /// and list operations report it. It carries no properties file: only the
    /// `WorkerConfigurationRevisionDescription` a describe answers with does.
    pub fn latest_revision(&self) -> WorkerConfigurationRevisionSummary {
        WorkerConfigurationRevisionSummary {
            creation_time: Some(date_time(self.creation_time)),
            description: self.description.clone(),
            revision: self.revision,
        }
    }

    /// This configuration's latest revision as `DescribeWorkerConfiguration`
    /// reports it: the summary plus the stored properties file.
    pub fn revision_description(&self) -> WorkerConfigurationRevisionDescription {
        WorkerConfigurationRevisionDescription {
            creation_time: Some(date_time(self.creation_time)),
            description: self.description.clone(),
            properties_file_content: Some(self.properties_file_content.clone()),
            revision: self.revision,
        }
    }
}

/// Per-service state for com-amazonaws-kafkaconnect-kafkaconnect. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsKafkaconnectKafkaconnectState {
    /// Stored custom plugins, keyed by `customPluginArn`.
    pub custom_plugins: HashMap<String, CustomPluginRecord>,
    /// Stored connectors, keyed by `connectorArn`.
    pub connectors: HashMap<String, ConnectorRecord>,
    /// Stored worker configurations, keyed by `workerConfigurationArn`.
    pub worker_configurations: HashMap<String, WorkerConfigurationRecord>,
    /// How many resources this service has minted an id for. Feeds the ARN
    /// suffix, so two resources of one name (one created after the other was
    /// deleted) never share an ARN.
    pub minted: u64,
}

impl ComAmazonawsKafkaconnectKafkaconnectState {
    /// Apply every change the clock already owes: a creating resource becomes
    /// available, and a deleting one is dropped, once its due instant has
    /// passed.
    ///
    /// Called at the top of EVERY handler, read or write, so what a caller sees
    /// depends on the clock alone and not on which operation happened to run.
    /// This is the one write a reporting operation may make: each change was
    /// owed at the same instant whether or not anybody called.
    pub fn settle_due(&mut self) {
        let now = Instant::now();

        self.custom_plugins
            .retain(|_, plugin| !removal_due(plugin.pending, now));
        for plugin in self.custom_plugins.values_mut() {
            if settling_due(plugin.pending, now) {
                plugin.state = CUSTOM_PLUGIN_ACTIVE.to_string();
                plugin.pending = None;
            }
        }

        self.connectors
            .retain(|_, connector| !removal_due(connector.pending, now));
        for connector in self.connectors.values_mut() {
            if settling_due(connector.pending, now) {
                connector.state = CONNECTOR_RUNNING.to_string();
                connector.pending = None;
            }
        }

        // A worker configuration has no creating state to leave (the model gives
        // `WorkerConfigurationState` only `ACTIVE` and `DELETING`), so the only
        // change the clock can owe one is the end of a delete.
        self.worker_configurations
            .retain(|_, worker| !removal_due(worker.pending, now));
    }

    /// Whether this store already holds as many custom plugins as it may
    /// ([`RESOURCE_QUOTA`], counted over the whole store), so the next
    /// `CreateCustomPlugin` must be refused.
    pub fn custom_plugin_quota_reached(&self) -> bool {
        self.custom_plugins.len() >= RESOURCE_QUOTA
    }

    /// Whether this store already holds as many connectors as it may
    /// ([`RESOURCE_QUOTA`], counted over the whole store), so the next
    /// `CreateConnector` must be refused.
    pub fn connector_quota_reached(&self) -> bool {
        self.connectors.len() >= RESOURCE_QUOTA
    }

    /// The stored plugin of this name, if any. A name identifies at most one
    /// plugin, which is what makes a repeat of it a conflict.
    pub fn custom_plugin_named(&self, name: &str) -> Option<&CustomPluginRecord> {
        self.custom_plugins
            .values()
            .find(|plugin| plugin.name == name)
    }

    /// Whether this store already holds as many worker configurations as it may
    /// ([`RESOURCE_QUOTA`], counted over the whole store), so the next
    /// `CreateWorkerConfiguration` must be refused.
    pub fn worker_configuration_quota_reached(&self) -> bool {
        self.worker_configurations.len() >= RESOURCE_QUOTA
    }

    /// The stored connector of this name, if any.
    pub fn connector_named(&self, name: &str) -> Option<&ConnectorRecord> {
        self.connectors
            .values()
            .find(|connector| connector.name == name)
    }

    /// The tags stored on the resource `arn` names, or `None` when this
    /// emulator holds no resource of that ARN.
    ///
    /// Every taggable resource this emulator holds is keyed by its own ARN, so
    /// one lookup covers all three kinds and a reader never has to know which
    /// kind an ARN names.
    pub fn tags_of(&self, arn: &str) -> Option<&HashMap<String, String>> {
        self.custom_plugins
            .get(arn)
            .map(|plugin| &plugin.tags)
            .or_else(|| self.connectors.get(arn).map(|connector| &connector.tags))
            .or_else(|| {
                self.worker_configurations
                    .get(arn)
                    .map(|worker| &worker.tags)
            })
    }

    /// The tags stored on the resource `arn` names, for a caller that is about
    /// to change them, or `None` when this emulator holds no resource of that
    /// ARN.
    ///
    /// The mutable twin of [`Self::tags_of`]. PRIVATE on purpose: every tag
    /// write goes through [`Self::attach_tags`] or [`Self::remove_tags`], so the
    /// cap on a resource's stored tag count cannot be bypassed by a handler
    /// holding the map directly.
    fn tags_of_mut(&mut self, arn: &str) -> Option<&mut HashMap<String, String>> {
        if let Some(plugin) = self.custom_plugins.get_mut(arn) {
            return Some(&mut plugin.tags);
        }
        if let Some(connector) = self.connectors.get_mut(arn) {
            return Some(&mut connector.tags);
        }
        self.worker_configurations
            .get_mut(arn)
            .map(|worker| &mut worker.tags)
    }

    /// Attach `incoming` to the resource `arn` names, per key: a key the
    /// resource already carries takes the new value, a key `incoming` does not
    /// name keeps the value it had.
    ///
    /// Refused, with nothing written, when the ARN names no resource this
    /// emulator holds, or when the merged set would hold more than
    /// [`MAX_TAGS_PER_RESOURCE`] keys — the one caller-driven dimension of this
    /// store that a repeated call could otherwise grow without bound.
    pub fn attach_tags(
        &mut self,
        arn: &str,
        incoming: HashMap<String, String>,
    ) -> Result<(), TagWriteRefusal> {
        let tags = self
            .tags_of_mut(arn)
            .ok_or(TagWriteRefusal::NoSuchResource)?;
        // The size the merge would leave: the keys already stored that the
        // request does not name, plus the request's own keys.
        let would_hold = tags
            .keys()
            .filter(|key| !incoming.contains_key(*key))
            .count()
            + incoming.len();
        if would_hold > MAX_TAGS_PER_RESOURCE {
            return Err(TagWriteRefusal::CapExceeded { would_hold });
        }
        for (key, value) in incoming {
            tags.insert(key, value);
        }
        Ok(())
    }

    /// Remove the named keys from the resource `arn` names, per key: a key the
    /// request does not name keeps its value, and a key the resource does not
    /// carry is a no-op (so the call is safe to retry).
    ///
    /// Refused only when the ARN names no resource this emulator holds — a
    /// removal can never grow the stored set.
    pub fn remove_tags(&mut self, arn: &str, keys: &[String]) -> Result<(), TagWriteRefusal> {
        let tags = self
            .tags_of_mut(arn)
            .ok_or(TagWriteRefusal::NoSuchResource)?;
        for key in keys {
            tags.remove(key);
        }
        Ok(())
    }

    /// The stored worker configuration of this name, if any. A name identifies
    /// at most one configuration, which is what makes a repeat of it a conflict.
    pub fn worker_configuration_named(&self, name: &str) -> Option<&WorkerConfigurationRecord> {
        self.worker_configurations
            .values()
            .find(|worker| worker.name == name)
    }

    /// Mint the random-looking suffix of a resource ARN: the UUID AWS gives a
    /// kafkaconnect resource.
    ///
    /// Derived from this service's own mint counter through SHA-256, with the
    /// version and variant nibbles set, so the result is a syntactically valid
    /// v4-shaped UUID that is unique within the emulator and reproducible for a
    /// given sequence of calls — never derived from the request's own values,
    /// which would collide across two resources of the same name.
    pub fn mint_resource_id(&mut self, kind: &str, name: &str) -> String {
        self.minted = self.minted.saturating_add(1);
        let seed = format!("{ARN_SERVICE}/{kind}/{name}/{}", self.minted);
        let digest = sha2::Sha256::digest(seed.as_bytes());
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let hex = hex::encode(bytes);
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }
}

/// Whether `pending` is a removal that is already owed at `now`.
fn removal_due(pending: Option<PendingChange>, now: Instant) -> bool {
    pending.is_some_and(|change| change.kind == PendingKind::Remove && change.is_due(now))
}

/// Whether `pending` is a move out of the creating state that is already owed.
fn settling_due(pending: Option<PendingChange>, now: Instant) -> bool {
    pending.is_some_and(|change| change.kind == PendingKind::Settle && change.is_due(now))
}

/// `at` as the `__timestampIso8601` the read operations carry.
pub fn date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// The longest Region LABEL this service will read out of a credential scope.
///
/// Every `*Arn` pattern in the AWS models bounds the Region field at
/// `[a-z0-9-]{1,20}`, so a longer value is not a Region any AWS endpoint has.
/// The same bound `service_common::arn` reads an ARN's Region under.
const MAX_REGION_LABEL: usize = 20;

/// Whether `value` is a Region LABEL rather than arbitrary text: non-empty, at
/// most [`MAX_REGION_LABEL`] characters, and composed only of `[a-z0-9-]`.
///
/// The region a request is read with is not just data: it is embedded in every
/// ARN this service mints AND in the authority of the endpoint URL the
/// in-process S3 resolution is addressed to, so a value carrying `/`, `@`, `:`,
/// `?` or `#` — which would re-point or truncate an authority — is not a Region
/// this service will use for either.
fn is_region_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_REGION_LABEL
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// The region every ARN this request mints carries, and the region its
/// in-process calls to other emulated services are addressed in: the one the
/// caller signed for, or [`DEFAULT_REGION`] when the request carries no
/// credential scope — or one whose region field is not a Region label at all
/// ([`is_region_label`]), which names no AWS endpoint and must never reach an
/// ARN or a URL authority.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_label(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

// ---------------------------------------------------------------------------
// The validation message every kafkaconnect parameter rejection carries.
//
// kafkaconnect answers a rejection it blames on a request member with a message
// that NAMES that member: `Invalid parameter <member>`, optionally followed by
// `: <reason>`. The model states no message text, so the form is taken from the
// service's own recorded rejections (`Invalid parameter nextToken.`,
// `Invalid parameter bucketArn: Invalid ARN.`,
// `Invalid parameter customPluginArn: The requested resource doesn't exist.`).
// Both builders live here so every handler in `src/ops/` renders the one form
// rather than each inventing a sentence.
//
// They are for a rejection the service blames on a member of the request: a
// value it cannot parse, one naming a resource it does not hold, and one whose
// value collides with state the account already holds (a name already taken) —
// the request's own `name`/`connectorName` is the member that conflicted, so the
// same member-naming form carries the modelled conflict reason. A quota refusal
// blames no member of the request and keeps its own wording.
// ---------------------------------------------------------------------------

/// The lead-in of every kafkaconnect parameter-rejection message.
const INVALID_PARAMETER: &str = "Invalid parameter";

/// The reason a parameter that names a resource this service does not hold is
/// rejected with. The apostrophe is U+2019, as the service writes it.
pub const RESOURCE_DOES_NOT_EXIST: &str = "The requested resource doesn\u{2019}t exist.";

/// The reason a parameter that must carry an ARN, and does not, is rejected
/// with.
pub const INVALID_ARN: &str = "Invalid ARN.";

/// The reason a create whose name is already taken is refused with: the
/// modelled `ConflictException` sentence, as its own documentation states it
/// ("HTTP Status Code 409: Conflict. A resource with this name already exists.
/// Retry your request with another name." — the status prefix is the HTTP status
/// itself and the retry advice is advice, so the reason is the middle sentence).
///
/// It names NO resource and interpolates nothing: the conflict is the same
/// whatever name was asked for and whatever ARN the held resource carries, and a
/// generated ARN in a message would differ on every run.
pub const NAME_ALREADY_EXISTS: &str = "A resource with this name already exists.";

/// The message for a rejection of the member `member`, with no further reason:
/// `Invalid parameter <member>.`
pub fn invalid_parameter(member: &str) -> String {
    format!("{INVALID_PARAMETER} {member}.")
}

/// The message for a rejection of the member `member` for `reason` (itself a
/// full sentence): `Invalid parameter <member>: <reason>`
pub fn invalid_parameter_because(member: &str, reason: &str) -> String {
    format!("{INVALID_PARAMETER} {member}: {reason}")
}

/// Whether `value` is a well-formed ARN naming `service`.
///
/// The grammar is the one AWS publishes for every ARN,
/// `arn:<partition>:<service>:<region>:<account-id>:<resource>`, read through
/// the ONE caller-supplied-ARN parse this emulator shares
/// (`service_common::arn`): six `:`-separated fields, the first literally `arn`,
/// a non-empty partition, the service field the one asked for, and a non-empty
/// resource. `region` and `account-id` are deliberately unconstrained there — an
/// S3 bucket ARN leaves both empty (`arn:aws:s3:::bucket`) while most other ARNs
/// fill them — and the resource keeps every `:` and `/` it carries.
pub fn is_arn_for(value: &str, service: &str) -> bool {
    ::service_common::arn::is_resource_arn_of(value, service)
}

/// The name of the bucket the ARN `value` names, or `None` when `value` is not
/// an S3 bucket ARN.
///
/// An S3 bucket ARN is partition-global and account-less
/// (`arn:aws:s3:::my-bucket`), so the bucket name is the whole resource field —
/// read through the one shared ARN parse. A value naming an object
/// (`…:::bucket/key`) or an access point (`…:accesspoint/name`) is not a bucket
/// ARN this emulator addresses a bucket by, so neither is reported.
pub fn s3_bucket_name(value: &str) -> Option<&str> {
    let bucket = ::service_common::arn::parse_of(value, S3_ARN_SERVICE)?.resource;
    (!bucket.is_empty() && !bucket.contains(['/', ':'])).then_some(bucket)
}

/// The member a refusal of the custom plugin's file blames: the request's
/// `location.s3Location`, the only member naming an object this service has to
/// read (`CustomPluginLocation$s3Location`, "The S3 bucket Amazon Resource Name
/// (ARN), file key, and object version of the plugin file stored in Amazon S3").
pub const S3_LOCATION_MEMBER: &str = "s3Location";

/// The reason a custom plugin file this account cannot read is rejected with.
///
/// The service takes the plugin's bytes on at create time, so a file it cannot
/// read is a bad REQUEST (400) rather than a resource that is missing: the
/// wording names the three things that make a file unreadable, as the service
/// words it.
pub const CUSTOM_PLUGIN_FILE_UNREADABLE: &str =
    "Could not access the specified custom plugin file. Make sure the bucket is in the same \
     region, the file exists and you have access to it.";

/// The Amazon S3 error codes in which S3 reports that the object a request
/// names cannot be READ: it is not there (`NoSuchBucket`, `NoSuchKey`,
/// `NoSuchVersion`) or reading it is not permitted (`AccessDenied`,
/// `AllAccessDisabled`, a bucket policy or an ownership check refusing the
/// read).
///
/// Keyed on S3's OWN code rather than on the HTTP status, because a status is
/// not evidence that S3 answered at all: a request that reached no S3 is a 404
/// too, and that is this emulator failing to ask rather than a verdict about the
/// object. The credential refusals (`InvalidAccessKeyId`,
/// `SignatureDoesNotMatch`) are left out for the same reason — they say the call
/// was made wrong, not that the caller may not read the file.
///
/// The list is deliberately CLOSED: a refusal code S3 adds later (an ownership
/// or a KMS refusal, say) falls into the warn-and-accept branch of
/// [`custom_plugin_file_is_unreadable`] rather than refusing the create, because
/// a code this service does not recognise is not evidence about the object.
const UNREADABLE_CODES: &[&str] = &[
    "NoSuchBucket",
    "NoSuchKey",
    "NoSuchVersion",
    "AccessDenied",
    "AllAccessDisabled",
];

/// An Amazon S3 client over the IN-PROCESS callback into this same emulator.
///
/// NOTHING here opens a socket or leaves the host: `ctx.callback()` is the
/// framework's documented seam for calling another emulated service, and the
/// `http_client` it hands over dispatches the request inside this process to
/// whichever mounted service claims it (the S3 emulator, on the request's own
/// `s3` credential scope). The `endpoint_url` below is therefore an ADDRESS the
/// in-process router reads, never a host any resolver looks up — and `region` is
/// the request's own signing region, already narrowed to a Region label by
/// [`request_region`], so the caller cannot steer that authority.
///
/// The credentials are the emulator's own root credentials, which only sign this
/// operation's in-process calls; they are never put in a response or a log.
fn s3_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    // The one value that reaches the endpoint's authority is narrowed HERE, in
    // every build: `request_region` already answers only a Region label, and
    // this is the guarantee the authority rests on, so it is re-established
    // rather than asserted (a `debug_assert!` is compiled out of the build that
    // ships). A value outside the grammar falls back to the default Region,
    // exactly as a request carrying no credential scope does.
    let region = if is_region_label(region) {
        region
    } else {
        DEFAULT_REGION
    };
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The in-process transport routes on the request's own path, and a
        // virtual-host bucket name is not a host this emulator resolves, so the
        // bucket is addressed in the path.
        .force_path_style(true)
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        // The transport is in-process: there is nothing to retry, time out or
        // watch for a stalled stream, and each of those would need a sleep
        // implementation this client is not given. The same goes for caching the
        // identity: the credentials are handed over directly.
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// Whether Amazon S3 reports that the custom plugin file `location` names
/// cannot be read.
///
/// A custom plugin's file is resolved when the plugin is CREATED — the service
/// takes the artifact on at that point — so a location naming an object that is
/// not there, or one this account may not read, is refused with the operation's
/// modelled `BadRequestException` and nothing is persisted. S3 is reached the
/// way any emulated service reaches another: through the in-process callback
/// ([`smithy_server_runtime::OperationContext::callback`], which dispatches
/// inside this process — see [`s3_client`]: no socket, no outbound request, no
/// host resolution), with `HeadObject` — a METADATA read, so the artifact's
/// bytes are never copied into this service to answer a create, however large
/// the plugin file is — and with the caller's `objectVersion` when it named one,
/// so the version the plugin pins is the version that has to resolve.
///
/// The verdict is read from S3's own error CODE and never from the HTTP status:
/// a request that reached no S3 at all is a 404 too, so a status cannot tell S3
/// reporting an unreadable object from this emulator failing to ask. (A ranged
/// read is not an option: `GetObject` with a `Range` is answered 501 by the S3
/// emulator, which would make every create unverifiable.)
///
/// `false` means the plugin may be stored. That includes the case where this
/// emulator has no S3 to ask (a router with only this service mounted, say):
/// nothing is then known about the object, and a refusal has to be grounded in
/// what S3 answered, never assumed.
pub async fn custom_plugin_file_is_unreadable(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    location: &S3Location,
) -> bool {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    // The caller has already refused a `bucketArn` that is not a bucket ARN with
    // its own message, so a value that names no bucket is not re-judged here.
    let Some(bucket) = s3_bucket_name(&location.bucket_arn) else {
        return false;
    };
    let mut read = s3_client(ctx, region)
        .head_object()
        .bucket(bucket)
        .key(&location.file_key);
    if let Some(object_version) = location.object_version.as_deref() {
        read = read.version_id(object_version);
    }
    match read.send().await {
        // Only that the read RESOLVED is in question, so this is a METADATA read:
        // the artifact's bytes are the caller's and are never copied into this
        // service to answer a create.
        Ok(_) => false,
        Err(error) => {
            if UNREADABLE_CODES.contains(&error.code().unwrap_or_default()) {
                // S3 says the object is not there, or not readable by this
                // account: the refusal is grounded.
                return true;
            }
            // Any other answer is this emulator failing to ask its own S3, not
            // S3 reporting an unreadable object, so it refuses nothing — and it
            // is worth a warning, because a deployment with no S3 mounted to ask
            // is one where this check does not run. The caller's own values are
            // rendered with their escaped `Debug` form, so a key carrying a
            // newline cannot forge a log line.
            ::tracing::warn!(
                bucket_arn = ?location.bucket_arn,
                file_key = ?location.file_key,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                code = ?error.code(),
                "the custom plugin file could not be resolved against the emulator's Amazon S3"
            );
            false
        }
    }
}

/// One page of a list operation: the items to answer with, and the `nextToken`
/// that resumes after them (`None` when this page is the last).
#[derive(Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_token: Option<String>,
}

/// A `nextToken` the service did not emit.
#[derive(Debug)]
pub struct BadPageToken;

/// Page `keyed` — `(sort key, item)` pairs, one per stored resource — honouring
/// `max_results` and resuming after `next_token`.
///
/// The key is the resource's ARN: unique and stable, so sorting by it gives both
/// a total order and a resume point that survives a resource being created or
/// deleted between two pages. The token is opaque (hex of that key), as the
/// model's `@paginated` contract requires: a client only ever echoes it back.
///
/// `max_results` is clamped rather than refused because a value outside the
/// `MaxResults` shape's `range 1..100` never reaches here: the framework refuses
/// it from the model's own constraint before the handler runs (the tests assert
/// 0 and 101 are 400s). The clamp is what keeps this helper total should a later
/// caller page an unconstrained count.
pub fn page<T>(
    mut keyed: Vec<(String, T)>,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Result<Page<T>, BadPageToken> {
    let resume_after = match next_token {
        Some(token) => Some(decode_page_token(token)?),
        None => None,
    };
    let page_size = max_results
        .unwrap_or(MAX_PAGE_SIZE)
        .clamp(MIN_PAGE_SIZE, MAX_PAGE_SIZE);
    let page_size = usize::try_from(page_size).unwrap_or(1);

    keyed.sort_by(|left, right| left.0.cmp(&right.0));

    let mut items = Vec::new();
    let mut next = None;
    for (key, item) in keyed {
        if resume_after.as_deref().is_some_and(|from| key.as_str() < from) {
            continue;
        }
        if items.len() == page_size {
            next = Some(encode_page_token(&key));
            break;
        }
        items.push(item);
    }
    Ok(Page {
        items,
        next_token: next,
    })
}

/// The opaque form of a resume key: the key behind a marker only this service
/// writes, hex-encoded. The marker is what makes a forged token detectable —
/// without it any hex string would decode to "some" resume key and be honoured
/// as a silent resume point.
fn encode_page_token(key: &str) -> String {
    hex::encode(format!("{PAGE_TOKEN_MARKER}{key}"))
}

/// The resume key inside an opaque token, or [`BadPageToken`] when the token is
/// not one this service emitted.
fn decode_page_token(token: &str) -> Result<String, BadPageToken> {
    let bytes = hex::decode(token).map_err(|_| BadPageToken)?;
    let decoded = String::from_utf8(bytes).map_err(|_| BadPageToken)?;
    decoded
        .strip_prefix(PAGE_TOKEN_MARKER)
        .map(ToString::to_string)
        .ok_or(BadPageToken)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsKafkaconnectKafkaconnectState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsKafkaconnectKafkaconnectState>)
}
