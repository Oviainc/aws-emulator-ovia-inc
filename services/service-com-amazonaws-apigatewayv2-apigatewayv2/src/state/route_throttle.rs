// SPDX-License-Identifier: CC0-1.0

//! The throttle a stage's route settings put in front of a route: what is metered, at what
//! rate, and when a request is refused for it.
//!
//! `RouteSettings` carries two members, and both are the token bucket API Gateway meters a
//! route with: "ThrottlingBurstLimit — specifies the throttling burst limit" is how many
//! requests may arrive at once, and "ThrottlingRateLimit — specifies the throttling rate
//! limit" is how many per second may arrive on average
//! (`smithy_query(query="shape", shape="RouteSettings")`). A request the bucket has no
//! capacity for is the one the gateway refuses itself, before the route's integration is
//! reached at all, so a route whose settings throttle it and its twin on the same stage
//! answer differently for the same api.
//!
//! The ARITHMETIC of the bucket is not written here: it is the tree's one token bucket
//! ([`::service_common::token_bucket`]), so this service and every other that meters a
//! published rate-and-burst pair cannot disagree about whether a request is admitted. What
//! this module owns is what the MODEL says — which members declare the pair, what an absent
//! one means, and that the thing metered is one route of one stage.
//!
//! The buckets are DATA-PLANE state: how much of each route's capacity the requests already
//! served have taken. A route the stage states NEITHER limit for is not metered at all: the
//! stage configured no throttle for it, and metering it against a limit nothing declared
//! would refuse requests no setting asked to be refused.

use std::collections::BTreeMap;
use std::time::Instant;

use ::service_common::token_bucket::{Bucket, Limits as BucketLimits};

/// The requests per second a route is metered at when its settings name a burst limit but no
/// rate: the account-level rate quota API Gateway publishes for a Region, which is the
/// throttle in force when nothing narrower is configured.
const ACCOUNT_RATE_LIMIT: f64 = 10_000.0;

/// The burst a route is metered with when its settings name a rate limit but no burst: the
/// account-level burst quota API Gateway publishes, for the same reason.
const ACCOUNT_BURST_LIMIT: f64 = 5_000.0;

/// The throttle one route is metered with, as the bucket reads it.
pub type Limits = BucketLimits;

/// The limits the stage's settings for one route state, or `None` for a route the stage
/// states neither limit for — which is not metered.
///
/// A member the settings leave out is the account-level quota that applies when nothing
/// narrower is configured, so a setting that names only one of the two still meters the
/// route by the one it names. A negative value names no capacity at all and is read as none,
/// the way a zero is.
pub fn limits_of(rate: Option<f64>, burst: Option<i32>) -> Option<Limits> {
    if rate.is_none() && burst.is_none() {
        return None;
    }
    Some(Limits {
        rate: rate.filter(|rate| rate.is_finite()).unwrap_or(ACCOUNT_RATE_LIMIT).max(0.0),
        burst: burst
            .map(|burst| f64::from(burst).max(0.0))
            .unwrap_or(ACCOUNT_BURST_LIMIT),
    })
}

/// The buckets of every metered route of this account, keyed by the api, the stage and the
/// route key the settings were stated for.
#[derive(Debug, Default)]
pub struct RouteThrottles {
    buckets: BTreeMap<(String, String, String), Bucket>,
}

impl RouteThrottles {
    /// Whether the route's throttle ADMITS this request, taking its capacity for it when it
    /// does.
    pub fn admits(
        &mut self,
        api_id: &str,
        stage_name: &str,
        route_key: &str,
        limits: Limits,
        now: Instant,
    ) -> bool {
        let key = (
            api_id.to_string(),
            stage_name.to_string(),
            route_key.to_string(),
        );
        self.buckets
            .entry(key)
            .or_insert_with(|| Bucket::full(limits, now))
            .admits(limits, now)
    }

    /// Forget every bucket of one api, for an api that is no longer there.
    pub fn clear_api(&mut self, api_id: &str) {
        self.buckets.retain(|(held, _, _), _| held != api_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// One row: what a failing assertion calls it, the two members the settings state, and
    /// the `(rate, burst)` the route is then metered at.
    type Row = (&'static str, Option<f64>, Option<i32>, Option<(f64, f64)>);

    #[test]
    fn a_route_is_metered_only_where_its_settings_state_a_limit() {
        // Rows differ along which of the two members the settings state: neither (not
        // metered), each alone (the other is the account quota), and both.
        let cases: [Row; 5] = [
            ("settings naming neither limit", None, None, None),
            (
                "a rate limit alone",
                Some(10.0),
                None,
                Some((10.0, ACCOUNT_BURST_LIMIT)),
            ),
            (
                "a burst limit alone",
                None,
                Some(7),
                Some((ACCOUNT_RATE_LIMIT, 7.0)),
            ),
            ("both limits", Some(2.0), Some(1), Some((2.0, 1.0))),
            ("limits that admit nothing", Some(0.0), Some(0), Some((0.0, 0.0))),
        ];
        for (case, rate, burst, expected) in cases {
            let actual = limits_of(rate, burst);
            let expected = expected.map(|(rate, burst)| Limits { rate, burst });
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn each_route_is_metered_against_its_own_settings() {
        let start = Instant::now();
        let mut throttles = RouteThrottles::default();
        let limits = Limits {
            rate: 2.0,
            burst: 2.0,
        };
        // Rows differ along WHEN the request to the metered route arrives: inside the burst,
        // past it, and after enough time for the rate to refill one request's capacity.
        let cases: [(&str, Duration, bool); 5] = [
            ("the first request of the burst", Duration::ZERO, true),
            ("the second request of the burst", Duration::ZERO, true),
            ("one past the burst", Duration::ZERO, false),
            ("still past it a moment later", Duration::from_millis(100), false),
            ("once the rate refilled a request", Duration::from_millis(600), true),
        ];
        for (case, elapsed, expected) in cases {
            let actual =
                throttles.admits("api", "$default", "GET /orders", limits, start + elapsed);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected admitted {expected}, got {actual}"
            );
        }
        // Another route's bucket is its own: one route's throttle never refuses a twin, and
        // neither does the same route key on another stage or another api.
        for (case, api, stage, route_key) in [
            ("the twin route of a throttled one", "api", "$default", "GET /twin"),
            ("the same route key on another stage", "api", "prod", "GET /orders"),
            ("the same route key of another api", "other", "$default", "GET /orders"),
        ] {
            assert!(
                throttles.admits(api, stage, route_key, limits, start),
                "case {case:?}: a route is metered against its own settings only"
            );
        }
        // And a bucket an api no longer has is forgotten with it.
        throttles.clear_api("api");
        assert!(
            throttles.admits("api", "$default", "GET /orders", limits, start),
            "case {:?}: an api that is gone leaves no bucket behind",
            "a metered route of a deleted api"
        );
    }
}
