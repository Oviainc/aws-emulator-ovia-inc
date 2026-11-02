// SPDX-License-Identifier: CC0-1.0

//! Account associations, end to end over the wire: `CreateAccountAssociation` ->
//! `GetAccountAssociation` -> `ListAccountAssociations`.
//!
//! RULE: `ListAccountAssociations` (GET `/account-associations`, `@readonly`) reports the
//! account associations the caller's region holds, in the order they were created,
//! projecting each STORED record onto an `AccountAssociationItem`; `ConnectorDestinationId`
//! selects by equality on the destination a record was created against; `MaxResults` cuts
//! the page at that many items and the `NextToken` it answers with resumes exactly where
//! the page stopped, so the token chain reports every record exactly once. A value outside
//! a modelled constraint — a `MaxResults` outside its `@range` (1..1000), a `NextToken` this
//! service never issued — is the modelled `ValidationException` (HTTP 400), and an empty
//! store answers an empty page with no token.
//!
//! The recorded unit this was derived from
//! (`iot-managed-integrations.list-account-associations-max-results-out-of-range-rejected`)
//! is ONE row of the tables below. The rows differ along what the rule generalises over:
//! the number of stored records, the page size (both modelled bounds and inside them), the
//! destination filtered for, the members each record carries, the region the request is
//! signed for, and the form of the rejected `MaxResults` and `NextToken`.
//!
//! `CreateAccountAssociation` (POST `/account-associations`, success 201) is what writes
//! those records and `GetAccountAssociation` (GET `/account-associations/{Id}`) reads one
//! back, so the list is asserted against the records a client really created rather than
//! against a seeded store. Every case drives the real restJson1 request path
//! (`MultiServiceRouter::dispatch`), so the routes, statuses, bodies and modelled errors
//! asserted here are the ones a client sees.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::state::{
    ACCOUNT_ASSOCIATION_PAGE_LIMIT, ACCOUNT_ASSOCIATION_RESOURCE_TYPE, ASSOCIATION_STATE_VALUES,
    INITIAL_ASSOCIATION_STATE,
};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's requests are signed for unless a case names another.
const REGION: &str = "us-east-1";

/// The collection route `CreateAccountAssociation` and `ListAccountAssociations` are bound
/// to (`smithy_query(query="operation",operation="ListAccountAssociations")` -> GET
/// `/account-associations`).
const COLLECTION: &str = "/account-associations";

/// The collection routes a connector destination and its C2C connector are created on
/// (`smithy_query(query="operation",operation="CreateConnectorDestination")` -> POST
/// `/connector-destinations`, `CreateCloudConnector` -> POST `/cloud-connectors`).
const CONNECTOR_DESTINATIONS: &str = "/connector-destinations";
const CLOUD_CONNECTORS: &str = "/cloud-connectors";

/// The wire error codes the model's `@httpError` shapes are rendered under.
const VALIDATION: &str = "ValidationException";
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The `@range` bounds of `MaxResults`
/// (`smithy_query(query="operation",operation="ListAccountAssociations")` -> `MaxResults`
/// range 1..1000).
const MAX_RESULTS_BOUNDS: (i64, i64) = (1, 1000);

/// The `@length` bounds of `AccountAssociationId`, in characters
/// (`smithy_query(query="shape",shape="AccountAssociationId")` -> length 1..64, pattern
/// `^[0-9a-zA-Z]+$`).
const ACCOUNT_ASSOCIATION_ID_BOUNDS: (usize, usize) = (1, 64);

/// The `@length` minimum of `AccountAssociationArn`
/// (`smithy_query(query="shape",shape="AccountAssociationItem")` -> `Arn` length 67..1011).
const ACCOUNT_ASSOCIATION_ARN_MIN_LENGTH: usize = 67;

/// One HTTP answer, reduced to what the assertions read.
struct Reply {
    status: u16,
    /// The restJson1 error code, from `x-amzn-errortype`.
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    fn member(&self, name: &str) -> Option<&Value> {
        self.body.get(name).filter(|value| !value.is_null())
    }

    fn string(&self, name: &str) -> Option<&str> {
        self.member(name).and_then(Value::as_str)
    }

    /// The reason an error body carries, on the member the model declares for it.
    fn message(&self) -> &str {
        self.string("Message")
            .or_else(|| self.string("message"))
            .unwrap_or_default()
    }

    /// The `Items` of a list answer, as `(AccountAssociationId, member)` is read below.
    fn items(&self) -> Vec<&Value> {
        self.member("Items")
            .and_then(Value::as_array)
            .map(|items| items.iter().collect())
            .unwrap_or_default()
    }

    /// The ids a list answer reports, in the order it reported them.
    fn item_ids(&self) -> Vec<String> {
        self.items()
            .iter()
            .map(|item| {
                item.get("AccountAssociationId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }
}

/// A router holding one `iotmanagedintegrations` service over a fresh shared engine: the
/// same assembly `core/router` performs, with one service mounted.
fn emulator() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // The signature itself is not under test here, so requests are unauthenticated.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::new(engine::InMemoryEngine::new())));
    router
}

/// Send one request, signed for `region`, and reduce the answer. The `Authorization`
/// credential scope is what routes the request to this service and names its region.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Reply {
    let payload = body.map_or_else(bytes::Bytes::new, |value| {
        bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize the request body"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://iotmanagedintegrations.{region}.amazonaws.com{path_and_query}"
        ))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{region}/\
                 iotmanagedintegrations/aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply {
        status,
        error_type,
        body,
    }
}

/// `CreateAccountAssociation` with `body`, signed for `region`.
async fn create_in(router: &MultiServiceRouter, region: &str, body: Value) -> Reply {
    call_in(router, region, "POST", COLLECTION, Some(body)).await
}

/// [`create_in`] in [`REGION`].
async fn create(router: &MultiServiceRouter, body: Value) -> Reply {
    create_in(router, REGION, body).await
}

/// `GetAccountAssociation` for `id`, signed for `region`.
async fn get_in(router: &MultiServiceRouter, region: &str, id: &str) -> Reply {
    call_in(router, region, "GET", &format!("{COLLECTION}/{id}"), None).await
}

/// [`get_in`] in [`REGION`].
async fn get(router: &MultiServiceRouter, id: &str) -> Reply {
    get_in(router, REGION, id).await
}

/// `ListAccountAssociations` with the raw `query` (empty for none), signed for `region`.
async fn list_in(router: &MultiServiceRouter, region: &str, query: &str) -> Reply {
    call_in(
        router,
        region,
        "GET",
        &format!("{COLLECTION}{query}"),
        None,
    )
    .await
}

/// [`list_in`] in [`REGION`].
async fn list(router: &MultiServiceRouter, query: &str) -> Reply {
    list_in(router, REGION, query).await
}

/// Create a connector destination in `region` through the wire and answer the
/// `ConnectorDestinationId` it minted.
///
/// `CreateAccountAssociation` resolves its `ConnectorDestinationId` against this service's
/// own connector-destination store, so every association below is created against a
/// destination a client really created rather than against an invented id; a connector
/// destination in turn names a C2C connector (`CloudConnectorId` is `@required`), so the
/// connector comes first. The id is MINTED by the service, which is why no case picks one.
async fn connector_destination_in(router: &MultiServiceRouter, region: &str) -> String {
    let connector = call_in(
        router,
        region,
        "POST",
        CLOUD_CONNECTORS,
        Some(json!({
            "Name": "partner cloud connector",
            "EndpointConfig": {"lambda": {"arn": "connector-handler"}},
        })),
    )
    .await;
    assert_eq!(
        connector.status, 201,
        "seeding a cloud connector in {region:?}: expected HTTP 201, got {} with body {:?}",
        connector.status, connector.body
    );
    let connector_id = connector.string("Id").unwrap_or_default().to_string();
    let destination = call_in(
        router,
        region,
        "POST",
        CONNECTOR_DESTINATIONS,
        Some(json!({
            "CloudConnectorId": connector_id,
            "AuthConfig": {},
            // The service requires the ARN of the secret a destination keeps its
            // credentials in. Inside the `SecretsManagerArn` `@pattern`
            // (`^arn:aws:secretsmanager:[0-9a-zA-Z-]{1,32}:\d{12}:secret:[A-Za-z0-9/_+=.@-]{8,520}$`)
            // and the `SecretsManagerVersionId` `@length` (32..64).
            "SecretsManager": {
                "arn": format!(
                    "arn:aws:secretsmanager:{region}:{}:secret:destination-A1b2C3",
                    engine::EMULATOR_ACCOUNT_ID
                ),
                "versionId": "a".repeat(32),
            },
        })),
    )
    .await;
    assert_eq!(
        destination.status, 201,
        "seeding a connector destination in {region:?}: expected HTTP 201, got {} with \
         body {:?}",
        destination.status, destination.body
    );
    destination.string("Id").unwrap_or_default().to_string()
}

/// [`connector_destination_in`] in [`REGION`].
async fn connector_destination(router: &MultiServiceRouter) -> String {
    connector_destination_in(router, REGION).await
}

/// The minimal create request body: only the `@required` `ConnectorDestinationId`.
fn minimal(connector_destination_id: &str) -> Value {
    json!({"ConnectorDestinationId": connector_destination_id})
}

/// Assert one create was accepted, and answer the `AccountAssociationId` it minted —
/// checking the answer against the members the model declares for it.
fn accepted_id(case: &str, reply: &Reply) -> String {
    assert_eq!(
        reply.status, 201,
        "case {case:?}: expected HTTP 201, got {} with body {:?}",
        reply.status, reply.body
    );
    let id = reply
        .string("AccountAssociationId")
        .unwrap_or_default()
        .to_string();
    let (min, max) = ACCOUNT_ASSOCIATION_ID_BOUNDS;
    assert!(
        (min..=max).contains(&id.chars().count()) && id.chars().all(|c| c.is_ascii_alphanumeric()),
        "case {case:?}: expected an AccountAssociationId of {min}..={max} alphanumerics, \
         got {id:?} in body {:?}",
        reply.body
    );
    assert_eq!(
        reply.string("AssociationState"),
        Some(INITIAL_ASSOCIATION_STATE),
        "case {case:?}: expected a fresh association to report {INITIAL_ASSOCIATION_STATE:?}, \
         got {:?}",
        reply.string("AssociationState")
    );
    // The association has no third-party OAuth authorization server to name, so the member
    // carries its modelled empty default rather than an invented URL.
    assert!(
        reply
            .string("OAuthAuthorizationUrl")
            .is_none_or(str::is_empty),
        "case {case:?}: expected no OAuthAuthorizationUrl, got {:?}",
        reply.string("OAuthAuthorizationUrl")
    );
    assert_arn(case, reply.string("Arn"), &id);
    id
}

/// Assert an `Arn` is the account-association ARN of `id`: the modelled
/// `AccountAssociationArn` pattern, with the resource segment naming this id.
fn assert_arn(case: &str, arn: Option<&str>, id: &str) {
    let arn = arn.unwrap_or_default();
    assert!(
        arn.len() >= ACCOUNT_ASSOCIATION_ARN_MIN_LENGTH
            && arn.starts_with("arn:aws:iotmanagedintegrations:")
            && arn.ends_with(&format!(":account-association/{id}")),
        "case {case:?}: expected an AccountAssociationArn of at least \
         {ACCOUNT_ASSOCIATION_ARN_MIN_LENGTH} characters naming account-association/{id}, \
         got {arn:?}"
    );
}

/// Assert one answer is the modelled refusal `status`/`code` carrying a reason.
fn assert_refused(case: &str, reply: &Reply, status: u16, code: &str) {
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some(code),
        "case {case:?}: expected the modelled {code} code, got {:?}",
        reply.error_type
    );
    assert!(
        !reply.message().is_empty(),
        "case {case:?}: expected the modelled Message member to carry a reason, got body {:?}",
        reply.body
    );
}

/// Assert `reply` is a 200 list answer whose items are exactly `expected`, in order.
fn assert_lists(case: &str, reply: &Reply, expected: &[String]) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected HTTP 200, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.item_ids(),
        expected,
        "case {case:?}: expected the stored records {expected:?} in insertion order, \
         got body {:?}",
        reply.body
    );
}

/// RULE: a create stores every member it was given and the reads project the STORED record
/// — `GetAccountAssociation` reports all of them, `ListAccountAssociations` the ones
/// `AccountAssociationItem` declares — so what a create wrote comes back on both paths.
#[tokio::test]
async fn a_create_stores_the_record_and_the_reads_project_it() {
    let router = emulator();

    // The rows differ along which optional members the create carries and along the form
    // and length of each one: only the required destination id, a named association, one
    // with a description, tags, general authorization, and every member at once. The
    // destination each row is created against is seeded through the API and its id is the
    // service's to mint, so the rows carry no destination id of their own.
    let members: Vec<(&str, Value)> = vec![
        ("required only", json!({})),
        ("name only", json!({"Name": "My Association"})),
        (
            "description only",
            json!({"Description": "an account association for the second destination"}),
        ),
        ("tags only", json!({"Tags": {"env": "test", "owner": "fixer"}})),
        (
            "general authorization only",
            json!({"GeneralAuthorization": {"AuthMaterialName": "material-1"}}),
        ),
        (
            "every member",
            json!({
                "Name": "N",
                "Description": "D",
                "Tags": {"only": "one"},
                "GeneralAuthorization": {"AuthMaterialName": "m+a/t=e-r_1"},
            }),
        ),
    ];

    // Each row associates with its OWN connector destination, so the records differ along
    // the destination they point at as well as along their members.
    let mut cases: Vec<(&str, Value)> = Vec::new();
    for (case, mut body) in members {
        body["ConnectorDestinationId"] = json!(connector_destination(&router).await);
        cases.push((case, body));
    }

    let mut minted: BTreeSet<String> = BTreeSet::new();
    let mut created: Vec<(String, String, Value)> = Vec::new();
    for (case, body) in &cases {
        let reply = create(&router, body.clone()).await;
        let id = accepted_id(case, &reply);
        assert!(
            minted.insert(id.clone()),
            "case {case:?}: every create mints its own identifier, but {id:?} was minted twice"
        );

        // The get reports every member the record stores.
        let read = get(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from the get, got {} with body {:?}",
            read.status, read.body
        );
        for member in ["ConnectorDestinationId", "Name", "Description"] {
            assert_eq!(
                read.string(member).map(str::to_string),
                body.get(member)
                    .and_then(Value::as_str)
                    .map(str::to_string),
                "case {case:?}: expected the get to report the stored {member}, got {:?}",
                read.member(member)
            );
        }
        for member in ["Tags", "GeneralAuthorization"] {
            assert_eq!(
                read.member(member),
                body.get(member),
                "case {case:?}: expected the get to report the stored {member}, got {:?}",
                read.member(member)
            );
        }
        assert_eq!(
            read.string("AssociationState"),
            Some(INITIAL_ASSOCIATION_STATE),
            "case {case:?}: expected the get to report {INITIAL_ASSOCIATION_STATE:?}, got {:?}",
            read.string("AssociationState")
        );
        assert!(
            read.member("ErrorMessage").is_none(),
            "case {case:?}: an association that has not failed carries no ErrorMessage, got {:?}",
            read.member("ErrorMessage")
        );
        assert_arn(case, read.string("Arn"), &id);
        created.push((case.to_string(), id, body.clone()));
    }

    // One list answer must carry the same projection of every record, in insertion order.
    let listed = list(&router, "").await;
    let expected: Vec<String> = created.iter().map(|(_, id, _)| id.clone()).collect();
    assert_lists("every record", &listed, &expected);
    for (item, (case, id, body)) in listed.items().iter().zip(created.iter()) {
        // The members `AccountAssociationItem` declares, each carrying the stored value.
        assert_eq!(
            item.get("AssociationState").and_then(Value::as_str),
            Some(INITIAL_ASSOCIATION_STATE),
            "case {case:?}: expected the item to report {INITIAL_ASSOCIATION_STATE:?}, \
             got {item:?}"
        );
        assert!(
            ASSOCIATION_STATE_VALUES
                .contains(&item.get("AssociationState").and_then(Value::as_str).unwrap_or_default()),
            "case {case:?}: expected an AssociationState the model declares, got {item:?}"
        );
        for member in ["ConnectorDestinationId", "Name", "Description"] {
            assert_eq!(
                item.get(member).and_then(Value::as_str),
                body.get(member).and_then(Value::as_str),
                "case {case:?}: expected the item to report the stored {member}, got {item:?}"
            );
        }
        assert_arn(case, item.get("Arn").and_then(Value::as_str), id);
        // `AccountAssociationItem` declares no tags, so the summary carries none even
        // where the record has them.
        assert!(
            item.get("Tags").is_none(),
            "case {case:?}: AccountAssociationItem declares no Tags member, got {item:?}"
        );
    }

    // A list is a pure read: repeating it reports the same page.
    let relisted = list(&router, "").await;
    assert_lists("every record, reread", &relisted, &expected);
}

/// RULE: a list reports the stored records in insertion order, `MaxResults` cuts the page
/// at that many items, and the `NextToken` it answers with resumes exactly where the page
/// stopped — so the token chain reports every record exactly once and the last page carries
/// no token. An empty store answers an empty page with no token.
#[tokio::test]
async fn a_list_pages_over_the_store_in_insertion_order() {
    let router = emulator();

    let empty = list(&router, "").await;
    assert_lists("empty store", &empty, &[]);
    assert!(
        empty.member("NextToken").is_none(),
        "case \"empty store\": an empty store answers no NextToken, got {:?}",
        empty.member("NextToken")
    );

    // Seven records, so the page sizes below divide the store evenly and unevenly. They
    // are spread over two connector destinations, so insertion order is not the order of
    // the destinations they point at.
    let destinations = [
        connector_destination(&router).await,
        connector_destination(&router).await,
    ];
    let mut inserted: Vec<String> = Vec::new();
    for n in 0..7usize {
        let reply = create(&router, minimal(&destinations[n % destinations.len()])).await;
        inserted.push(accepted_id(&format!("seed-{n}"), &reply));
    }

    let unpaged = list(&router, "").await;
    assert_lists("no MaxResults", &unpaged, &inserted);
    assert!(
        unpaged.member("NextToken").is_none(),
        "case \"no MaxResults\": a complete page carries no NextToken, got {:?}",
        unpaged.member("NextToken")
    );

    // Each page size: walking the token chain must visit every id exactly once, in
    // insertion order, and the last page must carry no token. 1 and 1000 are the modelled
    // bounds of `MaxResults`.
    let (min_results, max_results) = MAX_RESULTS_BOUNDS;
    for page_size in [min_results, 2, 3, 7, 8, max_results] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= inserted.len() + 1,
                "case {case:?}: paging did not terminate after {pages} pages, saw {seen:?}"
            );
            let query = match &token {
                None => format!("?MaxResults={page_size}"),
                Some(next) => format!("?MaxResults={page_size}&NextToken={next}"),
            };
            let page = list(&router, &query).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: expected HTTP 200, got {} with body {:?}",
                page.status, page.body
            );
            let page_ids = page.item_ids();
            assert!(
                page_ids.len() <= usize::try_from(page_size).unwrap_or(usize::MAX),
                "case {case:?}: a page holds at most MaxResults items, got {} in {page_ids:?}",
                page_ids.len()
            );
            assert!(
                page_ids.len() <= ACCOUNT_ASSOCIATION_PAGE_LIMIT,
                "case {case:?}: a page holds at most {ACCOUNT_ASSOCIATION_PAGE_LIMIT} items, \
                 got {} in {page_ids:?}",
                page_ids.len()
            );
            seen.extend(page_ids);
            match page.string("NextToken") {
                Some(next) => {
                    assert!(
                        !next.is_empty()
                            && next
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '=' | '_' | '-')),
                        "case {case:?}: NextToken must match ^[a-zA-Z0-9=_-]+$, got {next:?}"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            seen, inserted,
            "case {case:?}: the token chain reports every record exactly once, in insertion order"
        );
    }
}

/// RULE: `ConnectorDestinationId` selects the records created against exactly that
/// destination, by equality, and pages the same way an unfiltered list does.
#[tokio::test]
async fn a_list_filters_by_connector_destination_id() {
    let router = emulator();

    // Three destinations seeded through the API, with the records spread over them: one
    // destination holding several records, one holding a single record, and one holding
    // none at all.
    let seeded = [
        connector_destination(&router).await,
        connector_destination(&router).await,
        connector_destination(&router).await,
    ];
    let against = [0usize, 0, 0, 1];
    let mut per_destination: Vec<(String, String)> = Vec::new();
    for (n, which) in against.iter().enumerate() {
        let destination = seeded[*which].clone();
        let reply = create(&router, minimal(&destination)).await;
        let id = accepted_id(&format!("seed-{n} for {destination}"), &reply);
        per_destination.push((destination, id));
    }

    // Every seeded destination, a destination holding no record, and values that are a
    // PREFIX and an EXTENSION of a live destination id plus one in the other case — so an
    // equality test cannot be mistaken for a prefix, a suffix or a case-insensitive match.
    let filters: Vec<String> = seeded
        .iter()
        .cloned()
        .chain([
            seeded[0][..12].to_string(),
            format!("{}0", seeded[0]),
            seeded[0].to_uppercase(),
            "d".to_string(),
        ])
        .collect();
    for destination in filters {
        let destination = destination.as_str();
        let expected: Vec<String> = per_destination
            .iter()
            .filter(|(stored, _)| stored == destination)
            .map(|(_, id)| id.clone())
            .collect();
        let filtered = list(&router, &format!("?ConnectorDestinationId={destination}")).await;
        assert_lists(
            &format!("ConnectorDestinationId={destination}"),
            &filtered,
            &expected,
        );
        for item in filtered.items() {
            assert_eq!(
                item.get("ConnectorDestinationId").and_then(Value::as_str),
                Some(destination),
                "case {destination:?}: every item reports the destination filtered for, \
                 got {item:?}"
            );
        }

        // The filter and the page size compose: one item per page, resumed by the token.
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let query = match &token {
                None => format!("?ConnectorDestinationId={destination}&MaxResults=1"),
                Some(next) => format!(
                    "?ConnectorDestinationId={destination}&MaxResults=1&NextToken={next}"
                ),
            };
            let page = list(&router, &query).await;
            assert_eq!(
                page.status, 200,
                "case {destination:?}: expected HTTP 200 from a filtered page, got {} with \
                 body {:?}",
                page.status, page.body
            );
            seen.extend(page.item_ids());
            match page.string("NextToken") {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
            assert!(
                seen.len() <= per_destination.len(),
                "case {destination:?}: a filtered token chain did not terminate, saw {seen:?}"
            );
        }
        assert_eq!(
            seen, expected,
            "case {destination:?}: a filtered token chain reports the same records as one page"
        );
    }
}

/// RULE: a `MaxResults` outside its modelled `@range` is the modelled 400
/// `ValidationException` — the recorded unit's case — while every value inside the range is
/// accepted and bounds the page.
#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let router = emulator();
    let destination = connector_destination(&router).await;
    for n in 0..3u32 {
        let reply = create(&router, minimal(&destination)).await;
        accepted_id(&format!("seed-{n}"), &reply);
    }

    let (min_results, max_results) = MAX_RESULTS_BOUNDS;
    // Each bound, and one step past it on both sides; plus a negative value and the
    // largest 32-bit integer, which are outside the range by any reading.
    let refused = [
        min_results - 1,
        max_results + 1,
        -1,
        i64::from(i32::MAX),
        max_results * 2,
    ];
    for value in refused {
        let case = format!("MaxResults={value}");
        let reply = list(&router, &format!("?MaxResults={value}")).await;
        assert_refused(&case, &reply, 400, VALIDATION);
        assert!(
            reply.member("Items").is_none(),
            "case {case:?}: a refused list reports no Items, got {:?}",
            reply.body
        );
    }

    // Inside the range, both bounds included, the call is answered.
    for value in [min_results, min_results + 1, 2, 100, max_results] {
        let case = format!("MaxResults={value}");
        let reply = list(&router, &format!("?MaxResults={value}")).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            reply.status, reply.body
        );
        assert!(
            !reply.items().is_empty()
                && reply.items().len() <= usize::try_from(value).unwrap_or(usize::MAX),
            "case {case:?}: expected between 1 and {value} items, got {:?}",
            reply.body
        );
    }
}

/// RULE: a `NextToken` this service never issued is the modelled 400 rather than a page, so
/// a client cannot page a store it was never given a cursor into.
#[tokio::test]
async fn a_next_token_this_service_never_issued_is_refused() {
    let router = emulator();
    let destination = connector_destination(&router).await;
    for n in 0..3u32 {
        let reply = create(&router, minimal(&destination)).await;
        accepted_id(&format!("seed-{n}"), &reply);
    }

    // The rows differ along the form of the token: not a cursor at all, a short one, one of
    // the right length that is not hex, and a well-formed cursor beyond anything this store
    // has minted. Every one matches the modelled `NextToken` pattern, so the boundary
    // validator passes it and the handler is what must refuse it.
    for token in [
        "nonsense",
        "deadbeef",
        "zzzzzzzzzzzzzzzz",
        "00000000ffffffff",
        "=",
    ] {
        let case = format!("NextToken={token}");
        let reply = list(&router, &format!("?NextToken={token}")).await;
        assert_refused(&case, &reply, 400, VALIDATION);
    }

    // A token the service DID issue still pages, so the refusals above changed nothing.
    let first = list(&router, "?MaxResults=1").await;
    let token = first
        .string("NextToken")
        .expect("a store of three records answers a NextToken for a page of one")
        .to_string();
    let second = list(&router, &format!("?MaxResults=1&NextToken={token}")).await;
    assert_eq!(
        second.status, 200,
        "case \"issued token\": expected HTTP 200, got {} with body {:?}",
        second.status, second.body
    );
}

/// RULE: an account association is regional — only the region whose endpoint created it
/// lists or reads it — and an identifier a region holds no association under is the
/// modelled 404 naming the resource type and the identifier the request carried.
#[tokio::test]
async fn an_association_is_only_visible_in_the_region_that_created_it() {
    let router = emulator();

    // The rows differ along the region the create was signed for. A connector destination
    // is regional too, so each region's association is created against the destination of
    // its OWN region.
    let regions = ["us-east-1", "eu-central-1", "ap-southeast-2"];
    let mut per_region: Vec<(&str, String)> = Vec::new();
    for region in regions {
        let destination = connector_destination_in(&router, region).await;
        let reply = create_in(&router, region, minimal(&destination)).await;
        let id = accepted_id(&format!("create in {region}"), &reply);
        assert!(
            reply.string("Arn").unwrap_or_default().contains(region),
            "case {region:?}: expected the ARN to carry the signing region, got {:?}",
            reply.string("Arn")
        );
        per_region.push((region, id));
    }

    for (region, id) in &per_region {
        // The region that created it lists and reads exactly its own record.
        let listed = list_in(&router, region, "").await;
        assert_lists(region, &listed, std::slice::from_ref(id));
        let read = get_in(&router, region, id).await;
        assert_eq!(
            read.status, 200,
            "case {region:?}: expected HTTP 200 reading its own record, got {} with body {:?}",
            read.status, read.body
        );

        // No other region resolves it.
        for (other, _) in per_region.iter().filter(|(other, _)| other != region) {
            let case = format!("{id} from {other}");
            let elsewhere = get_in(&router, other, id).await;
            assert_refused(&case, &elsewhere, 404, NOT_FOUND);
            assert_eq!(
                elsewhere.string("ResourceType"),
                Some(ACCOUNT_ASSOCIATION_RESOURCE_TYPE),
                "case {case:?}: expected the 404 to name the resource type \
                 {ACCOUNT_ASSOCIATION_RESOURCE_TYPE:?}, got {:?}",
                elsewhere.string("ResourceType")
            );
            assert_eq!(
                elsewhere.string("ResourceId"),
                Some(id.as_str()),
                "case {case:?}: expected the 404 to report the identifier the request carried, \
                 got {:?}",
                elsewhere.string("ResourceId")
            );
        }
    }

    // An identifier nothing minted, in several forms the `AccountAssociationId` pattern
    // admits, including one at the modelled maximum length.
    for id in [
        "a".to_string(),
        "NoSuchAssociation1".to_string(),
        "0123456789".to_string(),
        "b".repeat(ACCOUNT_ASSOCIATION_ID_BOUNDS.1),
    ] {
        let read = get(&router, &id).await;
        assert_refused(&format!("never minted {id}"), &read, 404, NOT_FOUND);
    }
}

/// RULE: a retry carrying the `ClientToken` of a create that already succeeded answers with
/// that create's record and performs no further action, so the store still holds one
/// association for it.
#[tokio::test]
async fn a_replayed_client_token_answers_with_the_first_association() {
    // The rows differ along the token's form (the `ClientToken` pattern is
    // `^[a-zA-Z0-9=_-]+$`, length 1..64) and along the body it created. Each row's
    // `ConnectorDestinationId` is filled in below from the destination its own emulator
    // mints, since the id is the service's to generate.
    let cases: Vec<(&str, Value)> = vec![
        ("t", json!({})),
        ("token-1", json!({"Name": "Second"})),
        ("TOKEN_2=", json!({"Description": "third"})),
        (
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            json!({}),
        ),
    ];

    for (token, body) in cases {
        let router = emulator();
        let mut request = body.clone();
        request["ConnectorDestinationId"] = json!(connector_destination(&router).await);
        request["ClientToken"] = json!(token);

        let first = create(&router, request.clone()).await;
        let id = accepted_id(token, &first);

        for attempt in 2..=3 {
            let case = format!("{token}/attempt {attempt}");
            let retry = create(&router, request.clone()).await;
            let replayed = accepted_id(&case, &retry);
            assert_eq!(
                replayed, id,
                "case {case:?}: a replayed ClientToken answers with the first identifier {id:?}, \
                 got {replayed:?}"
            );
        }

        // It performed no further action: the store holds exactly the first record.
        let listed = list(&router, "").await;
        assert_lists(token, &listed, std::slice::from_ref(&id));
    }
}
