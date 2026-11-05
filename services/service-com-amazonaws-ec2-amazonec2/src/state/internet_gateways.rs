// SPDX-License-Identifier: CC0-1.0

//! The internet gateway store: the id form, the bounds, the byte charge, the one writer and
//! the error channel `CreateInternetGateway` and `DescribeInternetGateways` share.
//!
//! The gateways themselves live on the service's one state struct
//! ([`ComAmazonawsEc2Amazonec2State::internet_gateways`]) as the MODELLED
//! [`InternetGateway`] shape, for the reason [`ComAmazonawsEc2Amazonec2State::network_acls`]
//! holds a network ACL that way: the create and the describe answer the same shape, so
//! storing it is what stops them reporting one gateway differently. Only the primitives over
//! that store live here.
//!
//! Re-exported from [`crate::state`], so a handler reaches these as `crate::state::<item>`
//! like every other primitive there.

use super::{
    id_not_found, tag_list_bytes, ComAmazonawsEc2Amazonec2State, MAX_STORED_RECORDS_PER_STORE,
    STORED_RECORD_OVERHEAD_BYTES,
};
use crate::ops::types::{InternetGateway, Tag};
use crate::scaffolded::Scaffolded;

/// The id prefix an internet gateway is minted under.
///
/// EC2 names the resource's ids `igw-<hex>` — the spelling `AttachInternetGateway` and the
/// `internet-gateway-id` filter both use — which is the same `<prefix>-<hex>` form every
/// other EC2 resource id takes, so [`ComAmazonawsEc2Amazonec2State::next_id`] mints them.
pub const INTERNET_GATEWAY_ID_PREFIX: &str = "igw";

/// The one `ResourceType` a `CreateInternetGateway` tag specification may name: "The tags to
/// assign to the internet gateway" (`CreateInternetGatewayRequest$TagSpecifications`),
/// spelled as the modelled `ResourceType` enum spells it
/// (`smithy_query(query="shape",shape="ResourceType")`).
pub const INTERNET_GATEWAY_TAGGABLE_RESOURCE: &str = "internet-gateway";

/// The largest page `DescribeInternetGateways` serves, and the page it serves when the
/// request names no size.
///
/// The top of the modelled `MaxResults` range, `DescribeInternetGatewaysMaxResults`'s
/// `range 5..1000` (`smithy_query(query="operation",operation="DescribeInternetGateways")`),
/// so a request that names no page size is answered with the largest page the operation can
/// be ASKED for and the `NextToken` path behaves the same either way.
pub const MAX_INTERNET_GATEWAYS_PAGE_SIZE: usize = 1_000;

/// EC2's code for an internet gateway id that names no gateway of the account (EC2 API
/// "Error codes": `InvalidInternetGatewayID.NotFound`).
///
/// `InternetGatewayId` publishes no pattern
/// (`smithy_query(query="shape",shape="InternetGatewayId")`), so there is no spelling to
/// call an id malformed by: one code covers an id of any shape, the same reading
/// [`super::INVALID_TRAFFIC_MIRROR_FILTER_ID_NOT_FOUND`] has, because an id this service
/// never minted names no gateway of the account whether or not it reads like one.
pub const INVALID_INTERNET_GATEWAY_ID_NOT_FOUND: &str = "InvalidInternetGatewayID.NotFound";

/// EC2's quota code for one internet gateway too many (EC2 API "Error codes":
/// `InternetGatewayLimitExceeded`).
pub const INTERNET_GATEWAY_LIMIT_EXCEEDED: &str = "InternetGatewayLimitExceeded";

/// The caller-chosen bytes ONE stored internet gateway holds for the process's lifetime: its
/// tags, beside the per-record overhead every other store of this module charges.
///
/// Everything else on the record — its id, the owning account and the attachments
/// `AttachInternetGateway` records — is derived by this crate out of ids it minted, so it is
/// bounded already and not counted here. The ONE definition of that charge:
/// [`ComAmazonawsEc2Amazonec2State::insert_internet_gateway`] charges exactly it, so a writer
/// cannot store bytes the store's budget never saw.
pub fn internet_gateway_payload_bytes(tags: &[Tag]) -> usize {
    STORED_RECORD_OVERHEAD_BYTES.saturating_add(tag_list_bytes(tags))
}

/// Refuse a request that named an internet gateway id this account holds no gateway for:
/// HTTP 400 [`INVALID_INTERNET_GATEWAY_ID_NOT_FOUND`], in the
/// `The <resource> ID '<id>' does not exist` wording EC2 answers an unresolvable id with,
/// built from the id the REQUEST carried.
pub fn invalid_internet_gateway_id_not_found<E>(internet_gateway_id: &str) -> Scaffolded<E> {
    id_not_found(
        INVALID_INTERNET_GATEWAY_ID_NOT_FOUND,
        "internetGateway ID",
        internet_gateway_id,
    )
}

/// Refuse a create the store has no room for: HTTP 400 [`INTERNET_GATEWAY_LIMIT_EXCEEDED`],
/// the answer for a [`ComAmazonawsEc2Amazonec2State::insert_internet_gateway`] that stored
/// nothing.
pub fn internet_gateway_limit_exceeded<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(INTERNET_GATEWAY_LIMIT_EXCEEDED, 400, message)
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another internet gateway fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of [`Self::has_vpc_capacity`]: a
    /// create asks first so it can refuse with [`internet_gateway_limit_exceeded`] instead
    /// of overshooting.
    pub fn has_internet_gateway_capacity(&self) -> bool {
        self.internet_gateways.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Store an internet gateway carrying `tags` and answer the `InternetGatewayId` it was
    /// given.
    ///
    /// Everything the service itself owns is derived here rather than taken from the caller
    /// — the id and the owning account — so two writers cannot report the same gateway
    /// differently, exactly as [`Self::insert_network_acl`] does for a network ACL. A
    /// gateway starts life attached to no VPC: "After creating the internet gateway, you
    /// attach it to a VPC using AttachInternetGateway" (`CreateInternetGateway`), so its
    /// attachment set is EMPTY rather than absent, which is how a gateway's
    /// `attachmentSet` comes back on the wire.
    ///
    /// The caller-chosen bytes the record retains ([`internet_gateway_payload_bytes`]) are
    /// charged against the store's byte budget here, so no path can store them uncharged.
    ///
    /// `None`, with NOTHING stored, when the store is at [`MAX_STORED_RECORDS_PER_STORE`]
    /// gateways or the bytes this one would hold would cross [`super::MAX_STORED_BYTES`]:
    /// both bounds are enforced here, so a writer that forgets to ask cannot grow the store
    /// without bound.
    pub fn insert_internet_gateway(&mut self, tags: Vec<Tag>) -> Option<String> {
        let charged_bytes = internet_gateway_payload_bytes(&tags);
        if !self.has_internet_gateway_capacity() || self.would_exceed_stored_bytes(charged_bytes) {
            return None;
        }
        let internet_gateway_id = self.next_id(INTERNET_GATEWAY_ID_PREFIX);
        let internet_gateway = InternetGateway {
            attachments: Some(Vec::new()),
            internet_gateway_id: Some(internet_gateway_id.clone()),
            // "The ID of the Amazon Web Services account that owns the internet gateway"
            // (`InternetGateway$OwnerId`): this emulator serves one account.
            owner_id: Some(::engine::emulator_account_id().to_string()),
            tags: Some(tags),
        };
        self.add_stored_bytes(charged_bytes);
        self.internet_gateways
            .insert(internet_gateway_id.clone(), internet_gateway);
        Some(internet_gateway_id)
    }

    /// Remove the internet gateway `internet_gateway_id` names and answer it as it was last
    /// reported, or `None` when the store holds no such gateway.
    ///
    /// The answer is the record the removal took OUT of the store, on the contract
    /// [`Self::remove_network_acl`] has: what a delete reports is what the readers reported,
    /// never a second copy built beside it. The bytes the record holds go back to the store's
    /// budget on exactly the terms [`Self::insert_internet_gateway`] took them on, read off
    /// the tags the record holds NOW — `CreateTags` and `DeleteTags` reach a gateway's tags
    /// too, so the charge the store is carrying for this record is the one its current tags
    /// define, not the one its create happened to take.
    pub fn remove_internet_gateway(&mut self, internet_gateway_id: &str) -> Option<InternetGateway> {
        let internet_gateway = self.internet_gateways.remove(internet_gateway_id)?;
        self.release_stored_bytes(internet_gateway_payload_bytes(
            internet_gateway.tags.as_deref().unwrap_or_default(),
        ));
        Some(internet_gateway)
    }
}
