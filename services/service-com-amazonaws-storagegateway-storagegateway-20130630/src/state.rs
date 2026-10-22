// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-storagegateway-storagegateway-20130630 service.
//!
//! Two resources anchor the Storage Gateway control plane, and this module holds
//! the one copy of each that every handler reads and writes:
//!
//! * a **gateway** — minted by `ActivateGateway`, reported by `ListGateways` and
//!   `DescribeGatewayInformation`, removed by `DeleteGateway`;
//! * a **file share** — minted by `CreateNFSFileShare`, reported by
//!   `ListFileShares` and `DescribeNFSFileShares`, removed by `DeleteFileShare`.
//!
//! A share names its gateway (`FileShare::gateway_arn`) and that single field is
//! the whole gateway→share relation: there is deliberately no second index of
//! share ARNs on the gateway record, so the two can never disagree.
//!
//! Every projection the model asks for (`GatewayInfo`,
//! `DescribeGatewayInformationOutput`, `FileShareInfo`, `NFSFileShareInfo`) is
//! built here, from the stored record, so no handler invents a value.

use std::collections::BTreeMap;
use std::ops::Bound;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    CacheAttributes, DescribeGatewayInformationOutput, FileShareInfo, GatewayInfo,
    InvalidGatewayRequestException, NFSFileShareDefaults, NFSFileShareInfo, NetworkInterface,
    StorageGatewayError, Tag,
};

/// The region an ARN is built for when the request carries no SigV4 credential
/// scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace, as `OPERATIONS.md` and the model's
/// `aws.api#service` trait give it.
pub const ARN_NAMESPACE: &str = "storagegateway";

/// The gateway type `ActivateGateway` records when the request names none:
/// `ActivateGatewayInput$GatewayType` documents "The default value is CACHED".
pub const DEFAULT_GATEWAY_TYPE: &str = "CACHED";

/// The gateway types `ActivateGatewayInput$GatewayType` lists as valid values
/// ("STORED | CACHED | VTL | FILE_S3 | FILE_FSX_SMB"). The model constrains the
/// member only by length, so this list is what the handler refuses against.
pub const GATEWAY_TYPE_VALUES: &[&str] =
    &["STORED", "CACHED", "VTL", "FILE_S3", "FILE_FSX_SMB"];

/// The gateway type that backs an NFS file share: `CreateNFSFileShare` documents
/// "This operation is only supported for S3 File Gateways", and
/// `ActivateGatewayInput$GatewayType` names that type `FILE_S3`.
pub const S3_FILE_GATEWAY_TYPE: &str = "FILE_S3";

/// The operational state of an activated gateway: `GatewayInfo$GatewayOperationalState`
/// documents "Valid Values: DISABLED | ACTIVE".
pub const GATEWAY_OPERATIONAL_STATE_ACTIVE: &str = "ACTIVE";

/// The operating state `DescribeGatewayInformation` reports for a gateway whose
/// appliance is up. `ActivateGateway` documents "You must turn on the gateway VM
/// before you can activate your gateway", so an activated gateway is running the
/// instant the activation returns — there is no asynchronous work left for the
/// clock to settle.
pub const GATEWAY_STATE_RUNNING: &str = "RUNNING";

/// The endpoint type of a gateway activated against this emulator:
/// `DescribeGatewayInformationOutput$EndpointType` documents "Valid Values:
/// STANDARD | FIPS", and `ActivateGateway` carries no member asking for FIPS.
pub const ENDPOINT_TYPE_STANDARD: &str = "STANDARD";

/// The host platform reported for an emulated gateway. `HostEnvironment`'s values
/// are `EC2`, `HYPER-V`, `KVM`, `OTHER`, `SNOWBALL` and `VMWARE`; this emulator is
/// none of those hypervisors or devices, so `OTHER` is the only honest answer —
/// and it is why `Ec2InstanceId` / `Ec2InstanceRegion` / `HostEnvironmentId`,
/// which AWS reports "only available for certain host environments", stay absent.
pub const HOST_ENVIRONMENT_OTHER: &str = "OTHER";

/// The gateway appliance software version this emulator reports. It is the
/// emulator's OWN constant — no appliance is running — and it is stable so a
/// caller reading a gateway twice sees one version.
pub const SOFTWARE_VERSION: &str = "2.0.0.0";

/// The IPv4 address block the reported gateway network interface is drawn from:
/// RFC 5737 TEST-NET-1 (`192.0.2.0/24`), reserved for documentation, because the
/// emulator has no appliance with a real address to report.
pub const NETWORK_INTERFACE_BLOCK: &str = "192.0.2";

/// The status a file share carries while the gateway is still building it:
/// `FileShareStatus` documents "Valid Values: CREATING | UPDATING | AVAILABLE |
/// DELETING".
pub const FILE_SHARE_STATUS_CREATING: &str = "CREATING";

/// The status a file share carries once it is usable.
pub const FILE_SHARE_STATUS_AVAILABLE: &str = "AVAILABLE";

/// The `FileShareType` of a share `CreateNFSFileShare` minted: `FileShareType`
/// admits `NFS` and `SMB`.
pub const FILE_SHARE_TYPE_NFS: &str = "NFS";

/// The storage class a share records when the request names none:
/// `CreateNFSFileShareInput$DefaultStorageClass` documents "The default value is
/// S3_STANDARD".
pub const DEFAULT_STORAGE_CLASS: &str = "S3_STANDARD";

/// The object ACL a share records when the request names none:
/// `CreateNFSFileShareInput$ObjectACL` documents "The default value is private".
pub const DEFAULT_OBJECT_ACL: &str = "private";

/// The anonymous-user mapping a share records when the request names none.
/// `Squash` admits `RootSquash`, `NoSquash` and `AllSquash`; the Storage Gateway
/// User Guide's NFS file share settings default the mapping to `RootSquash`
/// (only root is mapped to the anonymous user), the most restrictive of the three.
pub const DEFAULT_SQUASH: &str = "RootSquash";

/// Whether a share guesses an object's MIME type when the request says nothing:
/// `CreateNFSFileShareInput$GuessMIMETypeEnabled` documents "The default value is
/// true".
pub const DEFAULT_GUESS_MIME_TYPE_ENABLED: bool = true;

/// The NFS mount point an S3 File Gateway exposes for a share. A share is mounted
/// at its own root, so `NFSFileShareInfo$Path` is the root path.
pub const FILE_SHARE_PATH: &str = "/";

/// The Unix file mode a share's NFS defaults carry when the request names none:
/// `NFSFileShareDefaults$FileMode` documents "The default value is 0666".
pub const DEFAULT_FILE_MODE: &str = "0666";

/// The Unix directory mode a share's NFS defaults carry when the request names
/// none: `NFSFileShareDefaults$DirectoryMode` documents "The default value is
/// 0777".
pub const DEFAULT_DIRECTORY_MODE: &str = "0777";

/// The owner and group id a share's NFS defaults carry when the request names
/// none. `NFSFileShareDefaults$OwnerId` / `$GroupId` document the default as
/// `nfsnobody`, which is uid/gid 65534 on Linux.
pub const NFSNOBODY_ID: i64 = 65534;

/// The largest page a list operation serves, and the page it serves when the
/// request names no `Limit`.
///
/// `ListGatewaysInput$Limit` and `ListFileSharesInput$Limit` are modelled
/// `@range(min: 1)` with no maximum and document no default, so this is the
/// emulator's OWN ceiling: a `Limit` above it is clamped, so no number a request
/// carries sizes a response, and `Marker` still reaches every record.
pub const MAX_PAGE_LIMIT: i32 = 100;

/// How long a file share reports `CREATING` before the clock owes it `AVAILABLE`.
///
/// Storage Gateway builds a share asynchronously — `CreateNFSFileShare` answers
/// with the ARN alone and the share is first reported `CREATING`. The emulator
/// chooses the span; it is deliberately NOT zero, so a caller acting straight
/// after the create still observes the transient status. The due instant is
/// stored on the record and materialised by whichever later call reads it:
/// nothing sleeps.
pub const FILE_SHARE_TRANSITION: Duration = Duration::from_millis(500);

/// The modeled `ErrorCode` a refusal names when the request's gateway ARN names
/// no activated gateway.
pub const ERROR_CODE_GATEWAY_NOT_FOUND: &str = "GatewayNotFound";

/// The modeled `ErrorCode` a refusal names when a member's value is one the
/// service does not accept.
pub const ERROR_CODE_INVALID_PARAMETERS: &str = "InvalidParameters";

/// The modeled `ErrorCode` a refusal names when the operation does not apply to
/// the gateway it was addressed to.
pub const ERROR_CODE_NOT_SUPPORTED: &str = "NotSupported";

/// The ceiling on how many gateways this store holds.
///
/// A generated emulator keeps its resources in memory for the life of the process
/// and an activation is the only thing that adds one, so the store needs a maximum
/// entry count — neither a TTL nor an eviction policy is available to it, because
/// `DescribeGatewayInformation` must still answer what `ActivateGateway` activated.
/// `ActivateGateway` refuses past this cap with this service's one modelled client
/// error. The number is the emulator's OWN bound, not a published AWS default:
/// Storage Gateway publishes its gateway quota as adjustable, so no single value is
/// the contract, and this one is far above any workflow an emulator serves.
pub const MAX_GATEWAYS: usize = 100;

/// The ceiling on how many file shares this store holds, over all gateways.
/// See [`MAX_GATEWAYS`]; `CreateNFSFileShare` refuses past it.
pub const MAX_FILE_SHARES: usize = 1_000;

/// The ceiling on how many file shares one gateway holds. Together with
/// [`MAX_GATEWAYS`] it bounds the shares a caller can create per gateway as well as
/// in total. AWS publishes a (lower, adjustable) file-shares-per-file-gateway
/// quota, so this is the emulator's own bound rather than that contract.
pub const MAX_FILE_SHARES_PER_GATEWAY: usize = 50;

/// A status change the clock owes a resource: the instant it falls due, and the
/// status the resource carries from then on.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    pub due: SystemTime,
    pub status: String,
}

/// One activated gateway, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct Gateway {
    /// `arn:aws:storagegateway:<region>:<account>:gateway/<gateway_id>`.
    pub arn: String,
    /// `GatewayId`, len 12..30, minted by [`ComAmazonawsStoragegatewayStoragegateway20130630State::mint_gateway_id`].
    pub gateway_id: String,
    pub name: String,
    pub timezone: String,
    /// The `GatewayRegion` the activation asked for — the Region the gateway
    /// stores its data in.
    pub region: String,
    pub gateway_type: String,
    pub operational_state: String,
    pub gateway_state: String,
    /// The activation key the gateway was activated with. Never reported: no
    /// modeled output member carries it.
    pub activation_key: String,
    pub tape_drive_type: Option<String>,
    pub medium_changer_type: Option<String>,
    pub host_environment: String,
    pub endpoint_type: String,
    pub software_version: String,
    pub network_interfaces: Vec<NetworkInterface>,
    pub tags: Vec<Tag>,
    pub activated_at: SystemTime,
}

impl Gateway {
    /// The `GatewayInfo` projection `ListGateways` reports.
    pub fn to_gateway_info(&self) -> GatewayInfo {
        GatewayInfo {
            gateway_id: Some(self.gateway_id.clone()),
            gateway_arn: Some(self.arn.clone()),
            gateway_type: Some(self.gateway_type.clone()),
            gateway_operational_state: Some(self.operational_state.clone()),
            gateway_name: Some(self.name.clone()),
            // Both EC2 members describe "the Amazon EC2 instance that was used to
            // launch the gateway"; this gateway runs on no EC2 instance (see
            // [`HOST_ENVIRONMENT_OTHER`]), so neither is reported.
            ec2_instance_id: None,
            ec2_instance_region: None,
            host_environment: Some(self.host_environment.clone()),
            // "only available for certain host environments" — not this one.
            host_environment_id: None,
            // A deprecation date belongs to a gateway appliance release that will
            // stop receiving updates; this emulator publishes no such schedule.
            deprecation_date: None,
            software_version: Some(self.software_version.clone()),
        }
    }

    /// The whole-gateway report `DescribeGatewayInformation` answers with.
    pub fn to_description(&self) -> DescribeGatewayInformationOutput {
        DescribeGatewayInformationOutput {
            gateway_arn: Some(self.arn.clone()),
            gateway_id: Some(self.gateway_id.clone()),
            gateway_name: Some(self.name.clone()),
            gateway_timezone: Some(self.timezone.clone()),
            gateway_state: Some(self.gateway_state.clone()),
            gateway_network_interfaces: Some(self.network_interfaces.clone()),
            gateway_type: Some(self.gateway_type.clone()),
            // Reported only when an update is pending, or has been applied; this
            // emulator ships one appliance version and never updates it.
            next_update_availability_date: None,
            last_software_update: None,
            ec2_instance_id: None,
            ec2_instance_region: None,
            tags: Some(self.tags.clone()),
            // Set by the VPC endpoint configuration a gateway is activated
            // against; this emulator models no VPC endpoints.
            vpc_endpoint: None,
            // Chosen through `UpdateGatewayInformation`, which is still a stub.
            cloud_watch_log_group_arn: None,
            host_environment: Some(self.host_environment.clone()),
            endpoint_type: Some(self.endpoint_type.clone()),
            software_updates_end_date: None,
            deprecation_date: None,
            // The metadata cache size is chosen through
            // `UpdateGatewayInformation`, which is still a stub, and the sizes a
            // gateway supports depend on hardware this emulator does not have.
            gateway_capacity: None,
            supported_gateway_capacities: None,
            host_environment_id: None,
            software_version: Some(self.software_version.clone()),
        }
    }
}

/// One file share, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct FileShare {
    /// `arn:aws:storagegateway:<region>:<account>:share/<file_share_id>`.
    pub arn: String,
    /// `FileShareId`, len 12..30.
    pub file_share_id: String,
    /// One of `FileShareType`'s values.
    pub share_type: String,
    /// One of `FileShareStatus`'s values.
    pub status: String,
    /// The status change the clock owes this share, if any.
    pub pending: Option<PendingTransition>,
    /// The ARN of the gateway this share lives on. This field IS the
    /// gateway→share relation.
    pub gateway_arn: String,
    /// The idempotency token the create carried.
    pub client_token: String,
    pub role: String,
    pub location_arn: String,
    pub path: String,
    pub file_share_name: Option<String>,
    pub default_storage_class: String,
    pub object_acl: String,
    pub client_list: Vec<String>,
    pub squash: String,
    pub read_only: bool,
    pub guess_mime_type_enabled: bool,
    pub requester_pays: bool,
    pub kms_encrypted: bool,
    pub kms_key: Option<String>,
    pub encryption_type: Option<String>,
    pub nfs_defaults: NFSFileShareDefaults,
    pub cache_attributes: Option<CacheAttributes>,
    pub notification_policy: Option<String>,
    pub vpc_endpoint_dns_name: Option<String>,
    pub bucket_region: Option<String>,
    pub audit_destination_arn: Option<String>,
    pub tags: Vec<Tag>,
    pub created_at: SystemTime,
}

impl FileShare {
    /// Materialise the status change the clock owes this share, if `now` is past
    /// its due instant.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(pending) = self.pending.as_ref() else {
            return;
        };
        if now < pending.due {
            return;
        }
        self.status = pending.status.clone();
        self.pending = None;
    }

    /// The `FileShareInfo` projection `ListFileShares` reports.
    pub fn to_file_share_info(&self) -> FileShareInfo {
        FileShareInfo {
            file_share_type: Some(self.share_type.clone()),
            file_share_arn: Some(self.arn.clone()),
            file_share_id: Some(self.file_share_id.clone()),
            file_share_status: Some(self.status.clone()),
            gateway_arn: Some(self.gateway_arn.clone()),
        }
    }

    /// The whole-share report `DescribeNFSFileShares` answers with.
    pub fn to_nfs_info(&self) -> NFSFileShareInfo {
        NFSFileShareInfo {
            nfs_file_share_defaults: Some(self.nfs_defaults.clone()),
            file_share_arn: Some(self.arn.clone()),
            file_share_id: Some(self.file_share_id.clone()),
            file_share_status: Some(self.status.clone()),
            gateway_arn: Some(self.gateway_arn.clone()),
            encryption_type: self.encryption_type.clone(),
            kms_encrypted: self.kms_encrypted,
            kms_key: self.kms_key.clone(),
            path: Some(self.path.clone()),
            role: Some(self.role.clone()),
            location_arn: Some(self.location_arn.clone()),
            default_storage_class: Some(self.default_storage_class.clone()),
            object_acl: Some(self.object_acl.clone()),
            client_list: Some(self.client_list.clone()),
            squash: Some(self.squash.clone()),
            read_only: Some(self.read_only),
            guess_mime_type_enabled: Some(self.guess_mime_type_enabled),
            requester_pays: Some(self.requester_pays),
            tags: Some(self.tags.clone()),
            file_share_name: self.file_share_name.clone(),
            cache_attributes: self.cache_attributes.clone(),
            notification_policy: self.notification_policy.clone(),
            vpc_endpoint_dns_name: self.vpc_endpoint_dns_name.clone(),
            bucket_region: self.bucket_region.clone(),
            audit_destination_arn: self.audit_destination_arn.clone(),
        }
    }
}

/// Per-service state for com-amazonaws-storagegateway-storagegateway-20130630. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsStoragegatewayStoragegateway20130630State {
    /// Every activated gateway, keyed by its ARN. A `BTreeMap` is what gives
    /// `ListGateways` the stable total order its `Marker` indexes into: an ARN
    /// ends in a fixed-width minted id, so ARNs sort in activation order.
    pub gateways: BTreeMap<String, Gateway>,
    /// Every file share, keyed by its ARN, in the same stable order for
    /// `ListFileShares`. A share carries the `ClientToken` it was created with, so
    /// this map is also the idempotency index — there is no second collection of
    /// tokens to outlive the shares it names.
    pub file_shares: BTreeMap<String, FileShare>,
    /// The counter gateway ids are minted from.
    pub gateway_id_seq: u64,
    /// The counter file share ids are minted from.
    pub file_share_id_seq: u64,
}

impl ComAmazonawsStoragegatewayStoragegateway20130630State {
    /// Mint an unused `GatewayId`: `sgw-` and eight hex digits, the shape Storage
    /// Gateway ids take, which is 12 characters — inside `GatewayId`'s 12..30.
    ///
    /// Eight hex digits hold 32 bits, so the counter is masked to that width; the
    /// loop skips an id already taken, which is what makes the wrap safe.
    pub fn mint_gateway_id(&mut self) -> String {
        loop {
            self.gateway_id_seq = self.gateway_id_seq.wrapping_add(1);
            let id = format!("sgw-{:08X}", self.gateway_id_seq & 0xFFFF_FFFF);
            if !self.gateways.values().any(|gateway| gateway.gateway_id == id) {
                return id;
            }
        }
    }

    /// Mint an unused `FileShareId`: `share-` and eight hex digits, 14 characters,
    /// inside `FileShareId`'s 12..30.
    pub fn mint_file_share_id(&mut self) -> String {
        loop {
            self.file_share_id_seq = self.file_share_id_seq.wrapping_add(1);
            let id = format!("share-{:08X}", self.file_share_id_seq & 0xFFFF_FFFF);
            if !self
                .file_shares
                .values()
                .any(|share| share.file_share_id == id)
            {
                return id;
            }
        }
    }

    /// Materialise every file share status change the clock owes, for a caller
    /// about to report on more than one of them.
    pub fn settle_file_shares(&mut self, now: SystemTime) {
        for share in self.file_shares.values_mut() {
            share.settle(now);
        }
    }

    /// How many file shares live on `gateway_arn`, which is what
    /// [`MAX_FILE_SHARES_PER_GATEWAY`] bounds.
    pub fn file_shares_on(&self, gateway_arn: &str) -> usize {
        self.file_shares
            .values()
            .filter(|share| share.gateway_arn == gateway_arn)
            .count()
    }

    /// The ARN of the share `client_token` already created on `gateway_arn`, if any.
    ///
    /// This is `CreateNFSFileShare`'s idempotency lookup, answered from the shares
    /// themselves: a token is remembered exactly as long as its share is, so a
    /// deleted share's token names nothing.
    pub fn share_created_with(&self, gateway_arn: &str, client_token: &str) -> Option<String> {
        self.file_shares
            .values()
            .find(|share| share.gateway_arn == gateway_arn && share.client_token == client_token)
            .map(|share| share.arn.clone())
    }

    /// Forget a gateway and, with it, every file share that lived on it.
    ///
    /// Returns the removed gateway, or `None` when the ARN named none.
    pub fn remove_gateway(&mut self, gateway_arn: &str) -> Option<Gateway> {
        let gateway = self.gateways.remove(gateway_arn)?;
        self.file_shares
            .retain(|_, share| share.gateway_arn != gateway_arn);
        Some(gateway)
    }

    /// Forget a file share.
    pub fn remove_file_share(&mut self, file_share_arn: &str) -> Option<FileShare> {
        self.file_shares.remove(file_share_arn)
    }
}

/// One page of a list operation: the records to report, and the marker a caller
/// passes back for the rest — present only when records remain.
#[derive(Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_marker: Option<String>,
}

/// The entries of a key-ordered map that come after `marker`.
///
/// `marker` is the key the previous page ended on; a marker no record carries
/// still positions correctly, because the keys are totally ordered. The iterator
/// is lazy, so a caller that takes one page walks one page.
pub fn after_marker<'a, V>(
    records: &'a BTreeMap<String, V>,
    marker: Option<&str>,
) -> impl Iterator<Item = (&'a String, &'a V)> {
    let lower = match marker {
        Some(marker) => Bound::Excluded(marker.to_string()),
        None => Bound::Unbounded,
    };
    records.range((lower, Bound::Unbounded))
}

/// Cut the page a request asked for out of `ordered` — the records that come
/// after its marker, each paired with the key it sorts by.
///
/// At most [`MAX_PAGE_LIMIT`] records are pulled from `ordered` (plus the one
/// that decides whether a marker is owed), so a page is bounded whatever the
/// request asks for and whatever the store holds.
pub fn paginate<T>(ordered: impl IntoIterator<Item = (String, T)>, limit: Option<i32>) -> Page<T> {
    let limit = limit.unwrap_or(MAX_PAGE_LIMIT).clamp(1, MAX_PAGE_LIMIT) as usize;

    let mut rest = ordered.into_iter();
    let page: Vec<(String, T)> = rest.by_ref().take(limit).collect();
    let next_marker = match rest.next() {
        Some(_) => page.last().map(|(key, _)| key.clone()),
        None => None,
    };

    Page {
        items: page.into_iter().map(|(_, item)| item).collect(),
        next_marker,
    }
}

/// The ARN of the gateway `gateway_id` names, in `region`.
pub fn gateway_arn(region: &str, gateway_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("gateway/{gateway_id}"))
}

/// The ARN of the file share `file_share_id` names, in `region`.
pub fn file_share_arn(region: &str, file_share_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("share/{file_share_id}"))
}

/// The region this request was signed for — the region this service's ARNs name.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The one network interface reported for an emulated gateway appliance.
///
/// The address is stable per gateway (it is derived from the minted id's
/// counter) and comes from the documentation-only block in
/// [`NETWORK_INTERFACE_BLOCK`]; the MAC and IPv6 addresses of an appliance that
/// does not exist are not invented.
pub fn gateway_network_interfaces(sequence: u64) -> Vec<NetworkInterface> {
    let host = 1 + (sequence % 254);
    vec![NetworkInterface {
        ipv4_address: Some(format!("{NETWORK_INTERFACE_BLOCK}.{host}")),
        mac_address: None,
        ipv6_address: None,
    }]
}

/// True when `value` is one of the time zone forms
/// `ActivateGatewayInput$GatewayTimezone` documents: "GMT", "GMT-hr:mm" or
/// "GMT+hr:mm".
pub fn is_gateway_timezone(value: &str) -> bool {
    let Some(offset) = value.strip_prefix("GMT") else {
        return false;
    };
    if offset.is_empty() {
        return true;
    }
    let Some(signed) = offset.strip_prefix(['+', '-']) else {
        return false;
    };
    let Some((hours, minutes)) = signed.split_once(':') else {
        return false;
    };
    let hours_ok = matches!(hours.len(), 1 | 2)
        && hours.bytes().all(|b| b.is_ascii_digit())
        && hours.parse::<u8>().is_ok_and(|hours| hours <= 23);
    let minutes_ok = minutes.len() == 2
        && minutes.bytes().all(|b| b.is_ascii_digit())
        && minutes.parse::<u8>().is_ok_and(|minutes| minutes <= 59);
    hours_ok && minutes_ok
}

/// The NFS defaults a share records, filling each member the request left out
/// with the default its own documentation states.
pub fn nfs_defaults_with_defaults(requested: Option<NFSFileShareDefaults>) -> NFSFileShareDefaults {
    let requested = requested.unwrap_or_default();
    NFSFileShareDefaults {
        file_mode: Some(
            requested
                .file_mode
                .unwrap_or_else(|| DEFAULT_FILE_MODE.to_string()),
        ),
        directory_mode: Some(
            requested
                .directory_mode
                .unwrap_or_else(|| DEFAULT_DIRECTORY_MODE.to_string()),
        ),
        group_id: Some(requested.group_id.unwrap_or(NFSNOBODY_ID)),
        owner_id: Some(requested.owner_id.unwrap_or(NFSNOBODY_ID)),
    }
}

/// An `InvalidGatewayRequestException` carrying `message` and the modeled
/// `ErrorCode` `error_code`.
///
/// Every operation in this service declares exactly two errors —
/// `InternalServerError` (500) and `InvalidGatewayRequestException` (400) — so
/// this is the one refusal a handler has for a bad request, and `error.errorCode`
/// is where it says WHICH refusal it is.
pub fn invalid_gateway_request(
    error_code: &str,
    message: impl Into<String>,
) -> InvalidGatewayRequestException {
    InvalidGatewayRequestException {
        message: Some(message.into()),
        error: Some(StorageGatewayError {
            error_code: Some(error_code.to_string()),
            error_details: None,
        }),
    }
}

/// The refusal for a gateway ARN that names no activated gateway.
pub fn gateway_not_found(gateway_arn: &str) -> InvalidGatewayRequestException {
    invalid_gateway_request(
        ERROR_CODE_GATEWAY_NOT_FOUND,
        format!("The gateway {gateway_arn} was not found."),
    )
}

/// The refusal for a file share ARN that names no file share.
pub fn file_share_not_found(file_share_arn: &str) -> InvalidGatewayRequestException {
    invalid_gateway_request(
        ERROR_CODE_INVALID_PARAMETERS,
        format!("The file share {file_share_arn} was not found."),
    )
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsStoragegatewayStoragegateway20130630State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsStoragegatewayStoragegateway20130630State>)
}
