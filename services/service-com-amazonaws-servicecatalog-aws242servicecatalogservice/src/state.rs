// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-servicecatalog-aws242servicecatalogservice service.

use std::collections::{BTreeMap, HashMap};
use std::time::SystemTime;

use crate::ops::types::{PortfolioDetail, Tag};

/// A persisted portfolio — the record every portfolio operation reads and writes.
///
/// Tags are a map rather than a list because the API addresses them by key:
/// `UpdatePortfolio`'s `AddTags` overwrites an existing key's value and
/// `RemoveTags` deletes by key, and a portfolio therefore cannot hold the same
/// key twice. The map is a `BTreeMap` so the `Tags` list a response carries has
/// a stable, deterministic order.
#[derive(Debug, Clone)]
pub struct Portfolio {
    /// The minted portfolio id (`port-…`), also this record's key in [`ComAmazonawsServicecatalogAws242servicecatalogserviceState::portfolios`].
    pub id: String,
    /// The ARN minted at create time and read back verbatim afterwards.
    pub arn: String,
    pub display_name: String,
    pub description: Option<String>,
    pub provider_name: String,
    /// Creation instant, minted once at create time.
    pub created_time: SystemTime,
    /// Tag key -> tag value.
    pub tags: BTreeMap<String, String>,
}

impl Portfolio {
    /// This record as the modelled `PortfolioDetail`: every member comes from
    /// stored state, so Create/Describe/List/Update all report the same values.
    pub fn detail(&self) -> PortfolioDetail {
        PortfolioDetail {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            display_name: Some(self.display_name.clone()),
            description: self.description.clone(),
            created_time: Some(::aws_smithy_types::DateTime::from(self.created_time)),
            provider_name: Some(self.provider_name.clone()),
        }
    }

    /// The stored tags as the modelled `Tags` list, in key order.
    pub fn tag_list(&self) -> Vec<Tag> {
        self.tags
            .iter()
            .map(|(key, value)| Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect()
    }
}

/// The most tags one portfolio can carry.
///
/// From the model: every operation that returns a portfolio's `Tags` targets the
/// `Tags` list, whose length is constrained `0..50`
/// (`smithy_query query="operation" operation="UpdatePortfolio"`), so a stored
/// portfolio can never hold more than 50 tags.
pub const MAX_PORTFOLIO_TAGS: usize = 50;

/// The most portfolios one account can hold, which also bounds this store.
///
/// AWS's documented default Service Catalog quota is 100 portfolios per account
/// per Region, and `CreatePortfolio` models `LimitExceededException` ("The current
/// limits of the service would have been exceeded by this operation") for exactly
/// that refusal. Because the live set is capped, so is every response built from
/// it: `ListPortfolios` cannot return more than this many records on a page.
pub const MAX_PORTFOLIOS: usize = 100;

/// Per-service state for com-amazonaws-servicecatalog-aws242servicecatalogservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsServicecatalogAws242servicecatalogserviceState {
    /// Every created, not-yet-deleted portfolio, keyed by portfolio id. A
    /// `BTreeMap` so `ListPortfolios` and its page tokens walk one stable order.
    pub portfolios: BTreeMap<String, Portfolio>,
    /// `CreatePortfolio` idempotency: `IdempotencyToken` -> portfolio id, so a
    /// repeated create replays the first one instead of minting a second
    /// portfolio.
    pub idempotency: HashMap<String, String>,
    /// Counter behind [`Self::next_portfolio_id`]. Deterministic, so a fresh
    /// emulator mints the same id sequence every run.
    pub next_id_seed: u64,
}

impl ComAmazonawsServicecatalogAws242servicecatalogserviceState {
    /// Mint an unused portfolio id of AWS's form: `port-` followed by 12
    /// lowercase base-36 characters, which satisfies the modelled `Id` pattern
    /// `^[a-zA-Z0-9_\-]*$` and its 1..100 length.
    pub fn next_portfolio_id(&mut self) -> String {
        loop {
            self.next_id_seed = self.next_id_seed.wrapping_add(1);
            let id = format!("port-{}", base36_12(scramble(self.next_id_seed)));
            if !self.portfolios.contains_key(&id) {
                return id;
            }
        }
    }
}

/// SplitMix64's finalizer: spreads a small counter over the whole 64-bit range so
/// successive ids do not look sequential. Deterministic and injective.
fn scramble(counter: u64) -> u64 {
    let mut z = counter.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// `value` as exactly 12 lowercase base-36 digits (`0-9a-z`), most significant
/// first. 36^12 > 4.7e18, so the modulo keeps the full width of the alphabet.
fn base36_12(value: u64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    const WIDTH: usize = 12;
    let mut n = value % 36u64.pow(WIDTH as u32);
    let mut out = [b'0'; WIDTH];
    for slot in out.iter_mut().rev() {
        *slot = DIGITS[(n % 36) as usize];
        n /= 36;
    }
    out.iter().map(|digit| char::from(*digit)).collect()
}

/// The failure a handler answers when this service's state cannot be reached —
/// i.e. the emulator was built on an engine type this service cannot use, so the
/// operation cannot be served at all. Unreachable in the assembled emulator,
/// which builds exactly one `InMemoryEngine`; never answer it by constructing a
/// private engine, whose writes no later request can see.
pub fn state_unavailable<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::NotImplemented(crate::scaffolded::NotImplemented {
        message: "this emulator was built on an engine this service cannot reach its state \
                  through, so no Service Catalog operation can be served"
            .to_string(),
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsServicecatalogAws242servicecatalogserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsServicecatalogAws242servicecatalogserviceState>)
}
