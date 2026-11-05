// SPDX-License-Identifier: CC0-1.0

//! The network ACL store: the stored ACL, the default one every VPC comes with, its entries
//! and the subnet associations it holds.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there. What a CALLER-created ACL adds to this store lives beside it in
//! [`crate::state::network_acls`].

use super::*;

/// The id prefix of a network ACL: `NetworkAcl$NetworkAclId` values are `acl-<hex>`, which
/// is also how `DeleteNetworkAcl` and the `network-acl-id` filter spell one.
pub const NETWORK_ACL_ID_PREFIX: &str = "acl";

/// The id prefix of an association between a network ACL and a subnet
/// (`NetworkAclAssociation$NetworkAclAssociationId`), which EC2 spells `aclassoc-<hex>`.
pub const NETWORK_ACL_ASSOCIATION_ID_PREFIX: &str = "aclassoc";

/// Allow the traffic a network ACL entry matches: the `RuleAction` member `allow`
/// (`smithy_query(query="shape",shape="RuleAction")`).
pub const RULE_ACTION_ALLOW: &str = "allow";

/// Deny the traffic a network ACL entry matches: the other `RuleAction` member.
pub const RULE_ACTION_DENY: &str = "deny";

/// The rule number of the allow-everything entry of a VPC's default network ACL, which
/// "allows all inbound and outbound traffic" through rule 100 in each direction (Amazon VPC
/// User Guide, "Default network ACL").
pub const DEFAULT_NETWORK_ACL_ALLOW_RULE_NUMBER: i32 = 100;

/// The rule number of the catch-all deny entry every network ACL ends with — the rule the
/// console shows as `*` and the API reports as 32767, the highest rule number an entry can
/// take (`CreateNetworkAclEntryRequest$RuleNumber`: "Constraints: Positive integer from 1 to
/// 32766" for a caller-created entry, so the catch-all sits one past what a caller can
/// write).
pub const DEFAULT_NETWORK_ACL_DENY_RULE_NUMBER: i32 = 32767;

/// The entries a VPC's default network ACL is created with: in each direction, an entry
/// allowing every protocol from every IPv4 address at
/// [`DEFAULT_NETWORK_ACL_ALLOW_RULE_NUMBER`], then the catch-all deny at
/// [`DEFAULT_NETWORK_ACL_DENY_RULE_NUMBER`] — "the default network ACL allows all inbound
/// and outbound traffic" and every ACL ends with a rule that denies what no other rule
/// matched (Amazon VPC User Guide, "Default network ACL").
///
/// The ONE definition of those entries, so a default ACL cannot be written two ways.
pub fn default_network_acl_entries() -> Vec<NetworkAclEntry> {
    let mut entries: Vec<NetworkAclEntry> = [false, true]
        .into_iter()
        .flat_map(|egress| {
            [
                (DEFAULT_NETWORK_ACL_ALLOW_RULE_NUMBER, RULE_ACTION_ALLOW),
                (DEFAULT_NETWORK_ACL_DENY_RULE_NUMBER, RULE_ACTION_DENY),
            ]
            .map(|(rule_number, rule_action)| {
                reported_network_acl_entry(NetworkAclEntry {
                    cidr_block: Some(ALL_IPV4_ADDRESSES.to_string()),
                    egress: Some(egress),
                    // ICMP type and code describe an entry whose protocol is ICMP, and a
                    // port range one whose protocol is TCP or UDP; this entry's protocol is
                    // every protocol, so it carries neither of them filled in.
                    icmp_type_code: None,
                    ipv6_cidr_block: None,
                    port_range: None,
                    protocol: Some(ALL_IP_PROTOCOLS.to_string()),
                    rule_action: Some(rule_action.to_string()),
                    rule_number: Some(rule_number),
                })
            })
        })
        .collect();
    // Stored in the order an ACL's entries are reported in, through the one definition of
    // that order.
    entries.sort_by_key(network_acl_entry_report_order);
    entries
}

/// The caller-chosen bytes one stored network ACL holds: its tags and the text of its
/// entries, on top of the fixed [`STORED_RECORD_OVERHEAD_BYTES`] every stored record costs.
/// The ids, the owning account and the `IsDefault` flag are derived by this module, so they
/// are fixed-size.
///
/// The ONE definition of that charge, the way [`flow_log_payload_bytes`] is for a flow log:
/// [`ComAmazonawsEc2Amazonec2State::insert_network_acl`] charges exactly it and
/// [`ComAmazonawsEc2Amazonec2State::remove_network_acl`] gives back exactly it. One entry's
/// own share of it is [`network_acl_entry_payload_bytes`], which is what an entry added to a
/// stored ACL is charged.
pub fn network_acl_payload_bytes(entries: &[NetworkAclEntry], tags: &[Tag]) -> usize {
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(tag_list_bytes(tags))
        .saturating_add(
            entries
                .iter()
                .map(network_acl_entry_payload_bytes)
                .sum::<usize>(),
        )
}

/// What ONE network ACL is stored with, beyond everything the service itself derives
/// ([`ComAmazonawsEc2Amazonec2State::insert_network_acl`] mints the id and names the owning
/// account, and an ACL starts life associated with no subnet).
#[derive(Debug, Clone)]
pub struct NetworkAclSpec {
    /// "The ID of the VPC for the network ACL" (`NetworkAcl$VpcId`).
    pub vpc_id: String,
    /// "Indicates whether this is the default network ACL for the VPC"
    /// (`NetworkAcl$IsDefault`): set only for the one ACL a VPC comes with.
    pub is_default: bool,
    /// "The entries (rules) in the network ACL" (`NetworkAcl$Entries`) — for a default ACL,
    /// [`default_network_acl_entries`].
    pub entries: Vec<NetworkAclEntry>,
    /// "Any tags assigned to the network ACL" (`NetworkAcl$Tags`).
    pub tags: Vec<Tag>,
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another network ACL fits inside the store's ceiling
    /// ([`MAX_STORED_RECORDS_PER_STORE`]), on the contract of [`Self::has_vpc_capacity`]: a
    /// writer asks first so it can refuse instead of overshooting.
    pub fn has_network_acl_capacity(&self) -> bool {
        self.network_acls.len() < MAX_STORED_RECORDS_PER_STORE
    }

    /// Store a network ACL and answer the `NetworkAclId` it was given.
    ///
    /// Everything the service itself owns is derived here rather than taken from the caller
    /// — the id and the owning account — so two writers cannot report the same ACL
    /// differently, exactly as [`Self::insert_subnet`] does for a subnet. An ACL starts
    /// life associated with no subnet: [`Self::associate_subnet_with_network_acl`] is what
    /// records an association.
    ///
    /// The caller-chosen bytes of a caller-created ACL ([`network_acl_payload_bytes`]) are
    /// charged against the store's byte budget here, so no writer can store them uncharged.
    /// A DEFAULT ACL is not charged, on the same terms as the default security group a VPC
    /// comes with ([`Self::insert_security_group`]): its entries and its absence of tags are
    /// what the SERVICE chose, which the budget ([`MAX_STORED_BYTES`], a bound on
    /// caller-supplied data) does not count — and [`Self::remove_network_acl`] gives back on
    /// exactly the same terms.
    ///
    /// `None`, with NOTHING stored, when the store is at [`MAX_STORED_RECORDS_PER_STORE`]
    /// ACLs or the bytes this one would hold would cross [`MAX_STORED_BYTES`]: both bounds
    /// are enforced here, the way [`Self::insert_coip_pool`] enforces its own, so a writer
    /// that forgets to ask cannot grow the store without bound.
    pub fn insert_network_acl(&mut self, spec: NetworkAclSpec) -> Option<String> {
        let charged_bytes = if spec.is_default {
            0
        } else {
            network_acl_payload_bytes(&spec.entries, &spec.tags)
        };
        if !self.has_network_acl_capacity() || self.would_exceed_stored_bytes(charged_bytes) {
            return None;
        }
        let network_acl_id = self.next_id(NETWORK_ACL_ID_PREFIX);
        let network_acl = NetworkAcl {
            // Nothing is associated with an ACL the instant it is stored; a subnet's
            // association is recorded as that subnet is stored.
            associations: Some(Vec::new()),
            entries: Some(spec.entries),
            is_default: Some(spec.is_default),
            network_acl_id: Some(network_acl_id.clone()),
            tags: Some(spec.tags),
            vpc_id: Some(spec.vpc_id),
            // "The ID of the Amazon Web Services account that owns the network ACL"
            // (`NetworkAcl$OwnerId`): this emulator serves one account.
            owner_id: Some(::engine::emulator_account_id().to_string()),
        };
        self.add_stored_bytes(charged_bytes);
        self.network_acls
            .insert(network_acl_id.clone(), network_acl);
        Some(network_acl_id)
    }

    /// Give the VPC `vpc_id` the default network ACL it comes with, and answer that ACL's
    /// id.
    ///
    /// "Your VPC automatically comes with a modifiable default network ACL" that "allows all
    /// inbound and outbound traffic" (Amazon VPC User Guide, "Default network ACL"), so the
    /// ACL is `IsDefault` and carries [`default_network_acl_entries`]. Called from
    /// [`Self::insert_vpc`], which is what makes it true of every VPC this store holds.
    ///
    /// Every subnet the VPC ALREADY holds is associated with it, so seeding a VPC whose
    /// subnets were stored first still leaves each of those subnets associated with an ACL —
    /// the same association [`Self::insert_subnet`] records for a subnet stored afterwards.
    ///
    /// `None`, with nothing stored, when [`Self::insert_network_acl`] refuses: the store is
    /// at its ACL ceiling.
    pub fn insert_default_network_acl(&mut self, vpc_id: &str) -> Option<String> {
        let network_acl_id = self.insert_network_acl(NetworkAclSpec {
            vpc_id: vpc_id.to_string(),
            is_default: true,
            entries: default_network_acl_entries(),
            tags: Vec::new(),
        })?;
        let subnet_ids: Vec<String> = self
            .subnets
            .iter()
            .filter(|(_, subnet)| subnet.vpc_id.as_deref() == Some(vpc_id))
            .map(|(subnet_id, _)| subnet_id.clone())
            .collect();
        for subnet_id in subnet_ids {
            self.associate_subnet_with_network_acl(&subnet_id, &network_acl_id);
        }
        Some(network_acl_id)
    }

    /// The id of the default network ACL of the VPC `vpc_id`, if the store holds one.
    ///
    /// A READ: a VPC missing one is not given one here. A VPC has at most one default ACL —
    /// [`Self::insert_vpc`] writes exactly one and nothing else sets `IsDefault` — so this
    /// answers the one ACL a subnet of that VPC is associated with by default.
    pub fn default_network_acl_id_of_vpc(&self, vpc_id: &str) -> Option<String> {
        self.network_acls
            .values()
            .find(|acl| acl.vpc_id.as_deref() == Some(vpc_id) && acl.is_default == Some(true))
            .and_then(|acl| acl.network_acl_id.clone())
    }

    /// Record that the subnet `subnet_id` is associated with the network ACL
    /// `network_acl_id`, and answer the `NetworkAclAssociationId` it was given.
    ///
    /// The one writer of an ACL's `Associations`, so every reader of an association reads
    /// the same record. The association id is minted out of the store's own counter, and the
    /// record holds no caller-chosen bytes — both sides of the pair are ids this service
    /// minted — so there is nothing to charge against the byte budget.
    ///
    /// `None`, with nothing written, for an ACL the store does not hold.
    pub fn associate_subnet_with_network_acl(
        &mut self,
        subnet_id: &str,
        network_acl_id: &str,
    ) -> Option<String> {
        if !self.network_acls.contains_key(network_acl_id) {
            return None;
        }
        let association_id = self.next_id(NETWORK_ACL_ASSOCIATION_ID_PREFIX);
        let network_acl = self.network_acls.get_mut(network_acl_id)?;
        network_acl
            .associations
            .get_or_insert_with(Vec::new)
            .push(NetworkAclAssociation {
                network_acl_association_id: Some(association_id.clone()),
                network_acl_id: Some(network_acl_id.to_string()),
                subnet_id: Some(subnet_id.to_string()),
            });
        Some(association_id)
    }

    /// Associate the subnet `subnet_id` with the default network ACL of the VPC `vpc_id` —
    /// what a subnet no caller has associated with an ACL of its own is associated with.
    ///
    /// `None`, with nothing written, when that VPC's default ACL is not in the store.
    pub fn associate_subnet_with_default_network_acl(
        &mut self,
        subnet_id: &str,
        vpc_id: &str,
    ) -> Option<String> {
        let network_acl_id = self.default_network_acl_id_of_vpc(vpc_id)?;
        self.associate_subnet_with_network_acl(subnet_id, &network_acl_id)
    }

    /// The association `association_id` names, whichever stored ACL holds it, as it is
    /// reported — or `None` when no stored ACL holds one with that id.
    ///
    /// A READ, and the ONE resolver of a `NetworkAclAssociationId`, so every operation that
    /// names an association by id resolves the same record
    /// [`Self::associate_subnet_with_network_acl`] wrote.
    pub fn network_acl_association(&self, association_id: &str) -> Option<NetworkAclAssociation> {
        self.network_acls
            .values()
            .flat_map(|network_acl| network_acl.associations.as_deref().unwrap_or_default())
            .find(|association| {
                association.network_acl_association_id.as_deref() == Some(association_id)
            })
            .cloned()
    }

    /// Put the network ACL `network_acl_id` in the place of the one the association
    /// `association_id` names, and answer the `NetworkAclAssociationId` of the association
    /// that replaces it.
    ///
    /// The subnet of the displaced association keeps being associated with exactly one ACL
    /// — the old record goes and a new one takes its place on the named ACL — so the
    /// invariant [`Self::insert_subnet`] establishes still holds afterwards. The new record
    /// is written through [`Self::associate_subnet_with_network_acl`], the one writer of an
    /// ACL's `Associations`, so its id comes from the store's own counter like every other.
    ///
    /// `None`, with nothing written, when no stored ACL holds that association or the store
    /// holds no ACL with that id.
    pub fn replace_network_acl_association(
        &mut self,
        association_id: &str,
        network_acl_id: &str,
    ) -> Option<String> {
        let subnet_id = self
            .network_acl_association(association_id)?
            .subnet_id
            .clone()?;
        if !self.network_acls.contains_key(network_acl_id) {
            return None;
        }
        for network_acl in self.network_acls.values_mut() {
            if let Some(associations) = network_acl.associations.as_mut() {
                associations.retain(|association| {
                    association.network_acl_association_id.as_deref() != Some(association_id)
                });
            }
        }
        self.associate_subnet_with_network_acl(&subnet_id, network_acl_id)
    }

    /// Remove the network ACL `network_acl_id` names and answer it as it was last reported,
    /// or `None` when the store holds no such ACL.
    ///
    /// The answer is the record the removal took OUT of the store, so what a delete reports
    /// is what the readers reported — never a second copy built beside it. The bytes the
    /// record held go back to the store's budget on exactly the terms
    /// [`Self::insert_network_acl`] took them on, including for a DEFAULT ACL, which was
    /// never charged: releasing for one would walk `stored_bytes` below what the store
    /// actually holds and silently weaken [`MAX_STORED_BYTES`].
    pub fn remove_network_acl(&mut self, network_acl_id: &str) -> Option<NetworkAcl> {
        let network_acl = self.network_acls.remove(network_acl_id)?;
        // The `CreateNetworkAcl` `ClientToken` that minted this ACL, if one did, goes with
        // it: a token left naming an ACL the store no longer holds would replay into a
        // dangling id.
        self.forget_network_acl_client_tokens_of(network_acl_id);
        if network_acl.is_default != Some(true) {
            self.release_stored_bytes(network_acl_payload_bytes(
                network_acl.entries.as_deref().unwrap_or_default(),
                network_acl.tags.as_deref().unwrap_or_default(),
            ));
        }
        Some(network_acl)
    }

    /// Drop every association that names the subnet `subnet_id`, whichever ACL holds it —
    /// what a subnet's removal owes, since an association is the pair and cannot outlive
    /// either side of it. The records hold no caller-supplied bytes, so there is nothing to
    /// give back to the budget.
    pub fn disassociate_subnet_from_network_acls(&mut self, subnet_id: &str) {
        for network_acl in self.network_acls.values_mut() {
            if let Some(associations) = network_acl.associations.as_mut() {
                associations.retain(|association| {
                    association.subnet_id.as_deref() != Some(subnet_id)
                });
            }
        }
    }
}

/// EC2's error code for a request that named a network ACL id no ACL of the account has
/// (EC2 API "Error codes": `InvalidNetworkAclID.NotFound`).
pub const INVALID_NETWORK_ACL_ID_NOT_FOUND: &str = "InvalidNetworkAclID.NotFound";

/// `InvalidNetworkAclID.Malformed` (HTTP 400): the request named something that is not a
/// network ACL id at all, so there is nothing to look up — the `Invalid<Kind>ID.Malformed`
/// spelling of this resource's own [`INVALID_NETWORK_ACL_ID_NOT_FOUND`], answered BEFORE the
/// lookup by [`require_well_formed_id`].
pub const INVALID_NETWORK_ACL_ID_MALFORMED: &str = "InvalidNetworkAclID.Malformed";

/// Refuse a request that named a network ACL id this account holds no ACL for, the way EC2
/// does: HTTP 400 [`INVALID_NETWORK_ACL_ID_NOT_FOUND`], worded by the one place this module
/// words a missing resource id ([`id_not_found`]) and naming the id the REQUEST named.
pub fn invalid_network_acl_id_not_found<E>(network_acl_id: &str) -> Scaffolded<E> {
    id_not_found(
        INVALID_NETWORK_ACL_ID_NOT_FOUND,
        "networkAcl ID",
        network_acl_id,
    )
}
