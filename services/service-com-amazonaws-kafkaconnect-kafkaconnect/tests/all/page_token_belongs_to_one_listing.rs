// SPDX-License-Identifier: CC0-1.0

//! Which listing a `nextToken` resumes, end to end over HTTP.
//!
//! RULE. An opaque `nextToken` is a resume point for exactly ONE listing: the
//! operation that minted it, over the key space that call paged — the region the
//! listing is answered through, and the parent resource where the listing is of
//! one resource's children. Handed to any OTHER listing it is refused exactly as
//! a token the service never issued is: HTTP 400 `BadRequestException` with
//! `Invalid parameter nextToken.`. It is never honoured as a resume point,
//! because a key from another listing's key space either hides the resources
//! this listing holds (every key sorting below it is skipped) or replays a page
//! the caller already consumed (no key sorts below it).
//!
//! The table spans every paginated listing of this crate
//! (`smithy_query(query="digest")`: ListConnectors, ListCustomPlugins,
//! ListWorkerConfigurations, ListConnectorOperations), each driven through TWO
//! regional endpoints, and the per-connector listing driven against TWO
//! connectors — so the rows differ along the operation, the region and the
//! parent resource the rule generalises over. Every ordered pair of listings is
//! exercised: a listing's own token resumes it, and no other's does.
//!
//! Every case drives the emulator the way a client does: a buffered `restJson1`
//! request through the same router the production binary assembles, with the
//! response body read back as JSON. Nothing reaches into the state struct.
//!
//! The sibling suites' request helpers are private to their own modules, so this
//! one carries its own copies of the few it needs.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kafkaconnect_kafkaconnect::state::{RESTART_PROGRESSION, TRANSITION_SPAN};
use service_com_amazonaws_kafkaconnect_kafkaconnect::EmulatedService;

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The two regional endpoints every listing is driven through: a token minted
/// through one is not a resume point for the other, which holds its own
/// independent resources.
const REGIONS: [&str; 2] = ["us-east-1", "eu-west-1"];

/// The refusal a listing answers a token that is not a resume point for it with.
const REFUSAL_MESSAGE: &str = "Invalid parameter nextToken.";

/// One emulator: a single shared engine behind the production router.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
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

/// Long enough that one change the clock owes after a call is due. The span is
/// the service's own constant, so this cannot drift from it.
async fn past_one_transition() {
    tokio::time::sleep(TRANSITION_SPAN + Duration::from_millis(400)).await;
}

/// Long enough that a restart's whole progression is due: one span per state it
/// passes through, both the service's own constants.
async fn past_the_whole_restart() {
    let steps = u32::try_from(RESTART_PROGRESSION.len()).expect("a small progression");
    tokio::time::sleep(TRANSITION_SPAN * steps + Duration::from_millis(400)).await;
}

/// The `CreateCustomPlugin` body for `name`.
fn plugin_request(name: &str) -> Value {
    json!({
        "name": name,
        "contentType": "ZIP",
        "location": {"s3Location": {
            "bucketArn": format!("arn:aws:s3:::plugin-bucket-{name}"),
            "fileKey": format!("plugins/{name}.zip"),
        }},
    })
}

/// The `CreateConnector` body for `name` in `region`, referencing `plugin_arn`.
fn connector_request(name: &str, region: &str, plugin_arn: &str) -> Value {
    json!({
        "connectorName": name,
        "capacity": {"provisionedCapacity": {"mcuCount": 1, "workerCount": 2}},
        "connectorConfiguration": {
            "connector.class": "io.confluent.connect.s3.S3SinkConnector",
            "tasks.max": "2",
        },
        "kafkaCluster": {"apacheKafkaCluster": {
            "bootstrapServers": format!("b-1.{name}.kafka.{region}.amazonaws.com:9092"),
            "vpc": {"subnets": ["subnet-0a1b2c3d"], "securityGroups": ["sg-01234567"]},
        }},
        "kafkaClusterClientAuthentication": {"authenticationType": "NONE"},
        "kafkaClusterEncryptionInTransit": {"encryptionType": "TLS"},
        "kafkaConnectVersion": "2.7.1",
        "plugins": [{"customPlugin": {"customPluginArn": plugin_arn, "revision": 1}}],
        "serviceExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/{name}-role"),
    })
}

/// POST `body` to `path` in `region` and answer the member `arn_member` of the
/// created resource.
async fn create(
    emulator: &MultiServiceRouter,
    region: &str,
    path: &str,
    body: Value,
    arn_member: &str,
) -> String {
    let (status, created) = call(emulator, region, "POST", path, body).await;
    assert_eq!(
        status, 200,
        "case ({region}, {path}): the create must succeed, got {status} {created}"
    );
    let arn = text(&created, arn_member);
    assert!(
        !arn.is_empty(),
        "case ({region}, {path}): the create must answer a {arn_member}, got {created}"
    );
    arn
}

/// One listing in the table: how to drive it, and how to read its page back.
struct Listing {
    /// What an assertion message calls this listing.
    case: String,
    /// The regional endpoint it is answered through.
    region: &'static str,
    /// Its request path, with no query string.
    path: String,
    /// The output member carrying the page's items.
    items: &'static str,
    /// The member of a summary that identifies the resource it reports.
    id: &'static str,
}

/// Drive `listing` with the query string `query` (appended as-is).
async fn page_of(emulator: &MultiServiceRouter, listing: &Listing, query: &str) -> (u16, Value) {
    let path = format!("{}{query}", listing.path);
    call(emulator, listing.region, "GET", &path, Value::Null).await
}

/// The identifiers of the resources a page reported.
fn reported(listing: &Listing, body: &Value) -> Vec<String> {
    body.get(listing.items)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| text(summary, listing.id))
        .collect()
}

/// Build the whole fixture: in BOTH regions two custom plugins, two worker
/// configurations and two connectors, and in the first region two connector
/// operations against each of two connectors — so every listing in the table
/// holds at least two pages at `maxResults=1`.
///
/// Answers the table of listings.
async fn fixture(emulator: &MultiServiceRouter) -> Vec<Listing> {
    let mut plugins: Vec<(&'static str, String)> = Vec::new();
    for region in REGIONS {
        for index in 0..2 {
            let name = format!("plugin-{region}-{index}");
            let arn = create(
                emulator,
                region,
                "/v1/custom-plugins",
                plugin_request(&name),
                "customPluginArn",
            )
            .await;
            if index == 0 {
                plugins.push((region, arn));
            }
        }
        for index in 0..2 {
            create(
                emulator,
                region,
                "/v1/worker-configurations",
                json!({
                    "name": format!("worker-{region}-{index}"),
                    // Base64 of `key.converter=org.apache.kafka.connect.storage.StringConverter`.
                    "propertiesFileContent":
                        "a2V5LmNvbnZlcnRlcj1vcmcuYXBhY2hlLmthZmthLmNvbm5lY3Quc3RvcmFnZS5TdHJpbmdDb252ZXJ0ZXI=",
                }),
                "workerConfigurationArn",
            )
            .await;
        }
    }
    // A connector may only reference a plugin the clock has made ACTIVE.
    past_one_transition().await;

    let mut connectors: Vec<(&'static str, String)> = Vec::new();
    for (region, plugin_arn) in &plugins {
        for index in 0..2 {
            let name = format!("connector-{region}-{index}");
            let arn = create(
                emulator,
                region,
                "/v1/connectors",
                connector_request(&name, region, plugin_arn),
                "connectorArn",
            )
            .await;
            connectors.push((region, arn));
        }
    }
    // Only a RUNNING connector can be restarted.
    past_one_transition().await;

    // The two connectors whose operations are listed: two restarts each, so each
    // of those listings pages. The restarts of the two connectors run side by
    // side, and a connector is RESTARTING until its restart has finished.
    let paged: Vec<&String> = connectors
        .iter()
        .filter(|(region, _)| *region == REGIONS[0])
        .map(|(_, arn)| arn)
        .collect();
    for round in 0..2 {
        for connector_arn in &paged {
            let (status, restarted) = call(
                emulator,
                REGIONS[0],
                "POST",
                &format!("/v1/connectors/{}/restart", label(connector_arn)),
                Value::Null,
            )
            .await;
            assert_eq!(
                status, 200,
                "case (round {round}, {connector_arn}): the restart must succeed, got \
                 {status} {restarted}"
            );
        }
        past_the_whole_restart().await;
    }

    let mut listings = Vec::new();
    for region in REGIONS {
        listings.push(Listing {
            case: format!("ListConnectors in {region}"),
            region,
            path: "/v1/connectors".to_string(),
            items: "connectors",
            id: "connectorArn",
        });
        listings.push(Listing {
            case: format!("ListCustomPlugins in {region}"),
            region,
            path: "/v1/custom-plugins".to_string(),
            items: "customPlugins",
            id: "customPluginArn",
        });
        listings.push(Listing {
            case: format!("ListWorkerConfigurations in {region}"),
            region,
            path: "/v1/worker-configurations".to_string(),
            items: "workerConfigurations",
            id: "workerConfigurationArn",
        });
    }
    for connector_arn in paged {
        listings.push(Listing {
            case: format!("ListConnectorOperations of {connector_arn}"),
            region: REGIONS[0],
            path: format!("/v1/connectors/{}/operations", label(connector_arn)),
            items: "connectorOperations",
            id: "connectorOperationArn",
        });
    }
    listings
}

#[tokio::test]
async fn a_token_resumes_the_listing_that_minted_it_and_no_other() {
    let emulator = emulator();
    let listings = fixture(&emulator).await;

    // Page one of every listing, at the smallest page the model allows, so each
    // answers a token and the resources it has NOT reported yet are known.
    let mut minted: Vec<(usize, String, Vec<String>, BTreeSet<String>)> = Vec::new();
    for (index, listing) in listings.iter().enumerate() {
        let (status, body) = page_of(&emulator, listing, "?maxResults=1").await;
        assert_eq!(
            status, 200,
            "case {:?}: the listing must succeed, got {status} {body}",
            listing.case
        );
        let first = reported(listing, &body);
        assert_eq!(
            first.len(),
            1,
            "case {:?}: maxResults=1 must answer one item, got {body}",
            listing.case
        );
        let token = text(&body, "nextToken");
        assert!(
            !token.is_empty(),
            "case {:?}: a truncated page must answer a nextToken, got {body}",
            listing.case
        );

        // Everything this listing holds, so the pages after the token can be
        // checked against what page one left.
        let (status, all) = page_of(&emulator, listing, "").await;
        assert_eq!(
            status, 200,
            "case {:?}: the unpaged listing must succeed, got {status} {all}",
            listing.case
        );
        let held: BTreeSet<String> = reported(listing, &all).into_iter().collect();
        assert!(
            held.len() >= 2,
            "case {:?}: the fixture must hold at least two items for paging, got {all}",
            listing.case
        );
        minted.push((index, token, first, held));
    }

    for (minter_index, token, first, _) in &minted {
        let minter = &listings[*minter_index];
        for (user_index, _, _, held) in &minted {
            let user = &listings[*user_index];
            let case = format!("the token of {:?} handed to {:?}", minter.case, user.case);
            let (status, body) =
                page_of(&emulator, user, &format!("?nextToken={}", label(token))).await;

            if minter_index == user_index {
                // Its OWN listing resumes: the page continues after the item page
                // one reported, reporting only items page one left, and together
                // the two pages cover everything the listing holds.
                assert_eq!(
                    status, 200,
                    "case {case:?}: a listing resumes its own token, got {status} {body}"
                );
                let rest: Vec<String> = reported(user, &body);
                assert!(
                    !rest.is_empty() && rest.iter().all(|id| !first.contains(id)),
                    "case {case:?}: expected the items page one left ({:?} already seen), got \
                     {rest:?}",
                    first
                );
                let covered: BTreeSet<String> =
                    first.iter().cloned().chain(rest.iter().cloned()).collect();
                assert_eq!(
                    &covered, held,
                    "case {case:?}: the resumed page plus page one must cover every stored item, \
                     got {covered:?}"
                );
            } else {
                // Any OTHER listing's token is not a resume point here: the
                // modelled 400, blaming the member that carried it — never a
                // page, empty or otherwise.
                assert_eq!(
                    status, 400,
                    "case {case:?}: a foreign token must be refused, got {status} {body}"
                );
                assert!(
                    error_code(&body).contains("BadRequestException"),
                    "case {case:?}: expected BadRequestException, got {body}"
                );
                assert_eq!(
                    text(&body, "message"),
                    REFUSAL_MESSAGE,
                    "case {case:?}: the refusal names the member that carried the token, got {body}"
                );
                assert!(
                    body.get(user.items).is_none(),
                    "case {case:?}: a refusal carries no page of items, got {body}"
                );
            }
        }
    }

    // A refused token changes nothing: every listing still reports everything it
    // held before the foreign tokens were handed to it.
    for (index, _, _, held) in &minted {
        let listing = &listings[*index];
        let (status, body) = page_of(&emulator, listing, "").await;
        assert_eq!(
            status, 200,
            "case {:?}: the listing must still succeed, got {status} {body}",
            listing.case
        );
        let still: BTreeSet<String> = reported(listing, &body).into_iter().collect();
        assert_eq!(
            &still, held,
            "case {:?}: a refused token must hide nothing, got {body}",
            listing.case
        );
    }
}
