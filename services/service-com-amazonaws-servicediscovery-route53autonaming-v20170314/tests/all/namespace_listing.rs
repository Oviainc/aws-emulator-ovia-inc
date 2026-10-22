// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListNamespaces`: the page a request asks for, the filters the
//! model documents, and the refusals a filter or a token this service never issued
//! earns.
//!
//! Every request goes through `MultiServiceRouter::dispatch`, i.e. the same path the
//! emulator serves in production (`awsJson1_1`: `POST /` with `X-Amz-Target`).

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_servicediscovery_route53autonaming_v20170314 as cloudmap;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` "Wire protocol".
const TARGET_PREFIX: &str = "Route53AutoNaming_v20170314";

/// The region the test requests are signed for.
const REGION: &str = "us-west-2";

/// One emulator with this service mounted on a fresh shared engine.
fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// The same emulator, plus the engine it was mounted on, so a test can seed namespace
/// kinds whose `Create…` operations are still scaffolds.
fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These tests drive the router directly rather than signing requests.
    router.set_request_authentication(false);
    router.register_service(cloudmap::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Dispatch one operation and return its HTTP status and JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/servicediscovery/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=unsigned"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("http://localhost/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header("authorization", authorization)
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("an awsJson1_1 body is JSON")
    };
    (status, json)
}

/// Dispatch an operation that must succeed, returning its output.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, json) = call(router, operation, body).await;
    assert_eq!(status, 200, "{operation} must succeed, got {status}: {json}");
    json
}

/// Dispatch an operation that must fail, returning its status and wire error code.
async fn failure(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, String) {
    let (status, json) = call(router, operation, body).await;
    let code = json["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    assert_ne!(status, 200, "{operation} must fail, got {json}");
    (status, code)
}

/// Create an HTTP namespace through the API and return its id.
async fn namespace(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(router, "CreateHttpNamespace", json!({ "Name": name })).await;
    let operation_id = created["OperationId"]
        .as_str()
        .expect("CreateHttpNamespace returns an OperationId")
        .to_string();
    let operation = ok(router, "GetOperation", json!({ "OperationId": operation_id })).await;
    operation["Operation"]["Targets"]["NAMESPACE"]
        .as_str()
        .expect("the CREATE_NAMESPACE operation targets the namespace it created")
        .to_string()
}

/// Store a namespace of `namespace_type` directly, for the kinds whose `Create…`
/// operation is still a scaffold (`CreatePublicDnsNamespace`,
/// `CreatePrivateDnsNamespace`). The record is the same one a create would write.
fn seed_namespace(
    engine: &Arc<engine::InMemoryEngine>,
    id: &str,
    name: &str,
    namespace_type: &str,
    http_name: Option<&str>,
) {
    let state =
        engine.state::<cloudmap::ComAmazonawsServicediscoveryRoute53autonamingV20170314State>();
    let mut store = state.lock();
    store.namespaces.insert(
        id.to_string(),
        cloudmap::state::NamespaceRecord {
            id: id.to_string(),
            arn: cloudmap::state::service_arn(REGION, &format!("namespace/{id}")),
            name: name.to_string(),
            namespace_type: namespace_type.to_string(),
            description: None,
            create_date: SystemTime::now(),
            creator_request_id: None,
            http_name: http_name.map(str::to_string),
            hosted_zone_id: (namespace_type != "HTTP").then(|| format!("Z{id}")),
            // A seeded DNS namespace states no SOA TTL, which the record reports as
            // absent rather than inventing one.
            soa_ttl: None,
            tags: Vec::new(),
        },
    );
    store
        .namespace_ids_by_name
        .insert(name.to_string(), id.to_string());
}

/// The namespace ids one `ListNamespaces` response reports, in the order served.
fn listed_ids(reply: &Value) -> Vec<String> {
    reply["Namespaces"]
        .as_array()
        .unwrap_or_else(|| panic!("Namespaces is a list, got {reply}"))
        .iter()
        .map(|summary| {
            summary["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("every summary names its namespace, got {summary}"))
                .to_string()
        })
        .collect()
}

/// Page `ListNamespaces` to exhaustion, returning one entry per page.
async fn pages(
    router: &MultiServiceRouter,
    max_results: Option<i64>,
    filters: Option<Value>,
) -> Vec<Vec<String>> {
    let mut token: Option<String> = None;
    let mut served = Vec::new();
    loop {
        let mut body = json!({});
        if let Some(max) = max_results {
            body["MaxResults"] = json!(max);
        }
        if let Some(filters) = filters.clone() {
            body["Filters"] = filters;
        }
        if let Some(token) = &token {
            body["NextToken"] = json!(token);
        }
        let reply = ok(router, "ListNamespaces", body).await;
        served.push(listed_ids(&reply));
        token = reply["NextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return served;
        }
        assert!(
            served.len() <= 64,
            "paging must terminate, got {} pages for MaxResults {max_results:?}",
            served.len()
        );
    }
}

/// A `ListNamespaces` listing serves every stored namespace exactly once, in one
/// order, for ANY page size — and hands back a `NextToken` only while namespaces
/// remain.
#[tokio::test]
async fn paging_serves_every_namespace_exactly_once_in_a_stable_order() {
    let router = emulator();
    let total: usize = 7;
    let mut created = BTreeSet::new();
    for index in 0..total {
        created.insert(namespace(&router, &format!("page-ns-{index}")).await);
    }

    // The single-page listing is the order every paged listing must reproduce.
    let whole = listed_ids(&ok(&router, "ListNamespaces", json!({})).await);
    assert_eq!(
        whole.iter().cloned().collect::<BTreeSet<_>>(),
        created,
        "one page of up to the documented 100 namespaces holds all {total}, got {whole:?}"
    );

    // The rows differ along the page size: below, at and above the stored count, plus
    // an absent MaxResults (the documented default of up to 100).
    for max_results in [Some(1), Some(2), Some(3), Some(6), Some(7), Some(8), None] {
        let served = pages(&router, max_results, None).await;
        let flattened: Vec<String> = served.iter().flatten().cloned().collect();
        assert_eq!(
            flattened, whole,
            "case MaxResults {max_results:?}: paging must serve the same namespaces in the \
             same order as a single page, got {flattened:?}"
        );
        assert_eq!(
            flattened.iter().cloned().collect::<BTreeSet<_>>().len(),
            flattened.len(),
            "case MaxResults {max_results:?}: no namespace may be served twice, got {flattened:?}"
        );

        let size = usize::try_from(max_results.unwrap_or(100)).expect("a positive page size");
        let expected_pages = total.div_ceil(size).max(1);
        assert_eq!(
            served.len(),
            expected_pages,
            "case MaxResults {max_results:?}: {total} namespaces fill {expected_pages} pages, \
             got {} pages: {served:?}",
            served.len()
        );
        for (index, page) in served.iter().enumerate() {
            let expected = if index + 1 == served.len() {
                total - index * size
            } else {
                size
            };
            assert_eq!(
                page.len(),
                expected,
                "case MaxResults {max_results:?}: page {index} must hold {expected} \
                 namespaces, got {page:?}"
            );
        }
    }
}

/// The `NextToken` a page hands back is the only one that resumes it: resuming twice
/// with the same token serves the same page, and listing changes nothing a later
/// caller sees.
#[tokio::test]
async fn a_page_token_resumes_the_same_place_and_a_listing_changes_nothing() {
    let router = emulator();
    for index in 0..4 {
        namespace(&router, &format!("token-ns-{index}")).await;
    }

    let first = ok(&router, "ListNamespaces", json!({ "MaxResults": 2 })).await;
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a truncated page carries a NextToken, got {first}"))
        .to_string();

    // Reading is idempotent: the same request twice answers identically, and the
    // resumed page does not move between the two reads either.
    let repeat = ok(&router, "ListNamespaces", json!({ "MaxResults": 2 })).await;
    assert_eq!(
        first, repeat,
        "case first page: listing twice must answer identically, got {repeat}"
    );
    let resumed = ok(
        &router,
        "ListNamespaces",
        json!({ "MaxResults": 2, "NextToken": token }),
    )
    .await;
    let resumed_again = ok(
        &router,
        "ListNamespaces",
        json!({ "MaxResults": 2, "NextToken": token }),
    )
    .await;
    assert_eq!(
        resumed, resumed_again,
        "case resumed page: the same token must serve the same page, got {resumed_again}"
    );
    assert_eq!(
        resumed["NextToken"],
        Value::Null,
        "case resumed page: 4 namespaces in pages of 2 end at the second page, got {resumed}"
    );

    // The modelled NextToken length is 0..4096, so the empty token is legal and starts
    // at the beginning.
    let from_empty = ok(
        &router,
        "ListNamespaces",
        json!({ "MaxResults": 2, "NextToken": "" }),
    )
    .await;
    assert_eq!(
        from_empty, first,
        "case empty NextToken: an empty token starts at the beginning, got {from_empty}"
    );
}

/// Each documented filter name selects on the namespace member
/// `NamespaceFilter$Values` names for it, under both documented conditions, and
/// several filters must all match.
#[tokio::test]
async fn every_documented_filter_selects_the_namespaces_it_names() {
    let (router, engine) = emulator_with_engine();
    // One namespace of each modelled NamespaceType; the DNS kinds are seeded because
    // their create operations are still scaffolds.
    let http_id = namespace(&router, "shop.internal").await;
    seed_namespace(
        &engine,
        "ns-seeded-public",
        "shop.example.com",
        "DNS_PUBLIC",
        Some("shop-public"),
    );
    seed_namespace(
        &engine,
        "ns-seeded-private",
        "warehouse.internal",
        "DNS_PRIVATE",
        None,
    );
    let everything: BTreeSet<String> = [
        http_id.clone(),
        "ns-seeded-public".to_string(),
        "ns-seeded-private".to_string(),
    ]
    .into_iter()
    .collect();

    let public: BTreeSet<String> = ["ns-seeded-public".to_string()].into_iter().collect();
    let private: BTreeSet<String> = ["ns-seeded-private".to_string()].into_iter().collect();
    let http: BTreeSet<String> = [http_id.clone()].into_iter().collect();
    let none: BTreeSet<String> = BTreeSet::new();

    // Rows differ along the filter name, the condition and the value matched.
    let cases: Vec<(&str, Value, BTreeSet<String>)> = vec![
        (
            "TYPE EQ HTTP",
            json!([{ "Name": "TYPE", "Values": ["HTTP"], "Condition": "EQ" }]),
            http.clone(),
        ),
        (
            "TYPE with the default condition",
            json!([{ "Name": "TYPE", "Values": ["DNS_PUBLIC"] }]),
            public.clone(),
        ),
        (
            "TYPE EQ DNS_PRIVATE",
            json!([{ "Name": "TYPE", "Values": ["DNS_PRIVATE"], "Condition": "EQ" }]),
            private.clone(),
        ),
        (
            "TYPE BEGINS_WITH DNS_ matches both DNS kinds",
            json!([{ "Name": "TYPE", "Values": ["DNS_"], "Condition": "BEGINS_WITH" }]),
            public.union(&private).cloned().collect(),
        ),
        (
            "NAME EQ the DNS_PUBLIC namespace's name",
            json!([{ "Name": "NAME", "Values": ["shop.example.com"] }]),
            public.clone(),
        ),
        (
            "NAME BEGINS_WITH shop. matches both shop namespaces",
            json!([{ "Name": "NAME", "Values": ["shop."], "Condition": "BEGINS_WITH" }]),
            http.union(&public).cloned().collect(),
        ),
        (
            "NAME EQ a name no namespace carries",
            json!([{ "Name": "NAME", "Values": ["absent.internal"] }]),
            none.clone(),
        ),
        (
            "HTTP_NAME EQ the HTTP namespace's HttpName",
            json!([{ "Name": "HTTP_NAME", "Values": ["shop.internal"] }]),
            http.clone(),
        ),
        (
            "HTTP_NAME BEGINS_WITH shop skips the namespace with no HttpName",
            json!([{ "Name": "HTTP_NAME", "Values": ["shop"], "Condition": "BEGINS_WITH" }]),
            http.union(&public).cloned().collect(),
        ),
        (
            "RESOURCE_OWNER EQ SELF matches every namespace this account created",
            json!([{ "Name": "RESOURCE_OWNER", "Values": ["SELF"] }]),
            everything.clone(),
        ),
        (
            "RESOURCE_OWNER EQ OTHER_ACCOUNTS matches nothing shared in",
            json!([{ "Name": "RESOURCE_OWNER", "Values": ["OTHER_ACCOUNTS"] }]),
            none.clone(),
        ),
        (
            "two filters a namespace must both satisfy",
            json!([
                { "Name": "TYPE", "Values": ["DNS_"], "Condition": "BEGINS_WITH" },
                { "Name": "NAME", "Values": ["shop."], "Condition": "BEGINS_WITH" },
            ]),
            public.clone(),
        ),
        (
            "two filters no namespace satisfies together",
            json!([
                { "Name": "TYPE", "Values": ["HTTP"] },
                { "Name": "NAME", "Values": ["warehouse.internal"] },
            ]),
            none.clone(),
        ),
        ("no filters at all", json!([]), everything.clone()),
    ];

    for (case, filters, expected) in cases {
        let reply = ok(
            &router,
            "ListNamespaces",
            json!({ "Filters": filters.clone() }),
        )
        .await;
        let got: BTreeSet<String> = listed_ids(&reply).into_iter().collect();
        assert_eq!(
            got, expected,
            "case {case:?}: expected {expected:?}, got {got:?}"
        );

        // The same filter applied one namespace per page selects the same set, since
        // `MaxResults` bounds the namespaces SCANNED, not the ones that match.
        let paged: BTreeSet<String> = pages(&router, Some(1), Some(filters))
            .await
            .into_iter()
            .flatten()
            .collect();
        assert_eq!(
            paged, expected,
            "case {case:?} paged one at a time: expected {expected:?}, got {paged:?}"
        );
    }
}

/// A filter the documentation does not allow, and a `NextToken` this service never
/// issued, are refused with the modelled `InvalidInput` (HTTP 400) — never with an
/// empty page and never with a 501.
#[tokio::test]
async fn an_unusable_filter_or_foreign_token_is_refused_as_invalid_input() {
    let router = emulator();
    namespace(&router, "refusal-ns").await;

    let cases: Vec<(&str, Value)> = vec![
        (
            "a filter name outside NamespaceFilterName",
            json!({ "Filters": [{ "Name": "NAMESPACE", "Values": ["HTTP"] }] }),
        ),
        (
            "the IN condition, which a namespace filter does not support",
            json!({ "Filters": [{ "Name": "NAME", "Values": ["refusal-ns"], "Condition": "IN" }] }),
        ),
        (
            "the BETWEEN condition, which a namespace filter does not support",
            json!({
                "Filters": [{ "Name": "NAME", "Values": ["refusal-ns"], "Condition": "BETWEEN" }]
            }),
        ),
        (
            "BEGINS_WITH on RESOURCE_OWNER, which supports only EQ",
            json!({
                "Filters": [{
                    "Name": "RESOURCE_OWNER",
                    "Values": ["SELF"],
                    "Condition": "BEGINS_WITH",
                }]
            }),
        ),
        (
            "two values where only one is allowed",
            json!({ "Filters": [{ "Name": "NAME", "Values": ["refusal-ns", "other-ns"] }] }),
        ),
        (
            "no value at all",
            json!({ "Filters": [{ "Name": "NAME", "Values": [] }] }),
        ),
        (
            "a TYPE value outside NamespaceType",
            json!({ "Filters": [{ "Name": "TYPE", "Values": ["DNS"] }] }),
        ),
        (
            "a RESOURCE_OWNER value that names no owner",
            json!({ "Filters": [{ "Name": "RESOURCE_OWNER", "Values": ["EVERYONE"] }] }),
        ),
        (
            "a NextToken this service never issued",
            json!({ "NextToken": "not-a-token" }),
        ),
        (
            "a NextToken minted for another paginated read's scope",
            json!({ "NextToken": hex::encode("GetInstancesHealthStatus\u{1f}i-1") }),
        ),
        (
            "a NextToken whose bytes carry no scope at all",
            json!({ "NextToken": hex::encode("services:srv-0") }),
        ),
        (
            "a NextToken naming a namespace id with no token envelope",
            json!({ "NextToken": "ns-0123456789abcdef" }),
        ),
    ];

    for (case, body) in cases {
        let (status, code) = failure(&router, "ListNamespaces", body.clone()).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidInput"),
            "case {case:?}: expected HTTP 400 InvalidInput, got {status} / {code:?} for {body}"
        );
    }
}

/// A value the MODEL itself rejects — before any handler runs — is refused with the
/// error code this service's model declares for invalid input, `InvalidInput`, and not
/// with the framework's generic `ValidationException`: Cloud Map models no such shape.
#[tokio::test]
async fn a_modelled_constraint_is_refused_as_invalid_input_too() {
    let router = emulator();
    namespace(&router, "boundary-ns").await;

    // Rows differ along the constraint tripped: each published bound of
    // `MaxResults` (@range(1..100)) and one past it on either side, and the
    // modelled `NextToken` length (0..4096) overrun.
    let cases: Vec<(&str, Value, bool)> = vec![
        ("MaxResults at the lower bound", json!({ "MaxResults": 1 }), true),
        ("MaxResults one below the lower bound", json!({ "MaxResults": 0 }), false),
        ("MaxResults at the upper bound", json!({ "MaxResults": 100 }), true),
        ("MaxResults one above the upper bound", json!({ "MaxResults": 101 }), false),
        (
            "a NextToken one character past the modelled length",
            json!({ "NextToken": "a".repeat(4097) }),
            false,
        ),
        (
            "a NextToken at the modelled length, so only its contents are wrong",
            json!({ "NextToken": "a".repeat(4096) }),
            false,
        ),
    ];

    for (case, body, accepted) in cases {
        if accepted {
            let reply = ok(&router, "ListNamespaces", body.clone()).await;
            assert_eq!(
                listed_ids(&reply).len(),
                1,
                "case {case:?}: a value inside the modelled range is served, got {reply}"
            );
            continue;
        }
        let (status, code) = failure(&router, "ListNamespaces", body.clone()).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidInput"),
            "case {case:?}: expected HTTP 400 InvalidInput, got {status} / {code:?} for {body}"
        );
    }
}

/// A `NamespaceSummary` is the stored record, so it agrees member for member with what
/// `GetNamespace` reports — including the derived `ServiceCount`.
#[tokio::test]
async fn a_summary_agrees_with_get_namespace_member_for_member() {
    let router = emulator();
    let id = namespace(&router, "summary-ns").await;
    ok(
        &router,
        "CreateService",
        json!({ "Name": "summary-svc", "NamespaceId": id }),
    )
    .await;

    let listed = ok(&router, "ListNamespaces", json!({})).await;
    let summary = listed["Namespaces"]
        .as_array()
        .expect("Namespaces is a list")
        .iter()
        .find(|summary| summary["Id"] == json!(id))
        .unwrap_or_else(|| panic!("the listing holds the namespace just created: {listed}"))
        .clone();
    let read = ok(&router, "GetNamespace", json!({ "Id": id })).await;
    let namespace = &read["Namespace"];

    // Every member `NamespaceSummary` declares (smithy_query shape="NamespaceSummary").
    for member in [
        "Id",
        "Arn",
        "ResourceOwner",
        "Name",
        "Type",
        "Description",
        "ServiceCount",
        "Properties",
        "CreateDate",
    ] {
        assert_eq!(
            summary[member], namespace[member],
            "member {member:?}: a summary must report what GetNamespace reports, got {summary} \
             against {namespace}"
        );
    }
    assert_eq!(
        summary["ServiceCount"],
        json!(1),
        "ServiceCount is derived from the stored services, got {summary}"
    );
    assert_eq!(
        summary["CreatorRequestId"],
        Value::Null,
        "CreatorRequestId is not a NamespaceSummary member, got {summary}"
    );
}
