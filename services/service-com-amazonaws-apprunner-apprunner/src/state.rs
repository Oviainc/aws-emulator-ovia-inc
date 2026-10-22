// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-apprunner-apprunner service.
//!
//! Besides the store itself this module holds the record types the App Runner
//! *service* resource is persisted as, and the normalisation the control plane
//! applies to a stored record. Both `CreateService` and `UpdateService` write the
//! same record, and all four readers (`DescribeService`, `ListServices`,
//! `ListOperations`, plus the writers' own responses) render it, so the record
//! shape and its defaulting live here rather than being copied per handler.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AutoScalingConfigurationSummary, EncryptionConfiguration, HealthCheckConfiguration,
    InstanceConfiguration, NetworkConfiguration, OperationSummary, Service,
    ServiceObservabilityConfiguration, ServiceSummary, SourceConfiguration,
};

/// `ServiceStatus` values this slice uses, from the model's `ServiceStatus` enum
/// (`SERVICE_STATUS_VALUES` in `ops::types`).
pub const SERVICE_OPERATION_IN_PROGRESS: &str = "OPERATION_IN_PROGRESS";
/// See [`SERVICE_OPERATION_IN_PROGRESS`].
pub const SERVICE_RUNNING: &str = "RUNNING";
/// See [`SERVICE_OPERATION_IN_PROGRESS`].
pub const SERVICE_DELETED: &str = "DELETED";

/// `OperationStatus` values this slice uses, from the model's `OperationStatus`
/// enum (`OPERATION_STATUS_VALUES` in `ops::types`).
pub const OPERATION_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`OPERATION_IN_PROGRESS`].
pub const OPERATION_SUCCEEDED: &str = "SUCCEEDED";

/// `OperationType` values this slice records, from the model's `OperationType`
/// enum (`OPERATION_TYPE_VALUES` in `ops::types`).
pub const OP_TYPE_CREATE_SERVICE: &str = "CREATE_SERVICE";
/// See [`OP_TYPE_CREATE_SERVICE`].
pub const OP_TYPE_UPDATE_SERVICE: &str = "UPDATE_SERVICE";
/// See [`OP_TYPE_CREATE_SERVICE`].
pub const OP_TYPE_DELETE_SERVICE: &str = "DELETE_SERVICE";

/// How long after the call that starts it an asynchronous App Runner operation
/// (create, update, delete) is owed its terminal state.
///
/// CHOSEN, not documented: App Runner publishes no duration for a deployment, so
/// this is the emulator's own span. It is deliberately non-zero, so a caller that
/// reads the service straight after `CreateService` still observes
/// `OPERATION_IN_PROGRESS` and the state errors that go with it, and it is never
/// slept on — the due instant is stored and materialised by whichever call next
/// touches the record.
pub const OPERATION_SETTLE_DELAY: Duration = Duration::from_millis(500);

/// How many operations one service keeps in its log.
///
/// CHOSEN: App Runner publishes no retention for a service's operation history,
/// only that `ListOperations` pages it. A cap is required, not cosmetic — a
/// client may update one service any number of times, and without it one live
/// resource's log would grow for the life of the process. The oldest entries are
/// evicted, so the log always holds the most recent operations, which is the end
/// `ListOperations` reports from.
pub const MAX_OPERATIONS_PER_SERVICE: usize = 100;

/// How many App Runner services one account can have in one Region.
///
/// The value is the quota `ServiceQuotaExceededException`'s own doc points at
/// ("You've reached your account quota for this resource type. For App Runner
/// per-resource quotas, see App Runner endpoints and quotas …"): the AWS General
/// Reference's App Runner endpoints-and-quotas page lists "Services" with a
/// default of 10 per Region per account, adjustable on request. This emulator
/// enforces that published default, which is also what keeps the store bounded
/// against a caller that only ever creates.
pub const MAX_SERVICES_PER_ACCOUNT: usize = 10;

/// A hard ceiling on the WHOLE store, across every Region.
///
/// [`MAX_SERVICES_PER_ACCOUNT`] alone does not bound it: the Region is read from
/// the request's own SigV4 credential scope, so a caller that varies that field
/// would get a fresh per-Region allowance each time. This cap dominates the
/// single growth site instead — at most `MAX_SERVICES_TOTAL` records can exist
/// however many Regions are addressed, and a Region partition exists only while
/// it holds a record, so the name index, the number of operation logs and the
/// work `settle_due` does under the lock are bounded by the same number. It sits
/// well above the per-Region quota, so no single Region's published allowance is
/// clipped by it.
pub const MAX_SERVICES_TOTAL: usize = 1_000;

/// The status an App Runner service reaches when the operation in flight on it
/// completes, and when the clock owes that completion.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    /// When the transition falls due. Never slept on; materialised in
    /// [`ComAmazonawsApprunnerApprunnerState::settle_due`].
    pub due_at: SystemTime,
    /// The `ServiceStatus` the record moves to.
    pub to_status: String,
    /// The operation whose completion this transition is.
    pub operation_id: String,
}

/// One persisted App Runner service.
#[derive(Debug, Clone)]
pub struct ServiceRecord {
    pub service_name: String,
    pub service_id: String,
    pub service_arn: String,
    pub service_url: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The Region this service lives in: the one its request was signed for, and
    /// the one its ARN and URL name. Every per-Region rule (name uniqueness, the
    /// service quota, what `ListServices` reports) is keyed on it.
    pub region: String,
    pub status: String,
    pub source_configuration: SourceConfiguration,
    pub instance_configuration: InstanceConfiguration,
    pub encryption_configuration: Option<EncryptionConfiguration>,
    pub health_check_configuration: HealthCheckConfiguration,
    pub auto_scaling_configuration_summary: AutoScalingConfigurationSummary,
    pub network_configuration: NetworkConfiguration,
    pub observability_configuration: Option<ServiceObservabilityConfiguration>,
    pub tags: BTreeMap<String, String>,
    pub pending: Option<PendingTransition>,
}

impl ServiceRecord {
    /// The modelled `Service` for this record.
    pub fn to_service(&self) -> Service {
        Service {
            service_name: self.service_name.clone(),
            service_id: self.service_id.clone(),
            service_arn: self.service_arn.clone(),
            service_url: Some(self.service_url.clone()),
            created_at: to_datetime(self.created_at),
            updated_at: to_datetime(self.updated_at),
            // No record ever carries a `DeletedAt`: the record and its operation
            // log are reclaimed at the instant the delete falls due, so nothing
            // can describe the service after that (`DescribeService` answers
            // `ResourceNotFoundException`). The member is left absent rather
            // than filled with an invented instant.
            deleted_at: None,
            status: self.status.clone(),
            source_configuration: self.source_configuration.clone(),
            instance_configuration: self.instance_configuration.clone(),
            encryption_configuration: self.encryption_configuration.clone(),
            health_check_configuration: Some(self.health_check_configuration.clone()),
            auto_scaling_configuration_summary: self.auto_scaling_configuration_summary.clone(),
            network_configuration: self.network_configuration.clone(),
            observability_configuration: self.observability_configuration.clone(),
        }
    }

    /// The modelled `ServiceSummary` for this record: identification only, no
    /// configuration detail (`ServiceSummary`'s own model doc).
    pub fn to_summary(&self) -> ServiceSummary {
        ServiceSummary {
            service_name: Some(self.service_name.clone()),
            service_id: Some(self.service_id.clone()),
            service_arn: Some(self.service_arn.clone()),
            service_url: Some(self.service_url.clone()),
            created_at: Some(to_datetime(self.created_at)),
            updated_at: Some(to_datetime(self.updated_at)),
            status: Some(self.status.clone()),
        }
    }
}

/// One entry of a service's operation log.
#[derive(Debug, Clone)]
pub struct OperationRecord {
    pub id: String,
    pub op_type: String,
    pub status: String,
    pub target_arn: String,
    pub started_at: SystemTime,
    pub ended_at: Option<SystemTime>,
    pub updated_at: SystemTime,
}

impl OperationRecord {
    /// The modelled `OperationSummary` for this entry.
    pub fn to_summary(&self) -> OperationSummary {
        OperationSummary {
            id: Some(self.id.clone()),
            r#type: Some(self.op_type.clone()),
            status: Some(self.status.clone()),
            target_arn: Some(self.target_arn.clone()),
            started_at: Some(to_datetime(self.started_at)),
            ended_at: self.ended_at.map(to_datetime),
            updated_at: Some(to_datetime(self.updated_at)),
        }
    }
}

/// Per-service state for com-amazonaws-apprunner-apprunner. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsApprunnerApprunnerState {
    /// The services that exist, keyed by `ServiceArn`. A record lives from
    /// `CreateService` until the instant its `DeleteService` falls due, when it
    /// and its operation log are both dropped: every map here is bounded by the
    /// services a caller currently has, and deleting one gives the memory back.
    pub services: BTreeMap<String, ServiceRecord>,
    /// The operation log, keyed by the `ServiceArn` the operations targeted. At
    /// most [`MAX_OPERATIONS_PER_SERVICE`] entries per service, oldest evicted,
    /// and the whole log is dropped with the service.
    pub operations: BTreeMap<String, Vec<OperationRecord>>,
    /// `(region, ServiceName)` -> `ServiceArn`, which both enforces the name's
    /// documented uniqueness ("unique across all the running App Runner services
    /// in your Amazon Web Services account in the Amazon Web Services Region")
    /// and orders `ListServices` stably within a Region. Freed with the service.
    pub name_index: BTreeMap<(String, String), String>,
    /// The earliest instant at which any stored transition falls due, so
    /// [`Self::settle_due`] can answer in constant time on the overwhelming
    /// majority of requests instead of scanning every service.
    pub next_due: Option<SystemTime>,
    /// How many ids this store has minted, which is what makes every minted id
    /// distinct. See [`Self::mint_hex_id`].
    pub minted: u64,
}

impl ComAmazonawsApprunnerApprunnerState {
    /// Materialise every transition the clock already owes at `now`.
    ///
    /// This is the only write a reporting operation makes: each applied
    /// transition was due at `due_at` whether or not anybody called, and it is
    /// applied with that instant, not with `now`, so the result does not depend
    /// on when it was observed. A delete that falls due drops the record, its
    /// name and its operation log, so the store holds only what a caller still
    /// has.
    ///
    /// Returns immediately when nothing is due, which is the common case: only
    /// the call that first observes a due transition pays for the scan.
    pub fn settle_due(&mut self, now: SystemTime) {
        match self.next_due {
            None => return,
            Some(next_due) if next_due > now => return,
            Some(_) => {}
        }

        let due: Vec<(String, PendingTransition)> = self
            .services
            .iter()
            .filter_map(|(arn, record)| {
                record
                    .pending
                    .as_ref()
                    .filter(|pending| pending.due_at <= now)
                    .map(|pending| (arn.clone(), pending.clone()))
            })
            .collect();

        for (arn, pending) in due {
            if pending.to_status == SERVICE_DELETED {
                // The service is gone: the record, the name it held and its
                // operation log all go with it.
                self.services.remove(&arn);
                self.name_index.retain(|_, indexed| indexed != &arn);
                self.operations.remove(&arn);
                continue;
            }
            if let Some(record) = self.services.get_mut(&arn) {
                record.status = pending.to_status.clone();
                record.updated_at = pending.due_at;
                record.pending = None;
            }
            if let Some(operation) = self
                .operations
                .get_mut(&arn)
                .and_then(|log| log.iter_mut().find(|entry| entry.id == pending.operation_id))
            {
                operation.status = OPERATION_SUCCEEDED.to_string();
                operation.ended_at = Some(pending.due_at);
                operation.updated_at = pending.due_at;
            }
        }

        self.next_due = self
            .services
            .values()
            .filter_map(|record| record.pending.as_ref().map(|pending| pending.due_at))
            .min();
    }

    /// Note that a transition falls due at `due_at`, so [`Self::settle_due`]
    /// knows when it has work without scanning.
    pub fn expect_due(&mut self, due_at: SystemTime) {
        self.next_due = Some(match self.next_due {
            Some(next_due) => next_due.min(due_at),
            None => due_at,
        });
    }

    /// The service this ARN names.
    pub fn live(&self, service_arn: &str) -> Option<&ServiceRecord> {
        self.services.get(service_arn)
    }

    /// The service this ARN names, in `region` only.
    ///
    /// An App Runner service belongs to the Region its ARN names, so a request
    /// signed for another Region does not resolve it: the ARN-taking operations
    /// answer `ResourceNotFoundException` for a cross-Region ARN.
    pub fn live_in(&self, region: &str, service_arn: &str) -> Option<&ServiceRecord> {
        self.live(service_arn)
            .filter(|record| record.region == region)
    }

    /// Append an entry to a service's operation log and return its id.
    ///
    /// The log is capped at [`MAX_OPERATIONS_PER_SERVICE`]: the oldest entries
    /// are evicted so repeated operations on one service cannot grow it without
    /// bound.
    pub fn record_operation(
        &mut self,
        service_arn: &str,
        op_type: &str,
        id: String,
        started_at: SystemTime,
    ) -> String {
        let log = self.operations.entry(service_arn.to_string()).or_default();
        log.push(OperationRecord {
            id: id.clone(),
            op_type: op_type.to_string(),
            status: OPERATION_IN_PROGRESS.to_string(),
            target_arn: service_arn.to_string(),
            started_at,
            ended_at: None,
            updated_at: started_at,
        });
        if log.len() > MAX_OPERATIONS_PER_SERVICE {
            let evict = log.len() - MAX_OPERATIONS_PER_SERVICE;
            log.drain(..evict);
        }
        id
    }

    /// The services in one Region as `(cursor key, summary)`, ordered by
    /// `ServiceName`. `ListServices` reports a Region, so a service in another
    /// Region is not in this Region's listing.
    pub fn service_page_items(&self, region: &str) -> Vec<(String, ServiceSummary)> {
        self.name_index
            .iter()
            .filter(|((indexed_region, _), _)| indexed_region == region)
            .filter_map(|((_, name), arn)| {
                self.live(arn)
                    .map(|record| (name.clone(), record.to_summary()))
            })
            .collect()
    }

    /// How many services this account has in `region`, which is the scope App
    /// Runner's service quota is published per.
    pub fn service_count(&self, region: &str) -> usize {
        self.services
            .values()
            .filter(|record| record.region == region)
            .count()
    }

    /// A fresh id of `hex_digits` hex characters.
    ///
    /// The id is the SHA-256 digest (the `sha2` crate) of a seed no two mints
    /// share — this store's monotonic mint counter, the instant of the mint and
    /// the resource the id is for — rendered with the `hex` crate. The counter is
    /// what guarantees distinctness; the instant and the context keep two
    /// emulator runs from minting the same sequence of ids.
    pub fn mint_hex_id(&mut self, hex_digits: usize, context: &str) -> String {
        use sha2::Digest;

        self.minted = self.minted.wrapping_add(1);
        let since_epoch = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();
        let seed = format!(
            "{context}/{}/{}.{}",
            self.minted,
            since_epoch.as_secs(),
            since_epoch.subsec_nanos()
        );
        let digest = hex::encode(sha2::Sha256::digest(seed.as_bytes()));
        digest[..hex_digits.min(digest.len())].to_string()
    }

    /// A fresh `ServiceId`.
    ///
    /// The model gives `ServiceId` `length(32, 32)` and, inconsistently, the
    /// 36-character hyphenated-UUID `pattern` — no string satisfies both. App
    /// Runner itself answers with the 32-character form, so the length wins.
    pub fn mint_service_id(&mut self, context: &str) -> String {
        self.mint_hex_id(32, context)
    }

    /// A fresh `UUID` for an operation: the model's `UUID` shape is 36 chars,
    /// hyphenated, with RFC 4122's version nibble in `[1-5]` and variant nibble
    /// in `[89ab]`, so the minted digest is laid out 8-4-4-4-12 with those two
    /// nibbles set to version 4 and the RFC's variant.
    pub fn mint_operation_id(&mut self, context: &str) -> String {
        let mut digits: Vec<char> = self.mint_hex_id(32, context).chars().collect();
        digits[12] = '4';
        digits[16] = match digits[16] {
            '0' | '1' | '2' | '3' => '8',
            '4' | '5' | '6' | '7' => '9',
            '8' | '9' | 'a' | 'b' => 'a',
            _ => 'b',
        };
        let digits: String = digits.into_iter().collect();
        format!(
            "{}-{}-{}-{}-{}",
            &digits[0..8],
            &digits[8..12],
            &digits[12..16],
            &digits[16..20],
            &digits[20..32]
        )
    }

    /// A service's operation log as `(cursor key, summary)`, most recent first.
    pub fn operation_page_items(&self, service_arn: &str) -> Vec<(String, OperationSummary)> {
        let mut log: Vec<&OperationRecord> = self
            .operations
            .get(service_arn)
            .map(|entries| entries.iter().collect())
            .unwrap_or_default();
        // Most recent first, as `ListOperations` reports a service's history;
        // the id breaks a tie between two operations started in the same instant
        // so the order — and so the pagination cursor — is total.
        log.sort_by(|left, right| {
            right
                .started_at
                .cmp(&left.started_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        log.into_iter()
            .map(|entry| (entry.id.clone(), entry.to_summary()))
            .collect()
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsApprunnerApprunnerState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsApprunnerApprunnerState>)
}

/// A `SystemTime` as the modelled `Timestamp` the wire carries.
pub fn to_datetime(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// The region this request was signed for, defaulting to the emulator's own
/// default region when the request carries no SigV4 scope.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// The region an unsigned request is answered in.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace (`OPERATIONS.md`, "ARN namespace: apprunner").
const ARN_SERVICE: &str = "apprunner";

/// `arn:aws:apprunner:<region>:<account>:service/<ServiceName>/<ServiceId>`.
pub fn service_arn(region: &str, service_name: &str, service_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("service/{service_name}/{service_id}"),
    )
}

/// The subdomain App Runner generates for a service:
/// `<ServiceId>.<region>.awsapprunner.com`.
pub fn service_url(region: &str, service_id: &str) -> String {
    format!("{service_id}.{region}.awsapprunner.com")
}

/// `InstanceConfiguration` with the model's documented defaults filled in:
/// `Cpu` "Default: 1 vCPU", `Memory` "Default: 2 GB".
pub fn instance_configuration_with_defaults(
    supplied: Option<InstanceConfiguration>,
) -> InstanceConfiguration {
    let supplied = supplied.unwrap_or_default();
    InstanceConfiguration {
        cpu: Some(supplied.cpu.unwrap_or_else(|| "1 vCPU".to_string())),
        memory: Some(supplied.memory.unwrap_or_else(|| "2 GB".to_string())),
        instance_role_arn: supplied.instance_role_arn,
    }
}

/// `HealthCheckConfiguration` with the model's documented defaults filled in:
/// `Protocol` "Default: TCP", `Path` "Default: \"/\"", `Interval` "Default: 5",
/// `Timeout` "Default: 2", `HealthyThreshold` "Default: 1",
/// `UnhealthyThreshold` "Default: 5".
pub fn health_check_with_defaults(
    supplied: Option<HealthCheckConfiguration>,
) -> HealthCheckConfiguration {
    let supplied = supplied.unwrap_or_default();
    HealthCheckConfiguration {
        protocol: Some(supplied.protocol.unwrap_or_else(|| "TCP".to_string())),
        path: Some(supplied.path.unwrap_or_else(|| "/".to_string())),
        interval: Some(supplied.interval.unwrap_or(5)),
        timeout: Some(supplied.timeout.unwrap_or(2)),
        healthy_threshold: Some(supplied.healthy_threshold.unwrap_or(1)),
        unhealthy_threshold: Some(supplied.unhealthy_threshold.unwrap_or(5)),
    }
}

/// `NetworkConfiguration` with the model's documented defaults filled in:
/// `IpAddressType` "If you do not specify an IpAddressType, it defaults to
/// select IPv4", `EgressType` "Set to DEFAULT for access to resources hosted on
/// public networks", `IsPubliclyAccessible` `@default false`.
pub fn network_with_defaults(supplied: Option<NetworkConfiguration>) -> NetworkConfiguration {
    use crate::ops::types::{EgressConfiguration, IngressConfiguration};

    let supplied = supplied.unwrap_or_default();
    let egress = supplied.egress_configuration.unwrap_or_default();
    NetworkConfiguration {
        egress_configuration: Some(EgressConfiguration {
            egress_type: Some(egress.egress_type.unwrap_or_else(|| "DEFAULT".to_string())),
            vpc_connector_arn: egress.vpc_connector_arn,
        }),
        ingress_configuration: Some(supplied.ingress_configuration.unwrap_or(
            IngressConfiguration {
                is_publicly_accessible: false,
            },
        )),
        ip_address_type: Some(supplied.ip_address_type.unwrap_or_else(|| "IPV4".to_string())),
    }
}

/// Refuse a `NetworkConfiguration` the model's own rules forbid:
/// `VpcConnectorArn` is "Only valid when EgressType = VPC", and `EgressType`
/// `VPC` is what "associate your service to a custom VPC specified by
/// VpcConnectorArn" needs that ARN for.
pub fn validate_network(network: &NetworkConfiguration) -> Result<(), String> {
    let Some(egress) = network.egress_configuration.as_ref() else {
        return Ok(());
    };
    let egress_type = egress.egress_type.as_deref().unwrap_or("DEFAULT");
    match (egress_type, egress.vpc_connector_arn.as_deref()) {
        ("VPC", None) => Err(
            "EgressConfiguration.VpcConnectorArn is required when EgressType is VPC".to_string(),
        ),
        ("VPC", Some(_)) | (_, None) => Ok(()),
        (other, Some(_)) => Err(format!(
            "EgressConfiguration.VpcConnectorArn is only valid when EgressType is VPC, not {other}"
        )),
    }
}

/// Refuse a `ServiceObservabilityConfiguration` the model's own rule forbids:
/// `ObservabilityConfigurationArn` is "Specified only when ObservabilityEnabled
/// is true".
pub fn validate_observability(
    observability: &ServiceObservabilityConfiguration,
) -> Result<(), String> {
    match (
        observability.observability_enabled,
        observability.observability_configuration_arn.is_some(),
    ) {
        (true, false) => Err(
            "ObservabilityConfiguration.ObservabilityConfigurationArn is required when \
             ObservabilityEnabled is true"
                .to_string(),
        ),
        (false, true) => Err(
            "ObservabilityConfiguration.ObservabilityConfigurationArn is only valid when \
             ObservabilityEnabled is true"
                .to_string(),
        ),
        _ => Ok(()),
    }
}

/// Refuse a `SourceConfiguration` that does not name exactly one source, and
/// fill in `AutoDeploymentsEnabled` when the request left it out.
///
/// `SourceConfiguration`'s model doc requires "either this member or
/// ImageRepository (but not both)". `AutoDeploymentsEnabled`'s doc gives the
/// default as "false for a source image that uses an ECR Public repository or an
/// ECR repository that's in an Amazon Web Services account other than the one
/// that the service is in … true in all other cases (which currently include a
/// source code repository or a source image using a same-account ECR
/// repository)".
pub fn normalise_source(mut source: SourceConfiguration) -> Result<SourceConfiguration, String> {
    match (
        source.code_repository.is_some(),
        source.image_repository.is_some(),
    ) {
        (true, true) => {
            return Err(
                "SourceConfiguration must specify either CodeRepository or ImageRepository, \
                 but not both"
                    .to_string(),
            )
        }
        (false, false) => {
            return Err(
                "SourceConfiguration must specify either CodeRepository or ImageRepository"
                    .to_string(),
            )
        }
        _ => {}
    }
    if source.auto_deployments_enabled.is_none() {
        source.auto_deployments_enabled = Some(default_auto_deployments(&source));
    }
    Ok(source)
}

/// The documented `AutoDeploymentsEnabled` default for a source — see
/// [`normalise_source`].
pub fn default_auto_deployments(source: &SourceConfiguration) -> bool {
    let Some(image) = source.image_repository.as_ref() else {
        // A source code repository: "true in all other cases".
        return true;
    };
    if image.image_repository_type == "ECR_PUBLIC" {
        return false;
    }
    // A private ECR image identifier is
    // `<account>.dkr.ecr.<region>.amazonaws.com/<repo>[:tag]`; a cross-account
    // one defaults to false, a same-account one to true.
    image
        .image_identifier
        .split('.')
        .next()
        .is_some_and(|account| account == ::engine::emulator_account_id())
}

/// The `AutoScalingConfigurationSummary` a service carries for the auto scaling
/// configuration it is associated with.
///
/// `CreateService`'s `AutoScalingConfigurationArn` doc: "If not provided, App
/// Runner associates the latest revision of a default auto scaling
/// configuration. Specify an ARN with a name and a revision number to associate
/// that revision … Specify just the name to associate the latest revision."
/// Auto scaling configuration resources themselves are out of this slice (their
/// operations still answer 501), so only the identification the ARN carries is
/// reported; nothing about the configuration's own revision history is invented.
pub fn auto_scaling_summary(
    region: &str,
    supplied_arn: Option<&str>,
) -> Result<AutoScalingConfigurationSummary, String> {
    /// The name App Runner gives the default auto scaling configuration.
    const DEFAULT_NAME: &str = "DefaultConfiguration";
    /// The revision a name-only ARN resolves to: the latest, which with no
    /// configuration resources persisted is the first.
    const LATEST_REVISION: i32 = 1;

    let (name, revision, arn, is_default) = match supplied_arn {
        None => (
            DEFAULT_NAME.to_string(),
            LATEST_REVISION,
            ::engine::emulator_arn(
                ARN_SERVICE,
                region,
                &format!("autoscalingconfiguration/{DEFAULT_NAME}/{LATEST_REVISION}"),
            ),
            // This call is what associated the default configuration, so this
            // much is known rather than claimed.
            Some(true),
        ),
        Some(arn) => {
            let (name, revision) = parse_auto_scaling_arn(arn).ok_or_else(|| {
                "AutoScalingConfigurationArn must be the ARN of an App Runner auto scaling \
                 configuration"
                    .to_string()
            })?;
            // Whether the caller's configuration is this account's default is a
            // fact about a resource this slice does not hold.
            (name, revision, arn.to_string(), None)
        }
    };

    Ok(AutoScalingConfigurationSummary {
        auto_scaling_configuration_arn: Some(arn),
        auto_scaling_configuration_name: Some(name),
        auto_scaling_configuration_revision: revision,
        // The configuration resource itself is out of this slice (its own
        // operations still answer 501), so nothing about its lifecycle is
        // claimed: its `Status`, `CreatedAt` and `HasAssociatedService` are left
        // absent rather than invented. `IsDefault` is reported only where this
        // call is what chose the default configuration.
        status: None,
        created_at: None,
        has_associated_service: None,
        is_default,
    })
}

/// The name and revision an auto scaling configuration ARN carries:
/// `arn:<partition>:apprunner:<region>:<account>:autoscalingconfiguration/<name>[/<revision>[/<id>]]`.
/// The partition, the service namespace and the resource kind must all be App
/// Runner's, so an ARN naming some other service's resource is refused rather
/// than echoed back. A name-only ARN resolves to revision 1, the latest with no
/// configuration revisions persisted. The region and account are not checked
/// against the request: App Runner's own member doc shows a plain ARN and this
/// slice holds no configuration resources to contradict it with.
fn parse_auto_scaling_arn(arn: &str) -> Option<(String, i32)> {
    let mut fields = arn.split(':');
    if fields.next()? != "arn" {
        return None;
    }
    if !fields.next()?.starts_with("aws") {
        return None;
    }
    if fields.next()? != ARN_SERVICE {
        return None;
    }
    let (_region, _account) = (fields.next()?, fields.next()?);
    // The resource portion may itself contain colons, so take what is left.
    let resource = fields.collect::<Vec<_>>().join(":");
    let mut segments = resource.split('/');
    if segments.next()? != "autoscalingconfiguration" {
        return None;
    }
    let name = segments.next().filter(|name| !name.is_empty())?.to_string();
    let revision = segments
        .next()
        .map_or(Some(1), |revision| revision.parse().ok())?;
    Some((name, revision))
}

/// Merge an `UpdateService` request's `SourceConfiguration` into the stored one.
///
/// `UpdateService`'s own doc: "You can update the source configuration and
/// instance configuration of the service … you can't change the name or the
/// encryption configuration of the service" — so only the members the request
/// names change, and the merge is member-wise. The `SourceConfiguration` rule
/// that exactly one of `CodeRepository` / `ImageRepository` is named still holds
/// for the merged result; naming one replaces the other, because a source cannot
/// be both.
pub fn merge_source(
    stored: &SourceConfiguration,
    supplied: SourceConfiguration,
) -> Result<SourceConfiguration, String> {
    if supplied.code_repository.is_some() && supplied.image_repository.is_some() {
        return Err(
            "SourceConfiguration must specify either CodeRepository or ImageRepository, \
             but not both"
                .to_string(),
        );
    }
    let mut merged = stored.clone();
    if supplied.code_repository.is_some() {
        merged.code_repository = supplied.code_repository;
        merged.image_repository = None;
    } else if supplied.image_repository.is_some() {
        merged.image_repository = supplied.image_repository;
        merged.code_repository = None;
    }
    if supplied.authentication_configuration.is_some() {
        merged.authentication_configuration = supplied.authentication_configuration;
    }
    merged.auto_deployments_enabled = supplied
        .auto_deployments_enabled
        .or(merged.auto_deployments_enabled);
    normalise_source(merged)
}

/// Merge an `UpdateService` request's `InstanceConfiguration` into the stored
/// one, member by member: a member the request leaves out keeps its stored
/// value.
pub fn merge_instance(
    stored: &InstanceConfiguration,
    supplied: InstanceConfiguration,
) -> InstanceConfiguration {
    InstanceConfiguration {
        cpu: supplied.cpu.or_else(|| stored.cpu.clone()),
        memory: supplied.memory.or_else(|| stored.memory.clone()),
        instance_role_arn: supplied
            .instance_role_arn
            .or_else(|| stored.instance_role_arn.clone()),
    }
}

/// Merge an `UpdateService` request's `HealthCheckConfiguration` into the stored
/// one, member by member.
pub fn merge_health_check(
    stored: &HealthCheckConfiguration,
    supplied: HealthCheckConfiguration,
) -> HealthCheckConfiguration {
    HealthCheckConfiguration {
        protocol: supplied.protocol.or_else(|| stored.protocol.clone()),
        path: supplied.path.or_else(|| stored.path.clone()),
        interval: supplied.interval.or(stored.interval),
        timeout: supplied.timeout.or(stored.timeout),
        healthy_threshold: supplied.healthy_threshold.or(stored.healthy_threshold),
        unhealthy_threshold: supplied.unhealthy_threshold.or(stored.unhealthy_threshold),
    }
}

/// Merge an `UpdateService` request's `NetworkConfiguration` into the stored
/// one, member by member. Each embedded object ("Consists of embedded objects
/// for each configurable network feature") is replaced as a whole, because its
/// own members only make sense together.
pub fn merge_network(
    stored: &NetworkConfiguration,
    supplied: NetworkConfiguration,
) -> NetworkConfiguration {
    NetworkConfiguration {
        egress_configuration: supplied
            .egress_configuration
            .or_else(|| stored.egress_configuration.clone()),
        ingress_configuration: supplied
            .ingress_configuration
            .or_else(|| stored.ingress_configuration.clone()),
        ip_address_type: supplied
            .ip_address_type
            .or_else(|| stored.ip_address_type.clone()),
    }
}

/// One page of `items`, resuming after the cursor `token` names.
///
/// `items` is `(cursor key, value)` in the order the operation reports them, so
/// the token is a position in that order rather than an index that would shift
/// as records are added. `MaxResults` absent means "the request retrieves all
/// available results in a single response" (both list operations' model doc).
/// `Err` is a token that names no position in the result set.
pub fn page<T: Clone>(
    items: &[(String, T)],
    token: Option<&str>,
    max_results: Option<i32>,
) -> Result<(Vec<T>, Option<String>), String> {
    let start = match token {
        None => 0,
        Some(token) => items
            .iter()
            .position(|(key, _)| key == token)
            .ok_or_else(|| {
                "NextToken does not name a position in this result set; it is from a \
                 different request or the records it named are gone"
                    .to_string()
            })?,
    };
    let limit = match max_results {
        // The modelled `MaxResults` range (1..20) is enforced at the boundary, so
        // a non-positive limit cannot arrive from the wire; as a public helper
        // this treats one as "no limit" rather than returning an empty page with
        // a token that would make a paging client loop forever.
        Some(max) if max > 0 => max as usize,
        _ => items.len(),
    };
    let end = start.saturating_add(limit).min(items.len());
    let values = items[start..end]
        .iter()
        .map(|(_, value)| value.clone())
        .collect();
    let next_token = items.get(end).map(|(key, _)| key.clone());
    Ok((values, next_token))
}
