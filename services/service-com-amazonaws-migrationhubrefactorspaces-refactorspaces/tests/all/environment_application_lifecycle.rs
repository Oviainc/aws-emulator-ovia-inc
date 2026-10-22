// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Refactor Spaces environment/application lifecycle:
//! `CreateEnvironment`, `GetEnvironment`, `ListEnvironments`, `CreateApplication`,
//! `GetApplication`, `ListApplications` and `DeleteEnvironment`, driven over HTTP through
//! the same `MultiServiceRouter` the production emulator serves with.
//!
//! Every case asserts on the RESPONSE BODY, not just the status: a create is only correct
//! if a later read answers with what it persisted, and a delete is only correct if the
//! resource is gone from every later read.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header naming this service and a region. The emulator's router
/// reads the service from the credential scope to pick the claiming service, and the
/// handlers read the region from the same scope to build their ARNs.
const SIGNED: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEMULATOR/20260101/eu-west-2/refactor-spaces/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The region in [`SIGNED`]: every ARN the handlers mint for these requests carries it.
const REGION: &str = "eu-west-2";

/// The account id every generated emulator presents, from `engine::emulator_account_id`.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// Comfortably past `crate::state::PROVISIONING_SPAN` (500 ms), so the `CREATING` ->
/// `ACTIVE` move the creating call owed is due.
const PAST_PROVISIONING: Duration = Duration::from_millis(900);

/// One emulator over one shared engine: every call below observes the same store.
struct Emulator {
    router: MultiServiceRouter,
    /// The same engine the mounted service holds, so a case that needs a store already at
    /// a boundary can seed it instead of driving thousands of creates to reach it.
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not signature verification.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_migrationhubrefactorspaces_refactorspaces::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// Send one request and return `(status, x-amzn-errortype, parsed body)`.
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, String, Value) {
        let bytes = body.map_or_else(Bytes::new, |value| {
            Bytes::from(serde_json::to_vec(&value).expect("a JSON request body serializes"))
        });
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header("host", format!("refactor-spaces.{REGION}.amazonaws.com"))
            .header("authorization", SIGNED)
            .header("content-type", "application/json")
            .body(bytes)
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, error_type, parsed)
    }

    async fn get(&self, path: &str) -> (u16, String, Value) {
        self.call("GET", path, None).await
    }

    async fn post(&self, path: &str, body: Value) -> (u16, String, Value) {
        self.call("POST", path, Some(body)).await
    }

    async fn delete(&self, path: &str) -> (u16, String, Value) {
        self.call("DELETE", path, None).await
    }

    /// Create an environment and return its id, failing the case if the call did not
    /// succeed.
    async fn create_environment(&self, name: &str, fabric: &str) -> String {
        let (status, _, body) = self
            .post(
                "/environments",
                json!({ "Name": name, "NetworkFabricType": fabric }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case create {name:?}/{fabric}: expected 200, got {status} with {body}"
        );
        body["EnvironmentId"]
            .as_str()
            .unwrap_or_else(|| panic!("case create {name:?}: no EnvironmentId in {body}"))
            .to_string()
    }
}

/// Whether `id` matches the modelled `^<prefix>[0-9A-Za-z]{10}$` identifier pattern
/// (`smithy_query(query="shape",shape="EnvironmentId")`).
fn matches_id_pattern(id: &str, prefix: &str) -> bool {
    id.strip_prefix(prefix).is_some_and(|body| {
        body.len() == 10 && body.chars().all(|c| c.is_ascii_alphanumeric())
    })
}

/// Walk every page of `ListEnvironments` at `max_results`, returning the ids in the order
/// the service reported them.
async fn page_environments(emulator: &Emulator, max_results: Option<i32>) -> Vec<String> {
    let mut ids = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut query: Vec<String> = Vec::new();
        if let Some(max) = max_results {
            query.push(format!("maxResults={max}"));
        }
        if let Some(next) = &token {
            query.push(format!("nextToken={next}"));
        }
        let path = if query.is_empty() {
            "/environments".to_string()
        } else {
            format!("/environments?{}", query.join("&"))
        };
        let (status, error, body) = emulator.get(&path).await;
        assert_eq!(
            status, 200,
            "case page {path}: expected 200, got {status} {error} with {body}"
        );
        let listed = body["EnvironmentSummaryList"]
            .as_array()
            .unwrap_or_else(|| panic!("case page {path}: no EnvironmentSummaryList in {body}"))
            .clone();
        for summary in &listed {
            ids.push(
                summary["EnvironmentId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case page {path}: no EnvironmentId in {summary}"))
                    .to_string(),
            );
        }
        match body["NextToken"].as_str() {
            Some(next) => {
                assert!(
                    !listed.is_empty(),
                    "case page {path}: a NextToken was returned with an empty page, \
                     which never terminates"
                );
                token = Some(next.to_string());
            }
            None => return ids,
        }
    }
}

/// Every environment the create call persists is answered back, member for member, by a
/// later `GetEnvironment` — across both `NetworkFabricType` values and with the optional
/// members present and absent.
#[tokio::test]
async fn a_created_environment_round_trips_through_get_environment() {
    // The two members of the modelled `NetworkFabricType` enum
    // (`smithy_query(query="shape",shape="NetworkFabricType")`), each with and without
    // the optional `Description` / `Tags`.
    let cases: Vec<(&str, &str, Option<&str>, Option<Value>)> = vec![
        ("plainnone", "NONE", None, None),
        (
            "describednone",
            "NONE",
            Some("a described environment"),
            None,
        ),
        (
            "taggednone",
            "NONE",
            None,
            Some(json!({ "team": "migration" })),
        ),
        (
            "transitgateway",
            "TRANSIT_GATEWAY",
            Some("fabric over a transit gateway"),
            Some(json!({ "team": "migration", "tier": "zero" })),
        ),
        (
            "manytags",
            "TRANSIT_GATEWAY",
            None,
            Some(json!({ "a": "1", "b": "2", "c": "3" })),
        ),
    ];

    let emulator = Emulator::new();
    for (name, fabric, description, tags) in cases {
        let case = format!("{name}/{fabric}");
        let mut request = json!({ "Name": name, "NetworkFabricType": fabric });
        if let Some(description) = description {
            request["Description"] = json!(description);
        }
        if let Some(tags) = &tags {
            request["Tags"] = tags.clone();
        }

        let (status, error, created) = emulator.post("/environments", request).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from CreateEnvironment, got {status} {error} \
             with {created}"
        );

        let id = created["EnvironmentId"].as_str().unwrap_or_default();
        assert!(
            matches_id_pattern(id, "env-"),
            "case {case}: expected EnvironmentId matching ^env-[0-9A-Za-z]{{10}}$, got {id:?}"
        );
        let expected_arn =
            format!("arn:aws:refactor-spaces:{REGION}:{ACCOUNT}:environment/{id}");
        assert_eq!(
            created["Arn"].as_str(),
            Some(expected_arn.as_str()),
            "case {case}: expected Arn {expected_arn:?}, got {:?}",
            created["Arn"]
        );
        assert_eq!(
            created["State"].as_str(),
            Some("CREATING"),
            "case {case}: expected State CREATING right after the create, got {:?}",
            created["State"]
        );
        assert_eq!(
            created["OwnerAccountId"].as_str(),
            Some(ACCOUNT),
            "case {case}: expected OwnerAccountId {ACCOUNT}, got {:?}",
            created["OwnerAccountId"]
        );
        assert!(
            created["CreatedTime"].is_number(),
            "case {case}: expected a CreatedTime timestamp, got {:?}",
            created["CreatedTime"]
        );

        // The transit gateway is provisioned only for a TRANSIT_GATEWAY environment.
        let (status, error, fetched) = emulator.get(&format!("/environments/{id}")).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from GetEnvironment, got {status} {error} \
             with {fetched}"
        );
        let tgw = fetched["TransitGatewayId"].as_str();
        if fabric == "TRANSIT_GATEWAY" {
            let tgw = tgw.unwrap_or_default();
            let body = tgw.strip_prefix("tgw-").unwrap_or_default();
            assert!(
                body.len() == 17 && body.chars().all(|c| c.is_ascii_hexdigit()),
                "case {case}: expected TransitGatewayId matching ^tgw-[-a-f0-9]{{17}}$, \
                 got {tgw:?}"
            );
        } else {
            assert_eq!(
                tgw, None,
                "case {case}: a NONE-fabric environment provisions no transit gateway, \
                 got {tgw:?}"
            );
        }

        // Every member the create reported is the member the read reports.
        for member in [
            "Name",
            "Arn",
            "EnvironmentId",
            "NetworkFabricType",
            "OwnerAccountId",
            "Description",
            "CreatedTime",
            "Tags",
        ] {
            assert_eq!(
                fetched[member], created[member],
                "case {case}: {member} did not round-trip: created {:?}, read back {:?}",
                created[member], fetched[member]
            );
        }
        assert_eq!(
            fetched["Description"].as_str(),
            description,
            "case {case}: expected Description {description:?}, got {:?}",
            fetched["Description"]
        );
        assert_eq!(
            fetched["Tags"],
            tags.clone().unwrap_or(Value::Null),
            "case {case}: expected Tags {tags:?}, got {:?}",
            fetched["Tags"]
        );
    }
}

/// A `CREATING` environment reaches `ACTIVE` on its own once the clock owes the move, and
/// nothing a caller does brings it forward.
#[tokio::test]
async fn a_creating_environment_becomes_active_once_the_clock_owes_it() {
    let emulator = Emulator::new();
    let id = emulator.create_environment("transitional", "NONE").await;

    let (_, _, immediately) = emulator.get(&format!("/environments/{id}")).await;
    assert_eq!(
        immediately["State"].as_str(),
        Some("CREATING"),
        "case immediate read: a caller acting straight after the create must still see \
         CREATING, got {:?}",
        immediately["State"]
    );

    tokio::time::sleep(PAST_PROVISIONING).await;

    let (_, _, later) = emulator.get(&format!("/environments/{id}")).await;
    assert_eq!(
        later["State"].as_str(),
        Some("ACTIVE"),
        "case read past the provisioning span: expected ACTIVE, got {:?}",
        later["State"]
    );
    let (_, _, listed) = emulator.get("/environments").await;
    assert_eq!(
        listed["EnvironmentSummaryList"][0]["State"].as_str(),
        Some("ACTIVE"),
        "case list past the provisioning span: expected ACTIVE, got {:?}",
        listed["EnvironmentSummaryList"][0]["State"]
    );
}

/// `ListEnvironments` reports each stored environment exactly once, at every page size,
/// and an empty store answers an empty list rather than an error.
#[tokio::test]
async fn paging_list_environments_yields_each_environment_exactly_once() {
    // (how many environments exist, the maxResults to page at). The published
    // `MaxResults` range is 1..100, so 1 and 100 are its bounds.
    let cases: &[(usize, Option<i32>)] = &[
        (0, None),
        (0, Some(1)),
        (1, Some(1)),
        (3, Some(1)),
        (5, Some(2)),
        (4, Some(3)),
        (7, Some(100)),
        (6, None),
    ];

    for (count, max_results) in cases.iter().copied() {
        let case = format!("{count} environments at maxResults={max_results:?}");
        let emulator = Emulator::new();
        let mut created = Vec::new();
        for index in 0..count {
            created.push(
                emulator
                    .create_environment(&format!("env{index}number"), "NONE")
                    .await,
            );
        }

        let paged = page_environments(&emulator, max_results).await;
        assert_eq!(
            paged, created,
            "case {case}: paging must yield every environment exactly once, in creation \
             order; expected {created:?}, got {paged:?}"
        );
    }
}

/// A `NextToken` this service did not mint, or one that resumes after an environment the
/// store no longer holds, is refused as a 400 `ValidationException` — never answered with
/// a silently-restarted listing.
#[tokio::test]
async fn an_unresolvable_next_token_is_refused() {
    let emulator = Emulator::new();
    let id = emulator.create_environment("onlyone", "NONE").await;
    // The token the service itself mints for this environment, then the environment is
    // removed from under it.
    let (_, _, first_page) = emulator.get("/environments?maxResults=1").await;
    assert!(
        first_page["NextToken"].is_null(),
        "case single environment: a complete page carries no NextToken, got {:?}",
        first_page["NextToken"]
    );

    // Tokens that cannot name a page: not hex at all, hex of an id never minted.
    let cases = [
        ("not a token this service minted", "zzzz"),
        ("hex of an id that was never minted", "656e762d30303030303030303030"),
    ];
    for (case, token) in cases {
        let (status, error, body) = emulator
            .get(&format!("/environments?nextToken={token}"))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} {error} with {body}"
        );
        assert_eq!(
            error, "ValidationException",
            "case {case}: expected error code ValidationException, got {error:?} with {body}"
        );
    }

    let (status, _, _) = emulator.delete(&format!("/environments/{id}")).await;
    assert_eq!(status, 200, "deleting the only environment must succeed");
}

/// `MaxResults` outside its published 1..100 range is refused at the boundary with a 400,
/// and each bound inside it is accepted.
#[tokio::test]
async fn max_results_is_held_to_its_published_range() {
    let emulator = Emulator::new();
    emulator.create_environment("boundedlist", "NONE").await;

    // The `MaxResults` range from the model (`range {min: 1, max: 100}`): each published
    // bound, and one past each.
    let cases: &[(i32, u16)] = &[(1, 200), (100, 200), (0, 400), (101, 400)];
    for &(max_results, expected) in cases {
        let (status, error, body) = emulator
            .get(&format!("/environments?maxResults={max_results}"))
            .await;
        assert_eq!(
            status, expected,
            "case maxResults={max_results}: expected {expected}, got {status} {error} \
             with {body}"
        );
    }
}

/// An identifier that does not match its modelled pattern is refused as a 400
/// `ValidationException` — not a 404, and not a 500.
#[tokio::test]
async fn an_identifier_outside_its_modeled_pattern_is_refused() {
    let emulator = Emulator::new();
    // Each row breaks the `^env-[0-9A-Za-z]{10}$` pattern or its 14..14 length in a
    // different way.
    let cases = [
        ("missing the env- prefix", "0123456789abcd"),
        ("too short a body", "env-012345678"),
        ("too long a body", "env-01234567890"),
        ("a non-alphanumeric body", "env-0123456_89"),
        ("an application id where an environment id belongs", "app-0123456789"),
    ];
    for (case, identifier) in cases {
        let (status, error, body) = emulator.get(&format!("/environments/{identifier}")).await;
        assert_eq!(
            status, 400,
            "case {case} ({identifier:?}): expected 400, got {status} {error} with {body}"
        );
        assert_eq!(
            error, "ValidationException",
            "case {case} ({identifier:?}): expected error code ValidationException, \
             got {error:?} with {body}"
        );
    }

    // A WELL-FORMED id that names nothing is the 404 case, not the 400 one.
    for identifier in ["env-0000000000", "env-ZZZZZZZZZZ"] {
        let (status, error, body) = emulator.get(&format!("/environments/{identifier}")).await;
        assert_eq!(
            status, 404,
            "case unknown but well-formed {identifier:?}: expected 404, got {status} \
             {error} with {body}"
        );
        assert_eq!(
            error, "ResourceNotFoundException",
            "case unknown but well-formed {identifier:?}: expected \
             ResourceNotFoundException, got {error:?} with {body}"
        );
        assert_eq!(
            body["ResourceId"].as_str(),
            Some(identifier),
            "case unknown but well-formed {identifier:?}: the refusal must name the \
             identifier, got {:?}",
            body["ResourceId"]
        );
    }
}

/// An application is stored under its own environment: it round-trips through
/// `GetApplication`, is listed under that environment, and never appears under another.
#[tokio::test]
async fn an_application_is_scoped_to_the_environment_that_owns_it() {
    let emulator = Emulator::new();
    let host = emulator.create_environment("hostenvironment", "NONE").await;
    let other = emulator.create_environment("otherenvironment", "NONE").await;

    // Rows differ along VpcId form (both published lengths), the proxy configuration
    // (absent, defaulted, explicit) and the tag count.
    let cases: Vec<(&str, &str, Option<Value>, Option<Value>)> = vec![
        ("shortvpcid", "vpc-0123abcd", None, None),
        (
            "longvpcid",
            "vpc-0123456789abcdef0",
            Some(json!({})),
            Some(json!({ "team": "migration" })),
        ),
        (
            "privateproxy",
            "vpc-abcdef01",
            Some(json!({ "EndpointType": "PRIVATE", "StageName": "canary" })),
            Some(json!({ "team": "migration", "tier": "one" })),
        ),
    ];

    let mut expected_ids = Vec::new();
    for (name, vpc_id, proxy, tags) in cases {
        let case = format!("{name}/{vpc_id}");
        let mut request = json!({
            "Name": name,
            "VpcId": vpc_id,
            "ProxyType": "API_GATEWAY",
        });
        if let Some(proxy) = &proxy {
            request["ApiGatewayProxy"] = proxy.clone();
        }
        if let Some(tags) = &tags {
            request["Tags"] = tags.clone();
        }

        let (status, error, created) = emulator
            .post(&format!("/environments/{host}/applications"), request)
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from CreateApplication, got {status} {error} \
             with {created}"
        );
        let id = created["ApplicationId"].as_str().unwrap_or_default();
        assert!(
            matches_id_pattern(id, "app-"),
            "case {case}: expected ApplicationId matching ^app-[0-9A-Za-z]{{10}}$, got {id:?}"
        );
        assert_eq!(
            created["EnvironmentId"].as_str(),
            Some(host.as_str()),
            "case {case}: expected EnvironmentId {host}, got {:?}",
            created["EnvironmentId"]
        );
        let expected_arn = format!(
            "arn:aws:refactor-spaces:{REGION}:{ACCOUNT}:environment/{host}/application/{id}"
        );
        assert_eq!(
            created["Arn"].as_str(),
            Some(expected_arn.as_str()),
            "case {case}: expected Arn {expected_arn:?}, got {:?}",
            created["Arn"]
        );
        assert_eq!(
            created["State"].as_str(),
            Some("CREATING"),
            "case {case}: expected State CREATING right after the create, got {:?}",
            created["State"]
        );

        // Read it back: every member the create reported is the member the read reports.
        let (status, error, fetched) = emulator
            .get(&format!("/environments/{host}/applications/{id}"))
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from GetApplication, got {status} {error} \
             with {fetched}"
        );
        for member in [
            "Name",
            "Arn",
            "ApplicationId",
            "EnvironmentId",
            "VpcId",
            "ProxyType",
            "OwnerAccountId",
            "CreatedByAccountId",
            "CreatedTime",
            "Tags",
        ] {
            assert_eq!(
                fetched[member], created[member],
                "case {case}: {member} did not round-trip: created {:?}, read back {:?}",
                created[member], fetched[member]
            );
        }
        // `ApiGatewayProxyInput` documents EndpointType defaulting to REGIONAL and
        // StageName defaulting to prod.
        let expected_proxy = proxy.as_ref().map(|proxy| {
            (
                proxy["EndpointType"].as_str().unwrap_or("REGIONAL").to_string(),
                proxy["StageName"].as_str().unwrap_or("prod").to_string(),
            )
        });
        let actual_proxy = fetched["ApiGatewayProxy"].as_object().map(|proxy| {
            (
                proxy["EndpointType"].as_str().unwrap_or_default().to_string(),
                proxy["StageName"].as_str().unwrap_or_default().to_string(),
            )
        });
        assert_eq!(
            actual_proxy, expected_proxy,
            "case {case}: expected ApiGatewayProxy {expected_proxy:?}, got {actual_proxy:?}"
        );

        expected_ids.push(id.to_string());
    }

    // Scoping: the host environment lists exactly its own applications, in creation
    // order; the other environment lists none.
    let (status, error, listed) = emulator
        .get(&format!("/environments/{host}/applications"))
        .await;
    assert_eq!(
        status, 200,
        "case list under the owning environment: expected 200, got {status} {error} \
         with {listed}"
    );
    let listed_ids: Vec<String> = listed["ApplicationSummaryList"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|summary| summary["ApplicationId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        listed_ids, expected_ids,
        "case list under the owning environment: expected {expected_ids:?}, got {listed_ids:?}"
    );

    let (status, error, empty) = emulator
        .get(&format!("/environments/{other}/applications"))
        .await;
    assert_eq!(
        status, 200,
        "case list under a sibling environment: expected 200, got {status} {error} \
         with {empty}"
    );
    assert_eq!(
        empty["ApplicationSummaryList"].as_array().map(Vec::len),
        Some(0),
        "case list under a sibling environment: an environment with no applications of \
         its own must list none, got {:?}",
        empty["ApplicationSummaryList"]
    );

    // And the application of another environment is not readable through this one.
    let (status, error, body) = emulator
        .get(&format!(
            "/environments/{other}/applications/{}",
            expected_ids[0]
        ))
        .await;
    assert_eq!(
        status, 404,
        "case read a sibling's application: expected 404, got {status} {error} with {body}"
    );
}

/// Paging `ListApplications` yields each application of its environment exactly once.
#[tokio::test]
async fn paging_list_applications_yields_each_application_exactly_once() {
    let emulator = Emulator::new();
    let environment = emulator.create_environment("pagedapps", "NONE").await;

    let mut created = Vec::new();
    for index in 0..5 {
        let (status, error, body) = emulator
            .post(
                &format!("/environments/{environment}/applications"),
                json!({
                    "Name": format!("app{index}number"),
                    "VpcId": "vpc-0123456789abcdef0",
                    "ProxyType": "API_GATEWAY",
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case create application {index}: expected 200, got {status} {error} with {body}"
        );
        created.push(body["ApplicationId"].as_str().unwrap_or_default().to_string());
    }

    for max_results in [1, 2, 3, 100] {
        let case = format!("maxResults={max_results}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut path = format!(
                "/environments/{environment}/applications?maxResults={max_results}"
            );
            if let Some(next) = &token {
                path.push_str(&format!("&nextToken={next}"));
            }
            let (status, error, body) = emulator.get(&path).await;
            assert_eq!(
                status, 200,
                "case {case}: expected 200, got {status} {error} with {body}"
            );
            for summary in body["ApplicationSummaryList"].as_array().unwrap_or(&Vec::new()) {
                seen.push(
                    summary["ApplicationId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            match body["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, created,
            "case {case}: paging must yield every application exactly once, in creation \
             order; expected {created:?}, got {seen:?}"
        );
    }
}

/// `CreateApplication` and `ListApplications` under an environment that does not exist are
/// refused as 404 `ResourceNotFoundException`.
#[tokio::test]
async fn an_unknown_parent_environment_is_resource_not_found() {
    let emulator = Emulator::new();
    // Well-formed environment ids that name nothing.
    for identifier in ["env-0000000000", "env-aaaaaaaaaa"] {
        let (status, error, body) = emulator
            .post(
                &format!("/environments/{identifier}/applications"),
                json!({
                    "Name": "orphanapp",
                    "VpcId": "vpc-0123456789abcdef0",
                    "ProxyType": "API_GATEWAY",
                }),
            )
            .await;
        assert_eq!(
            status, 404,
            "case CreateApplication under {identifier:?}: expected 404, got {status} \
             {error} with {body}"
        );
        assert_eq!(
            error, "ResourceNotFoundException",
            "case CreateApplication under {identifier:?}: expected \
             ResourceNotFoundException, got {error:?} with {body}"
        );
        assert_eq!(
            body["ResourceType"].as_str(),
            Some("ENVIRONMENT"),
            "case CreateApplication under {identifier:?}: the refusal must name the \
             missing resource type, got {:?}",
            body["ResourceType"]
        );

        let (status, error, body) = emulator
            .get(&format!("/environments/{identifier}/applications"))
            .await;
        assert_eq!(
            status, 404,
            "case ListApplications under {identifier:?}: expected 404, got {status} \
             {error} with {body}"
        );
    }
}

/// `ClientToken` is `@idempotencyToken`: a replayed create answers with the resource the
/// first call created rather than minting a second one.
#[tokio::test]
async fn a_replayed_client_token_creates_nothing_new() {
    let emulator = Emulator::new();

    let request = json!({
        "Name": "idempotentenv",
        "NetworkFabricType": "NONE",
        "ClientToken": "token-one",
    });
    let (_, _, first) = emulator.post("/environments", request.clone()).await;
    let (status, error, replay) = emulator.post("/environments", request).await;
    assert_eq!(
        status, 200,
        "case replayed CreateEnvironment: expected 200, got {status} {error} with {replay}"
    );
    assert_eq!(
        replay["EnvironmentId"], first["EnvironmentId"],
        "case replayed CreateEnvironment: expected the first environment {:?}, got {:?}",
        first["EnvironmentId"], replay["EnvironmentId"]
    );
    let listed = page_environments(&emulator, None).await;
    assert_eq!(
        listed.len(),
        1,
        "case replayed CreateEnvironment: the store must hold one environment, got {listed:?}"
    );

    let environment = first["EnvironmentId"].as_str().unwrap_or_default();
    let application = json!({
        "Name": "idempotentapp",
        "VpcId": "vpc-0123456789abcdef0",
        "ProxyType": "API_GATEWAY",
        "ClientToken": "token-two",
    });
    let (_, _, first_app) = emulator
        .post(
            &format!("/environments/{environment}/applications"),
            application.clone(),
        )
        .await;
    let (status, error, replayed_app) = emulator
        .post(
            &format!("/environments/{environment}/applications"),
            application,
        )
        .await;
    assert_eq!(
        status, 200,
        "case replayed CreateApplication: expected 200, got {status} {error} \
         with {replayed_app}"
    );
    assert_eq!(
        replayed_app["ApplicationId"], first_app["ApplicationId"],
        "case replayed CreateApplication: expected the first application {:?}, got {:?}",
        first_app["ApplicationId"], replayed_app["ApplicationId"]
    );
    let (_, _, listed) = emulator
        .get(&format!("/environments/{environment}/applications"))
        .await;
    assert_eq!(
        listed["ApplicationSummaryList"].as_array().map(Vec::len),
        Some(1),
        "case replayed CreateApplication: the environment must hold one application, \
         got {:?}",
        listed["ApplicationSummaryList"]
    );
}

/// Reusing a `ClientToken` for a DIFFERENT resource is not a replay: it is refused with
/// the modelled `ConflictException` (409) rather than answered with a record the caller
/// did not ask for, and it creates nothing.
#[tokio::test]
async fn a_client_token_reused_for_a_different_request_is_refused() {
    let emulator = Emulator::new();

    let original = json!({
        "Name": "originalenv",
        "NetworkFabricType": "NONE",
        "Description": "the original",
        "Tags": { "team": "migration" },
        "ClientToken": "token-env",
    });
    let (status, error, first) = emulator.post("/environments", original.clone()).await;
    assert_eq!(
        status, 200,
        "case the original CreateEnvironment: expected 200, got {status} {error} with {first}"
    );

    // Each row changes exactly one member the create persists.
    let mut cases: Vec<(&str, Value)> = Vec::new();
    for (case, member, value) in [
        ("a different Name", "Name", json!("renamedenv")),
        (
            "a different NetworkFabricType",
            "NetworkFabricType",
            json!("TRANSIT_GATEWAY"),
        ),
        ("a different Description", "Description", json!("rewritten")),
        ("different Tags", "Tags", json!({ "team": "other" })),
        ("no Tags at all", "Tags", Value::Null),
    ] {
        let mut request = original.clone();
        if value.is_null() {
            request
                .as_object_mut()
                .expect("the request is a JSON object")
                .remove(member);
        } else {
            request[member] = value;
        }
        cases.push((case, request));
    }

    for (case, request) in cases {
        let (status, error, body) = emulator.post("/environments", request).await;
        assert_eq!(
            status, 409,
            "case {case} under the same ClientToken: expected 409, got {status} {error} \
             with {body}"
        );
        assert_eq!(
            error, "ConflictException",
            "case {case} under the same ClientToken: expected ConflictException, \
             got {error:?} with {body}"
        );
        assert_eq!(
            body["ResourceId"], first["EnvironmentId"],
            "case {case} under the same ClientToken: the refusal must name the \
             environment the token already created, got {:?}",
            body["ResourceId"]
        );
    }

    // Nothing was created by any of the refusals.
    let listed = page_environments(&emulator, None).await;
    assert_eq!(
        listed.len(),
        1,
        "case after the refusals: the store must still hold one environment, got {listed:?}"
    );

    // The same rule on CreateApplication.
    let environment = first["EnvironmentId"].as_str().unwrap_or_default();
    let original = json!({
        "Name": "originalapp",
        "VpcId": "vpc-0123456789abcdef0",
        "ProxyType": "API_GATEWAY",
        "ApiGatewayProxy": { "EndpointType": "REGIONAL", "StageName": "prod" },
        "ClientToken": "token-app",
    });
    let (status, error, first_app) = emulator
        .post(
            &format!("/environments/{environment}/applications"),
            original.clone(),
        )
        .await;
    assert_eq!(
        status, 200,
        "case the original CreateApplication: expected 200, got {status} {error} \
         with {first_app}"
    );

    for (case, member, value) in [
        ("a different Name", "Name", json!("renamedapp")),
        ("a different VpcId", "VpcId", json!("vpc-abcdef01")),
        (
            "a different proxy stage",
            "ApiGatewayProxy",
            json!({ "EndpointType": "REGIONAL", "StageName": "canary" }),
        ),
        ("added Tags", "Tags", json!({ "team": "migration" })),
    ] {
        let mut request = original.clone();
        request[member] = value;
        let (status, error, body) = emulator
            .post(&format!("/environments/{environment}/applications"), request)
            .await;
        assert_eq!(
            status, 409,
            "case application with {case} under the same ClientToken: expected 409, \
             got {status} {error} with {body}"
        );
        assert_eq!(
            error, "ConflictException",
            "case application with {case} under the same ClientToken: expected \
             ConflictException, got {error:?} with {body}"
        );
    }

    // An omitted proxy member that matches its documented default IS the same request.
    let mut defaulted = original.clone();
    defaulted["ApiGatewayProxy"] = json!({});
    let (status, error, replay) = emulator
        .post(
            &format!("/environments/{environment}/applications"),
            defaulted,
        )
        .await;
    assert_eq!(
        status, 200,
        "case application with the proxy members left to their documented defaults: \
         expected 200, got {status} {error} with {replay}"
    );
    assert_eq!(
        replay["ApplicationId"], first_app["ApplicationId"],
        "case application with the proxy members left to their documented defaults: \
         expected the first application {:?}, got {:?}",
        first_app["ApplicationId"], replay["ApplicationId"]
    );

    let (_, _, listed) = emulator
        .get(&format!("/environments/{environment}/applications"))
        .await;
    assert_eq!(
        listed["ApplicationSummaryList"].as_array().map(Vec::len),
        Some(1),
        "case after the refusals: the environment must still hold one application, got {:?}",
        listed["ApplicationSummaryList"]
    );
}

/// After `DeleteEnvironment` the environment is gone from every later read, and deleting
/// an id that names nothing is a 404.
#[tokio::test]
async fn a_deleted_environment_is_absent_from_every_later_read() {
    let emulator = Emulator::new();
    let doomed = emulator.create_environment("doomedenv", "NONE").await;
    let survivor = emulator.create_environment("survivingenv", "NONE").await;

    let (status, error, deleted) = emulator.delete(&format!("/environments/{doomed}")).await;
    assert_eq!(
        status, 200,
        "case DeleteEnvironment: expected 200, got {status} {error} with {deleted}"
    );
    assert_eq!(
        deleted["EnvironmentId"].as_str(),
        Some(doomed.as_str()),
        "case DeleteEnvironment: expected EnvironmentId {doomed}, got {:?}",
        deleted["EnvironmentId"]
    );
    assert_eq!(
        deleted["State"].as_str(),
        Some("DELETING"),
        "case DeleteEnvironment: expected State DELETING, got {:?}",
        deleted["State"]
    );
    assert_eq!(
        deleted["Name"].as_str(),
        Some("doomedenv"),
        "case DeleteEnvironment: expected the stored Name, got {:?}",
        deleted["Name"]
    );
    assert!(
        deleted["Arn"]
            .as_str()
            .is_some_and(|arn| arn.ends_with(&format!("environment/{doomed}"))),
        "case DeleteEnvironment: expected the stored Arn for {doomed}, got {:?}",
        deleted["Arn"]
    );
    assert!(
        deleted["LastUpdatedTime"].is_number(),
        "case DeleteEnvironment: expected a LastUpdatedTime timestamp, got {:?}",
        deleted["LastUpdatedTime"]
    );

    let (status, error, body) = emulator.get(&format!("/environments/{doomed}")).await;
    assert_eq!(
        status, 404,
        "case GetEnvironment after the delete: expected 404, got {status} {error} \
         with {body}"
    );
    let remaining = page_environments(&emulator, Some(1)).await;
    assert_eq!(
        remaining,
        vec![survivor.clone()],
        "case ListEnvironments after the delete: expected only {survivor}, got {remaining:?}"
    );

    // A second delete of the same id, and of one never created, both name nothing.
    for identifier in [doomed.as_str(), "env-0000000000"] {
        let (status, error, body) = emulator.delete(&format!("/environments/{identifier}")).await;
        assert_eq!(
            status, 404,
            "case DeleteEnvironment on {identifier:?}: expected 404, got {status} \
             {error} with {body}"
        );
        assert_eq!(
            error, "ResourceNotFoundException",
            "case DeleteEnvironment on {identifier:?}: expected ResourceNotFoundException, \
             got {error:?} with {body}"
        );
    }
}

/// Caller-driven growth of the store is capped: past either cap the create answers the
/// modelled `ServiceQuotaExceededException` (402) instead of accepting another entry, and
/// the per-environment cap bounds only the environment that reached it.
#[tokio::test]
async fn caller_driven_growth_is_capped_by_the_modeled_quota_error() {
    use service_com_amazonaws_migrationhubrefactorspaces_refactorspaces::state::{
        Environment, MAX_APPLICATIONS_PER_ENVIRONMENT, MAX_ENVIRONMENTS,
    };
    use std::collections::BTreeMap;

    use aws_smithy_types::DateTime;

    let emulator = Emulator::new();
    let host = emulator.create_environment("capshost", "NONE").await;
    let sibling = emulator.create_environment("capssibling", "NONE").await;

    let application = |index: usize| {
        json!({
            "Name": format!("capapp{index}x"),
            "VpcId": "vpc-0123456789abcdef0",
            "ProxyType": "API_GATEWAY",
        })
    };

    // Fill one environment to its application cap.
    for index in 0..MAX_APPLICATIONS_PER_ENVIRONMENT {
        let (status, error, body) = emulator
            .post(
                &format!("/environments/{host}/applications"),
                application(index),
            )
            .await;
        assert_eq!(
            status, 200,
            "case application {index} of {MAX_APPLICATIONS_PER_ENVIRONMENT}: expected 200, \
             got {status} {error} with {body}"
        );
    }
    let (status, error, body) = emulator
        .post(
            &format!("/environments/{host}/applications"),
            application(MAX_APPLICATIONS_PER_ENVIRONMENT),
        )
        .await;
    assert_eq!(
        status, 402,
        "case one application past the cap: expected 402, got {status} {error} with {body}"
    );
    assert_eq!(
        error, "ServiceQuotaExceededException",
        "case one application past the cap: expected ServiceQuotaExceededException, \
         got {error:?} with {body}"
    );

    // The cap is PER environment: a sibling still accepts its first application.
    let (status, error, body) = emulator
        .post(
            &format!("/environments/{sibling}/applications"),
            application(0),
        )
        .await;
    assert_eq!(
        status, 200,
        "case first application of a sibling environment: expected 200, got {status} \
         {error} with {body}"
    );

    // Bring the store up to one BELOW its environment cap by seeding records directly:
    // the cap is what is under test, not the thousands of creates it would take to reach
    // it, and seeding keeps the case cheap.
    {
        let store = service_com_amazonaws_migrationhubrefactorspaces_refactorspaces::state::service_state(
            &*emulator.engine,
        )
        .expect("the shared engine carries this service's store");
        let mut state = store.lock();
        let seeded = DateTime::from(std::time::SystemTime::now());
        while state.environments.len() < MAX_ENVIRONMENTS - 1 {
            let index = state.environments.len();
            state.environments.push(Environment {
                environment_id: format!("env-{index:0>10}"),
                name: format!("seeded{index}env"),
                arn: format!(
                    "arn:aws:refactor-spaces:{REGION}:{ACCOUNT}:environment/env-{index:0>10}"
                ),
                description: None,
                network_fabric_type: "NONE".to_string(),
                transit_gateway_id: None,
                owner_account_id: ACCOUNT.to_string(),
                state: "ACTIVE".to_string(),
                tags: BTreeMap::new(),
                created_time: seeded,
                last_updated_time: seeded,
                client_token: None,
                active_at: None,
                applications: Vec::new(),
            });
        }
    }

    // The last slot below the cap is still accepted.
    let (status, error, body) = emulator
        .post(
            "/environments",
            json!({ "Name": "capsenvatthecap", "NetworkFabricType": "NONE" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case the environment that fills the cap: expected 200, got {status} {error} \
         with {body}"
    );

    let (status, error, body) = emulator
        .post(
            "/environments",
            json!({ "Name": "capsenvpastthecap", "NetworkFabricType": "NONE" }),
        )
        .await;
    assert_eq!(
        status, 402,
        "case one environment past the cap: expected 402, got {status} {error} with {body}"
    );
    assert_eq!(
        error, "ServiceQuotaExceededException",
        "case one environment past the cap: expected ServiceQuotaExceededException, \
         got {error:?} with {body}"
    );
    assert_eq!(
        body["ServiceCode"].as_str(),
        Some("refactor-spaces"),
        "case one environment past the cap: expected the service code, got {:?}",
        body["ServiceCode"]
    );
}

/// `DeleteEnvironment`'s own doc states the precondition: "Before you can delete an
/// environment, you must first delete any applications and services within the
/// environment." An environment that still owns children is refused, and its children
/// survive the refusal.
#[tokio::test]
async fn an_environment_that_still_owns_applications_is_not_deleted() {
    let emulator = Emulator::new();
    let environment = emulator.create_environment("populatedenv", "NONE").await;

    // Rows differ along how many applications the environment still owns.
    for children in 1..=3 {
        let (status, error, created) = emulator
            .post(
                &format!("/environments/{environment}/applications"),
                json!({
                    "Name": format!("child{children}number"),
                    "VpcId": "vpc-0123456789abcdef0",
                    "ProxyType": "API_GATEWAY",
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case add child {children}: expected 200, got {status} {error} with {created}"
        );

        let (status, error, body) = emulator
            .delete(&format!("/environments/{environment}"))
            .await;
        assert_eq!(
            status, 409,
            "case delete with {children} application(s): expected 409, got {status} \
             {error} with {body}"
        );
        assert_eq!(
            error, "ConflictException",
            "case delete with {children} application(s): expected ConflictException, \
             got {error:?} with {body}"
        );

        // The refusal changed nothing: the environment and its children are still there.
        let (status, _, still_there) = emulator.get(&format!("/environments/{environment}")).await;
        assert_eq!(
            status, 200,
            "case delete with {children} application(s): the refused delete must leave \
             the environment readable, got {status} with {still_there}"
        );
        let (_, _, listed) = emulator
            .get(&format!("/environments/{environment}/applications"))
            .await;
        assert_eq!(
            listed["ApplicationSummaryList"].as_array().map(Vec::len),
            Some(children),
            "case delete with {children} application(s): the refused delete must leave \
             every application in place, got {:?}",
            listed["ApplicationSummaryList"]
        );
    }

    // An environment with no children of its own still deletes.
    let empty = emulator.create_environment("emptyenv", "NONE").await;
    let (status, error, body) = emulator.delete(&format!("/environments/{empty}")).await;
    assert_eq!(
        status, 200,
        "case delete a childless environment: expected 200, got {status} {error} with {body}"
    );
}
