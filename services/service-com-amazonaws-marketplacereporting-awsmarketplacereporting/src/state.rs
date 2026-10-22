// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-marketplacereporting-awsmarketplacereporting service.
//!
//! AWS Marketplace Reporting models exactly one operation, `GetBuyerDashboard`,
//! which "generates an embedding URL for an Amazon QuickSight dashboard". The
//! records below are what that operation reads and writes:
//!
//! * [`Dashboard`] — the dashboards the service publishes, which are reporting
//!   data AWS Marketplace maintains for an account rather than resources a
//!   caller creates, so an entry is materialised the first time one is asked
//!   for.
//! * [`EmbedSession`] — one generated embedding URL, with the two lifetimes the
//!   operation documents ([`TOKEN_VALIDITY`] and [`SESSION_LIFETIME`]) and the
//!   allow list of embedding domains the generating request named. The session
//!   is what makes the URL's bearer token redeemable exactly once.
//! * [`CallerAccess`] — whether `GetBuyerDashboard` is available to a caller at
//!   all, which the operation's docs restrict to Organizations management
//!   accounts and delegated administrators registered for procurement insights.
//!   That grant is created in AWS Organizations, so no operation of THIS service
//!   can write one: a record in [`ComAmazonawsMarketplacereportingAwsmarketplacereportingState::access`]
//!   is the only seam onto the success path, and a caller with no record is
//!   refused (see [`CallerAccess::standalone_account`]).

use std::collections::hash_map::{Entry, RandomState};
use std::collections::HashMap;
use std::fmt;
use std::hash::BuildHasher;
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

/// How long the bearer token in a generated embed URL stays valid.
///
/// `GetBuyerDashboard` documents the value: the generated URL "contains a
/// temporary bearer token, valid for 5 minutes after it is generated. Once
/// redeemed within that period, it cannot be re-used again."
pub const TOKEN_VALIDITY: Duration = Duration::from_secs(5 * 60);

/// How long the session a generated embed URL opens lasts.
///
/// `GetBuyerDashboard` documents the value: the generated URL "has a session
/// lifetime of one hour. The 5-minute validity period runs separately from the
/// session lifetime." Separately means this span is measured from generation
/// too, not from redemption.
pub const SESSION_LIFETIME: Duration = Duration::from_secs(60 * 60);

/// The region an embed URL is built for when the request carries no SigV4
/// credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How many embed sessions the store holds before the oldest are forgotten.
///
/// Every session is released by the clock one [`SESSION_LIFETIME`] after it was
/// generated, so the store is already bounded by the calls one hour serves;
/// this is the emulator's OWN bound on that store for a caller that generates
/// faster than that, and it is not a claim about any AWS quota. Past it the
/// oldest sessions — the ones closest to lapsing anyway — are dropped, which a
/// caller cannot tell from a session whose hour ran out.
pub const EMBED_SESSION_LIMIT: usize = 1_000;

/// The prefix of every `DashboardIdentifier`, from that shape's `@pattern`
/// (`^arn:aws:aws-marketplace::[0-9]{12}:AWSMarketplace/ReportingData/…$`).
const DASHBOARD_ARN_PREFIX: &str = "arn:aws:aws-marketplace::";

/// What separates the account id from the dashboard's dataset path in a
/// `DashboardIdentifier`, from the same `@pattern`.
const DASHBOARD_ARN_INFIX: &str = ":AWSMarketplace/ReportingData/";

/// How many digits the account segment of a `DashboardIdentifier` carries
/// (`[0-9]{12}` in that shape's `@pattern`).
const ACCOUNT_ID_DIGITS: usize = 12;

/// One of the dashboards AWS Marketplace Reporting publishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashboardKind {
    /// `Agreement_V1/Dashboard/AgreementSummary_V1`.
    AgreementSummaryV1,
    /// `BillingEvent_V1/Dashboard/CostAnalysis_V1`.
    CostAnalysisV1,
}

impl DashboardKind {
    /// Every dashboard this service publishes: the `<dataset>/Dashboard/<name>`
    /// tail of its ARN, and the dashboard that tail names.
    ///
    /// The catalog is `DashboardIdentifier`'s own `@pattern`, whose alternation
    /// admits exactly these two tails — read it with
    /// `smithy_query(query="shape",shape="DashboardIdentifier")`.
    pub const CATALOG: [(&'static str, Self); 2] = [
        (
            "Agreement_V1/Dashboard/AgreementSummary_V1",
            Self::AgreementSummaryV1,
        ),
        (
            "BillingEvent_V1/Dashboard/CostAnalysis_V1",
            Self::CostAnalysisV1,
        ),
    ];

    /// The dashboard `tail` names, or `None` when it names none of them.
    pub fn from_arn_tail(tail: &str) -> Option<Self> {
        Self::CATALOG
            .iter()
            .find(|(candidate, _)| *candidate == tail)
            .map(|(_, kind)| *kind)
    }

    /// This dashboard's `<dataset>/Dashboard/<name>` ARN tail.
    pub fn arn_tail(self) -> &'static str {
        Self::CATALOG
            .iter()
            .find(|(_, kind)| *kind == self)
            .map_or("", |(tail, _)| *tail)
    }

    /// The dashboard's own name — the last segment of its ARN tail, and the
    /// dashboard segment of an embed URL.
    pub fn dashboard_name(self) -> &'static str {
        self.arn_tail().split('/').next_back().unwrap_or_default()
    }
}

/// One dashboard this service publishes, for one account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dashboard {
    /// The dashboard's ARN, as `DashboardIdentifier` carries it.
    pub identifier: String,
    /// The account the dashboard reports on — the ARN's account segment.
    pub account_id: String,
    /// Which dashboard it is, parsed from the ARN's tail.
    pub kind: DashboardKind,
}

impl Dashboard {
    /// The dashboard a `DashboardIdentifier` names, or `None` when the value
    /// names no dashboard this service publishes.
    ///
    /// The boundary validator refuses a value outside `DashboardIdentifier`'s
    /// `@pattern` before any handler runs, so over the wire this answers `None`
    /// only for a value the pattern admits and the catalog does not name.
    pub fn parse(identifier: &str) -> Option<Self> {
        let rest = identifier.strip_prefix(DASHBOARD_ARN_PREFIX)?;
        let (account_id, tail) = rest.split_once(DASHBOARD_ARN_INFIX)?;
        if account_id.len() != ACCOUNT_ID_DIGITS
            || !account_id.bytes().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        Some(Self {
            identifier: identifier.to_string(),
            account_id: account_id.to_string(),
            kind: DashboardKind::from_arn_tail(tail)?,
        })
    }
}

/// One generated embedding URL: its one-shot bearer token, the allow list the
/// generating request named, and the two instants the operation's documented
/// lifetimes fall due at.
pub struct EmbedSession {
    /// The bearer token the embed URL carries, and the key this session is
    /// stored under.
    pub token: String,
    /// The principal the session was generated for: the caller whose
    /// registration authorised the call, and the only one that may redeem it.
    pub principal: String,
    /// The dashboard the URL embeds, as the generating request named it.
    pub dashboard_identifier: String,
    /// The fully qualified domains allowed to embed this URL.
    pub embedding_domains: Vec<String>,
    /// The URL itself, carrying [`token`](Self::token).
    pub embed_url: String,
    /// When the URL was generated.
    pub created_at: SystemTime,
    /// `created_at` + [`TOKEN_VALIDITY`]: past it the token cannot be redeemed.
    pub token_expires_at: SystemTime,
    /// `created_at` + [`SESSION_LIFETIME`]: when the session the URL opens
    /// lapses. It runs separately from the token's own window.
    pub session_expires_at: SystemTime,
    /// When the token was redeemed, if it has been. A token is single-use, so a
    /// second redemption is refused once this is set.
    pub redeemed_at: Option<SystemTime>,
}

impl EmbedSession {
    /// A session for a URL generated at `created_at`, with both documented
    /// lifetimes measured from that instant.
    pub fn new(
        token: String,
        principal: String,
        dashboard_identifier: String,
        embedding_domains: Vec<String>,
        embed_url: String,
        created_at: SystemTime,
    ) -> Self {
        Self {
            token,
            principal,
            dashboard_identifier,
            embedding_domains,
            embed_url,
            created_at,
            token_expires_at: created_at + TOKEN_VALIDITY,
            session_expires_at: created_at + SESSION_LIFETIME,
            redeemed_at: None,
        }
    }

    /// Whether the token can still be redeemed at `now`: unredeemed, and inside
    /// its 5-minute window.
    pub fn token_is_valid(&self, now: SystemTime) -> bool {
        self.redeemed_at.is_none() && now < self.token_expires_at
    }

    /// Whether the session the URL opens is still live at `now`.
    pub fn session_is_live(&self, now: SystemTime) -> bool {
        now < self.session_expires_at
    }
}

/// Prints neither the bearer token nor the URL that carries it: the token is a
/// credential this service handed its caller, and a `Debug` of the store is the
/// one place it could otherwise reach a log. The framework's own
/// `MintedCredential` elides its secrets the same way.
impl fmt::Debug for EmbedSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmbedSession")
            .field("principal", &self.principal)
            .field("dashboard_identifier", &self.dashboard_identifier)
            .field("embedding_domains", &self.embedding_domains)
            .field("created_at", &self.created_at)
            .field("token_expires_at", &self.token_expires_at)
            .field("session_expires_at", &self.session_expires_at)
            .field("redeemed_at", &self.redeemed_at)
            .finish_non_exhaustive()
    }
}

/// What came of presenting an embed URL's bearer token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Redemption {
    /// The token was valid and is now spent; the session it opened lapses at
    /// `session_expires_at`.
    Granted { session_expires_at: SystemTime },
    /// The token was already redeemed: it is single-use.
    AlreadyRedeemed,
    /// The token was presented by a principal other than the one the session was
    /// generated for.
    NotTheRequester,
    /// The token's 5-minute window had passed.
    Expired,
    /// No session was generated with this token (or its hour has run out).
    Unknown,
}

/// What this service knows about one caller, as `GetBuyerDashboard`'s own
/// availability rule names it: "This API is available only to Amazon Web
/// Services Organization management accounts or delegated administrators
/// registered for the procurement insights
/// (procurement-insights.marketplace.amazonaws.com) feature."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallerAccess {
    /// The caller is the AWS Organizations management account.
    pub is_org_management_account: bool,
    /// The caller is a delegated administrator for the organization.
    pub delegated_admin: bool,
    /// The caller is registered for the procurement insights feature.
    pub procurement_insights_registered: bool,
}

impl CallerAccess {
    /// The registration a caller the store holds no record for is served under: a
    /// plain standalone account, which the operation's availability rule does not
    /// name — so the gate REFUSES it.
    ///
    /// Organization membership and procurement-insights enrolment are held by AWS
    /// Organizations (`EnableAWSServiceAccess` /
    /// `RegisterDelegatedAdministrator` for
    /// `procurement-insights.marketplace.amazonaws.com`), never by this service:
    /// no operation here creates an organization, joins one, or registers a
    /// delegated administrator, so this emulator can hold a grant only as a
    /// record somebody put in
    /// [`ComAmazonawsMarketplacereportingAwsmarketplacereportingState::access`].
    /// An account with no such record is what a fresh AWS account is — in no
    /// organization at all — and an authorization gate with no grant on record
    /// refuses rather than serves.
    pub const fn standalone_account() -> Self {
        Self {
            is_org_management_account: false,
            delegated_admin: false,
            procurement_insights_registered: false,
        }
    }

    /// The organization's management account — the first caller the operation's
    /// availability rule names. Recorded in
    /// [`ComAmazonawsMarketplacereportingAwsmarketplacereportingState::access`]
    /// to serve a caller the API is available to.
    pub const fn management_account() -> Self {
        Self {
            is_org_management_account: true,
            delegated_admin: false,
            procurement_insights_registered: true,
        }
    }

    /// A delegated administrator registered for the procurement insights feature
    /// — the other caller that rule names.
    pub const fn registered_delegated_admin() -> Self {
        Self {
            is_org_management_account: false,
            delegated_admin: true,
            procurement_insights_registered: true,
        }
    }

    /// Whether `GetBuyerDashboard` is available to this caller.
    ///
    /// The management account may call it; a delegated administrator may call it
    /// once registered for the procurement insights feature; anybody else has no
    /// permission to call it at all.
    pub const fn decision(self) -> AccessDecision {
        if self.is_org_management_account {
            AccessDecision::Allowed
        } else if !self.delegated_admin {
            AccessDecision::NoPermission
        } else if self.procurement_insights_registered {
            AccessDecision::Allowed
        } else {
            AccessDecision::InsufficientAccess
        }
    }
}

/// Whether a caller may call `GetBuyerDashboard`, and how it is refused when
/// not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessDecision {
    /// The API is available to this caller.
    Allowed,
    /// The caller is neither the management account nor a delegated
    /// administrator: `UnauthorizedException` (401), "you do not have permission
    /// to perform this action".
    NoPermission,
    /// The caller is a delegated administrator that is not registered for
    /// procurement insights: `AccessDeniedException` (403), "you do not have
    /// sufficient access to perform this action".
    InsufficientAccess,
}

/// Per-service state for com-amazonaws-marketplacereporting-awsmarketplacereporting. A handler reaches it through [`service_state`].
///
/// The dashboards this service publishes are NOT a map here: the catalog is the
/// two static entries of [`DashboardKind::CATALOG`], and a `DashboardIdentifier`
/// resolves to one of them by [`Dashboard::parse`] alone. A per-ARN record would
/// be keyed by a caller-chosen account segment (`[0-9]{12}`), which is an
/// unbounded growth site holding nothing the identifier does not already carry.
#[derive(Debug, Default)]
pub struct ComAmazonawsMarketplacereportingAwsmarketplacereportingState {
    /// Every embed session generated, keyed by the bearer token its URL carries,
    /// so presenting the token finds the session to redeem.
    pub embed_sessions: HashMap<String, EmbedSession>,
    /// What this service knows about a caller, keyed by the principal the
    /// request presented ([`caller_principal`]). A caller with no record is
    /// served under [`CallerAccess::standalone_account`], which the API is not
    /// available to: a record here is what GRANTS access, never what restricts
    /// it.
    pub access: HashMap<String, CallerAccess>,
    /// The counter embed tokens are minted from, so two calls never mint the
    /// same token.
    pub next_token_seq: u64,
}

impl ComAmazonawsMarketplacereportingAwsmarketplacereportingState {
    /// The registration recorded for `principal`, or
    /// [`CallerAccess::standalone_account`] when the store holds none.
    pub fn caller_access(&self, principal: &str) -> CallerAccess {
        self.access
            .get(principal)
            .copied()
            .unwrap_or_else(CallerAccess::standalone_account)
    }

    /// Whether `GetBuyerDashboard` is available to `principal`, from the
    /// registration this store holds for it.
    ///
    /// A pure read of recorded state: the same principal gets the same answer
    /// however often, and from however many threads at once, it is asked.
    pub fn access_decision(&self, principal: &str) -> AccessDecision {
        self.caller_access(principal).decision()
    }

    /// Mint the one-shot bearer token an embed URL carries.
    ///
    /// The counter guarantees two calls never mint the same token (the loop
    /// skips a collision, which is what makes its wrap safe); hashing it with
    /// the generation instant and a per-process random seed is what keeps the
    /// next token from being readable off the last one. This is an embedding
    /// code in a URL, NOT an AWS credential: it must never be minted through
    /// `ctx.credential_store()`, whose credentials are accepted as SigV4
    /// signing material for every operation of every service.
    pub fn mint_token(&mut self, created_at: SystemTime) -> String {
        let since_epoch = created_at
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        loop {
            self.next_token_seq = self.next_token_seq.wrapping_add(1);
            let entropy = RandomState::new().hash_one(self.next_token_seq);
            let mut digest = Sha256::new();
            digest.update(self.next_token_seq.to_be_bytes());
            digest.update(since_epoch.to_be_bytes());
            digest.update(entropy.to_be_bytes());
            let token = hex::encode(digest.finalize());
            if !self.embed_sessions.contains_key(&token) {
                return token;
            }
        }
    }

    /// Persist a generated embed session, and answer with the record as stored —
    /// which is what the response is then built from.
    ///
    /// Holding the store to [`EMBED_SESSION_LIMIT`] here, after the insert, is
    /// what bounds it against a caller generating faster than the clock releases
    /// sessions; the record just taken is the newest, so the trim never drops it
    /// and `None` is unreachable — a store that answers `None` is reported as a
    /// server error rather than panicked on.
    pub fn record_session(&mut self, session: EmbedSession) -> Option<&EmbedSession> {
        let token = session.token.clone();
        match self.embed_sessions.entry(token.clone()) {
            Entry::Occupied(mut held) => {
                held.insert(session);
            }
            Entry::Vacant(free) => {
                free.insert(session);
            }
        }
        self.trim_to_limit();
        self.embed_sessions.get(&token)
    }

    /// Redeem the bearer token an embed URL carries, on behalf of `principal`.
    ///
    /// A token is valid for [`TOKEN_VALIDITY`] and single-use: the first
    /// redemption inside that window, by the principal the session was generated
    /// for, opens the session; every later one — or one past the window, or one
    /// by another principal — is refused.
    pub fn redeem(&mut self, token: &str, principal: &str, now: SystemTime) -> Redemption {
        let Some(session) = self.embed_sessions.get_mut(token) else {
            return Redemption::Unknown;
        };
        if session.principal != principal {
            return Redemption::NotTheRequester;
        }
        if session.redeemed_at.is_some() {
            return Redemption::AlreadyRedeemed;
        }
        if !session.token_is_valid(now) {
            return Redemption::Expired;
        }
        session.redeemed_at = Some(now);
        Redemption::Granted {
            session_expires_at: session.session_expires_at,
        }
    }

    /// Release every session whose hour the clock has run out, and hold the
    /// store to [`EMBED_SESSION_LIMIT`].
    ///
    /// Both are owed at the instant they fall due whether or not anybody calls,
    /// so a later call materialising them changes nothing a caller could
    /// otherwise have observed.
    pub fn forget_lapsed_sessions(&mut self, now: SystemTime) {
        self.embed_sessions
            .retain(|_, session| session.session_is_live(now));
        self.trim_to_limit();
    }

    /// Hold the store to [`EMBED_SESSION_LIMIT`] by dropping the oldest
    /// sessions — the ones closest to lapsing anyway.
    fn trim_to_limit(&mut self) {
        let excess = self.embed_sessions.len().saturating_sub(EMBED_SESSION_LIMIT);
        if excess == 0 {
            return;
        }
        let mut by_age: Vec<(SystemTime, String)> = self
            .embed_sessions
            .iter()
            .map(|(token, session)| (session.created_at, token.clone()))
            .collect();
        by_age.sort();
        for (_, token) in by_age.into_iter().take(excess) {
            self.embed_sessions.remove(&token);
        }
    }
}

/// The URL a generated embed session is reached through.
///
/// `GetBuyerDashboard` generates "an embedding URL for an Amazon QuickSight
/// dashboard", so the URL names the QuickSight embedding host for the region the
/// request was signed for, and carries the session's one-shot bearer token
/// ahead of the dashboard it embeds. The token is this emulator's
/// ([`ComAmazonawsMarketplacereportingAwsmarketplacereportingState::mint_token`]), and so is the path it sits in.
pub fn embed_url(region: &str, dashboard: &Dashboard, token: &str) -> String {
    format!(
        "https://{region}.quicksight.aws.amazon.com/embed/{token}/dashboards/{}",
        dashboard.kind.dashboard_name()
    )
}

/// The bearer token carried by a URL [`embed_url`] built, so a caller holding
/// only the URL can redeem it.
pub fn token_from_embed_url(url: &str) -> Option<&str> {
    let (_, rest) = url.split_once("/embed/")?;
    rest.split('/').next().filter(|token| !token.is_empty())
}

/// True when `value` is the fully qualified form `EmbeddingDomains` documents:
/// "Fully qualified domains that you add to the allow list for access to the
/// generated URL … To include all subdomains under a specific domain, use *.
/// For example, https://*.amazon.com includes all subdomains under
/// https://aws.amazon.com."
///
/// So: a scheme, then either the `localhost` development host that
/// `EmbeddingDomain`'s `@pattern` admits, or a domain with a registrable label
/// under a top-level one, where `*` may stand for the subdomains — a whole
/// leftmost label, and nothing else. The member's `@pattern` is wider than that
/// (it admits `*` in any label, and a domain that is nothing but a wildcard and
/// a top-level label), which is why this is checked beyond the boundary
/// validator.
pub fn is_embedding_domain(value: &str) -> bool {
    let Some(host) = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
    else {
        return false;
    };
    // A path after the host is not part of the domain.
    let host = host.split('/').next().unwrap_or_default();
    if is_localhost(host) {
        return true;
    }
    // Only the localhost form carries a port.
    if host.contains(':') {
        return false;
    }
    let labels: Vec<&str> = host.split('.').collect();
    if labels.iter().any(|label| label.is_empty()) {
        return false;
    }
    let wildcard = labels.first().is_some_and(|label| *label == "*");
    let named = if wildcard { &labels[1..] } else { &labels[..] };
    // The wildcard stands for subdomains only, so no named label carries one
    // and at least two named labels are left under it.
    if named.iter().any(|label| label.contains('*')) || named.len() < 2 {
        return false;
    }
    named
        .last()
        .is_some_and(|tld| tld.len() >= 2 && tld.bytes().all(|byte| byte.is_ascii_alphabetic()))
}

/// True for the `localhost` host `EmbeddingDomain`'s `@pattern` names
/// explicitly (`http[s]*://localhost(:[0-9]{1,5})?`).
fn is_localhost(host: &str) -> bool {
    let (name, port) = host.split_once(':').unwrap_or((host, ""));
    name == "localhost" && port.bytes().all(|byte| byte.is_ascii_digit())
}

/// The region this request was signed for — what an embed URL is built for.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The principal this request presented: the SigV4 access key id, which is the
/// only caller identity a request carries. It is what a `CallerAccess` record is
/// keyed by.
pub fn caller_principal(parts: &::http::request::Parts) -> String {
    ::engine::extract_akid(&parts.headers)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMarketplacereportingAwsmarketplacereportingState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMarketplacereportingAwsmarketplacereportingState>)
}
