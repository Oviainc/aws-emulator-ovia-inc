// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-waf-awswaf-20150824 service.
//!
//! AWS WAF Classic is a change-token protocol: every mutating call carries a token
//! obtained from `GetChangeToken`, and the resources it mutates live in this struct.
//! Both halves are kept here, rather than in one operation's file, because several
//! operations read and write them.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// `ChangeTokenStatus` values, as the model declares them
/// (`crate::ops::types::CHANGE_TOKEN_STATUS_VALUES`): a token `GetChangeToken` minted
/// but nothing has used yet.
pub const CHANGE_TOKEN_PROVISIONED: &str = "PROVISIONED";
/// A token a mutating call has used, whose change is still propagating.
pub const CHANGE_TOKEN_PENDING: &str = "PENDING";
/// A token whose change has finished propagating.
pub const CHANGE_TOKEN_INSYNC: &str = "INSYNC";

/// How long a submitted change stays `PENDING` before the clock makes it `INSYNC`.
///
/// AWS documents the propagation delay only as "a few minutes", with no value, so this
/// span is CHOSEN: it is inside the emulator's 250-750 ms window for a transition the
/// clock owes, so a caller that polls `GetChangeTokenStatus` immediately still observes
/// `PENDING` and the transition happens without anything sleeping.
const CHANGE_PROPAGATION: Duration = Duration::from_millis(500);

/// The most change tokens this service keeps a status for.
///
/// `GetChangeToken` takes no input, so a caller can mint tokens in a loop; the store is
/// therefore BOUNDED and evicts in mint order. An evicted token is one the service no
/// longer accepts, which is exactly the modelled `WAFStaleDataException` case, so
/// eviction cannot make a mutating call succeed that should have failed. The value is
/// this emulator's own bound (AWS publishes no token-retention quota), chosen far above
/// any real client's in-flight token count.
const MAX_TRACKED_CHANGE_TOKENS: usize = 4096;

/// How many `IPSet`s one account may hold at a time.
///
/// AWS WAF Classic's default quota for conditions of one type per account, from the
/// "AWS WAF Classic quotas" page in the developer guide that
/// `WAFLimitsExceededException` points to ("the maximum number of … objects that you can
/// create for an AWS account"). At AWS the quota is adjustable; here it is the bound, and
/// the excess is refused with that modelled error.
pub const MAX_IP_SETS_PER_ACCOUNT: usize = 100;

/// How many `IPSetDescriptor`s one `IPSet` may hold.
///
/// AWS WAF Classic's documented quota of IP address ranges in one IP match condition,
/// from the same quotas page.
pub const MAX_DESCRIPTORS_PER_IP_SET: usize = 10_000;

/// One `IPSetDescriptor` of an `IPSet`: an IP address version and a CIDR range.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IpSetDescriptorEntry {
    /// `IPSetDescriptorType`: `IPV4` or `IPV6`.
    pub kind: String,
    /// The address range in CIDR notation.
    pub value: String,
}

/// A stored `IPSet`.
#[derive(Debug, Clone)]
pub struct IpSetEntry {
    pub ip_set_id: String,
    pub name: String,
    /// Insertion-ordered: `UpdateIPSet` appends an `INSERT` and removes a `DELETE`.
    pub descriptors: Vec<IpSetDescriptorEntry>,
    /// The sequence this `IPSet` was created at, so `ListIPSets` pages deterministically
    /// in creation order without a second collection to keep in step.
    pub created_seq: u64,
}

/// A change token minted by `GetChangeToken`.
#[derive(Debug, Clone)]
pub struct ChangeTokenEntry {
    pub token: String,
    /// One of [`CHANGE_TOKEN_PROVISIONED`], [`CHANGE_TOKEN_PENDING`], [`CHANGE_TOKEN_INSYNC`].
    pub status: String,
    /// When a `PENDING` token becomes `INSYNC`; `None` in every other status.
    pub insync_due_at: Option<Instant>,
}

/// Per-service state for com-amazonaws-waf-awswaf-20150824. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsWafAwswaf20150824State {
    /// Live `IPSet`s, keyed by `IPSetId`.
    pub ip_sets: HashMap<String, IpSetEntry>,
    /// The change tokens this service still accepts or reports on, keyed by the token
    /// value. Bounded at [`MAX_TRACKED_CHANGE_TOKENS`] entries.
    pub change_tokens: HashMap<String, ChangeTokenEntry>,
    /// The tokens of [`Self::change_tokens`] in mint order, which is the order they are
    /// evicted in once the store is full.
    change_token_order: VecDeque<String>,
    /// The token [`Self::issue_change_token`] last handed out that no mutating call has
    /// used yet — the value `GetChangeToken` keeps answering with until it is spent.
    /// `None` when nothing is outstanding.
    outstanding_change_token: Option<String>,
    /// Monotonic counter behind every id and token this service mints.
    pub next_id: u64,
}

impl ComAmazonawsWafAwswaf20150824State {
    /// The next value of the monotonic counter.
    fn next_sequence(&mut self) -> u64 {
        self.next_id = self.next_id.saturating_add(1);
        self.next_id
    }

    /// A unique id in the 8-4-4-4-12 hex layout AWS WAF ids and change tokens use.
    ///
    /// The counter occupies the low 16 hex digits, so two ids minted in the same
    /// nanosecond still differ; the wall clock occupies the high 16, so ids minted by
    /// two different runs of the emulator differ too.
    fn id_for(sequence: u64) -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0u64, |since| since.as_nanos() as u64);
        let hex = format!("{nanos:016x}{sequence:016x}");
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }

    /// The token `GetChangeToken` answers with.
    ///
    /// Documented on `GetChangeToken`: "If your application submits a GetChangeToken
    /// request and then submits a second GetChangeToken request before submitting a
    /// create, update, or delete request, the second GetChangeToken request returns the
    /// same value as the first GetChangeToken request." So an OUTSTANDING token — one
    /// already handed out that no mutating call has consumed — is handed out again, and a
    /// fresh value is minted only once the previous one has been spent or evicted.
    pub fn issue_change_token(&mut self) -> String {
        if let Some(outstanding) = self.outstanding_change_token.clone() {
            if self
                .change_tokens
                .get(&outstanding)
                .is_some_and(|entry| entry.status == CHANGE_TOKEN_PROVISIONED)
            {
                return outstanding;
            }
        }
        let token = self.mint_change_token();
        self.outstanding_change_token = Some(token.clone());
        token
    }

    /// Mint a change token in status `PROVISIONED` and record it, evicting the oldest
    /// tokens once [`MAX_TRACKED_CHANGE_TOKENS`] are tracked.
    fn mint_change_token(&mut self) -> String {
        let token = Self::id_for(self.next_sequence());
        self.change_tokens.insert(
            token.clone(),
            ChangeTokenEntry {
                token: token.clone(),
                status: CHANGE_TOKEN_PROVISIONED.to_string(),
                insync_due_at: None,
            },
        );
        self.change_token_order.push_back(token.clone());
        while self.change_token_order.len() > MAX_TRACKED_CHANGE_TOKENS {
            if let Some(evicted) = self.change_token_order.pop_front() {
                self.change_tokens.remove(&evicted);
            }
        }
        token
    }

    /// Consume `token` for a mutating call: a `PROVISIONED` token becomes `PENDING` and
    /// the call may proceed (`true`). A token that was never minted, or that some earlier
    /// call already used, is refused (`false`) — the caller answers `WAFStaleDataException`.
    ///
    /// Call this only once every other check on the request has passed: a refused call
    /// must leave its token usable.
    pub fn consume_change_token(&mut self, token: &str) -> bool {
        match self.change_tokens.get_mut(token) {
            Some(entry) if entry.status == CHANGE_TOKEN_PROVISIONED => {
                entry.status = CHANGE_TOKEN_PENDING.to_string();
                entry.insync_due_at = Some(Instant::now() + CHANGE_PROPAGATION);
                // Spent: the documented "a second GetChangeToken returns the same value"
                // holds only until a create, update or delete request uses the token, so
                // the next `GetChangeToken` has to mint a new one.
                if self.outstanding_change_token.as_deref() == Some(token) {
                    self.outstanding_change_token = None;
                }
                true
            }
            _ => false,
        }
    }

    /// The status of `token`, or `None` when no call ever minted it.
    ///
    /// Materialises the one transition the CLOCK owes — a `PENDING` token whose
    /// propagation span has elapsed is `INSYNC` whether or not anybody asked — and
    /// changes nothing else, so reporting a status cannot move a token a caller still
    /// holds for a mutating call.
    pub fn change_token_status(&mut self, token: &str) -> Option<String> {
        let entry = self.change_tokens.get_mut(token)?;
        if entry.status == CHANGE_TOKEN_PENDING
            && entry.insync_due_at.is_some_and(|due| Instant::now() >= due)
        {
            entry.status = CHANGE_TOKEN_INSYNC.to_string();
            entry.insync_due_at = None;
        }
        Some(entry.status.clone())
    }

    /// Create an empty `IPSet` named `name`, minting its `IPSetId`, and return the stored
    /// entry.
    pub fn insert_ip_set(&mut self, name: &str) -> IpSetEntry {
        let created_seq = self.next_sequence();
        let entry = IpSetEntry {
            ip_set_id: Self::id_for(created_seq),
            name: name.to_string(),
            descriptors: Vec::new(),
            created_seq,
        };
        self.ip_sets.insert(entry.ip_set_id.clone(), entry.clone());
        entry
    }

    /// Every live `IPSet` in creation order — the order `ListIPSets` pages through.
    pub fn ip_sets_in_creation_order(&self) -> Vec<&IpSetEntry> {
        let mut entries: Vec<&IpSetEntry> = self.ip_sets.values().collect();
        entries.sort_by_key(|entry| entry.created_seq);
        entries
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsWafAwswaf20150824State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsWafAwswaf20150824State>)
}
