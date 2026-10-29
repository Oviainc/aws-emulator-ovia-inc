// SPDX-License-Identifier: CC0-1.0

//! A rate-based rule RECOVERS: what the edge decides once a caller's requests have
//! fallen out of the rule's evaluation window.
//!
//! The rule this suite pins: a rate-based rule counts a caller's requests per
//! aggregation instance and applies its action while that instance's count over the
//! `EvaluationWindowSec` immediately preceding the request is over the `Limit` — so the
//! window is a SLIDING one, and a caller whose counted requests all lie further back
//! than the window is under the limit again and is served. The same arithmetic decides
//! what `GetRateBasedStatementManagedKeys` lists, so a key stops being reported at the
//! instant the rule stops acting on it.
//!
//! The rows differ along HOW FAR BACK the caller's requests lie: inside the window at
//! both ends of it and past it. Time is not waited for — the stored per-second counts
//! are moved back, which is exactly what the passage of time does to them — so the
//! window itself is the discriminator rather than the suite's runtime.
//!
//! The requests are driven through the data plane's own entry point
//! ([`service_com_amazonaws_wafv2_awswaf_20190729::service::edge::decide`]) on the SAME
//! shared engine the service is mounted on, and the managed keys are read over the
//! wire.

use serde_json::json;
use service_com_amazonaws_wafv2_awswaf_20190729::service::edge::{self, InspectedRequest};
use service_com_amazonaws_wafv2_awswaf_20190729::state::ComAmazonawsWafv2Awswaf20190729State;

use crate::web_acl_lifecycle::{visibility, Waf, TEST_REGION};

/// The `Scope` the web ACL lives in: an API Gateway stage is a REGIONAL protected
/// resource.
const SCOPE: &str = "REGIONAL";

/// `RateBasedStatement$Limit` is modelled `range(min 10)`, so this is the smallest
/// limit a rule can carry.
const LIMIT: i64 = 10;

/// The window the rule looks back over. "Valid settings are 60, 120, 300, and 600"
/// (`RateBasedStatement$EvaluationWindowSec`), so this is the shortest one a rule can
/// name.
const WINDOW_SEC: i64 = 60;

/// The web ACL's name, which is also the `WebACLName` the managed-keys report takes.
const WEB_ACL: &str = "rate-recovery";

/// The rate-based rule's name, which is also its metric dimension.
const RULE: &str = "by-ip";

/// The one caller every request below comes from, so one aggregation instance holds
/// them all.
const CALLER: &str = "203.0.113.7";

/// The protected resource the web ACL is put in front of: an API Gateway REST API
/// stage, the one ARN form `AssociateWebACLRequest$ResourceArn` documents for one.
fn stage_arn() -> String {
    format!("arn:aws:apigateway:{TEST_REGION}::/restapis/a1b2c3d4e5/stages/prod")
}

/// One request from [`CALLER`], as the edge inspects it.
fn request() -> InspectedRequest {
    InspectedRequest {
        method: "GET".to_string(),
        uri_path: "/prod/orders".to_string(),
        query_string: String::new(),
        headers: vec![("user-agent".to_string(), "rate-recovery".to_string())],
        body: Vec::new(),
        client_ip: CALLER.to_string(),
    }
}

/// Move every recorded per-second count `seconds` further into the past — what the
/// passage of that much time does to the window they are read through.
fn age_counts(
    state: &::engine::ServiceState<ComAmazonawsWafv2Awswaf20190729State>,
    seconds: i64,
) {
    let mut store = state.lock();
    for buckets in store.rate_based_counts.values_mut() {
        for bucket in buckets.iter_mut() {
            bucket.second -= seconds;
        }
    }
}

#[tokio::test]
async fn a_rate_based_rule_serves_the_caller_again_once_its_window_has_passed() {
    let (waf, engine) = Waf::sharing_engine();
    let created = waf
        .ok(
            "CreateWebACL",
            json!({
                "Name": WEB_ACL,
                "Scope": SCOPE,
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility(WEB_ACL),
                "Rules": [{
                    "Name": RULE,
                    "Priority": 0,
                    "Statement": {
                        "RateBasedStatement": {
                            "Limit": LIMIT,
                            "AggregateKeyType": "IP",
                            "EvaluationWindowSec": WINDOW_SEC,
                        }
                    },
                    "Action": { "Block": {} },
                    "VisibilityConfig": visibility(RULE),
                }],
            }),
        )
        .await;
    let web_acl_id = created["Summary"]["Id"]
        .as_str()
        .expect("Summary.Id")
        .to_string();
    let web_acl_arn = created["Summary"]["ARN"]
        .as_str()
        .expect("Summary.ARN")
        .to_string();
    waf.ok(
        "AssociateWebACL",
        json!({ "WebACLArn": web_acl_arn, "ResourceArn": stage_arn() }),
    )
    .await;

    let state = service_com_amazonaws_wafv2_awswaf_20190729::state::service_state(&*engine)
        .expect("the wafv2 store is reachable from the shared engine");

    /// Drive one request and answer whether the web ACL let it through.
    fn allowed(
        state: &::engine::ServiceState<ComAmazonawsWafv2Awswaf20190729State>,
        case: &str,
    ) -> bool {
        edge::decide(state, &stage_arn(), &request())
            .unwrap_or_else(|| panic!("case {case:?}: the associated web acl must decide"))
            .action
            .is_allow()
    }

    // Take the caller over the limit: every request up to it is served, and the one
    // past it is not.
    for sent in 1..=LIMIT {
        assert!(
            allowed(&state, "a request at or under the limit"),
            "case a request at or under the limit: request {sent} of {LIMIT} must be \
             served, it is not over the limit yet"
        );
    }
    assert!(
        !allowed(&state, "the request that takes the caller over the limit"),
        "case the request that takes the caller over the limit: expected the rule to \
         act on it"
    );

    /// How far back the caller's counted requests are moved, and what the rule then
    /// makes of one more request from the same caller.
    struct Case {
        case: &'static str,
        /// Seconds the recorded counts are moved further into the past.
        age_by: i64,
        /// Whether the next request is served.
        served: bool,
        /// Whether the caller's key is still among the ones over the limit.
        listed: bool,
    }

    let cases = [
        Case {
            case: "the counted requests are still where they were: inside the window",
            age_by: 0,
            served: false,
            listed: true,
        },
        Case {
            case: "the counted requests are inside the window by one second",
            age_by: WINDOW_SEC - 1,
            served: false,
            listed: true,
        },
        Case {
            case: "the counted requests lie one second past the window",
            age_by: WINDOW_SEC + 1,
            served: true,
            listed: false,
        },
        Case {
            case: "the counted requests lie a whole window further back still, so a \
                   recovered caller stays recovered",
            age_by: 2 * WINDOW_SEC,
            served: true,
            listed: false,
        },
    ];

    for case in &cases {
        age_counts(&state, case.age_by);

        let keys = waf
            .ok(
                "GetRateBasedStatementManagedKeys",
                json!({
                    "Scope": SCOPE,
                    "WebACLName": WEB_ACL,
                    "WebACLId": web_acl_id,
                    "RuleName": RULE,
                }),
            )
            .await;
        let listed: Vec<String> = keys["ManagedKeysIPV4"]["Addresses"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|address| address.as_str().map(str::to_string))
            .collect();
        let reports_caller = listed.iter().any(|address| address.starts_with(CALLER));
        assert_eq!(
            reports_caller, case.listed,
            "case {:?}: expected the caller among the keys over the limit = {}, got \
             {listed:?}",
            case.case, case.listed
        );

        let served = allowed(&state, case.case);
        assert_eq!(
            served, case.served,
            "case {:?}: expected the request to be served = {}, got served = {served}",
            case.case, case.served
        );
    }
}
