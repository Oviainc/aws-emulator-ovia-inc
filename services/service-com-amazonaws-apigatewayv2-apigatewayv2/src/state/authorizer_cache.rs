// SPDX-License-Identifier: CC0-1.0

//! The cached decisions of a stage's Lambda authorizers: what is cached, under which key,
//! for how long, and what takes an entry away again.
//!
//! A `REQUEST` authorizer's decision is the answer of the Lambda function it names, and
//! API Gateway does not call that function for every request: "the TTL (time to live)
//! period, in seconds, that specifies how long API Gateway caches authorizer results. If
//! you specify a value greater than 0, API Gateway caches authorizer responses. The maximum
//! value is 3600, or 1 hour. If you specify 0, API Gateway doesn't cache authorizer
//! responses" (`Authorizer$AuthorizerResultTtlInSeconds`), and the cache is keyed by what
//! the authorizer READS of the request: "API Gateway uses the identity sources as the
//! cache key" (`Authorizer$IdentitySource`). So two requests presenting the SAME
//! identity-source values share one decision while it is live, and a request presenting a
//! different value is a different key and is decided by its own invocation.
//!
//! The entry's life is the clock's: an entry is live until the TTL the authorizer declared
//! has passed since the invocation that produced it, after which the next request invokes
//! the function again. Nothing sleeps for that — the due instant is stored and compared
//! against the reading clock, so a request arriving after it observes the expiry.
//!
//! `ResetAuthorizersCache` ("Resets all authorizer cache entries on a stage. Supported only
//! for HTTP APIs") is what takes every entry of one stage away before its TTL is up, which
//! is why the entries are keyed by the stage they were decided for
//! ([`clear_stage`]).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// What one live cache entry holds: the decision, and the context the function returned
/// with it — both of which the request that re-uses the entry is served by, exactly as the
/// request that produced it was.
#[derive(Debug, Clone)]
pub struct CachedDecision {
    /// Whether the authorizer AUTHORIZED the request.
    pub authorized: bool,
    /// The `context` the function returned beside its decision, as the JSON object the
    /// integration event carries it in. Held as a rendered document rather than a parsed
    /// map, because what the event carries is the function's own object, whatever types
    /// its members hold.
    pub context: Option<String>,
    /// The principal the function identified, for a policy-document response
    /// (`principalId`). `None` for a simple response, which names no principal.
    pub principal_id: Option<String>,
}

/// One cached decision with the instant it stops being usable.
#[derive(Debug, Clone)]
struct Entry {
    decision: CachedDecision,
    /// The first instant at which this entry is no longer live: the invocation instant plus
    /// the authorizer's own TTL.
    expires_at: Instant,
}

/// Which decision one lookup or one write is about: the stage it is decided for, the
/// authorizer that decided it, and the identity-source values it was decided from.
///
/// The four travel together because they are one key — an authorizer is a child of an api, a
/// cache is reset per STAGE, and "API Gateway uses the identity sources as the cache key" —
/// so no caller can pass three of them and leave the fourth to a default.
#[derive(Debug, Clone, Copy)]
pub struct Decided<'a> {
    pub api_id: &'a str,
    pub stage_name: &'a str,
    pub authorizer_id: &'a str,
    pub identity: &'a [String],
}

/// Every live decision of every authorizer of this account, keyed by the stage it was
/// decided for and the identity it was decided about.
///
/// The key is `(ApiId, StageName, AuthorizerId, identity)`, where the identity is the
/// identity-source values joined by a separator no identity-source value can contain.
#[derive(Debug, Default)]
pub struct AuthorizerCache {
    entries: BTreeMap<(String, String, String, String), Entry>,
}

/// The byte that joins the identity-source values of one cache key. A header or query value
/// can hold any visible character, so the values are joined by a control character no HTTP
/// field value may carry (RFC 9110 §5.5), which keeps two different value lists from
/// colliding on one key.
const IDENTITY_SEPARATOR: char = '\u{1}';

/// The TTL an authorizer that declares none caches a decision for: "The default value is
/// 300" (`CreateAuthorizerRequest$AuthorizerResultTtlInSeconds`).
pub const DEFAULT_RESULT_TTL_SECONDS: i32 = 300;

/// How many decisions this store holds at once, across every api and stage.
///
/// A cache key carries an identity-source VALUE the caller chose, so a caller presenting a
/// fresh value per request would otherwise grow this map for as long as the TTLs last. The
/// bound is well above any real working set — a stage caches one entry per distinct identity
/// in flight — so it never displaces a decision a live caller would re-use, and it caps what
/// the map can cost whatever a caller sends.
const MAX_ENTRIES: usize = 4_096;

impl AuthorizerCache {
    /// The cache key for one decision.
    fn key(decided: &Decided<'_>) -> (String, String, String, String) {
        (
            decided.api_id.to_string(),
            decided.stage_name.to_string(),
            decided.authorizer_id.to_string(),
            decided.identity.join(&IDENTITY_SEPARATOR.to_string()),
        )
    }

    /// The live decision for this identity, or `None` when none was cached or the one
    /// cached has expired.
    ///
    /// An expired entry is REMOVED as it is read: the clock already made it unusable, so
    /// taking it out is materialising what the clock owed rather than a decision of this
    /// read.
    pub fn live(&mut self, decided: Decided<'_>, now: Instant) -> Option<CachedDecision> {
        let key = Self::key(&decided);
        let entry = self.entries.get(&key)?;
        if now < entry.expires_at {
            return Some(entry.decision.clone());
        }
        self.entries.remove(&key);
        None
    }

    /// Cache `decision` for `ttl`, as the authorizer's own TTL declares. A TTL of no time at
    /// all caches nothing — "If you specify 0, API Gateway doesn't cache authorizer
    /// responses" — so the next request invokes the function again.
    pub fn store(
        &mut self,
        decided: Decided<'_>,
        decision: CachedDecision,
        ttl: Duration,
        now: Instant,
    ) {
        if ttl.is_zero() {
            return;
        }
        self.make_room(now);
        self.entries.insert(
            Self::key(&decided),
            Entry {
                decision,
                expires_at: now + ttl,
            },
        );
    }

    /// Keep this store within [`MAX_ENTRIES`]: the entries the clock already expired go
    /// first, and if that leaves no room the one that expires SOONEST is dropped — the entry
    /// with the least life left to lose.
    fn make_room(&mut self, now: Instant) {
        if self.entries.len() < MAX_ENTRIES {
            return;
        }
        self.entries.retain(|_, entry| now < entry.expires_at);
        while self.entries.len() >= MAX_ENTRIES {
            let Some(soonest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.expires_at)
                .map(|(key, _)| key.clone())
            else {
                return;
            };
            self.entries.remove(&soonest);
        }
    }

    /// Take away every entry decided for one stage — what `ResetAuthorizersCache` performs.
    /// Answers how many entries were removed, so the caller can report what the reset did.
    pub fn clear_stage(&mut self, api_id: &str, stage_name: &str) -> usize {
        let before = self.entries.len();
        self.entries
            .retain(|(entry_api, entry_stage, _, _), _| {
                entry_api != api_id || entry_stage != stage_name
            });
        before - self.entries.len()
    }

    /// Take away every entry of one api, for an api that is no longer there: the
    /// authorizers the entries were decided by are gone with it.
    pub fn clear_api(&mut self, api_id: &str) {
        self.entries.retain(|(entry_api, _, _, _), _| entry_api != api_id);
    }

    /// How many entries are held, live or expired. For diagnostics and for the tests that
    /// pin what a reset removed.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is cached at all.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The TTL an authorizer caches a decision for, from its own
/// `AuthorizerResultTtlInSeconds` — the documented default when it declares none, and no
/// time at all for a value that is zero or negative, which caches nothing.
pub fn result_ttl(authorizer_result_ttl_in_seconds: Option<i32>) -> Duration {
    let seconds = authorizer_result_ttl_in_seconds.unwrap_or(DEFAULT_RESULT_TTL_SECONDS);
    Duration::from_secs(u64::try_from(seconds.max(0)).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A decision that authorized the request, carrying no context.
    fn allowed() -> CachedDecision {
        CachedDecision {
            authorized: true,
            context: None,
            principal_id: None,
        }
    }

    /// The key one row asks about.
    fn decided<'a>(
        api_id: &'a str,
        stage_name: &'a str,
        authorizer_id: &'a str,
        identity: &'a [String],
    ) -> Decided<'a> {
        Decided {
            api_id,
            stage_name,
            authorizer_id,
            identity,
        }
    }

    #[test]
    fn a_decision_is_cached_per_identity_and_only_while_it_is_live() {
        let ttl = Duration::from_secs(300);
        let start = Instant::now();
        let mut cache = AuthorizerCache::default();
        let one = ["one".to_string()];
        let two = ["two".to_string()];
        cache.store(
            decided("api", "$default", "auth", &one),
            allowed(),
            ttl,
            start,
        );
        // Rows differ along what is asked for: the identity that was cached, another
        // identity, another authorizer, another stage, and the same identity after the TTL.
        let cases: [(&str, &str, &str, &str, Duration, bool); 5] = [
            ("the identity the decision was made for", "api", "$default", "auth", Duration::ZERO, true),
            ("another identity source value", "api", "$default", "auth", Duration::ZERO, false),
            ("another authorizer of the same stage", "api", "$default", "other", Duration::ZERO, false),
            ("another stage of the same api", "api", "prod", "auth", Duration::ZERO, false),
            ("the same identity past the TTL", "api", "$default", "auth", ttl, false),
        ];
        for (case, api, stage, authorizer, elapsed, expected) in cases {
            let identity: &[String] = if case.contains("another identity") {
                &two
            } else {
                &one
            };
            let actual = cache
                .live(decided(api, stage, authorizer, identity), start + elapsed)
                .is_some();
            assert_eq!(
                actual, expected,
                "case {case:?}: expected a live entry: {expected}, got {actual}"
            );
        }
    }

    #[test]
    fn resetting_one_stage_leaves_every_other_stage_cached() {
        let ttl = Duration::from_secs(300);
        let now = Instant::now();
        let mut cache = AuthorizerCache::default();
        let identity = ["token".to_string()];
        for (api, stage) in [("api", "$default"), ("api", "prod"), ("other", "$default")] {
            cache.store(decided(api, stage, "auth", &identity), allowed(), ttl, now);
        }
        let removed = cache.clear_stage("api", "$default");
        assert_eq!(removed, 1, "expected one entry removed, got {removed}");
        for (case, api, stage, expected) in [
            ("the stage that was reset", "api", "$default", false),
            ("another stage of the same api", "api", "prod", true),
            ("the same stage name of another api", "other", "$default", true),
        ] {
            let actual = cache
                .live(decided(api, stage, "auth", &identity), now)
                .is_some();
            assert_eq!(
                actual, expected,
                "case {case:?}: expected a live entry: {expected}, got {actual}"
            );
        }
    }

    #[test]
    fn the_store_never_grows_past_its_bound() {
        // A caller presenting a fresh identity per request would otherwise grow the store
        // for as long as the TTLs last, so the bound holds whatever is cached.
        let now = Instant::now();
        let mut cache = AuthorizerCache::default();
        for identity in 0..(MAX_ENTRIES * 2) {
            let identity = [format!("identity-{identity}")];
            cache.store(
                decided("api", "$default", "auth", &identity),
                allowed(),
                Duration::from_secs(3600),
                now,
            );
            assert!(
                cache.len() <= MAX_ENTRIES,
                "case {:?}: expected at most {MAX_ENTRIES} entries, got {}",
                "one fresh identity per request",
                cache.len()
            );
        }
        // The entry stored last is the one a caller is re-using, and it is live.
        let last = [format!("identity-{}", MAX_ENTRIES * 2 - 1)];
        assert!(
            cache
                .live(decided("api", "$default", "auth", &last), now)
                .is_some(),
            "case {:?}: expected the decision just cached to be live",
            "the identity cached last"
        );
    }

    #[test]
    fn a_ttl_of_no_time_caches_nothing_and_an_absent_one_is_the_documented_default() {
        let now = Instant::now();
        let mut cache = AuthorizerCache::default();
        let identity = ["token".to_string()];
        cache.store(
            decided("api", "$default", "auth", &identity),
            allowed(),
            Duration::ZERO,
            now,
        );
        assert!(
            cache.is_empty(),
            "case {:?}: expected nothing cached, got {} entries",
            "a TTL of zero seconds",
            cache.len()
        );
        // Rows differ along the declared TTL: absent (the documented default), zero,
        // negative (no time at all), and a declared value.
        let cases: [(&str, Option<i32>, u64); 4] = [
            ("an authorizer declaring no TTL", None, 300),
            ("an authorizer declaring no caching", Some(0), 0),
            ("a value below the member's range", Some(-5), 0),
            ("an authorizer declaring an hour", Some(3600), 3600),
        ];
        for (case, declared, expected) in cases {
            let actual = result_ttl(declared);
            assert_eq!(
                actual,
                Duration::from_secs(expected),
                "case {case:?}: expected {expected}s, got {actual:?}"
            );
        }
    }
}
