// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-wafregional-awswaf-regional-20161128 service.
//!
//! AWS WAF Classic's `IPSet` and `SqlInjectionMatchSet` records and the change tokens
//! every mutating call consumes live here, so `GetChangeToken`,
//! `GetChangeTokenStatus`, `CreateIPSet`, `UpdateIPSet`, `GetIPSet`, `ListIPSets`,
//! `DeleteIPSet`, `CreateSqlInjectionMatchSet` and `GetSqlInjectionMatchSet` all read
//! and write ONE copy of each resource. The helpers below (id minting, the
//! change-token protocol, CIDR parsing, the `IPSet`/`IPSetSummary` and
//! `SqlInjectionMatchSet` projections) are shared by those handlers rather than
//! duplicated per operation.

use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;
use std::time::{Duration, Instant};

use crate::ops::types::{
    IPSet, IPSetDescriptor, IPSetSummary, SqlInjectionMatchSet, SqlInjectionMatchTuple,
};

/// `ChangeTokenStatus` wire values, from the model's `CHANGE_TOKEN_STATUS_VALUES`
/// (`INSYNC`, `PENDING`, `PROVISIONED`).
///
/// A token starts `PROVISIONED` when `GetChangeToken` mints it. `GetChangeToken`'s own
/// documentation states the rest: "When you use a change token in a create, update, or
/// delete request, the status of the change token changes to `PENDING`, which indicates
/// that AWS WAF is propagating the change to all AWS WAF servers."
pub const CHANGE_TOKEN_PROVISIONED: &str = "PROVISIONED";
/// A token a mutating call has consumed, whose change is still propagating.
pub const CHANGE_TOKEN_PENDING: &str = "PENDING";
/// A token whose change has finished propagating.
pub const CHANGE_TOKEN_INSYNC: &str = "INSYNC";

/// `IPSetDescriptorType` wire values, from the model's
/// `I_P_SET_DESCRIPTOR_TYPE_VALUES`.
pub const IP_SET_TYPE_IPV4: &str = "IPV4";
/// The IPv6 `IPSetDescriptorType`.
pub const IP_SET_TYPE_IPV6: &str = "IPV6";

/// `ChangeAction` wire values, from the model's `CHANGE_ACTION_VALUES`.
pub const CHANGE_ACTION_INSERT: &str = "INSERT";
/// The `DELETE` `ChangeAction`.
pub const CHANGE_ACTION_DELETE: &str = "DELETE";

/// How long a consumed change token stays `PENDING` before the clock owes it
/// `INSYNC`.
///
/// The model documents the `PENDING` phase ("AWS WAF is propagating the change to all
/// AWS WAF servers") but not how long it lasts, so this span is CHOSEN, not cited: it
/// is long enough that a caller polling `GetChangeTokenStatus` straight after a
/// mutating call still observes `PENDING`, and short enough that the propagation is
/// over by the time that caller polls again. Nothing ever sleeps for it — the due
/// instant is stored and compared on read.
pub const CHANGE_TOKEN_PROPAGATION: Duration = Duration::from_millis(400);

/// The IPv6 prefix lengths AWS WAF accepts.
///
/// Source: `UpdateIPSet`'s own documentation — "AWS WAF supports IPv6 address ranges:
/// /24, /32, /48, /56, /64, and /128."
pub const IPV6_PREFIX_LENGTHS: &[u8] = &[24, 32, 48, 56, 64, 128];

/// The most `IPSetDescriptor`s one `UpdateIPSet` request may carry.
///
/// Source: `UpdateIPSet`'s own documentation — "You can insert a maximum of 1000
/// addresses in a single request."
pub const MAX_UPDATES_PER_REQUEST: usize = 1_000;

/// The most `IPSet`s this emulator stores for the account before `CreateIPSet`
/// answers its modelled `WAFLimitsExceededException`.
///
/// The model states no quota, so this bound is the one AWS WAF Classic publishes for
/// an account (50 IPSets); with [`MAX_IP_SET_DESCRIPTORS`] bounding each one's
/// contents, it keeps the IPSet state a caller can grow bounded.
pub const MAX_IP_SETS: usize = 50;

/// The most `IPSetDescriptor`s one `IPSet` may hold.
///
/// The model states no quota, so this bound is the one AWS WAF Classic publishes for
/// an IPSet (10,000 IP address ranges). Together with [`MAX_IP_SETS`] it bounds the
/// total descriptor state a caller can grow, not just the number of IPSets.
pub const MAX_IP_SET_DESCRIPTORS: usize = 10_000;

/// The most `SqlInjectionMatchSet`s this emulator stores for the account before
/// `CreateSqlInjectionMatchSet` answers its modelled `WAFLimitsExceededException`.
///
/// The model states no quota, so this bound is the one AWS WAF Classic publishes for an
/// account (100 conditions of each type, a `SqlInjectionMatchSet` being one condition),
/// which keeps the match-set state a caller can grow bounded.
pub const MAX_SQL_INJECTION_MATCH_SETS: usize = 100;

/// The most change-token records kept before the oldest settled ones are dropped.
///
/// A change token is a short-lived record on the real service too — a status a client
/// polls for and then forgets — so keeping every token a long-running emulator ever
/// minted would grow without end. Chosen far above the number of tokens any one
/// client has in flight.
pub const MAX_CHANGE_TOKENS: usize = 1_000;

/// The largest page `ListIPSets` returns, and the page size a `Limit` of 0 (the
/// model's declared default) asks for.
///
/// Source: `PaginationLimit`'s modelled `@range` of 0..100.
pub const MAX_PAGE_SIZE: usize = 100;

/// A descriptor's identity: the parsed IP address and the prefix length.
///
/// `UpdateIPSet` documents that one IPv6 address has several spellings
/// ("1111:0000:0000:0000:0000:0000:0000:0111/128", "1111:0:0:0:0:0:0:0111/128",
/// "1111::0111/128", "1111::111/128" are listed as formats of the same address), so
/// two descriptors are the same descriptor when their addresses and prefix lengths
/// are equal — not when their text is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Cidr {
    pub addr: IpAddr,
    pub prefix_len: u8,
}

/// The CIDR `value` denotes for descriptor type `kind`, or `None` when it is not an
/// address of that family with a prefix length AWS WAF supports.
///
/// The address itself is parsed by `std::net::IpAddr`, so every spelling std accepts
/// is accepted here and normalises to the same [`Cidr`]. The supported prefix lengths
/// come from `UpdateIPSet`'s documentation: "AWS WAF supports IPv4 address ranges: /8
/// and any range between /16 through /32. AWS WAF supports IPv6 address ranges: /24,
/// /32, /48, /56, /64, and /128."
pub fn parse_cidr(kind: &str, value: &str) -> Option<Cidr> {
    let (addr, prefix_len) = value.split_once('/')?;
    let addr: IpAddr = addr.parse().ok()?;
    let prefix_len: u8 = prefix_len.parse().ok()?;
    let supported = match (kind, addr) {
        (IP_SET_TYPE_IPV4, IpAddr::V4(_)) => prefix_len == 8 || (16..=32).contains(&prefix_len),
        (IP_SET_TYPE_IPV6, IpAddr::V6(_)) => IPV6_PREFIX_LENGTHS.contains(&prefix_len),
        // A family that does not match the address (an IPv4 address declared IPV6, or
        // the reverse) is not a descriptor AWS WAF can hold.
        _ => false,
    };
    supported.then_some(Cidr { addr, prefix_len })
}

/// One stored `IPSetDescriptor`.
#[derive(Debug, Clone)]
pub struct IPSetDescriptorEntry {
    /// `IPSetDescriptorType` as it goes on the wire: [`IP_SET_TYPE_IPV4`] or
    /// [`IP_SET_TYPE_IPV6`].
    pub kind: String,
    /// The CIDR as the caller wrote it, so `GetIPSet` reads back what was stored
    /// rather than a re-rendering of it.
    pub value: String,
    /// The descriptor's identity — see [`Cidr`]. Two inserts of one address under
    /// different spellings are one descriptor.
    pub cidr: Cidr,
}

/// One stored `IPSet`.
#[derive(Debug, Clone)]
pub struct IPSetEntry {
    pub ip_set_id: String,
    pub name: String,
    /// The descriptors in insertion order, deduplicated by [`Cidr`].
    pub descriptors: Vec<IPSetDescriptorEntry>,
    /// Membership index over the [`Cidr`] keys of `descriptors`, kept in step with it
    /// by [`IPSetEntry::insert_descriptor`] and [`IPSetEntry::remove_descriptor`], so
    /// deduplicating one update costs a lookup rather than a scan of the whole set.
    cidrs: BTreeSet<Cidr>,
}

impl IPSetEntry {
    /// A new, empty IPSet record.
    pub fn new(ip_set_id: String, name: String) -> Self {
        Self {
            ip_set_id,
            name,
            descriptors: Vec::new(),
            cidrs: BTreeSet::new(),
        }
    }
    /// This record projected onto the modelled `IPSet` shape the Get and Create
    /// responses carry: the stored record's own fields, never request input.
    pub fn to_modelled(&self) -> IPSet {
        IPSet {
            ip_set_id: self.ip_set_id.clone(),
            name: Some(self.name.clone()),
            ip_set_descriptors: self
                .descriptors
                .iter()
                .map(|descriptor| IPSetDescriptor {
                    r#type: descriptor.kind.clone(),
                    value: descriptor.value.clone(),
                })
                .collect(),
        }
    }

    /// This record projected onto the `IPSetSummary` shape `ListIPSets` returns.
    pub fn summary(&self) -> IPSetSummary {
        IPSetSummary {
            ip_set_id: self.ip_set_id.clone(),
            name: self.name.clone(),
        }
    }

    /// Add a descriptor, unless an equal one (same address, same prefix length) is
    /// already stored. Answers whether the set changed.
    pub fn insert_descriptor(&mut self, kind: &str, value: &str, cidr: Cidr) -> bool {
        if !self.cidrs.insert(cidr) {
            return false;
        }
        self.descriptors.push(IPSetDescriptorEntry {
            kind: kind.to_string(),
            value: value.to_string(),
            cidr,
        });
        true
    }

    /// Remove the descriptor equal to `cidr`, if the set holds one. Answers whether
    /// the set changed.
    pub fn remove_descriptor(&mut self, cidr: &Cidr) -> bool {
        if !self.cidrs.remove(cidr) {
            return false;
        }
        self.descriptors.retain(|stored| &stored.cidr != cidr);
        true
    }
}

/// One stored `SqlInjectionMatchSet`.
#[derive(Debug, Clone)]
pub struct SqlInjectionMatchSetEntry {
    pub sql_injection_match_set_id: String,
    pub name: String,
    /// The `SqlInjectionMatchTuple`s in insertion order. A newly created set holds
    /// none: `CreateSqlInjectionMatchSet`'s documentation sends the caller to
    /// `UpdateSqlInjectionMatchSet` "to specify the parts of web requests in which you
    /// want to allow, block, or count malicious SQL code".
    pub tuples: Vec<SqlInjectionMatchTuple>,
}

impl SqlInjectionMatchSetEntry {
    /// A new, empty `SqlInjectionMatchSet` record.
    pub fn new(sql_injection_match_set_id: String, name: String) -> Self {
        Self {
            sql_injection_match_set_id,
            name,
            tuples: Vec::new(),
        }
    }

    /// This record projected onto the modelled `SqlInjectionMatchSet` shape the Create
    /// and Get responses carry: the stored record's own fields, never request input.
    pub fn to_modelled(&self) -> SqlInjectionMatchSet {
        SqlInjectionMatchSet {
            sql_injection_match_set_id: self.sql_injection_match_set_id.clone(),
            name: Some(self.name.clone()),
            sql_injection_match_tuples: self.tuples.clone(),
        }
    }
}

/// One minted change token.
#[derive(Debug, Clone)]
pub struct ChangeTokenRecord {
    /// A `ChangeTokenStatus` wire value: [`CHANGE_TOKEN_PROVISIONED`],
    /// [`CHANGE_TOKEN_PENDING`] or [`CHANGE_TOKEN_INSYNC`].
    pub status: String,
    /// When the clock owes this token `INSYNC`, set when a mutating call consumed it.
    pub insync_at: Option<Instant>,
    /// Mint order, so the oldest settled records are the ones dropped once
    /// [`MAX_CHANGE_TOKENS`] is reached.
    pub seq: u64,
}

/// Per-service state for com-amazonaws-wafregional-awswaf-regional-20161128. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsWafregionalAwswafRegional20161128State {
    /// Every stored `IPSet`, keyed by its `IPSetId`. A `BTreeMap` so `ListIPSets`
    /// walks them in one stable order and a `NextMarker` taken on one page resumes at
    /// the right place on the next.
    pub ip_sets: BTreeMap<String, IPSetEntry>,
    /// Every stored `SqlInjectionMatchSet`, keyed by its `SqlInjectionMatchSetId`. A
    /// `BTreeMap` for the same reason as `ip_sets`: one stable walk order for the
    /// paginated list.
    pub sql_injection_match_sets: BTreeMap<String, SqlInjectionMatchSetEntry>,
    /// Every change token this service minted and has not yet dropped, keyed by the
    /// token value, so `GetChangeTokenStatus` can report one and a mutating call can
    /// tell a fresh token from a used one.
    pub change_tokens: BTreeMap<String, ChangeTokenRecord>,
    /// The token `GetChangeToken` hands out until a mutating call consumes it.
    ///
    /// `GetChangeToken` documents this: "If your application submits a
    /// `GetChangeToken` request and then submits a second `GetChangeToken` request
    /// before submitting a create, update, or delete request, the second
    /// `GetChangeToken` request returns the same value as the first."
    pub outstanding_change_token: Option<String>,
    /// Monotonic counter behind every minted id and change token.
    next_seq: u64,
}

impl ComAmazonawsWafregionalAwswafRegional20161128State {
    /// Mint a fresh, unique 36-character UUID-shaped id, as AWS WAF Classic's
    /// `ResourceId` and `ChangeToken` values are.
    ///
    /// The monotonic sequence number occupies the last group, so every value this
    /// method returns differs from every other one it has returned.
    pub fn mint_id(&mut self) -> String {
        self.next_seq = self.next_seq.wrapping_add(1);
        let seq = self.next_seq;
        format!(
            "{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}",
            (seq >> 16) as u32,
            (seq >> 48) as u16,
            (seq >> 36) & 0xfff,
            (seq >> 24) & 0xfff,
            seq & 0xffff_ffff_ffff,
        )
    }

    /// The change token `GetChangeToken` answers with: the outstanding one when a
    /// minted token has not been consumed yet, else a freshly minted `PROVISIONED`
    /// one.
    pub fn change_token(&mut self) -> String {
        if let Some(outstanding) = &self.outstanding_change_token {
            if self.change_tokens.contains_key(outstanding) {
                return outstanding.clone();
            }
        }
        let token = self.mint_id();
        let seq = self.next_seq;
        self.change_tokens.insert(
            token.clone(),
            ChangeTokenRecord {
                status: CHANGE_TOKEN_PROVISIONED.to_string(),
                insync_at: None,
                seq,
            },
        );
        self.outstanding_change_token = Some(token.clone());
        self.prune_change_tokens();
        token
    }

    /// Consume `token` for a mutating call: a `PROVISIONED` token becomes `PENDING`
    /// and the clock owes it `INSYNC` [`CHANGE_TOKEN_PROPAGATION`] later.
    ///
    /// Answers `false` for a token this service never minted, and for one a mutating
    /// call already used — the caller answers both with the modelled
    /// `WAFStaleDataException`, since `GetChangeToken` requires that "each create,
    /// update, or delete request must use a unique change token".
    pub fn consume_change_token(&mut self, token: &str) -> bool {
        let Some(record) = self.change_tokens.get_mut(token) else {
            return false;
        };
        if record.status != CHANGE_TOKEN_PROVISIONED {
            return false;
        }
        record.status = CHANGE_TOKEN_PENDING.to_string();
        record.insync_at = Some(Instant::now() + CHANGE_TOKEN_PROPAGATION);
        if self.outstanding_change_token.as_deref() == Some(token) {
            self.outstanding_change_token = None;
        }
        true
    }

    /// The status of `token`, or `None` when this service holds no such token.
    ///
    /// This materialises the one transition the CLOCK owes — a consumed token whose
    /// propagation span has elapsed becomes `INSYNC` — which was due at that instant
    /// whether or not anybody asked. Nothing else about the token changes on a read.
    pub fn change_token_status(&mut self, token: &str) -> Option<String> {
        let now = Instant::now();
        let record = self.change_tokens.get_mut(token)?;
        if record.insync_at.is_some_and(|due| now >= due) {
            record.status = CHANGE_TOKEN_INSYNC.to_string();
            record.insync_at = None;
        }
        Some(record.status.clone())
    }

    /// Drop the oldest tokens no mutating call can still consume, until at most
    /// [`MAX_CHANGE_TOKENS`] records remain. The outstanding token is never dropped.
    fn prune_change_tokens(&mut self) {
        while self.change_tokens.len() > MAX_CHANGE_TOKENS {
            let oldest = self
                .change_tokens
                .iter()
                .filter(|(token, _)| {
                    self.outstanding_change_token.as_deref() != Some(token.as_str())
                })
                .min_by_key(|(_, record)| record.seq)
                .map(|(token, _)| token.clone());
            match oldest {
                Some(token) => {
                    self.change_tokens.remove(&token);
                }
                None => break,
            }
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsWafregionalAwswafRegional20161128State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsWafregionalAwswafRegional20161128State>)
}
