// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-networkmanager-networkmanager service.
//!
//! The global-network / site / link lifecycle is the core resource tree of Network
//! Manager: a global network owns sites, a site owns links (and, in a later slice,
//! devices). Everything
//! the implemented operations answer is computed from the records below — no handler
//! echoes its input back without storing it.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{Bandwidth, GlobalNetwork, Link, Location, Site, Tag};

/// Members of `GlobalNetworkState` / `SiteState` / `LinkState` this slice uses. All three
/// enums carry the same four values (`smithy_query shape=GlobalNetworkState,SiteState,LinkState`:
/// AVAILABLE, DELETING, PENDING, UPDATING), mirrored in
/// `ops::types::GLOBAL_NETWORK_STATE_VALUES` / `SITE_STATE_VALUES` / `LINK_STATE_VALUES`.
pub const STATE_PENDING: &str = "PENDING";
/// See [`STATE_PENDING`].
pub const STATE_AVAILABLE: &str = "AVAILABLE";
/// See [`STATE_PENDING`].
pub const STATE_DELETING: &str = "DELETING";

/// The ARN namespace the model declares for this service (`arnNamespace: networkmanager`).
const ARN_NAMESPACE: &str = "networkmanager";

/// The resource kinds this slice mints: both the id prefix (`global-network-<hex>`,
/// `site-<hex>`, `link-<hex>`) and the ARN resource type (`global-network/<id>`,
/// `site/<id>`, `link/<id>`).
pub const GLOBAL_NETWORK_KIND: &str = "global-network";
/// See [`GLOBAL_NETWORK_KIND`].
pub const SITE_KIND: &str = "site";
/// See [`GLOBAL_NETWORK_KIND`].
pub const LINK_KIND: &str = "link";

/// Network Manager is a global (partition-scoped) service, so its ARNs carry an empty
/// region segment: `arn:aws:networkmanager::<account>:global-network/<id>`.
const ARN_REGION: &str = "";

/// The `ServiceCode` every modelled `ServiceQuotaExceededException` this service raises
/// carries: the model's own `arnNamespace`/signing name for Network Manager.
pub const SERVICE_CODE: &str = ARN_NAMESPACE;

/// The `LimitCode` of the two quotas this slice enforces. The model types `LimitCode` as
/// a free-form string and enumerates no values (`smithy_query
/// shape=ServiceQuotaExceededException`), so each code names its own quota.
pub const LIMIT_GLOBAL_NETWORKS_PER_ACCOUNT: &str = "global-networks-per-account";
/// See [`LIMIT_GLOBAL_NETWORKS_PER_ACCOUNT`].
pub const LIMIT_SITES_PER_GLOBAL_NETWORK: &str = "sites-per-global-network";
/// See [`LIMIT_GLOBAL_NETWORKS_PER_ACCOUNT`].
pub const LIMIT_LINKS_PER_SITE: &str = "links-per-site";

/// How many global networks one account may hold here, and how many sites one global
/// network may hold.
///
/// Network Manager publishes an account-level quota for both, adjustable per account, and
/// the model carries neither number, so these two are CHOSEN: their purpose is that a
/// caller cannot grow the emulator's store without bound. Reaching one is answered with
/// the operation's modelled `ServiceQuotaExceededException` (HTTP 402) and stores nothing,
/// exactly as exceeding a real quota does.
pub const MAX_GLOBAL_NETWORKS: usize = 100;
/// See [`MAX_GLOBAL_NETWORKS`].
pub const MAX_SITES_PER_GLOBAL_NETWORK: usize = 100;
/// How many links one site may hold. CHOSEN for the same reason as
/// [`MAX_SITES_PER_GLOBAL_NETWORK`]: the model carries no number and the published quota
/// is per-account adjustable, so this one only bounds the store.
pub const MAX_LINKS_PER_SITE: usize = 100;

/// How long after the creating call a `PENDING` resource settles to `AVAILABLE`.
///
/// AWS documents no span for this transition, so the value is CHOSEN; the emulator
/// contract fixes a chosen settle span inside 250–750 ms and forbids zero, so a caller
/// that reads straight after the create still observes the transient `PENDING` state.
/// Nothing sleeps for it: the due instant is stored and compared against the clock.
pub const SETTLE_DELAY: Duration = Duration::from_millis(500);

/// A stored global network. `state` is not a field: it is a function of the clock
/// (see [`GlobalNetworkRecord::state_at`]), so reading one never writes.
#[derive(Debug, Clone)]
pub struct GlobalNetworkRecord {
    pub id: String,
    pub arn: String,
    pub description: Option<String>,
    pub created_at: SystemTime,
    /// The instant the `PENDING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
    pub tags: Vec<Tag>,
}

impl GlobalNetworkRecord {
    /// This network's lifecycle state as of `now`.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if now >= self.available_at {
            STATE_AVAILABLE
        } else {
            STATE_PENDING
        }
    }

    /// The modelled `GlobalNetwork` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> GlobalNetwork {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `GlobalNetwork` for this record, forcing `state` — used by
    /// `DeleteGlobalNetwork`, which reports the record it just removed as `DELETING`.
    pub fn to_shape_with_state(&self, state: &str) -> GlobalNetwork {
        GlobalNetwork {
            global_network_id: Some(self.id.clone()),
            global_network_arn: Some(self.arn.clone()),
            description: self.description.clone(),
            created_at: Some(epoch_seconds(self.created_at)),
            state: Some(state.to_string()),
            tags: Some(self.tags.clone()),
        }
    }
}

/// A stored site, back-referencing the global network that owns it.
#[derive(Debug, Clone)]
pub struct SiteRecord {
    pub id: String,
    pub arn: String,
    pub global_network_id: String,
    pub description: Option<String>,
    pub location: Option<Location>,
    pub created_at: SystemTime,
    /// The instant the `PENDING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
    pub tags: Vec<Tag>,
}

impl SiteRecord {
    /// This site's lifecycle state as of `now`.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if now >= self.available_at {
            STATE_AVAILABLE
        } else {
            STATE_PENDING
        }
    }

    /// The modelled `Site` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> Site {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `Site` for this record, forcing `state` — used by `DeleteSite`.
    pub fn to_shape_with_state(&self, state: &str) -> Site {
        Site {
            site_id: Some(self.id.clone()),
            site_arn: Some(self.arn.clone()),
            global_network_id: Some(self.global_network_id.clone()),
            description: self.description.clone(),
            location: self.location.clone(),
            created_at: Some(epoch_seconds(self.created_at)),
            state: Some(state.to_string()),
            tags: Some(self.tags.clone()),
        }
    }
}

/// A stored link, back-referencing the site (and through it the global network) it
/// belongs to. Like the records above, `state` is a function of the clock.
#[derive(Debug, Clone)]
pub struct LinkRecord {
    pub id: String,
    pub arn: String,
    pub global_network_id: String,
    pub site_id: String,
    pub description: Option<String>,
    /// The link's `Type`, a free-form string the model does not enumerate.
    pub link_type: Option<String>,
    /// The upload/download speeds in Mbps, as the request supplied them.
    pub bandwidth: Bandwidth,
    pub provider: Option<String>,
    pub created_at: SystemTime,
    /// The instant the `PENDING` -> `AVAILABLE` transition the clock owes becomes due.
    pub available_at: SystemTime,
    pub tags: Vec<Tag>,
}

impl LinkRecord {
    /// This link's lifecycle state as of `now`.
    pub fn state_at(&self, now: SystemTime) -> &'static str {
        if now >= self.available_at {
            STATE_AVAILABLE
        } else {
            STATE_PENDING
        }
    }

    /// The modelled `Link` for this record, as of `now`.
    pub fn to_shape(&self, now: SystemTime) -> Link {
        self.to_shape_with_state(self.state_at(now))
    }

    /// The modelled `Link` for this record, forcing `state` — used by `DeleteLink`, which
    /// reports the record it just removed as `DELETING`.
    pub fn to_shape_with_state(&self, state: &str) -> Link {
        Link {
            link_id: Some(self.id.clone()),
            link_arn: Some(self.arn.clone()),
            global_network_id: Some(self.global_network_id.clone()),
            site_id: Some(self.site_id.clone()),
            description: self.description.clone(),
            r#type: self.link_type.clone(),
            bandwidth: Some(self.bandwidth.clone()),
            provider: self.provider.clone(),
            created_at: Some(epoch_seconds(self.created_at)),
            state: Some(state.to_string()),
            tags: Some(self.tags.clone()),
        }
    }
}

/// Per-service state for com-amazonaws-networkmanager-networkmanager. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsNetworkmanagerNetworkmanagerState {
    /// Global networks by `GlobalNetworkId`. A `BTreeMap`, so listing order is stable
    /// and a pagination token can name a key offset into it.
    pub global_networks: BTreeMap<String, GlobalNetworkRecord>,
    /// Sites by `SiteId`, each carrying its owning `GlobalNetworkId`.
    pub sites: BTreeMap<String, SiteRecord>,
    /// Links by `LinkId`, each carrying the `SiteId` and `GlobalNetworkId` it belongs to.
    pub links: BTreeMap<String, LinkRecord>,
    /// Monotonic source for generated resource-id suffixes, so ids are unique within a
    /// run and deterministic across runs.
    pub id_counter: u64,
}

impl ComAmazonawsNetworkmanagerNetworkmanagerState {
    /// Mint the next id for `prefix`, e.g. `global-network-00000000000000001`.
    ///
    /// Network Manager ids are `<kind>-<hex>` (the model's own patterns for the kinds
    /// that constrain them read `^core-network-([0-9a-f]{8,17})$`), so the suffix is 17
    /// lowercase hex digits of the monotonic counter.
    pub fn next_id(&mut self, prefix: &str) -> String {
        self.id_counter = self.id_counter.saturating_add(1);
        format!("{prefix}-{:017x}", self.id_counter)
    }

    /// The ARN of a resource of this service: `arn:aws:networkmanager::<account>:<kind>/<id>`.
    pub fn resource_arn(kind: &str, id: &str) -> String {
        ::engine::emulator_arn(ARN_NAMESPACE, ARN_REGION, &format!("{kind}/{id}"))
    }

    /// How many sites `global_network_id` owns, for the per-global-network quota.
    pub fn site_count_of(&self, global_network_id: &str) -> usize {
        self.sites
            .values()
            .filter(|site| site.global_network_id == global_network_id)
            .count()
    }

    /// The ids of the sites owned by `global_network_id`, in `SiteId` order.
    pub fn site_ids_of(&self, global_network_id: &str) -> Vec<String> {
        self.sites
            .values()
            .filter(|site| site.global_network_id == global_network_id)
            .map(|site| site.id.clone())
            .collect()
    }

    /// How many links `site_id` holds, for the per-site quota.
    pub fn link_count_of_site(&self, site_id: &str) -> usize {
        self.links
            .values()
            .filter(|link| link.site_id == site_id)
            .count()
    }

    /// The ids of the links attached to `site_id`, in `LinkId` order.
    pub fn link_ids_of_site(&self, site_id: &str) -> Vec<String> {
        self.links
            .values()
            .filter(|link| link.site_id == site_id)
            .map(|link| link.id.clone())
            .collect()
    }
}

/// `SystemTime` as a Smithy timestamp (epoch seconds, sub-second part kept).
fn epoch_seconds(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let secs = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64());
    ::aws_smithy_types::DateTime::from_secs_f64(secs)
}

/// One page of a paginated listing.
pub struct Page<T> {
    /// The items on this page, in key order.
    pub items: Vec<T>,
    /// The token that resumes after the last item, or `None` when this is the last page.
    pub next_token: Option<String>,
}

/// The prefix every pagination token this service mints carries, so a token minted by
/// something else (or a truncated one) is recognised as unparseable.
const PAGE_TOKEN_PREFIX: &str = "networkmanager:";

/// Encode the continuation token that resumes AFTER `last_key`.
///
/// The token is opaque to the caller: hex of a prefixed key offset into the relevant
/// `BTreeMap`, which is why the listing map is ordered. Hex, so the token survives the
/// query string a `NextToken` travels back in (`@httpQuery("nextToken")`) unescaped,
/// whatever the key holds.
fn encode_page_token(last_key: &str) -> String {
    hex::encode(format!("{PAGE_TOKEN_PREFIX}{last_key}"))
}

/// Decode a continuation token minted by [`encode_page_token`], or `None` when the token
/// is not one this service produced — the caller answers that with a `ValidationException`.
pub fn decode_page_token(token: &str) -> Option<String> {
    let text = String::from_utf8(hex::decode(token).ok()?).ok()?;
    text.strip_prefix(PAGE_TOKEN_PREFIX).map(str::to_string)
}

/// Take one page of `entries` (which must be sorted by key) starting after `after`.
///
/// `max_results` is the caller's page size; absent means "every remaining item". A
/// `next_token` is emitted only when items remain after this page, so a client that
/// follows tokens sees every item exactly once.
pub fn paginate<T>(
    entries: Vec<(String, T)>,
    after: Option<&str>,
    max_results: Option<i32>,
) -> Page<T> {
    let start = match after {
        Some(key) => entries
            .iter()
            .position(|(candidate, _)| candidate.as_str() > key)
            .unwrap_or(entries.len()),
        None => 0,
    };
    let limit = max_results
        .and_then(|requested| usize::try_from(requested).ok())
        .unwrap_or(usize::MAX)
        // A page is never empty while items remain: `MaxResults` is modelled `1..500`, so
        // 0 cannot arrive, and clamping keeps the token below from skipping an item.
        .max(1);
    let end = start.saturating_add(limit).min(entries.len());
    let next_token = if end < entries.len() {
        entries
            .get(end.saturating_sub(1))
            .map(|(key, _)| encode_page_token(key))
    } else {
        None
    };
    let items = entries
        .into_iter()
        .skip(start)
        .take(end.saturating_sub(start))
        .map(|(_, item)| item)
        .collect();
    Page { items, next_token }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsNetworkmanagerNetworkmanagerState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsNetworkmanagerNetworkmanagerState>)
}
