// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig service.
//!
//! Holds the cluster -> control panel -> routing control hierarchy: every resource a
//! create call minted, keyed by the ARN that call handed the client. A handler reaches
//! it through [`service_state`] and answers from what is stored here — a describe or a
//! list never recomputes a body from its own request.
//!
//! Two values are DERIVED rather than stored, so no read can see them stale:
//!
//! * `Status` — computed from the instant the create call made the deployment due
//!   ([`deploy_due`]), so a resource reports `PENDING` until then and `DEPLOYED`
//!   afterwards without any operation writing it.
//! * `ControlPanel$RoutingControlCount` — counted from [`routing_controls`] on every
//!   projection ([`Store::routing_control_count`]).
//!
//! [`routing_controls`]: Store::routing_controls

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::ops::types::{Cluster, ClusterEndpoint, ControlPanel, RoutingControl};

/// The ARN service segment Route 53 Application Recovery Controller's recovery-control
/// resources carry: AWS documents them as
/// `arn:aws:route53-recovery-control::<account>:cluster/<id>` (the model's
/// `arnNamespace`/`endpointPrefix`, `route53-recovery-control-config`, names the API
/// endpoint, not the resource namespace).
pub const ARN_SERVICE: &str = "route53-recovery-control";

/// Resource ARNs in this service are account-wide, not Regional — AWS writes them with
/// an empty Region segment (`arn:aws:route53-recovery-control::<account>:...`).
pub const ARN_REGION: &str = "";

/// The `Status` value a resource reports before its deployment is due.
pub const STATUS_PENDING: &str = "PENDING";

/// The `Status` value a resource reports once its deployment is due.
pub const STATUS_DEPLOYED: &str = "DEPLOYED";

/// The name AWS gives the control panel it creates together with a cluster.
pub const DEFAULT_CONTROL_PANEL_NAME: &str = "DefaultControlPanel";

/// How long after the create call a resource's `PENDING` -> `DEPLOYED` transition
/// becomes due.
///
/// AWS returns these resources `PENDING` and deploys them asynchronously, so the span
/// is this emulator's own choice; it is deliberately non-zero so a caller that reads
/// straight back still observes the transient status.
const DEPLOY_DELAY: Duration = Duration::from_millis(500);

/// The page size a list operation serves when the request names no `MaxResults`.
///
/// `ListRoutingControls$MaxResults` is modelled `range 1..1000`, so "unspecified" is
/// served as the whole modelled maximum.
pub const DEFAULT_PAGE_SIZE: usize = 1000;

/// The most clusters this store holds at once.
///
/// The model declares `ServiceQuotaExceededException` ("You attempted to create more
/// resources than the service allows based on service quotas") but states no number, so
/// the figure is CHOSEN: it exists to bound caller-driven growth of the store, not to
/// reproduce an AWS quota.
pub const MAX_CLUSTERS: usize = 100;

/// The most control panels this store holds at once — CHOSEN, for the same reason as
/// [`MAX_CLUSTERS`]. One of every cluster's panels is the default panel it was created
/// with, so this bounds the caller-created ones too.
pub const MAX_CONTROL_PANELS: usize = 1_000;

/// The most routing controls this store holds at once — CHOSEN, for the same reason as
/// [`MAX_CLUSTERS`].
pub const MAX_ROUTING_CONTROLS: usize = 10_000;

/// The most `ClientToken` records the idempotency ledger remembers at once — CHOSEN,
/// for the same reason: a caller must not be able to grow the ledger without limit.
///
/// A forgotten token simply has no memory left: a replayed create then meets the
/// name-uniqueness check instead of returning the first resource.
pub const MAX_IDEMPOTENCY_TOKENS: usize = 10_000;

/// The Regions Route 53 ARC publishes a cluster data-plane endpoint in — "an array of
/// the five cluster endpoints (one for each supported Amazon Web Services Region)"
/// (`CreateCluster`'s documentation, via
/// `smithy_query(query="doc",shape="CreateCluster")`).
pub const CLUSTER_ENDPOINT_REGIONS: &[&str] = &[
    "ap-northeast-1",
    "ap-southeast-2",
    "eu-west-1",
    "us-east-1",
    "us-west-2",
];

/// The instant a create call performed now makes the resource's deployment due.
pub fn deploy_due() -> Instant {
    Instant::now() + DEPLOY_DELAY
}

/// The `Status` a resource whose deployment is due at `due` reports right now.
///
/// Purely a function of the clock, so reporting it writes nothing.
pub fn status_at(due: Instant) -> &'static str {
    if Instant::now() >= due {
        STATUS_DEPLOYED
    } else {
        STATUS_PENDING
    }
}

/// The five Regional data-plane endpoints a cluster publishes.
///
/// `DUALSTACK` clusters are reached over AWS's dual-stack endpoint suffix
/// (`<region>.api.aws`); `IPV4` clusters over the IPv4-only one
/// (`<region>.amazonaws.com`).
pub fn cluster_endpoints(cluster_id: &str, network_type: &str) -> Vec<(String, String)> {
    let suffix = if network_type == "DUALSTACK" {
        "api.aws"
    } else {
        "amazonaws.com"
    };
    CLUSTER_ENDPOINT_REGIONS
        .iter()
        .map(|region| {
            (
                (*region).to_string(),
                format!("https://{cluster_id}.route53-recovery-cluster.{region}.{suffix}/v1"),
            )
        })
        .collect()
}

/// One stored cluster.
#[derive(Debug, Clone)]
pub struct ClusterRecord {
    /// Creation order, so `ListClusters`-style ordering is stable: the minted id is
    /// opaque and carries no order.
    pub seq: u64,
    pub arn: String,
    pub name: String,
    /// The account that owns the cluster — this emulator's account id.
    pub owner: String,
    pub network_type: String,
    /// `(Region, Endpoint)` pairs, in the order `ClusterEndpoints` reports them.
    pub endpoints: Vec<(String, String)>,
    pub tags: BTreeMap<String, String>,
    /// The control panel AWS creates together with the cluster, and the one
    /// `CreateRoutingControl` attaches to when the request names none.
    pub default_control_panel_arn: String,
    /// When this cluster's `PENDING` -> `DEPLOYED` transition became due.
    pub deploy_due: Instant,
}

impl ClusterRecord {
    /// The cluster's id: the resource tail of its ARN, which is the host label its
    /// data-plane endpoints are published under.
    pub fn cluster_id(&self) -> &str {
        self.arn
            .rsplit_once('/')
            .map_or(self.arn.as_str(), |(_, id)| id)
    }

    /// Move the cluster onto `network_type`, re-deriving the Regional data-plane
    /// endpoints that network type decides and making the deployment of those endpoints
    /// due again — so a caller cannot write one half of the transition without the
    /// other.
    ///
    /// A request naming the network the cluster is already on writes NOTHING: there is
    /// no endpoint to re-derive and no deployment to redo. The endpoints are stored
    /// rather than projected so that one function ([`cluster_endpoints`]) decides them
    /// for a created and for an updated cluster alike.
    ///
    /// Answers whether the record changed, so a caller can tell a redeploy from a
    /// no-op.
    pub fn set_network_type(&mut self, network_type: String) -> bool {
        if self.network_type == network_type {
            return false;
        }
        let endpoints = cluster_endpoints(self.cluster_id(), &network_type);
        self.endpoints = endpoints;
        self.network_type = network_type;
        self.deploy_due = deploy_due();
        true
    }

    /// This record as the modelled `Cluster` shape — the single projection create and
    /// describe both answer with.
    pub fn to_cluster(&self) -> Cluster {
        Cluster {
            cluster_arn: Some(self.arn.clone()),
            cluster_endpoints: Some(
                self.endpoints
                    .iter()
                    .map(|(region, endpoint)| ClusterEndpoint {
                        endpoint: Some(endpoint.clone()),
                        region: Some(region.clone()),
                    })
                    .collect(),
            ),
            name: Some(self.name.clone()),
            status: Some(status_at(self.deploy_due).to_string()),
            owner: Some(self.owner.clone()),
            network_type: Some(self.network_type.clone()),
        }
    }
}

/// One stored control panel.
#[derive(Debug, Clone)]
pub struct ControlPanelRecord {
    /// Creation order within the service, for a stable list page order.
    pub seq: u64,
    pub arn: String,
    pub cluster_arn: String,
    pub name: String,
    pub owner: String,
    /// True for the panel created together with the cluster.
    pub default_control_panel: bool,
    pub tags: BTreeMap<String, String>,
    pub deploy_due: Instant,
}

impl ControlPanelRecord {
    /// This record as the modelled `ControlPanel` shape.
    ///
    /// `routing_control_count` is passed in because it is counted from the routing
    /// controls stored under this panel, never kept on the record.
    pub fn to_control_panel(&self, routing_control_count: i32) -> ControlPanel {
        ControlPanel {
            cluster_arn: Some(self.cluster_arn.clone()),
            control_panel_arn: Some(self.arn.clone()),
            default_control_panel: Some(self.default_control_panel),
            name: Some(self.name.clone()),
            routing_control_count: Some(routing_control_count),
            status: Some(status_at(self.deploy_due).to_string()),
            owner: Some(self.owner.clone()),
        }
    }
}

/// One stored routing control.
#[derive(Debug, Clone)]
pub struct RoutingControlRecord {
    /// Creation order within the service, which is the order `ListRoutingControls`
    /// pages through.
    pub seq: u64,
    pub arn: String,
    pub control_panel_arn: String,
    pub cluster_arn: String,
    pub name: String,
    pub owner: String,
    pub deploy_due: Instant,
}

impl RoutingControlRecord {
    /// This record as the modelled `RoutingControl` shape.
    pub fn to_routing_control(&self) -> RoutingControl {
        RoutingControl {
            control_panel_arn: Some(self.control_panel_arn.clone()),
            name: Some(self.name.clone()),
            routing_control_arn: Some(self.arn.clone()),
            status: Some(status_at(self.deploy_due).to_string()),
            owner: Some(self.owner.clone()),
        }
    }
}

/// What a `ClientToken` the store has already seen means for the call replaying it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenOutcome {
    /// The token is new: perform the create.
    Fresh,
    /// The token already created this resource with the same request: answer with it.
    Replay(String),
    /// The token already created a resource from a DIFFERENT request: refuse.
    Mismatch,
}

/// One `ClientToken` the store has honoured.
#[derive(Debug, Clone)]
struct IdempotencyRecord {
    /// The ARN the first call minted.
    arn: String,
    /// The request the first call carried, so a token replayed with different
    /// parameters is distinguishable from a true retry.
    fingerprint: String,
}

/// Per-service state for com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsRoute53recoverycontrolconfigRoute53recoverycontrolconfigState {
    /// Every created cluster, keyed by its ARN — the only identity a client holds.
    pub clusters: BTreeMap<String, ClusterRecord>,
    /// Every created control panel, keyed by its ARN.
    pub control_panels: BTreeMap<String, ControlPanelRecord>,
    /// Every created routing control, keyed by its ARN.
    pub routing_controls: BTreeMap<String, RoutingControlRecord>,
    /// `"<Operation>#<ClientToken>"` -> the resource that token created. Held to
    /// [`MAX_IDEMPOTENCY_TOKENS`] by [`Self::remember_token`].
    idempotency: BTreeMap<String, IdempotencyRecord>,
    /// The order tokens entered `idempotency`, so the ledger can be held to
    /// [`MAX_IDEMPOTENCY_TOKENS`] by forgetting the oldest.
    idempotency_order: VecDeque<String>,
    /// Advanced once per minted id, so an id is never a function of the request.
    id_seed: u64,
    /// Advanced once per created resource, giving each record its creation order.
    seq_counter: u64,
}

/// A convenient alias for this service's store, which the handlers name a lot.
pub type Store = ComAmazonawsRoute53recoverycontrolconfigRoute53recoverycontrolconfigState;

impl ComAmazonawsRoute53recoverycontrolconfigRoute53recoverycontrolconfigState {
    /// Mint a fresh id in the 8-4-4-4-12 hexadecimal form AWS gives these resources.
    ///
    /// Drawn from a counter the state advances (seeded from the clock on first use),
    /// never from the request, so creating the same resource twice yields two ids.
    pub fn mint_id(&mut self) -> String {
        if self.id_seed == 0 {
            self.id_seed = now_nanos();
        }
        self.id_seed = splitmix64(self.id_seed);
        let high = self.id_seed;
        let low = splitmix64(high);
        let hex = format!("{high:016x}{low:016x}");
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }

    /// The next creation-order number.
    pub fn next_seq(&mut self) -> u64 {
        self.seq_counter = self.seq_counter.wrapping_add(1);
        self.seq_counter
    }

    /// Whether the store already holds [`MAX_CLUSTERS`] clusters, so another create
    /// must be refused rather than grow it.
    pub fn clusters_full(&self) -> bool {
        self.clusters.len() >= MAX_CLUSTERS
    }

    /// Whether the store already holds [`MAX_CONTROL_PANELS`] control panels.
    pub fn control_panels_full(&self) -> bool {
        self.control_panels.len() >= MAX_CONTROL_PANELS
    }

    /// Whether the store already holds [`MAX_ROUTING_CONTROLS`] routing controls.
    pub fn routing_controls_full(&self) -> bool {
        self.routing_controls.len() >= MAX_ROUTING_CONTROLS
    }

    /// How many routing controls are stored under `control_panel_arn`.
    ///
    /// This is what `ControlPanel$RoutingControlCount` reports, so the count can never
    /// drift from the routing controls a list call returns.
    pub fn routing_control_count(&self, control_panel_arn: &str) -> i32 {
        self.routing_controls
            .values()
            .filter(|rc| rc.control_panel_arn == control_panel_arn)
            .count()
            .try_into()
            .unwrap_or(i32::MAX)
    }

    /// `control_panel_arn`'s routing controls in creation order.
    pub fn ordered_routing_controls(&self, control_panel_arn: &str) -> Vec<&RoutingControlRecord> {
        let mut found: Vec<&RoutingControlRecord> = self
            .routing_controls
            .values()
            .filter(|rc| rc.control_panel_arn == control_panel_arn)
            .collect();
        found.sort_by_key(|rc| rc.seq);
        found
    }

    /// Every stored cluster, in creation order — the order an account-wide list pages
    /// through.
    pub fn ordered_clusters(&self) -> Vec<&ClusterRecord> {
        let mut found: Vec<&ClusterRecord> = self.clusters.values().collect();
        found.sort_by_key(|cluster| cluster.seq);
        found
    }

    /// The stored control panels in creation order: those of one cluster when
    /// `cluster_arn` names one, otherwise every panel in the account.
    ///
    /// `ListControlPanels` scopes its page with the optional filter; the operations that
    /// work on one cluster pass `Some(..)`.
    pub fn ordered_control_panels(&self, cluster_arn: Option<&str>) -> Vec<&ControlPanelRecord> {
        let mut found: Vec<&ControlPanelRecord> = self
            .control_panels
            .values()
            .filter(|cp| cluster_arn.is_none_or(|arn| cp.cluster_arn == arn))
            .collect();
        found.sort_by_key(|cp| cp.seq);
        found
    }

    /// What a create call carrying `token` for `operation` should do, given the request
    /// `fingerprint`.
    ///
    /// A request with no token is always [`TokenOutcome::Fresh`]: `@idempotencyToken`
    /// makes the member optional, and without one there is nothing to replay.
    pub fn token_outcome(
        &self,
        operation: &str,
        token: Option<&str>,
        fingerprint: &str,
    ) -> TokenOutcome {
        let Some(token) = token else {
            return TokenOutcome::Fresh;
        };
        match self.idempotency.get(&token_key(operation, token)) {
            None => TokenOutcome::Fresh,
            Some(record) if record.fingerprint == fingerprint => {
                TokenOutcome::Replay(record.arn.clone())
            }
            Some(_) => TokenOutcome::Mismatch,
        }
    }

    /// Remember that `token` created `arn` from the request `fingerprint`, forgetting
    /// the oldest records once the ledger is at [`MAX_IDEMPOTENCY_TOKENS`].
    pub fn remember_token(
        &mut self,
        operation: &str,
        token: Option<&str>,
        fingerprint: &str,
        arn: &str,
    ) {
        let Some(token) = token else {
            return;
        };
        let key = token_key(operation, token);
        let record = IdempotencyRecord {
            arn: arn.to_string(),
            fingerprint: fingerprint.to_string(),
        };
        if self.idempotency.insert(key.clone(), record).is_none() {
            self.idempotency_order.push_back(key);
        }
        while self.idempotency_order.len() > MAX_IDEMPOTENCY_TOKENS {
            if let Some(oldest) = self.idempotency_order.pop_front() {
                self.idempotency.remove(&oldest);
            }
        }
    }

    /// Forget every token that created a resource whose ARN starts with `arn_prefix`.
    ///
    /// Called by the deletes, so a create/delete loop cannot grow the ledger and no
    /// token can report a resource that is gone as if it were still there. The prefix
    /// form is what releases a control panel's routing-control tokens with it: AWS
    /// nests a routing control's ARN inside its control panel's.
    pub fn forget_tokens_for(&mut self, arn_prefix: &str) {
        let ledger = &mut self.idempotency;
        let order = &mut self.idempotency_order;
        ledger.retain(|_, record| !record.arn.starts_with(arn_prefix));
        order.retain(|key| ledger.contains_key(key));
    }

    /// How many `ClientToken` records the ledger currently holds.
    pub fn idempotency_len(&self) -> usize {
        self.idempotency.len()
    }
}

/// A `ClientToken` is scoped to the operation that honoured it, so the same token on
/// two different creates is two independent tokens.
fn token_key(operation: &str, token: &str) -> String {
    format!("{operation}#{token}")
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRoute53recoverycontrolconfigRoute53recoverycontrolconfigState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRoute53recoverycontrolconfigRoute53recoverycontrolconfigState>)
}

/// One page of a list operation: the items to return and the `NextToken` that resumes
/// after them (`None` when this page is the last one).
#[derive(Debug)]
pub struct Page<'a, T> {
    pub items: Vec<&'a T>,
    pub next_token: Option<String>,
}

/// Cut the page `next_token` and `max_results` select out of `ordered`.
///
/// The cursor is the KEY of the first item of the next page, not an index, so it needs
/// no server-side bookkeeping — a list call stays a pure read — and an item created or
/// deleted between two pages never shifts an already-returned item into view twice.
/// `None` when `next_token` names no item in `ordered`, which the caller answers with
/// the list operation's modelled client error.
pub fn paginate<'a, T, K>(
    ordered: &[&'a T],
    key: K,
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Option<Page<'a, T>>
where
    K: Fn(&T) -> &str,
{
    let start = match next_token {
        None => 0,
        Some(token) => ordered.iter().position(|item| key(item) == token)?,
    };
    let page_size = match max_results {
        Some(n) if n > 0 => n as usize,
        _ => DEFAULT_PAGE_SIZE,
    };
    let end = start.saturating_add(page_size).min(ordered.len());
    Some(Page {
        items: ordered[start..end].to_vec(),
        next_token: ordered.get(end).map(|item| key(item).to_string()),
    })
}

/// A request's `Tags` map as a deterministic string, for an idempotency fingerprint.
pub fn tags_fingerprint(tags: Option<&std::collections::HashMap<String, String>>) -> String {
    let mut pairs: Vec<(&str, &str)> = tags
        .map(|t| t.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect())
        .unwrap_or_default();
    pairs.sort_unstable();
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// A request's `Tags` map as the ordered map a record stores.
pub fn tags_map(tags: Option<std::collections::HashMap<String, String>>) -> BTreeMap<String, String> {
    tags.unwrap_or_default().into_iter().collect()
}

/// Wall-clock nanoseconds since the epoch, used only to seed the id counter.
fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(1, |d| d.as_nanos() as u64)
        | 1
}

/// SplitMix64: a bijection on `u64`, so advancing the id counter through it never
/// revisits a value before the counter wraps.
fn splitmix64(state: u64) -> u64 {
    let mut z = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Changing a cluster's network type re-derives its endpoints and makes the
    /// deployment due again; requesting the network it is already on writes nothing.
    #[test]
    fn setting_the_network_type_redeploys_only_when_it_changes() {
        let arn = "arn:aws:route53-recovery-control::000000000000:cluster/abc";
        // Rows: every ordered pair of the values `NetworkType` declares
        // (`smithy_query(query="shape",shape="NetworkType")` -> DUALSTACK | IPV4).
        for from in ["DUALSTACK", "IPV4"] {
            for to in ["DUALSTACK", "IPV4"] {
                let case = format!("{from} -> {to}");
                let mut record = ClusterRecord {
                    seq: 1,
                    arn: arn.to_string(),
                    name: "c".to_string(),
                    owner: "000000000000".to_string(),
                    network_type: from.to_string(),
                    endpoints: cluster_endpoints("abc", from),
                    tags: BTreeMap::new(),
                    default_control_panel_arn: String::new(),
                    deploy_due: Instant::now(),
                };
                let was_due = record.deploy_due;

                let changed = record.set_network_type(to.to_string());

                assert_eq!(
                    changed,
                    from != to,
                    "case {case}: the record changes exactly when the network type does, \
                     got {changed}"
                );
                assert_eq!(
                    record.network_type, to,
                    "case {case}: expected the requested network type, got {}",
                    record.network_type
                );
                assert_eq!(
                    record.endpoints,
                    cluster_endpoints("abc", to),
                    "case {case}: the endpoints must be the ones its network type decides"
                );
                assert_eq!(
                    record.deploy_due > was_due,
                    from != to,
                    "case {case}: only a change re-deploys the cluster"
                );
            }
        }
    }

    /// A record the capacity tests can store without going through a handler.
    fn routing_control(arn: &str) -> RoutingControlRecord {
        RoutingControlRecord {
            seq: 0,
            arn: arn.to_string(),
            control_panel_arn: "panel".to_string(),
            cluster_arn: "cluster".to_string(),
            name: arn.to_string(),
            owner: "000000000000".to_string(),
            deploy_due: Instant::now(),
        }
    }

    /// The ledger never grows past its ceiling, however many tokens a caller spends,
    /// and what it keeps is the newest.
    #[test]
    fn the_idempotency_ledger_is_bounded_and_keeps_the_newest_tokens() {
        let mut state = Store::default();
        let total = MAX_IDEMPOTENCY_TOKENS + 25;
        for n in 0..total {
            state.remember_token("CreateCluster", Some(&format!("token-{n}")), "fp", "arn");
        }
        assert!(
            state.idempotency_len() <= MAX_IDEMPOTENCY_TOKENS,
            "case {total} tokens: the ledger must stay within {MAX_IDEMPOTENCY_TOKENS}, got {}",
            state.idempotency_len()
        );
        let newest = format!("token-{}", total - 1);
        assert_eq!(
            state.token_outcome("CreateCluster", Some(&newest), "fp"),
            TokenOutcome::Replay("arn".to_string()),
            "case newest token: it must still be remembered"
        );
        assert_eq!(
            state.token_outcome("CreateCluster", Some("token-0"), "fp"),
            TokenOutcome::Fresh,
            "case oldest token: it must have been evicted"
        );
    }

    /// Deleting a resource releases the tokens that created it, so a create/delete loop
    /// cannot accumulate ledger entries.
    #[test]
    fn deleting_a_resource_releases_its_tokens() {
        let mut state = Store::default();
        let panel = "arn:aws:route53-recovery-control::000000000000:controlpanel/p";
        let control = format!("{panel}/routingcontrol/r");
        state.remember_token("CreateControlPanel", Some("t-panel"), "fp", panel);
        state.remember_token("CreateRoutingControl", Some("t-control"), "fp", &control);
        assert_eq!(
            state.idempotency_len(),
            2,
            "case two tokens spent: both are remembered"
        );

        // The prefix form releases the panel's routing-control tokens with it.
        state.forget_tokens_for(panel);
        assert_eq!(
            state.idempotency_len(),
            0,
            "case panel deleted: every token naming it or a resource under it is released, \
             got {}",
            state.idempotency_len()
        );
        for (case, token) in [("panel", "t-panel"), ("routing control", "t-control")] {
            assert_eq!(
                state.token_outcome("CreateControlPanel", Some(token), "fp"),
                TokenOutcome::Fresh,
                "case released {case} token: it must no longer name a deleted resource"
            );
        }
    }

    /// Each resource store reports itself full exactly at its ceiling, which is what the
    /// create handlers refuse past.
    #[test]
    fn a_resource_store_reports_itself_full_at_its_ceiling() {
        let mut state = Store::default();
        assert!(
            !state.routing_controls_full(),
            "case empty store: it is not full"
        );
        for n in 0..MAX_ROUTING_CONTROLS {
            let arn = format!("arn/{n}");
            state.routing_controls.insert(arn.clone(), routing_control(&arn));
            assert_eq!(
                state.routing_controls_full(),
                n + 1 >= MAX_ROUTING_CONTROLS,
                "case {} stored: full must flip only at {MAX_ROUTING_CONTROLS}",
                n + 1
            );
        }
        assert!(
            state.routing_controls_full(),
            "case {MAX_ROUTING_CONTROLS} stored: the store is full"
        );
    }
}
