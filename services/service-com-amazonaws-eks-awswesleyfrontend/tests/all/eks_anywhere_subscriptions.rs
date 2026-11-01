// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the EKS Anywhere subscription slice:
//! `CreateEksAnywhereSubscription` and `DescribeEksAnywhereSubscription`, driven
//! over the wire through the same `MultiServiceRouter` the production binary mounts.
//!
//! Every suite asserts a RULE over a TABLE of inputs rather than one recorded
//! request: the rows differ along the term duration and unit, the license count, the
//! auto-renew flag, which optional members travel with the subscription, and which
//! documented value a bad request misses. Each assertion names its case and prints
//! the actual value.
//!
//! The wire harness (`Answer`, `emulator`, `call`) is the shared one in
//! `tests/all/wire.rs`.

use std::time::Duration;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    MAX_SUBSCRIPTION_LICENSE_QUANTITY, MIN_SUBSCRIPTION_LICENSE_QUANTITY,
    SUBSCRIPTION_ACTIVATION_DELAY, SUBSCRIPTION_LICENSE_TYPE_CLUSTER, SUBSCRIPTION_MONTH,
    SUBSCRIPTION_NOT_FOUND_MESSAGE, SUBSCRIPTION_TERM_DURATIONS, SUBSCRIPTION_TERM_UNIT_MONTHS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, Answer};

const REGION: &str = "us-west-2";

/// A little longer than the span the clock owes a transition, so a read after it is
/// certainly past the due instant.
fn past(delay: Duration) -> Duration {
    delay + Duration::from_millis(250)
}

async fn create_subscription(router: &MultiServiceRouter, body: &serde_json::Value) -> Answer {
    call(
        router,
        REGION,
        "POST",
        "/eks-anywhere-subscriptions",
        &body.to_string(),
    )
    .await
}

async fn describe_subscription(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!("/eks-anywhere-subscriptions/{id}"),
        "",
    )
    .await
}

/// The `subscription` document an answer carries.
fn subscription(answer: &Answer) -> &serde_json::Value {
    &answer.body["subscription"]
}

// ---------------------------------------------------------------------------
// RULE: a created subscription reads back as the record that was written — every
// member the request supplied, plus the server-generated id, ARN and dates —
// reporting `CREATING` until the clock owes it `ACTIVE`.
// ---------------------------------------------------------------------------

/// The table spans what the rule generalises over: both published term durations
/// (`SUBSCRIPTION_TERM_DURATIONS`, from `EksAnywhereSubscriptionTerm$duration`), the
/// license count at each published bound, the auto-renew flag and whether tags and a
/// license type travel with the request.
#[tokio::test]
async fn a_created_subscription_reads_back_exactly_as_stored() {
    let router = emulator();

    let cases: Vec<(&str, serde_json::Value)> = vec![
        (
            "the shortest published term, one license, no optional members",
            serde_json::json!({
                "name": "twelve-months",
                "term": { "duration": 12, "unit": "MONTHS" },
                "licenseQuantity": MIN_SUBSCRIPTION_LICENSE_QUANTITY,
            }),
        ),
        (
            "the longest published term, the maximum licenses, auto renewing, tagged",
            serde_json::json!({
                "name": "Thirty_Six-months",
                "term": { "duration": 36, "unit": "MONTHS" },
                "licenseQuantity": MAX_SUBSCRIPTION_LICENSE_QUANTITY,
                "licenseType": SUBSCRIPTION_LICENSE_TYPE_CLUSTER,
                "autoRenew": true,
                "tags": { "env": "prod", "team": "platform" },
                "clientRequestToken": "token-thirty-six",
            }),
        ),
        (
            "a mid-range license count with a token and no tags",
            serde_json::json!({
                "name": "fifty",
                "term": { "duration": 12, "unit": "MONTHS" },
                "licenseQuantity": 50,
                "clientRequestToken": "token-fifty",
            }),
        ),
    ];

    for (case, request) in &cases {
        let created = create_subscription(&router, request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateEksAnywhereSubscription, got {created:?}"
        );

        let id = subscription(&created)["id"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        assert!(
            !id.is_empty(),
            "case {case:?}: the answer must carry the id the create minted, got {created:?}"
        );
        assert_eq!(
            subscription(&created)["status"],
            serde_json::json!("CREATING"),
            "case {case:?}: expected status CREATING, got {:?}",
            subscription(&created)["status"]
        );
        assert_eq!(
            subscription(&created)["term"], request["term"],
            "case {case:?}: expected term {:?}, got {:?}",
            request["term"],
            subscription(&created)["term"]
        );
        assert_eq!(
            subscription(&created)["licenseQuantity"], request["licenseQuantity"],
            "case {case:?}: expected licenseQuantity {:?}, got {:?}",
            request["licenseQuantity"],
            subscription(&created)["licenseQuantity"]
        );
        // "Valid value is CLUSTER": a request that names no license type takes the one
        // type the API defines.
        assert_eq!(
            subscription(&created)["licenseType"],
            serde_json::json!(SUBSCRIPTION_LICENSE_TYPE_CLUSTER),
            "case {case:?}: expected licenseType {SUBSCRIPTION_LICENSE_TYPE_CLUSTER}, got {:?}",
            subscription(&created)["licenseType"]
        );
        let expected_auto_renew = request
            .get("autoRenew")
            .cloned()
            .unwrap_or(serde_json::Value::Bool(false));
        assert_eq!(
            subscription(&created)["autoRenew"], expected_auto_renew,
            "case {case:?}: expected autoRenew {expected_auto_renew:?}, got {:?}",
            subscription(&created)["autoRenew"]
        );
        match request.get("tags") {
            Some(tags) => assert_eq!(
                subscription(&created)["tags"], *tags,
                "case {case:?}: expected tags {tags:?}, got {:?}",
                subscription(&created)["tags"]
            ),
            // `TagMap` is bounded 1..50, so an untagged subscription reports no map.
            None => assert!(
                subscription(&created)["tags"].is_null(),
                "case {case:?}: an untagged subscription reports no tags, got {:?}",
                subscription(&created)["tags"]
            ),
        }
        assert!(
            subscription(&created)["arn"]
                .as_str()
                .is_some_and(|arn| arn.contains(&id) && arn.contains("eks")),
            "case {case:?}: expected an eks ARN carrying the id, got {:?}",
            subscription(&created)["arn"]
        );
        // "The Unix timestamp in seconds for when the subscription will expire": the
        // effective date plus the term the request asked for.
        let effective = subscription(&created)["effectiveDate"]
            .as_f64()
            .unwrap_or_default();
        let expiration = subscription(&created)["expirationDate"]
            .as_f64()
            .unwrap_or_default();
        let months = request["term"]["duration"].as_f64().unwrap_or_default();
        let expected_span = SUBSCRIPTION_MONTH.as_secs_f64() * months;
        assert!(
            (expiration - effective - expected_span).abs() < 1.0,
            "case {case:?}: expected the term to span {expected_span} seconds, got {}",
            expiration - effective
        );

        // The round trip: the read reports the record the create wrote.
        let described = describe_subscription(&router, &id).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected 200 from DescribeEksAnywhereSubscription, got {described:?}"
        );
        for member in [
            "id",
            "arn",
            "createdAt",
            "effectiveDate",
            "expirationDate",
            "licenseQuantity",
            "licenseType",
            "term",
            "autoRenew",
            "tags",
        ] {
            assert_eq!(
                subscription(&described)[member], subscription(&created)[member],
                "case {case:?}: expected {member} {:?} on the read, got {:?}",
                subscription(&created)[member],
                subscription(&described)[member]
            );
        }
    }

    // The one member a read may legitimately differ on is `status`: the clock owes
    // every subscription above its move out of `CREATING`.
    tokio::time::sleep(past(SUBSCRIPTION_ACTIVATION_DELAY)).await;
    for case in &cases {
        let created = create_subscription(
            &router,
            &serde_json::json!({
                "name": format!("{}-again", case.1["name"].as_str().unwrap_or("x")),
                "term": case.1["term"],
                "licenseQuantity": 1,
            }),
        )
        .await;
        let id = subscription(&created)["id"].as_str().unwrap_or_default();
        tokio::time::sleep(past(SUBSCRIPTION_ACTIVATION_DELAY)).await;
        let described = describe_subscription(&router, id).await;
        assert_eq!(
            subscription(&described)["status"],
            serde_json::json!("ACTIVE"),
            "case {:?}: expected status ACTIVE once the clock owes it, got {described:?}",
            case.0
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a term or a license count outside the published values is refused as a bad
// parameter, and no subscription is created.
// ---------------------------------------------------------------------------

/// The table spans HOW a request misses the published values: the duration (below,
/// between and above the two published ones, and the `@default` of zero), the unit
/// (absent), and the license count at and past each published bound.
///
/// Source for the accepted values: `EksAnywhereSubscriptionTerm$duration` ("valid
/// values are 12 and 36"), `$unit` ("valid value is MONTHS") and
/// `CreateEksAnywhereSubscriptionRequest$licenseQuantity` ("valid values are between
/// 1 and 100"), read through the constants the state slice states them in.
#[tokio::test]
async fn a_term_or_license_count_outside_the_published_values_is_refused() {
    let router = emulator();

    let mut cases: Vec<(String, serde_json::Value)> = vec![
        (
            "a duration below both published ones".to_string(),
            serde_json::json!({ "duration": 5, "unit": "MONTHS" }),
        ),
        (
            "a duration between the two published ones".to_string(),
            serde_json::json!({ "duration": 24, "unit": "MONTHS" }),
        ),
        (
            "a duration above both published ones".to_string(),
            serde_json::json!({ "duration": 48, "unit": "MONTHS" }),
        ),
        (
            "the member's own default duration of zero".to_string(),
            serde_json::json!({ "unit": "MONTHS" }),
        ),
        (
            "a published duration with no unit at all".to_string(),
            serde_json::json!({ "duration": 12 }),
        ),
    ];
    // Each published duration one short and one long is refused; each published
    // duration itself is accepted by the suite above.
    for duration in SUBSCRIPTION_TERM_DURATIONS {
        for offset in [-1, 1] {
            cases.push((
                format!("one month off the published duration {duration}"),
                serde_json::json!({ "duration": duration + offset, "unit": SUBSCRIPTION_TERM_UNIT_MONTHS }),
            ));
        }
    }

    for (index, (case, term)) in cases.iter().enumerate() {
        let name = format!("refused-term-{index}");
        let answer = create_subscription(
            &router,
            &serde_json::json!({
                "name": name,
                "term": term,
                "licenseQuantity": 1,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        let code = error_code(&answer);
        assert_eq!(
            code, "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {code:?}"
        );
    }

    // The license count, at and past each published bound.
    let license_cases: Vec<(&str, i64, bool)> = vec![
        ("the published minimum", MIN_SUBSCRIPTION_LICENSE_QUANTITY as i64, true),
        ("one below the published minimum", MIN_SUBSCRIPTION_LICENSE_QUANTITY as i64 - 1, false),
        ("far below it, a negative count", -7, false),
        ("the published maximum", MAX_SUBSCRIPTION_LICENSE_QUANTITY as i64, true),
        ("one above the published maximum", MAX_SUBSCRIPTION_LICENSE_QUANTITY as i64 + 1, false),
    ];
    for (index, (case, quantity, accepted)) in license_cases.iter().enumerate() {
        let name = format!("license-count-{index}");
        let answer = create_subscription(
            &router,
            &serde_json::json!({
                "name": name,
                "term": { "duration": 12, "unit": "MONTHS" },
                "licenseQuantity": quantity,
            }),
        )
        .await;
        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {answer:?}"
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400, got {answer:?}"
            );
            let code = error_code(&answer);
            assert_eq!(
                code, "InvalidParameterException",
                "case {case:?}: expected InvalidParameterException, got {code:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: the name identifies one subscription per Region, a retry of the create is
// absorbed, and an id no subscription carries is a not-found.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_name_identifies_one_subscription_and_an_unknown_id_is_not_found() {
    let router = emulator();
    let request = serde_json::json!({
        "name": "unique-name",
        "term": { "duration": 12, "unit": "MONTHS" },
        "licenseQuantity": 2,
        "clientRequestToken": "token-unique",
    });

    let first = create_subscription(&router, &request).await;
    assert_eq!(
        first.status, 200,
        "case first create: expected 200, got {first:?}"
    );
    let id = subscription(&first)["id"].as_str().unwrap_or_default().to_owned();

    // The SAME token is the SDK's retry of that call: it is answered with the
    // subscription the first call created.
    let retried = create_subscription(&router, &request).await;
    assert_eq!(
        retried.status, 200,
        "case retry: expected 200, got {retried:?}"
    );
    assert_eq!(
        subscription(&retried)["id"],
        serde_json::json!(id),
        "case retry: expected the first subscription's id {id:?}, got {:?}",
        subscription(&retried)["id"]
    );

    // A DIFFERENT token under the same name is a second subscription of a name the
    // Region already carries.
    let mut conflicting = request.clone();
    conflicting["clientRequestToken"] = serde_json::json!("token-other");
    let answer = create_subscription(&router, &conflicting).await;
    assert_eq!(
        answer.status, 400,
        "case duplicate name: expected HTTP 400, got {answer:?}"
    );
    let code = error_code(&answer);
    assert_eq!(
        code, "InvalidParameterException",
        "case duplicate name: expected InvalidParameterException, got {code:?}"
    );

    // An id nothing in this Region carries, in the shapes a client can send. The
    // refusal is worded for the subscription and names nothing the request sent;
    // `refusal_selection` is the suite that spans that rule.
    for (case, unknown) in [
        ("a well-formed UUID nothing carries", "11111111-2222-4333-8444-555555555555"),
        ("a value that is not a UUID at all", "not-a-subscription"),
    ] {
        let described = describe_subscription(&router, unknown).await;
        assert_eq!(
            described.status, 404,
            "case {case:?}: expected HTTP 404, got {described:?}"
        );
        let code = error_code(&described);
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {code:?}"
        );
        assert_eq!(
            described.body["message"].as_str(),
            Some(SUBSCRIPTION_NOT_FOUND_MESSAGE),
            "case {case:?}: expected the subscription's own not-found sentence, got {:?}",
            described.body["message"]
        );
        assert_eq!(
            described.body["subscriptionId"],
            serde_json::Value::Null,
            "case {case:?}: the refusal names no subscription, got {described:?}"
        );
    }

    // A subscription belongs to the Region it was created in: the same id in another
    // Region names nothing.
    let elsewhere = call(
        &router,
        "eu-central-1",
        "GET",
        &format!("/eks-anywhere-subscriptions/{id}"),
        "",
    )
    .await;
    assert_eq!(
        elsewhere.status, 404,
        "case another Region: expected HTTP 404, got {elsewhere:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: the listing reports the subscriptions of the Region the request is signed
// for, filtered by `includeStatus` and paged by `maxResults`/`nextToken`.
// ---------------------------------------------------------------------------

/// The table spans what the rule generalises over: the page size (one, several, and
/// more than the Region holds), and the status filter (none, the status every
/// subscription is in, and one none of them is in).
#[tokio::test]
async fn the_listing_pages_and_filters_the_regions_subscriptions() {
    let router = emulator();
    let mut created_ids = Vec::new();
    for index in 0..5 {
        let answer = create_subscription(
            &router,
            &serde_json::json!({
                "name": format!("listed-{index}"),
                "term": { "duration": 12, "unit": "MONTHS" },
                "licenseQuantity": 1,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "setup: expected 200 from CreateEksAnywhereSubscription, got {answer:?}"
        );
        created_ids.push(
            subscription(&answer)["id"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        );
    }
    created_ids.sort();

    async fn list(router: &MultiServiceRouter, query: &str) -> Answer {
        let uri = if query.is_empty() {
            "/eks-anywhere-subscriptions".to_string()
        } else {
            format!("/eks-anywhere-subscriptions?{query}")
        };
        call(router, REGION, "GET", &uri, "").await
    }

    fn listed_ids(answer: &Answer) -> Vec<String> {
        answer.body["subscriptions"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item["id"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    }

    // Every subscription of the Region, in one page.
    let all = list(&router, "").await;
    assert_eq!(
        all.status, 200,
        "case the whole listing: expected 200, got {all:?}"
    );
    assert_eq!(
        listed_ids(&all),
        created_ids,
        "case the whole listing: expected every created id, got {:?}",
        listed_ids(&all)
    );
    assert!(
        all.body["nextToken"].is_null(),
        "case the whole listing: the last page carries no token, got {:?}",
        all.body["nextToken"]
    );

    // Paged, at three page sizes: every page's ids in order, and nothing repeated or
    // skipped.
    for page_size in [1_usize, 2, 50] {
        let mut walked = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let query = match token.as_deref() {
                Some(token) => format!("maxResults={page_size}&nextToken={token}"),
                None => format!("maxResults={page_size}"),
            };
            let page = list(&router, &query).await;
            assert_eq!(
                page.status, 200,
                "case page size {page_size}: expected 200, got {page:?}"
            );
            let ids = listed_ids(&page);
            assert!(
                ids.len() <= page_size,
                "case page size {page_size}: a page must not exceed it, got {} ids",
                ids.len()
            );
            walked.extend(ids);
            match page.body["nextToken"].as_str() {
                Some(next) => token = Some(next.to_owned()),
                None => break,
            }
        }
        assert_eq!(
            walked, created_ids,
            "case page size {page_size}: the walk must report every id exactly once, got {walked:?}"
        );
    }

    // "Filtered by includeStatus": every subscription above has settled by now.
    tokio::time::sleep(past(SUBSCRIPTION_ACTIVATION_DELAY)).await;
    let active = list(&router, "includeStatus=ACTIVE").await;
    assert_eq!(
        listed_ids(&active),
        created_ids,
        "case the ACTIVE filter: expected every settled subscription, got {:?}",
        listed_ids(&active)
    );
    let expiring = list(&router, "includeStatus=EXPIRED").await;
    assert_eq!(
        listed_ids(&expiring),
        Vec::<String>::new(),
        "case the EXPIRED filter: expected none, got {:?}",
        listed_ids(&expiring)
    );

    // A token this listing did not issue is a bad parameter, not an empty page.
    let bad_token = list(&router, "nextToken=not-a-token-this-service-minted").await;
    assert_eq!(
        bad_token.status, 400,
        "case a forged token: expected HTTP 400, got {bad_token:?}"
    );
    let code = error_code(&bad_token);
    assert_eq!(
        code, "InvalidParameterException",
        "case a forged token: expected InvalidParameterException, got {code:?}"
    );

    // Another Region holds none of them.
    let elsewhere = call(&router, "eu-central-1", "GET", "/eks-anywhere-subscriptions", "").await;
    assert_eq!(
        listed_ids(&elsewhere),
        Vec::<String>::new(),
        "case another Region: expected an empty listing, got {:?}",
        listed_ids(&elsewhere)
    );
}
