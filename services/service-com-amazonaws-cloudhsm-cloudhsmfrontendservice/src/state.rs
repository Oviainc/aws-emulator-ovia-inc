// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cloudhsm-cloudhsmfrontendservice service.
//!
//! The HSM lifecycle lives here: `CreateHsm` writes an [`Hsm`] record, `DescribeHsm`
//! renders that stored record back, `ListHsms` pages the stored ARNs, `ModifyHsm`
//! overwrites only the members the request supplied, and `DeleteHsm` removes the
//! record. Nothing is echoed from a request: every member a Describe reports comes
//! out of the record the Create persisted or out of the identifiers this module
//! allocated for it.
//!
//! The helpers the five handlers share (identifier allocation, subnet placement, the
//! epoch-seconds wire form, the dotted-quad check the model's `IpAddress` pattern
//! cannot express, and the list cursor codec) live here too, so the handlers agree on
//! one implementation of each instead of carrying five copies that can drift apart.
//!
//! EVERY store here is bounded. `hsms` is capped at [`MAX_HSMS`] and a create past the
//! cap is refused; `hsm_client_tokens` only ever gains an entry alongside an `hsms`
//! record and [`remove_hsm`] prunes both together, so it is bounded by the same cap.
//! Nothing else is keyed by caller-supplied data: a subnet's VPC and zone are DERIVED
//! by [`subnet_placement`] rather than remembered, precisely so a caller naming fresh
//! subnet ids cannot grow the process.
//!
//! [`remove_hsm`]: ComAmazonawsCloudhsmCloudhsmfrontendserviceState::remove_hsm
//!
//! The HAPG, Luna-client and tagging families are still 501 scaffolds; their stores
//! (`hapgs`, `luna_clients`, `tags`) belong on
//! [`ComAmazonawsCloudhsmCloudhsmfrontendserviceState`] beside [`hsms`] when they land.
//!
//! [`hsms`]: ComAmazonawsCloudhsmCloudhsmfrontendserviceState::hsms

use std::collections::{BTreeMap, BTreeSet};
use std::net::Ipv4Addr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ops::types::DescribeHsmOutput;

/// The ARN namespace and sigv4 name this service stamps on every ARN it mints
/// (`smithy_query(query="service_info")`: `arnNamespace: cloudhsm`).
pub const ARN_NAMESPACE: &str = "cloudhsm";

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `HsmStatus` a freshly created HSM carries. `CreateHsm`'s own model doc: "It can
/// take up to 20 minutes to create and provision an HSM. You can monitor the status
/// of the HSM with the DescribeHsm operation. The HSM is ready to be initialized when
/// the status changes to RUNNING."
pub const HSM_STATUS_PENDING: &str = "PENDING";

/// `HsmStatus` a provisioned HSM carries — the terminal status of the create, per the
/// same `CreateHsm` doc sentence quoted on [`HSM_STATUS_PENDING`].
pub const HSM_STATUS_RUNNING: &str = "RUNNING";

/// The `Status` `DeleteHsm` reports for the HSM it just removed. `DeleteHsm`'s output
/// `Status` is a free-form `String` (`^[\w :+=./\\-]*$`), not the `HsmStatus` enum, and
/// the record is gone by the time the call answers.
pub const HSM_STATUS_DELETED: &str = "DELETED";

/// How long after `CreateHsm` the clock owes the `PENDING` -> `RUNNING` transition.
///
/// Real provisioning takes up to 20 minutes (the `CreateHsm` doc), which no test or
/// client can wait for, so the emulator compresses it. It is deliberately NOT zero: a
/// caller that describes the HSM straight after creating it must still observe
/// `PENDING`, the way it would against AWS.
pub const PROVISIONING_DELAY: Duration = Duration::from_millis(500);

/// How many ARNs one `ListHsms` page carries.
///
/// The model declares `NextToken` on `ListHsms` but publishes no page size, so this is
/// the emulator's own: what matters on the wire is that a caller following `NextToken`
/// sees every stored ARN exactly once, in a stable order, and that no token is emitted
/// when the stored set fits in one page.
pub const LIST_PAGE_SIZE: usize = 50;

/// The most HSM records this emulator keeps.
///
/// This is NOT an AWS quota — CloudHSM Classic's HSM limit is per-account and
/// adjustable, and nothing here reproduces it. It is the emulator's memory ceiling:
/// every successful `CreateHsm` inserts a record that lives until a caller deletes it,
/// so without a ceiling a caller could grow the process until the host runs out of
/// memory. A create that would cross it is refused with the modelled
/// `CloudHsmServiceException`.
pub const MAX_HSMS: usize = 1_000;

/// The appliance vendor CloudHSM Classic reports. The classic service provisions
/// SafeNet Luna SA appliances, which is what its `VendorName`/`HsmType` describe.
pub const HSM_VENDOR_NAME: &str = "SafeNet Inc.";

/// The appliance model CloudHSM Classic reports in `HsmType`.
pub const HSM_TYPE: &str = "safenet-luna-sa-7000";

/// The appliance software version CloudHSM Classic reports in `SoftwareVersion`. The
/// model's `ClientVersion` enum (`smithy_query(query="shape",shape="ClientVersion")`:
/// `5.1`, `5.3`) is the 5.x client family this matches.
pub const HSM_SOFTWARE_VERSION: &str = "5.3.13";

/// The availability-zone letters a region's zones are named with: an AWS AZ name is
/// `<region><letter>`, and the largest regions publish six zones (`a`..`f`).
const AVAILABILITY_ZONE_SUFFIXES: &[u8] = b"abcdef";

/// Where a subnet sits: the VPC that contains it and the zone it is in.
///
/// In AWS these come from the subnet itself, which this emulator does not model, so
/// [`subnet_placement`] DERIVES them from the subnet id instead. Deriving rather than
/// allocating is deliberate: the mapping is a pure function of the subnet id, so
/// `VpcId`/`AvailabilityZone` are stable across reads and shared by every HSM in one
/// subnet without a per-subnet table that a caller naming fresh subnet ids could grow
/// without bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubnetPlacement {
    pub vpc_id: String,
    pub availability_zone: String,
}

/// One stored HSM: every member `CreateHsm` was given, plus the identifiers the service
/// allocated for it, so `DescribeHsm` reports what `CreateHsm` persisted instead of
/// recomputing or echoing anything.
#[derive(Debug, Clone)]
pub struct Hsm {
    pub hsm_arn: String,
    /// The `hsm-<8 hex>` id the ARN ends in.
    pub hsm_id: String,
    /// `HsmSerialNumber`: digits, at most 16 (`^\d{1,16}$`).
    pub serial_number: String,
    /// One of `HSM_STATUS_VALUES`; [`HSM_STATUS_PENDING`] until [`provisioned_at`].
    ///
    /// [`provisioned_at`]: Hsm::provisioned_at
    pub status: String,
    pub status_details: Option<String>,
    pub availability_zone: String,
    /// `eni-<8 hex>`, allocated at create.
    pub eni_id: String,
    pub eni_ip: Option<String>,
    pub subnet_id: String,
    pub vpc_id: String,
    pub iam_role_arn: String,
    pub external_id: Option<String>,
    pub syslog_ip: Option<String>,
    /// The `SshKey` the create was given; `DescribeHsm` reports it as `SshPublicKey`.
    pub ssh_public_key: String,
    /// Epoch seconds, as the `Timestamp` members' `^\d*$` wire form.
    pub ssh_key_last_updated: String,
    pub subscription_type: String,
    pub subscription_start_date: String,
    pub subscription_end_date: Option<String>,
    pub vendor_name: String,
    pub hsm_type: String,
    pub software_version: String,
    pub server_cert_uri: Option<String>,
    pub server_cert_last_updated: Option<String>,
    /// `PartitionArn`s. An HSM `CreateHsm` provisions is uninitialized, so it carries
    /// no partitions until a client initializes it.
    pub partitions: Vec<String>,
    /// The instant the clock owes this HSM the `PENDING` -> `RUNNING` transition:
    /// creation + [`PROVISIONING_DELAY`]. Stored, never slept on.
    pub provisioned_at: SystemTime,
}

impl Hsm {
    /// Materialise the transition the clock has already made due.
    ///
    /// A provisioning HSM becomes `RUNNING` [`PROVISIONING_DELAY`] after its create,
    /// whether or not anybody calls: this only writes down a change that was already
    /// owed at `now`, so a reporting operation may call it.
    pub fn materialize_due(&mut self, now: SystemTime) {
        if self.status == HSM_STATUS_PENDING && now >= self.provisioned_at {
            self.status = HSM_STATUS_RUNNING.to_string();
        }
    }

    /// The stored record as the `DescribeHsm` response.
    pub fn to_describe_output(&self) -> DescribeHsmOutput {
        DescribeHsmOutput {
            hsm_arn: Some(self.hsm_arn.clone()),
            status: Some(self.status.clone()),
            status_details: self.status_details.clone(),
            availability_zone: Some(self.availability_zone.clone()),
            eni_id: Some(self.eni_id.clone()),
            eni_ip: self.eni_ip.clone(),
            subscription_type: Some(self.subscription_type.clone()),
            subscription_start_date: Some(self.subscription_start_date.clone()),
            subscription_end_date: self.subscription_end_date.clone(),
            vpc_id: Some(self.vpc_id.clone()),
            subnet_id: Some(self.subnet_id.clone()),
            iam_role_arn: Some(self.iam_role_arn.clone()),
            serial_number: Some(self.serial_number.clone()),
            vendor_name: Some(self.vendor_name.clone()),
            hsm_type: Some(self.hsm_type.clone()),
            software_version: Some(self.software_version.clone()),
            ssh_public_key: Some(self.ssh_public_key.clone()),
            ssh_key_last_updated: Some(self.ssh_key_last_updated.clone()),
            server_cert_uri: self.server_cert_uri.clone(),
            server_cert_last_updated: self.server_cert_last_updated.clone(),
            partitions: Some(self.partitions.clone()),
        }
    }
}

/// Per-service state for com-amazonaws-cloudhsm-cloudhsmfrontendservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCloudhsmCloudhsmfrontendserviceState {
    /// Every stored HSM, keyed by its ARN — the resource's identity in this API. A
    /// `BTreeMap`, so `ListHsms` and therefore its pagination walk a stable order.
    pub hsms: BTreeMap<String, Hsm>,
    /// `ClientToken` -> `HsmArn`, so a retried `CreateHsm` carrying the token of a
    /// create this service already served answers with the ARN it minted then instead
    /// of provisioning a second HSM.
    pub hsm_client_tokens: BTreeMap<String, String>,
    /// Monotonic counter the HSM id, ENI id and serial number are derived from, so each
    /// is unique within this emulator without being reused after a delete.
    pub next_hsm_seq: u64,
}

impl ComAmazonawsCloudhsmCloudhsmfrontendserviceState {
    /// The next HSM sequence number, consumed. Identifiers derived from it
    /// ([`hsm_id_for`], [`eni_id_for`], [`serial_number_for`], [`default_eni_ip_for`])
    /// are therefore distinct for every HSM this emulator provisions.
    pub fn next_hsm_sequence(&mut self) -> u64 {
        self.next_hsm_seq = self.next_hsm_seq.saturating_add(1);
        self.next_hsm_seq
    }

    /// The ARN stored under `client_token` by an earlier create, if that HSM still
    /// exists. A token whose HSM has been deleted is not an answer: the create it
    /// stood for has been undone, so a retry provisions a new HSM.
    pub fn hsm_for_client_token(&self, client_token: &str) -> Option<String> {
        let arn = self.hsm_client_tokens.get(client_token)?;
        self.hsms.contains_key(arn).then(|| arn.clone())
    }

    /// The ARN of the HSM whose `SerialNumber` is `serial_number`.
    pub fn hsm_arn_by_serial_number(&self, serial_number: &str) -> Option<String> {
        self.hsms
            .values()
            .find(|hsm| hsm.serial_number == serial_number)
            .map(|hsm| hsm.hsm_arn.clone())
    }

    /// Remove the HSM `hsm_arn` names, together with every idempotency token that
    /// stood for its create.
    pub fn remove_hsm(&mut self, hsm_arn: &str) -> Option<Hsm> {
        let removed = self.hsms.remove(hsm_arn)?;
        self.hsm_client_tokens.retain(|_, arn| arn != hsm_arn);
        Some(removed)
    }
}

/// The region to stamp into the ARNs a request mints: the SigV4 credential scope's
/// region, else [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:cloudhsm:<region>:<account>:hsm-<8 hex>`, the `HsmArn` form the model
/// publishes (`^arn:aws(-iso)?:cloudhsm:[a-zA-Z0-9\-]*:[0-9]{12}:hsm-[0-9a-f]{8}$`).
pub fn hsm_arn(region: &str, hsm_id: &str) -> String {
    ::emulator_server::emulator_arn(ARN_NAMESPACE, region, hsm_id)
}

/// Where the subnet `subnet_id` sits, in `region`.
///
/// In AWS the VPC and the zone are properties of the subnet, which this emulator does
/// not model, so both are DERIVED from the subnet id with a stable digest: the mapping
/// is a pure function, so every HSM in one subnet reports the same `VpcId` and
/// `AvailabilityZone`, a different subnet resolves to a different VPC, and no
/// per-subnet table exists for a caller naming fresh subnet ids to grow.
///
/// `sha2` is the maintained crate for the digest; only its first five bytes are used —
/// four for the `vpc-<8 hex>` id and one to pick the zone letter.
pub fn subnet_placement(subnet_id: &str, region: &str) -> SubnetPlacement {
    use ::sha2::Digest;

    let digest = ::sha2::Sha256::digest(format!("cloudhsm/subnet-placement/{subnet_id}"));
    let suffix =
        AVAILABILITY_ZONE_SUFFIXES[digest[4] as usize % AVAILABILITY_ZONE_SUFFIXES.len()] as char;
    SubnetPlacement {
        vpc_id: format!("vpc-{}", ::hex::encode(&digest[..4])),
        availability_zone: format!("{region}{suffix}"),
    }
}

/// The `hsm-<8 hex>` id of the `seq`-th HSM this emulator provisions.
pub fn hsm_id_for(seq: u64) -> String {
    format!("hsm-{:08x}", seq & 0xffff_ffff)
}

/// The `eni-<8 hex>` id of the ENI allocated to the `seq`-th HSM.
pub fn eni_id_for(seq: u64) -> String {
    format!("eni-{:08x}", seq & 0xffff_ffff)
}

/// The `HsmSerialNumber` (`^\d{1,16}$`) of the `seq`-th HSM: twelve digits, so it is
/// serial-shaped rather than a bare counter, and distinct per HSM.
pub fn serial_number_for(seq: u64) -> String {
    format!("{:012}", 100_000_000_000_u64.saturating_add(seq))
}

/// The `EniIp` allocated to the `seq`-th HSM when the create does not name one.
///
/// `CreateHsm`'s doc says the service picks one "from the CIDR range of the subnet".
/// This emulator does not model subnet CIDRs, so it allocates from the private
/// `10.0.0.0/16` block instead; the address is a function of the HSM's sequence number,
/// so it is stable across reads and unique per HSM.
pub fn default_eni_ip_for(seq: u64) -> String {
    let host = seq % 254 + 1;
    let third = (seq / 254) % 256;
    Ipv4Addr::new(10, 0, third as u8, host as u8).to_string()
}

/// An instant as the `^\d*$` epoch-seconds string the model's `Timestamp` members
/// carry.
pub fn epoch_seconds(at: SystemTime) -> String {
    at.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

/// Whether `value` is a usable IPv4 address.
///
/// The model's `IpAddress` pattern (`^\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}$`) bounds only
/// the DIGIT COUNT of each octet, so `999.1.2.3` satisfies it and reaches the handler.
/// `std::net::Ipv4Addr`'s parser is the octet-range check the pattern cannot express.
pub fn is_ipv4_address(value: &str) -> bool {
    value.parse::<Ipv4Addr>().is_ok()
}

/// One page of `ListHsms`: the ARNs to report, and the opaque token that resumes
/// after them.
#[derive(Debug)]
pub struct HsmPage {
    pub hsm_arns: Vec<String>,
    pub next_token: Option<String>,
}

/// Encode a pagination cursor (the ARN the next page starts at).
///
/// Hex rather than the ARN in the clear: a `NextToken` is opaque, so a caller has no
/// structure to read out of it and nothing to hand-craft one from. Hex is also a subset
/// of `PaginationToken`'s `^[a-zA-Z0-9+/]*$`. `hex` is the maintained crate for the
/// codec.
fn encode_cursor(hsm_arn: &str) -> String {
    ::hex::encode(hsm_arn)
}

/// The inverse of [`encode_cursor`]; `None` for a token this service did not mint.
fn decode_cursor(token: &str) -> Option<String> {
    String::from_utf8(::hex::decode(token).ok()?).ok()
}

/// Cut the stored ARNs into the page that resumes at `next_token`, at most
/// [`LIST_PAGE_SIZE`] per page. `None` means the token is not one this service minted.
pub fn paginate(all: &BTreeSet<String>, next_token: Option<&str>) -> Option<HsmPage> {
    let cursor = match next_token {
        // An empty `NextToken` satisfies `PaginationToken`'s pattern and decodes to an
        // empty cursor; treat it as "from the start", not as a forged token.
        Some(token) if !token.is_empty() => Some(decode_cursor(token)?),
        _ => None,
    };
    let mut remaining = all
        .iter()
        .filter(|arn| {
            cursor
                .as_deref()
                .is_none_or(|resume_at| arn.as_str() >= resume_at)
        })
        .cloned();
    let hsm_arns: Vec<String> = remaining.by_ref().take(LIST_PAGE_SIZE).collect();
    Some(HsmPage {
        next_token: remaining.next().map(|arn| encode_cursor(&arn)),
        hsm_arns,
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCloudhsmCloudhsmfrontendserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCloudhsmCloudhsmfrontendserviceState>)
}
