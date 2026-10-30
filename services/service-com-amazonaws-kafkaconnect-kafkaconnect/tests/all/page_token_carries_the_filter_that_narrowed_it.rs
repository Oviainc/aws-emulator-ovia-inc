// SPDX-License-Identifier: CC0-1.0

//! Which key space a `nextToken` resumes when the REQUEST narrowed the listing,
//! end to end over HTTP.
//!
//! RULE. A page token is a resume point for exactly ONE key space: the operation
//! that minted it, the region it was answered through, and the filter value the
//! minting request carried. A request that filters differently pages a different
//! key space — its keys are not the minting call's keys — so its token is
//! refused there exactly as a token the service never issued is: HTTP 400
//! `BadRequestException` with `Invalid parameter nextToken.`. Honouring it would
//! compare a resume key against foreign keys and answer 200 with a page that
//! silently drops every resource sorting below that key. An absent filter and an
//! empty filter narrow nothing, so those two are the SAME key space and their
//! tokens are interchangeable.
//!
//! The table spans every filtered listing this crate serves
//! (`smithy_query(query="operation", operation="ListCustomPlugins,ListConnectors,
//! ListWorkerConfigurations")`: the filter members are `namePrefix`,
//! `connectorNamePrefix` and `namePrefix`), and for each one the rows differ
//! along the filter the token is replayed under: absent, empty, disjoint, a
//! different spelling selecting the same resources, narrower, exact, and one
//! selecting nothing at all. Each listing is also driven the other way round — a
//! token minted with NO filter, replayed filtered and unfiltered — so the rule is
//! asserted in both directions.
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

use service_com_amazonaws_kafkaconnect_kafkaconnect::state::TRANSITION_SPAN;
use service_com_amazonaws_kafkaconnect_kafkaconnect::EmulatedService;

/// The canonical emulator account id every ARN carries.
const ACCOUNT: &str = "000000000000";

/// The one regional endpoint every case here is answered through: this suite
/// varies the FILTER, and the region dimension has its own suite.
const REGION: &str = "us-east-1";

/// The refusal a listing answers a token that is not a resume point for it with.
const REFUSAL_MESSAGE: &str = "Invalid parameter nextToken.";

/// The filter the narrowed token of every row is minted under.
const MINTED_UNDER: &str = "zz-";

/// The resources every listing's fixture holds, one per name.
///
/// Two names fall OUTSIDE the narrowing and sort before everything inside it, so
/// a resume key from the narrowed listing honoured by the wider one would drop
/// them; three fall inside it, so the narrowed listing itself pages.
const NAMES: [&str; 5] = [
    "aa-outside",
    "aa-support",
    "zz-alpha",
    "zz-beta",
    "zz-gamma",
];

/// The names [`MINTED_UNDER`] selects out of [`NAMES`].
const NARROWED: [&str; 3] = ["zz-alpha", "zz-beta", "zz-gamma"];

/// The custom plugin every connector in the fixture is built from: one of
/// [`NAMES`], so the plugin listing holds exactly the fixture's names.
const SUPPORT_PLUGIN: &str = "aa-support";

/// The filters a token minted under [`MINTED_UNDER`] is replayed under, none of
/// which is the key space that minted it: no filter at all, the empty filter
/// (which narrows nothing, so it is the whole region's key space), a disjoint
/// one, a different spelling that happens to select the same resources, a
/// narrower one, an exact one, and one that selects nothing.
const FOREIGN_FILTERS: [Option<&str>; 7] = [
    None,
    Some(""),
    Some("aa-"),
    Some("zz"),
    Some("zz-a"),
    Some("zz-alpha"),
    Some("qq-"),
];

/// One emulator: a single shared engine behind the production router.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 credential scope naming this service and [`REGION`]. No authenticator
/// is registered, so the signature is never checked; the scope is what routes the
/// request and names the region every minted ARN carries.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/kafkaconnect/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// Send one request and read back `(status, body)`.
async fn call(
    emulator: &MultiServiceRouter,
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
        .uri(format!("https://kafkaconnect.{REGION}.amazonaws.com{path}"))
        .header("authorization", authorization())
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

/// A query-string value, percent-encoded the way an AWS SDK encodes one.
fn encoded(value: &str) -> String {
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

/// One filtered listing in the table: how to drive it, and how to read a page
/// back.
struct Listing {
    /// What an assertion message calls this listing.
    case: &'static str,
    /// Its request path, with no query string.
    path: &'static str,
    /// The query parameter that narrows it.
    filter: &'static str,
    /// The output member carrying the page's items.
    items: &'static str,
    /// The member of a summary carrying the resource's name.
    name: &'static str,
}

/// Every listing of this service a request can narrow by a name prefix.
const LISTINGS: [Listing; 3] = [
    Listing {
        case: "ListCustomPlugins",
        path: "/v1/custom-plugins",
        filter: "namePrefix",
        items: "customPlugins",
        name: "name",
    },
    Listing {
        case: "ListConnectors",
        path: "/v1/connectors",
        filter: "connectorNamePrefix",
        items: "connectors",
        name: "connectorName",
    },
    Listing {
        case: "ListWorkerConfigurations",
        path: "/v1/worker-configurations",
        filter: "namePrefix",
        items: "workerConfigurations",
        name: "name",
    },
];

/// Drive `listing` with the query parameters `params` (values encoded here).
async fn page_of(
    emulator: &MultiServiceRouter,
    listing: &Listing,
    params: &[(&str, &str)],
) -> (u16, Value) {
    let query: Vec<String> = params
        .iter()
        .map(|(name, value)| format!("{name}={}", encoded(value)))
        .collect();
    let path = format!("{}?{}", listing.path, query.join("&"));
    call(emulator, "GET", &path, Value::Null).await
}

/// The names of the resources a page reported.
fn reported(listing: &Listing, body: &Value) -> Vec<String> {
    body.get(listing.items)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| text(summary, listing.name))
        .collect()
}

/// The query parameters that page `listing` at `size`, under `filter` when the
/// row carries one.
fn request(
    listing: &Listing,
    filter: Option<&'static str>,
    size: &'static str,
    token: Option<&str>,
) -> Vec<(&'static str, String)> {
    let mut params: Vec<(&'static str, String)> = vec![("maxResults", size.to_string())];
    if let Some(filter) = filter {
        params.push((listing.filter, filter.to_string()));
    }
    if let Some(token) = token {
        params.push(("nextToken", token.to_string()));
    }
    params
}

/// `request`'s parameters as [`page_of`] takes them.
fn borrowed<'a>(params: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
    params
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect()
}

/// Everything `listing` reports under `filter`, in one unpaged call.
async fn held(
    emulator: &MultiServiceRouter,
    listing: &Listing,
    filter: Option<&'static str>,
) -> BTreeSet<String> {
    let params = request(listing, filter, "100", None);
    let (status, body) = page_of(emulator, listing, &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case ({}, filter {filter:?}): the unpaged listing must succeed, got {status} {body}",
        listing.case
    );
    reported(listing, &body).into_iter().collect()
}

/// POST `body` to `path` and answer the member `arn_member` of the created
/// resource.
async fn create(
    emulator: &MultiServiceRouter,
    path: &str,
    body: Value,
    arn_member: &str,
) -> String {
    let (status, created) = call(emulator, "POST", path, body).await;
    assert_eq!(
        status, 200,
        "case {path}: the create must succeed, got {status} {created}"
    );
    let arn = text(&created, arn_member);
    assert!(
        !arn.is_empty(),
        "case {path}: the create must answer a {arn_member}, got {created}"
    );
    arn
}

/// One custom plugin, one worker configuration and one connector per name in
/// [`NAMES`], so each of the three listings holds exactly those names.
async fn fixture(emulator: &MultiServiceRouter) {
    let mut support_plugin = String::new();
    for name in NAMES {
        let arn = create(
            emulator,
            "/v1/custom-plugins",
            json!({
                "name": name,
                "contentType": "ZIP",
                "location": {"s3Location": {
                    "bucketArn": format!("arn:aws:s3:::plugin-bucket-{name}"),
                    "fileKey": format!("plugins/{name}.zip"),
                }},
            }),
            "customPluginArn",
        )
        .await;
        if name == SUPPORT_PLUGIN {
            support_plugin = arn;
        }
        create(
            emulator,
            "/v1/worker-configurations",
            json!({
                "name": name,
                // Base64 of `key.converter=org.apache.kafka.connect.storage.StringConverter`.
                "propertiesFileContent":
                    "a2V5LmNvbnZlcnRlcj1vcmcuYXBhY2hlLmthZmthLmNvbm5lY3Quc3RvcmFnZS5TdHJpbmdDb252ZXJ0ZXI=",
            }),
            "workerConfigurationArn",
        )
        .await;
    }
    // A connector may only reference a plugin the clock has made ACTIVE.
    past_one_transition().await;

    for name in NAMES {
        create(
            emulator,
            "/v1/connectors",
            json!({
                "connectorName": name,
                "capacity": {"provisionedCapacity": {"mcuCount": 1, "workerCount": 2}},
                "connectorConfiguration": {
                    "connector.class": "io.confluent.connect.s3.S3SinkConnector",
                    "tasks.max": "2",
                },
                "kafkaCluster": {"apacheKafkaCluster": {
                    "bootstrapServers": format!("b-1.{name}.kafka.{REGION}.amazonaws.com:9092"),
                    "vpc": {"subnets": ["subnet-0a1b2c3d"], "securityGroups": ["sg-01234567"]},
                }},
                "kafkaClusterClientAuthentication": {"authenticationType": "NONE"},
                "kafkaClusterEncryptionInTransit": {"encryptionType": "TLS"},
                "kafkaConnectVersion": "2.7.1",
                "plugins": [{"customPlugin": {
                    "customPluginArn": support_plugin,
                    "revision": 1,
                }}],
                "serviceExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/{name}-role"),
            }),
            "connectorArn",
        )
        .await;
    }
}

/// Assert `body` is the refusal a listing answers a token that is not a resume
/// point for it with, and that it carries no page.
fn assert_refused(case: &str, listing: &Listing, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: a token from another key space must be refused, got {status} {body}"
    );
    assert!(
        error_code(body).contains("BadRequestException"),
        "case {case:?}: expected BadRequestException, got {body}"
    );
    assert_eq!(
        text(body, "message"),
        REFUSAL_MESSAGE,
        "case {case:?}: the refusal names the member that carried the token, got {body}"
    );
    assert!(
        body.get(listing.items).is_none(),
        "case {case:?}: a refusal carries no page of items, got {body}"
    );
}

#[tokio::test]
async fn a_page_token_resumes_only_the_filter_that_minted_it() {
    let emulator = emulator();
    fixture(&emulator).await;

    for listing in &LISTINGS {
        let whole = held(&emulator, listing, None).await;
        let expected_whole: BTreeSet<String> = NAMES.iter().map(ToString::to_string).collect();
        assert_eq!(
            whole, expected_whole,
            "case {:?}: the unfiltered listing must report every stored resource, got {whole:?}",
            listing.case
        );
        let narrowed = held(&emulator, listing, Some(MINTED_UNDER)).await;
        let expected_narrowed: BTreeSet<String> = NARROWED.iter().map(ToString::to_string).collect();
        assert_eq!(
            narrowed, expected_narrowed,
            "case {:?}: the filter {MINTED_UNDER:?} must narrow the listing to the names it \
             matches, got {narrowed:?}",
            listing.case
        );

        // Page one of the NARROWED listing: its token resumes after the item it
        // reported, a key with resources of the wider listing sorting below it.
        let params = request(listing, Some(MINTED_UNDER), "1", None);
        let (status, body) = page_of(&emulator, listing, &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the narrowed listing must succeed, got {status} {body}",
            listing.case
        );
        let first = reported(listing, &body);
        assert_eq!(
            first.len(),
            1,
            "case {:?}: maxResults=1 must answer one item, got {body}",
            listing.case
        );
        let narrowed_token = text(&body, "nextToken");
        assert!(
            !narrowed_token.is_empty(),
            "case {:?}: a truncated page must answer a nextToken, got {body}",
            listing.case
        );

        // No OTHER filter honours that token: every one of them pages a key
        // space the resume key says nothing about.
        for foreign in FOREIGN_FILTERS {
            let case = format!(
                "the {MINTED_UNDER:?} token of {:?} replayed under filter {foreign:?}",
                listing.case
            );
            let params = request(listing, foreign, "100", Some(&narrowed_token));
            let (status, body) = page_of(&emulator, listing, &borrowed(&params)).await;
            assert_refused(&case, listing, status, &body);
        }

        // Its OWN filter resumes it: the rest of the narrowed listing, and the
        // two pages together cover it exactly once each.
        let params = request(listing, Some(MINTED_UNDER), "100", Some(&narrowed_token));
        let (status, body) = page_of(&emulator, listing, &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: a listing resumes its own token, got {status} {body}",
            listing.case
        );
        let rest = reported(listing, &body);
        assert!(
            rest.iter().all(|name| !first.contains(name)),
            "case {:?}: expected the items page one left ({first:?} already seen), got {rest:?}",
            listing.case
        );
        let covered: BTreeSet<String> = first.iter().cloned().chain(rest.iter().cloned()).collect();
        assert_eq!(
            covered, narrowed,
            "case {:?}: page one plus the resumed page must cover the narrowed listing, got \
             {covered:?}",
            listing.case
        );
        assert!(
            text(&body, "nextToken").is_empty(),
            "case {:?}: the last page of the narrowed listing carries no nextToken, got {body}",
            listing.case
        );

        // The other direction: a token minted with NO filter. An absent and an
        // empty filter narrow nothing, so both resume it; a narrowing one pages
        // other keys, so it does not.
        let params = request(listing, None, "1", None);
        let (status, body) = page_of(&emulator, listing, &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the unfiltered listing must succeed, got {status} {body}",
            listing.case
        );
        let first_whole = reported(listing, &body);
        let whole_token = text(&body, "nextToken");
        assert!(
            !whole_token.is_empty(),
            "case {:?}: a truncated unfiltered page must answer a nextToken, got {body}",
            listing.case
        );
        for unnarrowed in [None, Some("")] {
            let case = format!(
                "the unfiltered token of {:?} replayed under filter {unnarrowed:?}",
                listing.case
            );
            let params = request(listing, unnarrowed, "100", Some(&whole_token));
            let (status, body) = page_of(&emulator, listing, &borrowed(&params)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a filter that narrows nothing is the same key space, got \
                 {status} {body}"
            );
            let rest = reported(listing, &body);
            let covered: BTreeSet<String> =
                first_whole.iter().cloned().chain(rest.iter().cloned()).collect();
            assert_eq!(
                covered, whole,
                "case {case:?}: page one plus the resumed page must cover every stored resource, \
                 got {covered:?}"
            );
        }
        let case = format!(
            "the unfiltered token of {:?} replayed under filter {MINTED_UNDER:?}",
            listing.case
        );
        let params = request(listing, Some(MINTED_UNDER), "100", Some(&whole_token));
        let (status, body) = page_of(&emulator, listing, &borrowed(&params)).await;
        assert_refused(&case, listing, status, &body);

        // Every refusal changed nothing: the listing still holds what it held.
        let still = held(&emulator, listing, None).await;
        assert_eq!(
            still, whole,
            "case {:?}: a refused token must hide nothing, got {still:?}",
            listing.case
        );
    }
}
