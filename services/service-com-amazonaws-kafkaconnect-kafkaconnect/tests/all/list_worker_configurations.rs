// SPDX-License-Identifier: CC0-1.0

//! `ListWorkerConfigurations`, end to end over HTTP.
//!
//! Every case drives the emulator the way a client does: a buffered `restJson1`
//! request through the same router the production binary assembles, and the
//! response body read back as JSON. Nothing reaches into the state struct, so a
//! handler that answered from the request instead of from state would fail here.
//!
//! The suites assert the rule a regional listing follows, over a SET of inputs
//! rather than one configuration: the table spans names inside and outside the
//! filtered prefix, a description present, empty and absent, and properties
//! files from empty to kilobytes; the paging is driven at every page size
//! spanning the modelled `maxResults` range (1..100, from
//! `smithy_query(query="shape", shape="ListWorkerConfigurationsRequest")`) and at
//! each published bound and one past it; and the region cases list the same
//! names through two regional endpoints.
//!
//! The sibling suites' request helpers are private to their own modules, so this
//! one carries its own copies of the few it needs.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kafkaconnect_kafkaconnect::state::TRANSITION_SPAN;
use service_com_amazonaws_kafkaconnect_kafkaconnect::EmulatedService;

/// The region this suite's calls are signed for unless a case names another.
const REGION: &str = "eu-west-1";

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// One emulator: a single shared engine behind the production router.
fn emulator() -> MultiServiceRouter {
    let engine: std::sync::Arc<engine::InMemoryEngine> =
        std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 credential scope naming this service and `region`. No authenticator
/// is registered, so the signature is never checked; the scope is what routes
/// the request and names the region every minted ARN carries.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/kafkaconnect/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// Send one request signed for `region` and read back `(status, body)`.
async fn call(
    emulator: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: Value,
) -> (u16, Value) {
    let bytes = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(&body).expect("the request body serializes")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://kafkaconnect.{region}.amazonaws.com{path}"))
        .header("authorization", authorization(region))
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(bytes))
        .expect("the request builds");
    let response = emulator.dispatch(request).await;
    let status = response.status().as_u16();
    let body = response.into_body();
    let value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or_else(|e| panic!("a JSON response body: {e}"))
    };
    (status, value)
}

/// The single path segment an `@httpLabel` ARN occupies, percent-encoded the way
/// an AWS SDK encodes a label.
fn label(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// The error code a failure response carries (`__type`), or `""`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A member of a JSON object as a string, for an assertion message.
fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `workerConfigurations` array of a listing response.
fn listed(body: &Value) -> Vec<Value> {
    body.get("workerConfigurations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// One row of the worker-configuration table: what a create request carries.
struct WorkerCase {
    name: &'static str,
    description: Option<&'static str>,
    /// `propertiesFileContent`: the base64 `connect-distributed.properties`
    /// body. `__sensitiveString` carries no length constraint, so every size
    /// from empty upwards is a legal request.
    properties: String,
}

/// The table. The names span inside and outside the `page-` prefix the paging
/// suite filters on; the description is present, empty and absent; and the
/// properties file spans empty, one line and kilobytes.
fn worker_cases() -> Vec<WorkerCase> {
    vec![
        WorkerCase {
            name: "page-one-line",
            description: Some("the one-line configuration"),
            // Base64 of `key.converter=org.apache.kafka.connect.storage.StringConverter`.
            properties: "a2V5LmNvbnZlcnRlcj1vcmcuYXBhY2hlLmthZmthLmNvbm5lY3Quc3RvcmFnZS5TdHJpbmdDb252ZXJ0ZXI=".to_string(),
        },
        WorkerCase {
            name: "page-no-description",
            description: None,
            properties: "QUJD".to_string(),
        },
        WorkerCase {
            name: "page-empty-description",
            description: Some(""),
            properties: String::new(),
        },
        WorkerCase {
            name: "page-kilobytes",
            description: Some("kilobytes of properties"),
            // `QUJD` is base64 of `ABC`, so repeating it stays a legal body.
            properties: "QUJD".repeat(1024),
        },
        WorkerCase {
            name: "p",
            description: Some("a name at the modelled minimum length"),
            properties: "QUJD".to_string(),
        },
        WorkerCase {
            name: "outside-the-prefix",
            description: None,
            properties: "QUJD".to_string(),
        },
    ]
}

/// Create one row in `region` and answer its `workerConfigurationArn`.
async fn create(emulator: &MultiServiceRouter, region: &str, case: &WorkerCase) -> String {
    let mut body = json!({
        "name": case.name,
        "propertiesFileContent": case.properties,
    });
    if let Some(description) = case.description {
        body["description"] = json!(description);
    }
    let (status, created) = call(emulator, region, "POST", "/v1/worker-configurations", body).await;
    assert_eq!(
        status, 200,
        "case {:?}: CreateWorkerConfiguration must succeed, got {status} {created}",
        case.name
    );
    text(&created, "workerConfigurationArn")
}

#[tokio::test]
async fn a_listing_reports_every_stored_worker_configuration_from_state() {
    let emulator = emulator();
    let cases = worker_cases();
    let mut created: Vec<(String, String)> = Vec::new();
    for case in &cases {
        created.push((case.name.to_string(), create(&emulator, REGION, case).await));
    }

    let (status, body) = call(
        &emulator,
        REGION,
        "GET",
        "/v1/worker-configurations",
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "ListWorkerConfigurations must succeed, got {status} {body}"
    );
    let items = listed(&body);
    assert_eq!(
        items.len(),
        cases.len(),
        "case {:?}: every stored configuration is reported, expected {} entries, got {} in {body}",
        "the whole table",
        cases.len(),
        items.len()
    );

    for (case, (_, arn)) in cases.iter().zip(&created) {
        let summary = items
            .iter()
            .find(|summary| &text(summary, "workerConfigurationArn") == arn)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected the created ARN {arn:?} in the listing, got {body}",
                    case.name
                )
            });
        assert_eq!(
            text(summary, "name"),
            case.name,
            "case {:?}: the summary carries the stored name, got {summary}",
            case.name
        );
        assert_eq!(
            summary.get("description").and_then(Value::as_str),
            case.description,
            "case {:?}: the summary carries the stored description, got {summary}",
            case.name
        );
        // `WorkerConfigurationState` has no creating value, so a stored
        // configuration is reported ACTIVE.
        assert_eq!(
            text(summary, "workerConfigurationState"),
            "ACTIVE",
            "case {:?}: a stored configuration is listed ACTIVE, got {summary}",
            case.name
        );
        assert!(
            summary.get("creationTime").is_some(),
            "case {:?}: the summary carries its creationTime, got {summary}",
            case.name
        );
        let revision = &summary["latestRevision"];
        assert_eq!(
            revision["revision"].as_i64(),
            Some(1),
            "case {:?}: a new configuration is at revision 1, got {revision}",
            case.name
        );
        assert!(
            revision.get("creationTime").is_some(),
            "case {:?}: the revision summary carries its creationTime, got {revision}",
            case.name
        );
        // `WorkerConfigurationSummary` has no `propertiesFileContent` member, so
        // the sensitive body must not come back on the listing path.
        assert!(
            summary.get("propertiesFileContent").is_none()
                && revision.get("propertiesFileContent").is_none(),
            "case {:?}: a listing carries no propertiesFileContent, got {summary}",
            case.name
        );
        assert!(
            arn.starts_with(&format!(
                "arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:worker-configuration/{}/",
                case.name
            )),
            "case {:?}: the listed ARN is the one the create minted, got {arn:?}",
            case.name
        );
    }

    // A listing is a read: it must not change what a later caller sees.
    let (_, again) = call(
        &emulator,
        REGION,
        "GET",
        "/v1/worker-configurations",
        Value::Null,
    )
    .await;
    assert_eq!(
        body, again,
        "a listing must not change what the next caller sees, got {again}"
    );
}

#[tokio::test]
async fn listing_worker_configurations_filters_by_prefix_and_pages_each_one_exactly_once() {
    let emulator = emulator();
    let cases = worker_cases();
    let mut under_prefix = BTreeSet::new();
    for case in &cases {
        let arn = create(&emulator, REGION, case).await;
        if case.name.starts_with("page-") {
            under_prefix.insert(arn);
        }
    }

    // Every page size spanning the modelled `maxResults` range pages the same
    // configurations under the prefix, each exactly once.
    for page_size in [1_i32, 2, 3, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut path = format!("/v1/worker-configurations?namePrefix=page-&maxResults={page_size}");
            if let Some(token) = &token {
                path.push_str(&format!("&nextToken={}", label(token)));
            }
            let (status, body) = call(&emulator, REGION, "GET", &path, Value::Null).await;
            assert_eq!(
                status, 200,
                "case page size {page_size}: the listing must succeed, got {status} {body}"
            );
            let items = listed(&body);
            assert!(
                items.len() <= usize::try_from(page_size).expect("a positive page size"),
                "case page size {page_size}: a page must not exceed maxResults, got {} items",
                items.len()
            );
            for summary in &items {
                assert!(
                    text(summary, "name").starts_with("page-"),
                    "case page size {page_size}: the prefix filter must hold, got {summary}"
                );
                seen.push(text(summary, "workerConfigurationArn"));
            }
            pages += 1;
            assert!(
                pages <= 10,
                "case page size {page_size}: paging must terminate, got {pages} pages"
            );
            match body.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        let unique: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case page size {page_size}: paging must not repeat a configuration, saw {seen:?}"
        );
        assert_eq!(
            unique, under_prefix,
            "case page size {page_size}: paging must return every configuration under the prefix, \
             saw {seen:?}"
        );
    }

    // A prefix nothing is stored under is an empty page, not an error.
    let (status, body) = call(
        &emulator,
        REGION,
        "GET",
        "/v1/worker-configurations?namePrefix=nothing-starts-with-this",
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: an unmatched prefix is still a 200, got {status} {body}",
        "unmatched prefix"
    );
    assert!(
        listed(&body).is_empty() && body.get("nextToken").and_then(Value::as_str).is_none(),
        "case {:?}: an unmatched prefix is an empty, unpaged page, got {body}",
        "unmatched prefix"
    );
}

#[tokio::test]
async fn the_modelled_max_results_bounds_and_an_unissued_token_are_refused() {
    let emulator = emulator();
    for case in worker_cases() {
        create(&emulator, REGION, &case).await;
    }

    // The bounds come from the model, not from this test: `maxResults` targets
    // `MaxResults` (range 1..100). Each published bound is accepted and the value
    // one past it is refused.
    for (case, max_results, want) in [
        ("at the minimum", 1, 200_u16),
        ("at the maximum", 100, 200),
        ("one below the minimum", 0, 400),
        ("one past the maximum", 101, 400),
    ] {
        let (status, body) = call(
            &emulator,
            REGION,
            "GET",
            &format!("/v1/worker-configurations?maxResults={max_results}"),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, want,
            "case {case:?}: maxResults={max_results} must answer HTTP {want}, got {status} {body}"
        );
        if want == 400 {
            assert!(
                error_code(&body).contains("BadRequestException"),
                "case {case:?}: expected BadRequestException, got {body}"
            );
        }
    }

    // A token this service never issued is a bad request, not an empty page and
    // not a silently honoured resume point. Both forms are refused: one that is
    // not even the token's encoding, and one that is — hex that decodes to a
    // plausible key — but carries none of this service's marker.
    for forged in [
        "not-a-token".to_string(),
        "6161".to_string(),
        hex::encode(format!(
            "arn:aws:kafkaconnect:{REGION}:{ACCOUNT}:worker-configuration/page-one-line/x"
        )),
    ] {
        let (status, body) = call(
            &emulator,
            REGION,
            "GET",
            &format!("/v1/worker-configurations?nextToken={}", label(&forged)),
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {forged:?}: an unissued nextToken is a 400, got {status} {body}"
        );
        assert!(
            error_code(&body).contains("BadRequestException"),
            "case {forged:?}: expected BadRequestException, got {body}"
        );
        assert_eq!(
            text(&body, "message"),
            "Invalid parameter nextToken.",
            "case {forged:?}: the refusal blames the member that carried the token, got {body}"
        );
    }
}

#[tokio::test]
async fn a_listing_reports_only_the_worker_configurations_of_the_region_it_is_answered_through() {
    let emulator = emulator();
    // The same names in two regions: a name is unique per account and Region, so
    // each region holds its own independent configuration.
    let regions = ["eu-west-1", "us-east-2"];
    let cases = worker_cases();
    let mut minted: Vec<(&str, BTreeSet<String>)> = Vec::new();
    for region in regions {
        let mut arns = BTreeSet::new();
        for case in &cases {
            arns.insert(create(&emulator, region, case).await);
        }
        minted.push((region, arns));
    }

    for (region, arns) in &minted {
        let (status, body) = call(
            &emulator,
            region,
            "GET",
            "/v1/worker-configurations",
            Value::Null,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: the listing must succeed, got {status} {body}"
        );
        let reported: BTreeSet<String> = listed(&body)
            .iter()
            .map(|summary| text(summary, "workerConfigurationArn"))
            .collect();
        assert_eq!(
            &reported, arns,
            "case {region:?}: a Region reports its OWN configurations and no other's, got {body}"
        );
        for arn in &reported {
            assert_eq!(
                arn.split(':').nth(3).unwrap_or_default(),
                *region,
                "case {region:?}: every reported ARN carries the Region it was listed through, \
                 got {arn:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_deleting_worker_configuration_is_listed_until_the_clock_removes_it() {
    let emulator = emulator();
    let cases = worker_cases();
    let mut arns: Vec<(&str, String)> = Vec::new();
    for case in &cases {
        arns.push((case.name, create(&emulator, REGION, case).await));
    }

    // One of the stored configurations is deleted; the rest stay ACTIVE.
    let (deleted_name, deleted_arn) = arns[0].clone();
    let (status, body) = call(
        &emulator,
        REGION,
        "DELETE",
        &format!("/v1/worker-configurations/{}", label(&deleted_arn)),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {deleted_name:?}: the delete must succeed, got {status} {body}"
    );

    let (status, body) = call(
        &emulator,
        REGION,
        "GET",
        "/v1/worker-configurations",
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {deleted_name:?}: the listing must succeed, got {status} {body}"
    );
    let items = listed(&body);
    let deleting = items
        .iter()
        .find(|summary| text(summary, "workerConfigurationArn") == deleted_arn)
        .unwrap_or_else(|| {
            panic!("case {deleted_name:?}: a deleting configuration is still listed, got {body}")
        });
    assert_eq!(
        text(deleting, "workerConfigurationState"),
        "DELETING",
        "case {deleted_name:?}: a configuration whose delete is under way is listed DELETING, \
         got {deleting}"
    );
    for (name, arn) in &arns[1..] {
        let summary = items
            .iter()
            .find(|summary| &text(summary, "workerConfigurationArn") == arn)
            .unwrap_or_else(|| {
                panic!("case {name:?}: an untouched configuration stays listed, got {body}")
            });
        assert_eq!(
            text(summary, "workerConfigurationState"),
            "ACTIVE",
            "case {name:?}: deleting one configuration leaves the others ACTIVE, got {summary}"
        );
    }

    // Once the clock has the removal due, the listing no longer reports it —
    // and still reports every other configuration.
    tokio::time::sleep(TRANSITION_SPAN + Duration::from_millis(400)).await;
    let (status, body) = call(
        &emulator,
        REGION,
        "GET",
        "/v1/worker-configurations",
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {deleted_name:?}: the listing must succeed, got {status} {body}"
    );
    let reported: BTreeSet<String> = listed(&body)
        .iter()
        .map(|summary| text(summary, "workerConfigurationArn"))
        .collect();
    assert!(
        !reported.contains(&deleted_arn),
        "case {deleted_name:?}: a removed configuration is gone from the listing, got {body}"
    );
    let expected: BTreeSet<String> = arns[1..].iter().map(|(_, arn)| arn.clone()).collect();
    assert_eq!(
        reported, expected,
        "case {deleted_name:?}: every configuration that was not deleted stays listed, got {body}"
    );
}
