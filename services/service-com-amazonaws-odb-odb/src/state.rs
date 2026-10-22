// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-odb-odb service.
//!
//! The service's primary resource is the **ODB network**: every Exadata
//! infrastructure, VM cluster and autonomous database of an Oracle Database@AWS
//! deployment hangs off one. Everything the ODB-network handlers answer with is
//! derived from the records kept here, so a write made by one request is what the
//! next request reads.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use ::aws_smithy_types::DateTime;
use ::ipnet::Ipv4Net;

use crate::ops::types::{
    CrossRegionS3RestoreSourcesAccess, KmsAccess, ManagedServices, OdbNetwork, OdbNetworkSummary,
    S3Access, StsAccess, ZeroEtlAccess,
};

/// The `ResourceStatus` an ODB network carries while it is still being provisioned.
pub const STATUS_PROVISIONING: &str = "PROVISIONING";
/// The `ResourceStatus` a provisioned ODB network carries.
pub const STATUS_AVAILABLE: &str = "AVAILABLE";
/// The `ResourceStatus` an ODB network carries while an update is being applied.
pub const STATUS_UPDATING: &str = "UPDATING";

/// The `Access` value for an enabled access configuration.
pub const ACCESS_ENABLED: &str = "ENABLED";
/// The `Access` value for a disabled access configuration — what an ODB network
/// created without an explicit `*Access` member carries.
pub const ACCESS_DISABLED: &str = "DISABLED";

/// How long a freshly created ODB network stays `PROVISIONING`, and an updated one
/// stays `UPDATING`, before the clock owes it the terminal `AVAILABLE`.
///
/// CHOSEN, not documented: AWS publishes no provisioning duration for an ODB network.
/// The emulator contract is a non-zero span, so a caller that reads straight after the
/// call still observes the transient status and only a later read sees the network
/// settled.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(400);

/// The `resourceType` this service reports for an ODB network in an error body.
pub const ODB_NETWORK_RESOURCE_TYPE: &str = "ODB_NETWORK";

/// This emulator's name for the per-account ODB network quota. AWS's own quota code is
/// not in the model, and `ServiceQuotaExceededException$quotaCode` is `@required`, so
/// the limit is named after itself.
pub const ODB_NETWORKS_QUOTA_CODE: &str = "OdbNetworksPerAccount";

/// The most peered CIDR ranges one ODB network may hold at a time.
///
/// CHOSEN: AWS publishes no figure. The model caps one `peeredCidrsToBeAdded` list at
/// 1024 entries, which bounds a single call but not the accumulation across calls, so
/// this is the bound that keeps a record — and therefore the store — finite. A request
/// that would take a network past it is refused with the modelled `ValidationException`.
pub const MAX_PEERED_CIDRS_PER_ODB_NETWORK: usize = 64;

/// The most cross-Region Amazon S3 restore sources one ODB network may hold at a time,
/// with the same contract as [`MAX_PEERED_CIDRS_PER_ODB_NETWORK`]. A restore source is
/// an Amazon Web Services Region, and there are fewer than this many.
pub const MAX_CROSS_REGION_RESTORE_SOURCES_PER_ODB_NETWORK: usize = 64;

/// The most ODB networks one account may hold at a time; a create beyond it is refused
/// with the modelled `ServiceQuotaExceededException` (HTTP 402).
///
/// CHOSEN: AWS quotas ODB networks per account but does not publish the figure. The
/// bound is also what keeps the store finite — a record is released only by
/// `DeleteOdbNetwork`.
pub const MAX_ODB_NETWORKS_PER_ACCOUNT: usize = 100;

/// How many ids a mint tries before giving up, so no request can spin here. A candidate
/// carries 96 random bits and the store holds at most
/// [`MAX_ODB_NETWORKS_PER_ACCOUNT`] records, so the first candidate is free in every
/// realistic run.
const MINT_ATTEMPTS: usize = 8;

/// The CIDR ranges OCI reserves, which an ODB network's client or backup subnet must
/// not use.
///
/// SOURCE: the `clientSubnetCidr` / `backupSubnetCidr` member docs in this service's
/// Smithy model ("Must not use the following CIDR ranges that are reserved by OCI:
/// 100.106.0.0/16 and 100.107.0.0/16 169.254.0.0/16 224.0.0.0 - 239.255.255.255
/// 240.0.0.0 - 255.255.255.255"). The last two ranges are the multicast and
/// reserved-for-future-use /4 blocks, written here in CIDR form.
pub const OCI_RESERVED_CIDRS: &[&str] = &[
    "100.106.0.0/16",
    "100.107.0.0/16",
    "169.254.0.0/16",
    "224.0.0.0/4",
    "240.0.0.0/4",
];

/// The `@pattern` the model declares on `ResourceDisplayName`, the shape every
/// `displayName` member targets.
///
/// SOURCE: `smithy_query(query="operation", operation="CreateOdbNetwork")` reports
/// `displayName` as `ResourceDisplayName` with this pattern. The negative lookahead is
/// why it has to be checked here: the vendored boundary validator compiles a `@pattern`
/// with the `regex` crate, which has no look-around, and treats a pattern it cannot
/// compile as matching (`vendor/smithy-schema-server/src/constraints.rs`).
pub const RESOURCE_DISPLAY_NAME_PATTERN: &str = r"^[a-zA-Z_](?!.*--)[a-zA-Z0-9_-]*$";

/// [`RESOURCE_DISPLAY_NAME_PATTERN`], compiled once.
static RESOURCE_DISPLAY_NAME_REGEX: LazyLock<::fancy_regex::Regex> = LazyLock::new(|| {
    ::fancy_regex::Regex::new(RESOURCE_DISPLAY_NAME_PATTERN)
        .expect("ResourceDisplayName's pattern compiles")
});

/// Whether `value` satisfies [`RESOURCE_DISPLAY_NAME_PATTERN`].
pub fn display_name_matches(value: &str) -> bool {
    RESOURCE_DISPLAY_NAME_REGEX.is_match(value).unwrap_or(false)
}

/// The form of an Amazon Web Services Region name, which is what a cross-Region Amazon
/// S3 restore source names.
///
/// SOURCE: the published Region codes (`us-east-1`, `ap-southeast-3`, `eu-central-1`,
/// `us-gov-west-1`, `us-iso-east-1`): lowercase partition and area words joined by
/// hyphens and ending in an index. The model targets the member at a plain `StringList`,
/// so this is the only check that keeps an arbitrary caller string out of the record.
pub const AWS_REGION_PATTERN: &str = r"^[a-z]{2}(-[a-z]+)+-[0-9]{1,2}$";

/// [`AWS_REGION_PATTERN`], compiled once.
static AWS_REGION_REGEX: LazyLock<::fancy_regex::Regex> = LazyLock::new(|| {
    ::fancy_regex::Regex::new(AWS_REGION_PATTERN).expect("the Region pattern compiles")
});

/// Whether `value` is an Amazon Web Services Region name ([`AWS_REGION_PATTERN`]).
pub fn is_region_name(value: &str) -> bool {
    AWS_REGION_REGEX.is_match(value).unwrap_or(false)
}

/// The region the caller signed this request for, defaulting to `us-east-1` for an
/// unsigned request — the same default the emulator's own clients use.
pub fn request_region(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| "us-east-1".to_string())
}

/// This service's ARN for an ODB network:
/// `arn:aws:odb:<region>:<account>:odb-network/<id>`.
pub fn odb_network_arn(region: &str, odb_network_id: &str) -> String {
    ::engine::emulator_arn("odb", region, &format!("odb-network/{odb_network_id}"))
}

/// A fresh opaque lowercase-hex ODB network id.
///
/// 24 characters, which satisfies both the `ResourceIdOrArn` id form
/// (`[a-zA-Z0-9_~.-]{6,64}`) that the read operations accept and the `ResourceArn`
/// resource segment (`[a-z0-9-_]{6,64}`) the ARN is built from.
pub fn new_odb_network_id() -> String {
    let mut id = ::uuid::Uuid::new_v4().simple().to_string();
    id.truncate(24);
    id
}

/// The resource id a `ResourceIdOrArn` value addresses: the value itself when it is a
/// bare id, or the ARN's resource segment when it is an ARN.
pub fn resource_id_of(id_or_arn: &str) -> &str {
    if id_or_arn.starts_with("arn:") {
        id_or_arn.split('/').next_back().unwrap_or(id_or_arn)
    } else {
        id_or_arn
    }
}

/// Parse an IPv4 CIDR block, normalised to its network address, or `None` when the
/// value is not one.
pub fn parse_cidr(value: &str) -> Option<Ipv4Net> {
    value
        .parse::<Ipv4Net>()
        .ok()
        .map(|network| network.trunc())
}

/// Whether two CIDR blocks share any address. Two CIDR blocks are either disjoint or
/// one contains the other, so containment either way is the whole test.
pub fn cidrs_overlap(left: Ipv4Net, right: Ipv4Net) -> bool {
    left.contains(&right) || right.contains(&left)
}

/// The canonical text of an IPv4 CIDR block — its network address and prefix length —
/// or `None` when the value is not one.
///
/// A stored CIDR range is kept in this form so that two spellings of one range
/// (`10.9.0.0/16` and `10.9.0.1/16`) are one entry rather than two.
pub fn canonical_cidr(value: &str) -> Option<String> {
    parse_cidr(value).map(|network| network.to_string())
}

/// `stored` with `additions` appended, skipping an entry it already holds, or `None`
/// when the result would hold more than `maximum` entries.
///
/// The caller-grown lists on an ODB network record are bounded HERE: the model caps one
/// request's list, which bounds a single call but not the accumulation across calls.
/// `stored` is left untouched on refusal, so a request that is past the bound changes
/// nothing. Membership is tested through a set, so the work done while the state lock is
/// held stays logarithmic in the stored length rather than quadratic.
pub fn extended_within(
    stored: &[String],
    additions: &[String],
    maximum: usize,
) -> Option<Vec<String>> {
    let mut present: BTreeSet<&str> = stored.iter().map(String::as_str).collect();
    let mut extended = stored.to_vec();
    for addition in additions {
        if !present.insert(addition.as_str()) {
            continue;
        }
        if extended.len() >= maximum {
            return None;
        }
        extended.push(addition.clone());
    }
    Some(extended)
}

/// The OCI-reserved range this CIDR block intrudes on, if any.
pub fn reserved_cidr_conflict(candidate: Ipv4Net) -> Option<&'static str> {
    OCI_RESERVED_CIDRS.iter().copied().find(|reserved| {
        parse_cidr(reserved).is_some_and(|reserved| cidrs_overlap(candidate, reserved))
    })
}

/// The `ManagedResourceStatus` that reports an `Access` flag: an enabled access
/// configuration is `ENABLED`, a disabled one `DISABLED`.
fn managed_status(access: &str) -> String {
    if access == ACCESS_ENABLED {
        ACCESS_ENABLED.to_string()
    } else {
        ACCESS_DISABLED.to_string()
    }
}

/// One ODB network, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct OdbNetworkRecord {
    pub odb_network_id: String,
    pub odb_network_arn: String,
    pub display_name: String,
    pub status: String,
    pub status_reason: Option<String>,
    pub availability_zone: Option<String>,
    pub availability_zone_id: Option<String>,
    pub client_subnet_cidr: String,
    pub backup_subnet_cidr: Option<String>,
    pub custom_domain_name: Option<String>,
    pub default_dns_prefix: Option<String>,
    pub peered_cidrs: Vec<String>,
    pub s3_access: String,
    pub zero_etl_access: String,
    pub sts_access: String,
    pub kms_access: String,
    pub s3_policy_document: Option<String>,
    pub sts_policy_document: Option<String>,
    pub kms_policy_document: Option<String>,
    pub cross_region_s3_restore_sources: Vec<String>,
    pub tags: BTreeMap<String, String>,
    pub created_at: DateTime,
    /// The instant the transient status (`PROVISIONING` / `UPDATING`) falls due for
    /// `AVAILABLE`.
    pub settles_at: Instant,
}

impl OdbNetworkRecord {
    /// Materialise the status change the clock already owes this network. A read calls
    /// this because the transition was due at that instant whether or not anybody asked.
    fn settle(&mut self, now: Instant) {
        let transient = self.status == STATUS_PROVISIONING || self.status == STATUS_UPDATING;
        if transient && now >= self.settles_at {
            self.status = STATUS_AVAILABLE.to_string();
        }
    }

    /// Start a transient status that the clock owes `AVAILABLE` after
    /// [`TRANSITION_SPAN`].
    pub fn begin_transition(&mut self, status: &str) {
        self.status = status.to_string();
        self.settles_at = Instant::now() + TRANSITION_SPAN;
    }

    /// How far provisioning has got: a settled network is complete, a network still in
    /// a transient status has not started reporting progress.
    pub fn percent_progress(&self) -> f32 {
        if self.status == STATUS_AVAILABLE {
            100.0
        } else {
            0.0
        }
    }

    /// The `managedServices` view of this record: the access configuration the stored
    /// `*Access` flags and policy documents describe.
    pub fn managed_services(&self) -> ManagedServices {
        ManagedServices {
            // Not modelled by this emulator: no VPC Lattice service network, resource
            // gateway or VPC endpoint is created, so there is no ARN or endpoint to
            // report. The access configuration below is what the record determines.
            service_network_arn: None,
            resource_gateway_arn: None,
            managed_services_ipv4_cidrs: None,
            service_network_endpoint: None,
            managed_s3_backup_access: None,
            zero_etl_access: Some(ZeroEtlAccess {
                status: Some(managed_status(&self.zero_etl_access)),
                cidr: None,
            }),
            s3_access: Some(S3Access {
                status: Some(managed_status(&self.s3_access)),
                ipv4_addresses: None,
                domain_name: None,
                s3_policy_document: self.s3_policy_document.clone(),
            }),
            sts_access: Some(StsAccess {
                status: Some(managed_status(&self.sts_access)),
                ipv4_addresses: None,
                domain_name: None,
                sts_policy_document: self.sts_policy_document.clone(),
            }),
            kms_access: Some(KmsAccess {
                status: Some(managed_status(&self.kms_access)),
                ipv4_addresses: None,
                domain_name: None,
                kms_policy_document: self.kms_policy_document.clone(),
            }),
            cross_region_s3_restore_sources_access: (!self
                .cross_region_s3_restore_sources
                .is_empty())
            .then(|| {
                self.cross_region_s3_restore_sources
                    .iter()
                    .map(|region| CrossRegionS3RestoreSourcesAccess {
                        region: Some(region.clone()),
                        ipv4_addresses: None,
                        status: Some(ACCESS_ENABLED.to_string()),
                    })
                    .collect()
            }),
        }
    }

    /// The `OdbNetwork` structure `GetOdbNetwork` answers with.
    pub fn to_odb_network(&self) -> OdbNetwork {
        OdbNetwork {
            odb_network_id: self.odb_network_id.clone(),
            display_name: Some(self.display_name.clone()),
            status: Some(self.status.clone()),
            status_reason: self.status_reason.clone(),
            odb_network_arn: Some(self.odb_network_arn.clone()),
            availability_zone: self.availability_zone.clone(),
            availability_zone_id: self.availability_zone_id.clone(),
            client_subnet_cidr: Some(self.client_subnet_cidr.clone()),
            backup_subnet_cidr: self.backup_subnet_cidr.clone(),
            custom_domain_name: self.custom_domain_name.clone(),
            default_dns_prefix: self.default_dns_prefix.clone(),
            peered_cidrs: Some(self.peered_cidrs.clone()),
            // The emulator onboards no OCI tenancy, so it has no OCI identifiers or
            // DNS forwarding configuration to report.
            oci_network_anchor_id: None,
            oci_network_anchor_url: None,
            oci_resource_anchor_name: None,
            oci_vcn_id: None,
            oci_vcn_url: None,
            oci_dns_forwarding_configs: None,
            created_at: Some(self.created_at),
            percent_progress: Some(self.percent_progress()),
            managed_services: Some(self.managed_services()),
            ec2_placement_group_ids: None,
        }
    }

    /// The `OdbNetworkSummary` structure `ListOdbNetworks` reports this network as.
    pub fn to_summary(&self) -> OdbNetworkSummary {
        let network = self.to_odb_network();
        OdbNetworkSummary {
            odb_network_id: network.odb_network_id,
            display_name: network.display_name,
            status: network.status,
            status_reason: network.status_reason,
            odb_network_arn: network.odb_network_arn,
            availability_zone: network.availability_zone,
            availability_zone_id: network.availability_zone_id,
            client_subnet_cidr: network.client_subnet_cidr,
            backup_subnet_cidr: network.backup_subnet_cidr,
            custom_domain_name: network.custom_domain_name,
            default_dns_prefix: network.default_dns_prefix,
            peered_cidrs: network.peered_cidrs,
            oci_network_anchor_id: network.oci_network_anchor_id,
            oci_network_anchor_url: network.oci_network_anchor_url,
            oci_resource_anchor_name: network.oci_resource_anchor_name,
            oci_vcn_id: network.oci_vcn_id,
            oci_vcn_url: network.oci_vcn_url,
            oci_dns_forwarding_configs: network.oci_dns_forwarding_configs,
            created_at: network.created_at,
            percent_progress: network.percent_progress,
            managed_services: network.managed_services,
            ec2_placement_group_ids: network.ec2_placement_group_ids,
        }
    }
}

/// Per-service state for com-amazonaws-odb-odb. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsOdbOdbState {
    /// Every live ODB network, keyed by its `odbNetworkId` — the service's primary
    /// index, ordered so `ListOdbNetworks` pages over a stable sequence.
    pub odb_networks: BTreeMap<String, OdbNetworkRecord>,
    /// `clientToken` -> `odbNetworkId`, so an `@idempotent` `CreateOdbNetwork` replay
    /// answers with the network the first call minted instead of a second one.
    pub odb_network_tokens: BTreeMap<String, String>,
}

impl ComAmazonawsOdbOdbState {
    /// An ODB network id no stored network holds, or `None` when [`MINT_ATTEMPTS`]
    /// candidates were all taken.
    pub fn mint_odb_network_id(&self) -> Option<String> {
        (0..MINT_ATTEMPTS)
            .map(|_| new_odb_network_id())
            .find(|candidate| !self.odb_networks.contains_key(candidate))
    }

    /// Store an ODB network, recording its `clientToken` when the request carried one.
    pub fn insert_odb_network(&mut self, record: OdbNetworkRecord, client_token: Option<&str>) {
        if let Some(token) = client_token {
            self.odb_network_tokens
                .insert(token.to_string(), record.odb_network_id.clone());
        }
        self.odb_networks
            .insert(record.odb_network_id.clone(), record);
    }

    /// The network a `ResourceIdOrArn` addresses, with any clock-owed status change
    /// materialised.
    pub fn odb_network(&mut self, id_or_arn: &str) -> Option<&OdbNetworkRecord> {
        self.odb_network_mut(id_or_arn).map(|record| &*record)
    }

    /// The network a `ResourceIdOrArn` addresses, for mutation, with any clock-owed
    /// status change materialised first.
    pub fn odb_network_mut(&mut self, id_or_arn: &str) -> Option<&mut OdbNetworkRecord> {
        let record = self.odb_networks.get_mut(resource_id_of(id_or_arn))?;
        record.settle(Instant::now());
        Some(record)
    }

    /// The network a `clientToken` replay belongs to, if the token was seen before and
    /// its network still exists.
    pub fn odb_network_for_token(&mut self, client_token: &str) -> Option<&OdbNetworkRecord> {
        let id = self.odb_network_tokens.get(client_token)?.clone();
        self.odb_network_mut(&id).map(|record| &*record)
    }

    /// Forget a network (and any `clientToken` that named it), as `DeleteOdbNetwork`
    /// does.
    pub fn remove_odb_network(&mut self, id_or_arn: &str) -> Option<OdbNetworkRecord> {
        let record = self.odb_networks.remove(resource_id_of(id_or_arn))?;
        self.odb_network_tokens
            .retain(|_, id| *id != record.odb_network_id);
        Some(record)
    }

    /// Every stored network, settled and ordered by `odbNetworkId` — the stable order
    /// `ListOdbNetworks` pages over.
    pub fn odb_networks_by_id(&mut self) -> Vec<OdbNetworkRecord> {
        let now = Instant::now();
        self.odb_networks
            .values_mut()
            .map(|record| {
                record.settle(now);
                record.clone()
            })
            .collect()
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsOdbOdbState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsOdbOdbState>)
}
