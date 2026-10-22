// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-s3outposts-s3outposts service.
//!
//! This API has two resources. An **Outpost** carries the S3 on Outposts
//! capacity and is reported by `ListOutpostsWithS3`; an **endpoint** is what a
//! VPC reaches that capacity through, minted by `CreateEndpoint`, reported by
//! `ListEndpoints` / `ListSharedEndpoints` and removed by `DeleteEndpoint`.
//! [`OutpostRecord`] and [`EndpointRecord`] below are the one copy of each that
//! every handler of this service reads and writes.
//!
//! ## Why the store starts with NO Outposts
//!
//! An Outpost is hardware Amazon installs at a customer site, and this service
//! models NO operation that creates one — `ListOutpostsWithS3` only reports the
//! Outposts "that have S3 provisioned ... to which the calling Amazon Web
//! Services account has access". A resource no operation of the API creates
//! exists only where something outside the API provisioned it, so an account
//! that was never provisioned has none and `ListOutpostsWithS3` reports an empty
//! inventory. The store therefore starts EMPTY: inventing Outposts would report
//! capacity, and a cross-account owner, for hardware nobody installed.
//!
//! The one thing outside the API that can say an Outpost was provisioned is the
//! operator running this emulator, through [`PROVISIONED_OUTPOSTS_ENV`]: each
//! entry it names enters the inventory as an [`OutpostRecord`], and
//! `CreateEndpoint` then has a parent Outpost to attach an endpoint to. Unset —
//! the default — means no capacity, which is what a new AWS account has.
//!
//! ## Why an ARN is projected and not stored
//!
//! Every ARN this service reports (`Outpost$OutpostArn`, `Outpost$S3OutpostArn`,
//! `Endpoint$EndpointArn`, `CreateEndpointResult$EndpointArn`) is built from the
//! record's ids and owner account plus the Region the request was signed for, so
//! all the ARNs in one answer name one Region — the Region the caller is talking
//! to. The store holds one set of resources rather than one per Region, so
//! storing a Region on the record would let an endpoint's ARN name a different
//! Region from its own parent Outpost's ARN.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{Endpoint, NetworkInterface, Outpost, ResourceNotFoundException};

/// `EndpointStatus`'s transient value: "Pending".
pub const ENDPOINT_STATUS_PENDING: &str = "Pending";

/// `EndpointStatus`'s ready value: "Available".
pub const ENDPOINT_STATUS_AVAILABLE: &str = "Available";

/// `EndpointAccessType`'s "Private", which `CreateEndpointRequest$AccessType`
/// documents as "the default access type value".
pub const ACCESS_TYPE_PRIVATE: &str = "Private";

/// `EndpointAccessType`'s "CustomerOwnedIp".
pub const ACCESS_TYPE_CUSTOMER_OWNED_IP: &str = "CustomerOwnedIp";

/// The largest page the three list operations serve, and the page they serve
/// when the request names no `MaxResults`.
///
/// `MaxResults` is modelled `@range(min: 0, max: 100)` with `@default(0)`, and
/// `ListOutpostsWithS3Request$MaxResults` documents "The limit is 100".
pub const PAGE_LIMIT: i32 = 100;

/// How long a new endpoint stays `Pending` before the clock owes it
/// `Available`.
///
/// `CreateEndpoint` documents that "it can take up to 5 minutes for this action
/// to finish"; the emulator chooses a span instead, and it is deliberately NOT
/// zero, so a caller listing endpoints straight after the create still sees the
/// transient status. The due instant is stored on the record and materialised by
/// whichever later call reports the endpoint — nothing sleeps.
pub const ENDPOINT_TRANSITION: Duration = Duration::from_millis(500);

/// The Region an ARN names when the request carries no SigV4 credential scope to
/// read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The environment variable an operator uses to tell this emulated service which
/// Outposts have S3 on Outposts capacity provisioned, since the API models no
/// operation that provisions one: comma-separated
/// `<OutpostId>:<CapacityInBytes>[:<OwnerAccountId>]` entries, read once when
/// this service's store is created
/// (`S3_OUTPOSTS_PROVISIONED_OUTPOSTS="op-0a1b2c3d4e5f60718:52776558133248"`).
///
/// Unset means no capacity at all, which is what an account that never had S3 on
/// Outposts provisioned has: `ListOutpostsWithS3` reports an empty inventory and
/// every reference to an Outpost is the modelled 404. An entry whose owner is
/// another account is one shared with the caller through Resource Access
/// Manager, which is what `Outpost$OwnerId` ("useful for comparing owned versus
/// shared outposts") distinguishes and what `ListSharedEndpoints` lists over.
pub const PROVISIONED_OUTPOSTS_ENV: &str = "S3_OUTPOSTS_PROVISIONED_OUTPOSTS";

/// The Outposts `setting` declares: comma-separated
/// `<OutpostId>:<CapacityInBytes>[:<OwnerAccountId>]` entries, surrounding space
/// trimmed. The owner defaults to the account this emulator serves, so an
/// operator naming only an id and a capacity gets an Outpost the caller owns.
///
/// An entry that is not of that form, or whose values are not ones this API
/// could report (see [`OutpostRecord::provisioned`]), is SKIPPED rather than
/// guessed at: a typo provisions nothing instead of provisioning something
/// unintended.
pub fn parse_provisioned_outposts(setting: &str) -> Vec<OutpostRecord> {
    setting
        .split(',')
        .filter(|entry| !entry.trim().is_empty())
        .filter_map(|entry| {
            let mut fields = entry.split(':').map(str::trim);
            let outpost_id = fields.next()?;
            let capacity_in_bytes: i64 = fields.next()?.parse().ok()?;
            let owner_id = match fields.next() {
                Some(owner) => owner.to_string(),
                None => caller_account(),
            };
            // A fourth field means the entry is not the form documented above,
            // so it says something this parser does not understand.
            if fields.next().is_some() {
                return None;
            }
            OutpostRecord::provisioned(outpost_id, &owner_id, capacity_in_bytes)
        })
        .collect()
}

/// How many `/26` CIDRs fit in the `/16` the emulator reserves for endpoint
/// commitments — and so the largest number of endpoints the store holds at once.
const ENDPOINT_CIDR_BLOCK_COUNT: u32 = 1024;

/// A status change the clock owes an endpoint: the instant it falls due, and the
/// status the endpoint carries from then on.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    pub due: SystemTime,
    pub status: String,
}

/// One Outpost with S3 on Outposts capacity, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct OutpostRecord {
    /// `OutpostId`'s `^(op-[a-f0-9]{17}|\d{12}|ec2)$`.
    pub outpost_id: String,
    /// The 12-digit account that owns the Outpost — the caller's own account for
    /// an owned Outpost, another account for one shared with the caller.
    pub owner_id: String,
    pub capacity_in_bytes: i64,
}

impl OutpostRecord {
    /// The record for an Outpost provisioned with S3 capacity, or `None` when the
    /// values are not ones this API could ever report: `OutpostId`'s
    /// `^(op-[a-f0-9]{17}|\d{12}|ec2)$` for the id, `AwsAccountId`'s `^\d{12}$`
    /// for the owner, and a capacity above zero — an Outpost with no S3 capacity
    /// is not one `ListOutpostsWithS3` reports at all.
    pub fn provisioned(outpost_id: &str, owner_id: &str, capacity_in_bytes: i64) -> Option<Self> {
        (is_outpost_id(outpost_id) && is_account_outpost_id(owner_id) && capacity_in_bytes > 0)
            .then(|| Self {
                outpost_id: outpost_id.to_string(),
                owner_id: owner_id.to_string(),
                capacity_in_bytes,
            })
    }

    /// `arn:aws:outposts:<region>:<owner>:outpost/<id>`, as
    /// `OutpostArn`'s pattern spells it.
    pub fn outpost_arn(&self, region: &str) -> String {
        format!(
            "arn:aws:outposts:{region}:{}:outpost/{}",
            self.owner_id, self.outpost_id
        )
    }

    /// `arn:aws:s3-outposts:<region>:<owner>:outpost/<id>/s3`, as
    /// `S3OutpostArn`'s pattern spells it: the name RAM shares.
    pub fn s3_outpost_arn(&self, region: &str) -> String {
        format!(
            "arn:aws:s3-outposts:{region}:{}:outpost/{}/s3",
            self.owner_id, self.outpost_id
        )
    }

    /// The `Outpost` projection `ListOutpostsWithS3` returns.
    pub fn to_outpost(&self, region: &str) -> Outpost {
        Outpost {
            outpost_arn: Some(self.outpost_arn(region)),
            s3_outpost_arn: Some(self.s3_outpost_arn(region)),
            outpost_id: Some(self.outpost_id.clone()),
            owner_id: Some(self.owner_id.clone()),
            capacity_in_bytes: self.capacity_in_bytes,
        }
    }
}

/// One S3 on Outposts endpoint, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct EndpointRecord {
    /// `EndpointId`'s `^[a-zA-Z0-9]{19}$`, minted by
    /// [`ComAmazonawsS3outpostsS3outpostsState::mint_endpoint_id`].
    pub endpoint_id: String,
    /// The Outpost this endpoint is on.
    pub outpost_id: String,
    /// "The VPC CIDR committed by this endpoint", allocated by
    /// [`ComAmazonawsS3outpostsS3outpostsState::allocate_cidr_block`].
    pub cidr_block: String,
    /// One of `EndpointStatus`'s values.
    pub status: String,
    /// The status change the clock owes this endpoint, if any.
    pub pending: Option<PendingTransition>,
    pub creation_time: SystemTime,
    /// The `NetworkInterfaceId`s of the interfaces provisioned for the endpoint.
    pub network_interfaces: Vec<String>,
    pub vpc_id: String,
    pub subnet_id: String,
    pub security_group_id: String,
    /// One of `EndpointAccessType`'s values.
    pub access_type: String,
    pub customer_owned_ipv4_pool: Option<String>,
    /// The 12-digit account that created, and so owns, this endpoint.
    pub owner_account: String,
}

impl EndpointRecord {
    /// `arn:aws:s3-outposts:<region>:<owner>:outpost/<outpost>/endpoint/<id>`,
    /// as `EndpointArn`'s pattern spells it.
    pub fn endpoint_arn(&self, region: &str) -> String {
        format!(
            "arn:aws:s3-outposts:{region}:{}:outpost/{}/endpoint/{}",
            self.owner_account, self.outpost_id, self.endpoint_id
        )
    }

    /// Materialise the status change the clock owes this endpoint, if `now` is
    /// past its due instant.
    ///
    /// Only `status` moves: `CreationTime` is "the time the endpoint was
    /// created" and never changes.
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

    /// The `Endpoint` structure every list operation reports.
    pub fn to_endpoint(&self, region: &str) -> Endpoint {
        Endpoint {
            endpoint_arn: Some(self.endpoint_arn(region)),
            outposts_id: Some(self.outpost_id.clone()),
            cidr_block: Some(self.cidr_block.clone()),
            status: Some(self.status.clone()),
            creation_time: Some(::aws_smithy_types::DateTime::from(self.creation_time)),
            network_interfaces: Some(
                self.network_interfaces
                    .iter()
                    .map(|id| NetworkInterface {
                        network_interface_id: Some(id.clone()),
                    })
                    .collect(),
            ),
            vpc_id: Some(self.vpc_id.clone()),
            subnet_id: Some(self.subnet_id.clone()),
            security_group_id: Some(self.security_group_id.clone()),
            access_type: Some(self.access_type.clone()),
            customer_owned_ipv4_pool: self.customer_owned_ipv4_pool.clone(),
            // `FailedReason` is "the failure reason, if any, for a create or
            // delete endpoint operation", so it belongs to an endpoint in
            // `Create_Failed` or `Delete_Failed`. No path in this emulator
            // reaches either status — a create either stores the endpoint or is
            // refused up front — so there is never a failure reason to report.
            failed_reason: None,
        }
    }
}

/// Per-service state for com-amazonaws-s3outposts-s3outposts. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsS3outpostsS3outpostsState {
    /// Every Outpost with S3 capacity, keyed by its id. The `BTreeMap` is what
    /// gives `ListOutpostsWithS3` the one stable total order its pagination
    /// cursor indexes into.
    ///
    /// BOUNDED: nothing a client can send adds an entry — the API models no
    /// operation that provisions an Outpost, so the only writer is
    /// [`Self::provision_outpost`], reached from [`PROVISIONED_OUTPOSTS_ENV`]
    /// once when the store is created.
    pub outposts: BTreeMap<String, OutpostRecord>,
    /// Every endpoint, keyed by `(OutpostId, EndpointId)` — the pair
    /// `DeleteEndpoint` names — which is also the stable order the endpoint
    /// listings page over.
    ///
    /// BOUNDED: an endpoint is only stored once [`Self::allocate_cidr_block`]
    /// has committed one of the `ENDPOINT_CIDR_BLOCK_COUNT` ranges the emulator
    /// reserves, so the map never holds more than that many records and a create
    /// past the bound is refused rather than stored.
    pub endpoints: BTreeMap<(String, String), EndpointRecord>,
    /// The VPC the emulator has associated with a subnet. `Endpoint$VpcId` is
    /// "the ID of the VPC used for the endpoint" and a subnet belongs to exactly
    /// one VPC; this emulator models no EC2 subnets, so it mints one VPC id the
    /// first time a subnet is used and reports that same id for every later
    /// endpoint in the same subnet.
    ///
    /// BOUNDED by [`Self::endpoints`]: an entry exists only for a subnet a
    /// stored endpoint sits in. [`Self::remove_endpoint`] drops the entry with
    /// the last endpoint in that subnet, so a create/delete sequence cannot grow
    /// this map — and a subnet that no endpoint is left in is simply one this
    /// emulator no longer knows a VPC for, which the next endpoint there mints
    /// afresh.
    pub subnet_vpcs: BTreeMap<String, String>,
    /// The counter endpoint ids are minted from.
    pub endpoint_id_seq: u64,
    /// The counter VPC ids are minted from.
    pub vpc_id_seq: u64,
    /// The counter network-interface ids are minted from.
    pub network_interface_id_seq: u64,
    /// The next `/26` index [`Self::allocate_cidr_block`] offers.
    pub cidr_block_seq: u32,
}

impl Default for ComAmazonawsS3outpostsS3outpostsState {
    /// An account with NO resources: no endpoints, and no Outposts beyond the
    /// ones [`PROVISIONED_OUTPOSTS_ENV`] says were provisioned (none, unless it
    /// is set).
    ///
    /// Nothing is invented here. The API models no operation that provisions an
    /// Outpost, so an account nobody provisioned capacity for reports none, and
    /// every answer that mentions an Outpost is computed from the records the
    /// operator declared.
    fn default() -> Self {
        let mut state = Self {
            outposts: BTreeMap::new(),
            endpoints: BTreeMap::new(),
            subnet_vpcs: BTreeMap::new(),
            endpoint_id_seq: 0,
            vpc_id_seq: 0,
            network_interface_id_seq: 0,
            cidr_block_seq: 0,
        };

        for record in ::std::env::var(PROVISIONED_OUTPOSTS_ENV)
            .map(|setting| parse_provisioned_outposts(&setting))
            .unwrap_or_default()
        {
            state.provision_outpost(record);
        }

        state
    }
}

impl ComAmazonawsS3outpostsS3outpostsState {
    /// Put `record` in the inventory `ListOutpostsWithS3` reports, replacing the
    /// record of an Outpost already there.
    ///
    /// This is the ONLY way an Outpost enters the store, because the API models
    /// no operation that provisions one: it is reached from
    /// [`PROVISIONED_OUTPOSTS_ENV`] at init, and from a test standing up an
    /// account that does have capacity.
    pub fn provision_outpost(&mut self, record: OutpostRecord) {
        self.outposts.insert(record.outpost_id.clone(), record);
    }

    /// Mint an unused endpoint id matching `EndpointId`'s `^[a-zA-Z0-9]{19}$`.
    ///
    /// A `u64` is at most 16 hex digits, so the width-19 padding always yields
    /// exactly 19 of them; the loop skips an id already taken, which is what
    /// makes a wrapped counter safe.
    pub fn mint_endpoint_id(&mut self) -> String {
        loop {
            self.endpoint_id_seq = self.endpoint_id_seq.wrapping_add(1);
            let id = format!("{:019x}", self.endpoint_id_seq);
            if !self.endpoints.keys().any(|(_, existing)| existing == &id) {
                return id;
            }
        }
    }

    /// The VPC id this emulator reports for `subnet_id`, minting one the first
    /// time that subnet is used.
    pub fn vpc_for_subnet(&mut self, subnet_id: &str) -> String {
        if let Some(vpc_id) = self.subnet_vpcs.get(subnet_id) {
            return vpc_id.clone();
        }
        self.vpc_id_seq = self.vpc_id_seq.wrapping_add(1);
        let vpc_id = format!("vpc-{:017x}", self.vpc_id_seq);
        self.subnet_vpcs
            .insert(subnet_id.to_string(), vpc_id.clone());
        vpc_id
    }

    /// Mint a network-interface id for an endpoint's interface.
    pub fn mint_network_interface_id(&mut self) -> String {
        self.network_interface_id_seq = self.network_interface_id_seq.wrapping_add(1);
        format!("eni-{:017x}", self.network_interface_id_seq)
    }

    /// Commit the next free `/26` to an endpoint, or `None` when every block in
    /// the reserved range is already committed.
    ///
    /// `Endpoint$CidrBlock` is "the VPC CIDR committed by this endpoint". This
    /// emulator models no EC2 subnet to carve that range out of, so it reserves
    /// the RFC 1918 block `10.0.0.0/16` for endpoint commitments and hands each
    /// endpoint a distinct, aligned `/26` out of it: no two live endpoints are
    /// ever committed the same range, and a deleted endpoint's range is offered
    /// again.
    pub fn allocate_cidr_block(&mut self) -> Option<String> {
        for _ in 0..ENDPOINT_CIDR_BLOCK_COUNT {
            let index = self.cidr_block_seq % ENDPOINT_CIDR_BLOCK_COUNT;
            self.cidr_block_seq = self.cidr_block_seq.wrapping_add(1);
            let candidate = format!("10.0.{}.{}/26", index / 4, (index % 4) * 64);
            if !self
                .endpoints
                .values()
                .any(|endpoint| endpoint.cidr_block == candidate)
            {
                return Some(candidate);
            }
        }
        None
    }

    /// Materialise every status change the clock owes, for a caller that is
    /// about to report on all of them.
    pub fn settle_all(&mut self, now: SystemTime) {
        for endpoint in self.endpoints.values_mut() {
            endpoint.settle(now);
        }
    }

    /// Remove the endpoint `key` names and release what its record held: the
    /// `/26` it committed is offered to a later endpoint again, and the VPC
    /// mapping for its subnet is dropped once no endpoint is left in that
    /// subnet.
    ///
    /// Releasing the mapping is what keeps [`Self::subnet_vpcs`] bounded by
    /// [`Self::endpoints`]: no sequence of creates and deletes can leave an
    /// entry behind for a subnet nothing is stored in.
    pub fn remove_endpoint(&mut self, key: &(String, String)) -> Option<EndpointRecord> {
        let removed = self.endpoints.remove(key)?;
        if !self
            .endpoints
            .values()
            .any(|endpoint| endpoint.subnet_id == removed.subnet_id)
        {
            self.subnet_vpcs.remove(&removed.subnet_id);
        }
        Some(removed)
    }
}

/// The account this emulator serves every request for: the one canonical
/// emulator account.
pub fn caller_account() -> String {
    ::engine::emulator_account_id().to_string()
}

/// The Region this request was signed for — what every ARN in the answer names.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The page a list operation serves for `max_results`.
///
/// `MaxResults` is modelled `@range(min: 0, max: 100)` with `@default(0)`, so a
/// request that names no page size reaches a handler as `0`: that is "no
/// preference", and the page served is then the limit the model publishes.
/// Anything outside the range was refused by the boundary validator before the
/// handler ran, which the debug assertion surfaces rather than silently
/// repairing.
pub fn page_size(max_results: i32) -> usize {
    debug_assert!(
        (0..=PAGE_LIMIT).contains(&max_results),
        "MaxResults {max_results} should have been refused at the boundary"
    );
    let requested = if max_results <= 0 {
        PAGE_LIMIT
    } else {
        max_results
    };
    usize::try_from(requested.clamp(1, PAGE_LIMIT)).expect("a page size in 1..=100 fits in usize")
}

/// True when `value` satisfies `OutpostId`'s `^(op-[a-f0-9]{17}|\d{12}|ec2)$`.
///
/// The boundary validator already refuses a modelled `OutpostId` member, so this
/// is for a value the model does not constrain: a `NextToken`, which this
/// service's pagination carries an Outpost id in.
pub fn is_outpost_id(value: &str) -> bool {
    if value == "ec2" {
        return true;
    }
    if let Some(hex) = value.strip_prefix("op-") {
        return hex.len() == 17 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    }
    is_account_outpost_id(value)
}

/// True when `value` satisfies `EndpointId`'s `^[a-zA-Z0-9]{19}$`.
pub fn is_endpoint_id(value: &str) -> bool {
    value.len() == 19 && value.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The cursor an endpoint listing returns to resume at `key`: the
/// `(OutpostId, EndpointId)` pair the next page starts at.
///
/// Both ids and the separator are inside `NextToken`'s own
/// `^[A-Za-z0-9\+\:\/\=\?\#-_]+$`, so the cursor a response carries is a value
/// the next request may legally send back.
pub fn encode_endpoint_cursor(key: &(String, String)) -> String {
    format!("{}/{}", key.0, key.1)
}

/// The `(OutpostId, EndpointId)` pair an endpoint listing's cursor names, or
/// `None` when the token is not one this service issued.
pub fn decode_endpoint_cursor(token: &str) -> Option<(String, String)> {
    let (outpost_id, endpoint_id) = token.split_once('/')?;
    (is_outpost_id(outpost_id) && is_endpoint_id(endpoint_id))
        .then(|| (outpost_id.to_string(), endpoint_id.to_string()))
}

/// The one `ResourceNotFoundException$Message` this service answers an
/// unresolvable reference under an `OutpostId` with, whichever branch of
/// `OutpostId`'s `^(op-[a-f0-9]{17}|\d{12}|ec2)$` the request used.
///
/// The text is a service constant, never built from the request: it names no id
/// and does not say which resource under the Outpost failed to resolve, so one
/// text covers every operation and every reference. Recorded behaviour — the
/// model carries no message text for `ResourceNotFoundException` — and it
/// reports what actually went wrong: an Outpost that does not exist or has no S3
/// capacity provisioned on it. An authorization failure is this service's
/// separate `AccessDeniedException` (403), not a 404 message.
pub const OUTPOST_NOT_FOUND_MESSAGE: &str =
    "The specified outpost does not exist, or does not have S3 capacity provisioned on it.";

/// True when `value` is the 12-digit (`\d{12}`) branch of `OutpostId`'s
/// `^(op-[a-f0-9]{17}|\d{12}|ec2)$` — the account-id form.
pub fn is_account_outpost_id(value: &str) -> bool {
    value.len() == 12 && value.bytes().all(|b| b.is_ascii_digit())
}

/// The `ResourceNotFoundException` payload for a reference under an `OutpostId`
/// that resolves to nothing this caller has access to — the Outpost itself, or
/// an endpoint on it.
///
/// It takes no argument on purpose: the message is one fixed constant, not a
/// function of the request's ids, of the form of the `OutpostId` or of which
/// resource failed to resolve. Every reference below an Outpost is resolved by
/// first resolving that Outpost, so the Outpost-level text is the only one a
/// caller ever sees.
pub fn not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(OUTPOST_NOT_FOUND_MESSAGE.to_string()),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsS3outpostsS3outpostsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsS3outpostsS3outpostsState>)
}
