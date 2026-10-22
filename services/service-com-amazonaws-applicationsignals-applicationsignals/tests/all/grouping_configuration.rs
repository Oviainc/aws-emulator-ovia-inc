// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the account's grouping configuration over HTTP:
//! `PutGroupingConfiguration`, `ListGroupingAttributeDefinitions` and
//! `DeleteGroupingConfiguration`.
//!
//! Every test drives the real `restJson1` surface through the router that production
//! mounts, so the routes, the status codes and the JSON bodies are the ones a client sees.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The SigV4 credential scope routes the request to this service. Signature verification is
/// off (see `set_request_authentication`), so the signature bytes are never read.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-2/application-signals/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`].
const REGION: &str = "eu-west-2";

/// The canonical emulator account id this store belongs to (`engine::emulator_account_id`).
const ACCOUNT: &str = "000000000000";

/// A router with just this service mounted on a fresh engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_applicationsignals_applicationsignals::EmulatedService::new(engine),
    );
    router
}

/// Send one request and return its status, its raw body and the parsed JSON body
/// (`Value::Null` when the body is empty or not JSON).
async fn raw_call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Vec<u8>, Value) {
    let payload = body.map_or_else(Vec::new, |body| {
        serde_json::to_vec(&body).expect("serialize the request body")
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://application-signals.{REGION}.amazonaws.com{uri}"
        ))
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(payload))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = response.into_body().to_vec();
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    (status, body, parsed)
}

/// [`raw_call`] without the raw bytes.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> (u16, Value) {
    let (status, _raw, parsed) = raw_call(router, method, uri, body).await;
    (status, parsed)
}

/// `PUT /grouping-configuration` with the given definitions.
async fn put(router: &MultiServiceRouter, definitions: &Value) -> (u16, Value) {
    call(
        router,
        "PUT",
        "/grouping-configuration",
        Some(json!({ "GroupingAttributeDefinitions": definitions })),
    )
    .await
}

/// `POST /grouping-attribute-definitions` — every input member is an `@httpQuery`
/// binding, so the request carries no body.
async fn list(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/grouping-attribute-definitions{query}"),
        None,
    )
    .await
}

/// `DELETE /grouping-configuration` — the operation's input is `smithy.api#Unit`, so the
/// request carries nothing at all.
async fn delete(router: &MultiServiceRouter) -> (u16, Vec<u8>, Value) {
    raw_call(router, "DELETE", "/grouping-configuration", None).await
}

/// One row of the definition-set table: a label and the `GroupingAttributeDefinitions` a put
/// stores.
struct Case {
    label: &'static str,
    definitions: Value,
}

/// The definition sets the rule is asserted over.
///
/// The rows differ along what the rule generalises over: how MANY definitions the
/// configuration holds, which of `GroupingAttributeDefinition`'s two optional members each
/// one supplies (`GroupingSourceKeys`, `DefaultGroupingValue` —
/// `smithy_query(query="shape",shape="GroupingAttributeDefinition")`), and how many source
/// keys a definition derives its value from. The grouping names are drawn from the modelled
/// `GroupingName` pattern `^[a-zA-Z0-9\s+\-=\._:/@]*$` with `length 1..128`, including both
/// published length bounds.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "one definition, source keys and a default",
            definitions: json!([{
                "GroupingName": "BusinessUnit",
                "GroupingSourceKeys": ["business_unit", "team"],
                "DefaultGroupingValue": "unassigned"
            }]),
        },
        Case {
            label: "one definition, no optional members at all",
            definitions: json!([{ "GroupingName": "Environment" }]),
        },
        Case {
            label: "one definition, a default but no source keys",
            definitions: json!([{
                "GroupingName": "Cost:Center/Shared",
                "DefaultGroupingValue": "none"
            }]),
        },
        Case {
            label: "three definitions, mixed optional members",
            definitions: json!([
                {
                    "GroupingName": "A",
                    "GroupingSourceKeys": ["a"]
                },
                {
                    "GroupingName": "deploy.group+1",
                    "GroupingSourceKeys": ["k1", "k2", "k3", "k4"],
                    "DefaultGroupingValue": "fallback"
                },
                { "GroupingName": "owner@team" }
            ]),
        },
        Case {
            // The shortest name the modelled `length 1..128` admits.
            label: "shortest grouping name the model admits",
            definitions: json!([{ "GroupingName": "x" }]),
        },
        Case {
            // The longest name the modelled `length 1..128` admits.
            label: "longest grouping name the model admits",
            definitions: json!([{ "GroupingName": "a".repeat(128) }]),
        },
        Case {
            // `GroupingAttributeDefinitions` carries no `length` constraint, so the empty
            // list is a configuration with no definitions rather than a refusal.
            label: "no definitions",
            definitions: json!([]),
        },
    ]
}

/// The definitions a `ListGroupingAttributeDefinitions` page reports.
fn listed(page: &Value) -> &Value {
    &page["GroupingAttributeDefinitions"]
}

/// A put persists the definitions, a list reports exactly them, and a delete clears them —
/// for every shape of definition set.
///
/// The delete answers HTTP 200 with the empty JSON body `{}` (the modelled
/// `DeleteGroupingConfigurationOutput` declares no members), and after it the account's
/// configuration is gone: the list reports no definitions and no `UpdatedAt`.
#[tokio::test]
async fn put_persists_list_reports_and_delete_clears_each_definition_set() {
    for case in cases() {
        let router = router();

        let (status, stored) = put(&router, &case.definitions).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the put, got {status} with {stored}",
            case.label
        );
        let configuration = &stored["GroupingConfiguration"];
        assert_eq!(
            configuration["GroupingAttributeDefinitions"], case.definitions,
            "case {:?}: expected the put to answer the stored definitions {}, got {:?}",
            case.label, case.definitions, configuration["GroupingAttributeDefinitions"]
        );
        assert!(
            configuration["UpdatedAt"].is_number(),
            "case {:?}: expected the stored configuration to carry UpdatedAt, got {:?}",
            case.label,
            configuration["UpdatedAt"]
        );

        // The reader answers out of the store, so it reports the same definitions and the
        // same write timestamp.
        let (status, page) = list(&router, "").await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the list, got {status} with {page}",
            case.label
        );
        assert_eq!(
            *listed(&page), case.definitions,
            "case {:?}: expected the list to report {}, got {:?}",
            case.label, case.definitions, listed(&page)
        );
        assert_eq!(
            page["UpdatedAt"], configuration["UpdatedAt"],
            "case {:?}: expected the list to report the stored UpdatedAt {:?}, got {:?}",
            case.label, configuration["UpdatedAt"], page["UpdatedAt"]
        );

        let (status, raw, body) = delete(&router).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the delete, got {status} with {body}",
            case.label
        );
        assert_eq!(
            String::from_utf8_lossy(&raw),
            "{}",
            "case {:?}: expected the delete to answer the empty body {{}}, got {:?}",
            case.label,
            String::from_utf8_lossy(&raw)
        );

        let (status, page) = list(&router, "").await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the list after the delete, got {status} with {page}",
            case.label
        );
        assert_eq!(
            *listed(&page),
            json!([]),
            "case {:?}: expected no definitions after the delete, got {:?}",
            case.label,
            listed(&page)
        );
        assert_eq!(
            page["UpdatedAt"],
            Value::Null,
            "case {:?}: expected no UpdatedAt once nothing is configured, got {:?}",
            case.label,
            page["UpdatedAt"]
        );
    }
}

/// The delete is `@idempotent` and models no not-found error, so it answers HTTP 200 with an
/// empty body whether or not a configuration exists — on a store that never had one, and
/// repeatedly after one was removed.
#[tokio::test]
async fn delete_is_idempotent_whether_or_not_a_configuration_exists() {
    let router = router();

    // Nothing has ever been configured.
    for attempt in 1..=2 {
        let (status, raw, body) = delete(&router).await;
        assert_eq!(
            status, 200,
            "case unconfigured attempt {attempt}: expected HTTP 200, got {status} with {body}"
        );
        assert_eq!(
            String::from_utf8_lossy(&raw),
            "{}",
            "case unconfigured attempt {attempt}: expected the empty body {{}}, got {:?}",
            String::from_utf8_lossy(&raw)
        );
    }

    // Now with a configuration in place: the first delete removes it and every later one
    // answers identically.
    let (status, stored) = put(&router, &json!([{ "GroupingName": "Environment" }])).await;
    assert_eq!(status, 200, "expected the setup put to succeed, got {status} with {stored}");

    for attempt in 1..=3 {
        let (status, raw, body) = delete(&router).await;
        assert_eq!(
            status, 200,
            "case configured attempt {attempt}: expected HTTP 200, got {status} with {body}"
        );
        assert_eq!(
            String::from_utf8_lossy(&raw),
            "{}",
            "case configured attempt {attempt}: expected the empty body {{}}, got {:?}",
            String::from_utf8_lossy(&raw)
        );
        let (status, page) = list(&router, "").await;
        assert_eq!(
            status, 200,
            "case configured attempt {attempt}: expected HTTP 200 from the list, got {status} with {page}"
        );
        assert_eq!(
            *listed(&page),
            json!([]),
            "case configured attempt {attempt}: expected no definitions, got {:?}",
            listed(&page)
        );
    }
}

/// There is ONE grouping configuration per account, so a second put REPLACES the first
/// rather than merging with it, and a delete after a replace leaves nothing behind.
#[tokio::test]
async fn a_second_put_replaces_the_stored_configuration() {
    let router = router();
    let rows = cases();

    let mut previous_updated = 0.0_f64;
    for case in &rows {
        let (status, stored) = put(&router, &case.definitions).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the put, got {status} with {stored}",
            case.label
        );

        let (status, page) = list(&router, "").await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the list, got {status} with {page}",
            case.label
        );
        assert_eq!(
            *listed(&page), case.definitions,
            "case {:?}: expected the put to replace the previous definitions with {}, got {:?}",
            case.label, case.definitions, listed(&page)
        );

        // `UpdatedAt` is "the timestamp when this grouping configuration was last updated",
        // so each write moves it forward rather than keeping the first write's value.
        let updated = page["UpdatedAt"].as_f64().unwrap_or_default();
        assert!(
            updated >= previous_updated,
            "case {:?}: expected UpdatedAt to be at least the previous {previous_updated}, got {updated}",
            case.label
        );
        previous_updated = updated;
    }

    let (status, _raw, body) = delete(&router).await;
    assert_eq!(status, 200, "expected HTTP 200 from the delete, got {status} with {body}");
    let (_status, page) = list(&router, "").await;
    assert_eq!(
        *listed(&page),
        json!([]),
        "expected the delete to clear the replaced configuration, got {:?}",
        listed(&page)
    );
}

/// `GroupingName` identifies a definition, so one configuration cannot define the same name
/// twice: the put is refused with a 400 `ValidationException` and stores nothing.
#[tokio::test]
async fn a_put_that_repeats_a_grouping_name_is_refused() {
    let router = router();

    // Each row repeats one name; they differ in where the repeat sits and how many other
    // definitions surround it.
    let rows = [
        (
            "repeated immediately",
            json!([
                { "GroupingName": "BusinessUnit" },
                { "GroupingName": "BusinessUnit", "DefaultGroupingValue": "other" }
            ]),
        ),
        (
            "repeated after another definition",
            json!([
                { "GroupingName": "Environment" },
                { "GroupingName": "BusinessUnit", "GroupingSourceKeys": ["bu"] },
                { "GroupingName": "Environment", "GroupingSourceKeys": ["env"] }
            ]),
        ),
    ];

    for (label, definitions) in rows {
        let (status, error) = put(&router, &definitions).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400 for a repeated GroupingName, got {status} with {error}"
        );
        assert_eq!(
            error["__type"].as_str(),
            Some("ValidationException"),
            "case {label:?}: expected a ValidationException, got {error}"
        );

        let (_status, page) = list(&router, "").await;
        assert_eq!(
            *listed(&page),
            json!([]),
            "case {label:?}: a refused put must store nothing, got {:?}",
            listed(&page)
        );
    }

    // A name that differs only in case is a DIFFERENT name: the modelled pattern admits
    // both, and nothing in the model folds case, so this put is accepted.
    let distinct = json!([
        { "GroupingName": "BusinessUnit" },
        { "GroupingName": "businessunit" }
    ]);
    let (status, stored) = put(&router, &distinct).await;
    assert_eq!(
        status, 200,
        "case names differing only in case: expected HTTP 200, got {status} with {stored}"
    );
}

/// The reader selects out of THIS account's store: a request naming the emulator's own
/// account reports the stored definitions, one naming any other account reports none, and
/// `IncludeLinkedAccounts` changes nothing either way because there is no source account to
/// add. A `NextToken` this service never issued is refused.
#[tokio::test]
async fn the_reader_answers_only_for_this_account() {
    let router = router();
    let definitions = json!([{ "GroupingName": "BusinessUnit", "GroupingSourceKeys": ["bu"] }]);
    let (status, stored) = put(&router, &definitions).await;
    assert_eq!(status, 200, "expected the setup put to succeed, got {status} with {stored}");

    // Every query that selects this account's configuration, with and without the
    // linked-accounts flag.
    for query in [
        String::new(),
        "?IncludeLinkedAccounts=true".to_string(),
        "?IncludeLinkedAccounts=false".to_string(),
        format!("?AwsAccountId={ACCOUNT}"),
        format!("?AwsAccountId={ACCOUNT}&IncludeLinkedAccounts=true"),
    ] {
        let (status, page) = list(&router, &query).await;
        assert_eq!(
            status, 200,
            "case {query:?}: expected HTTP 200, got {status} with {page}"
        );
        assert_eq!(
            *listed(&page), definitions,
            "case {query:?}: expected the stored definitions {definitions}, got {:?}",
            listed(&page)
        );
    }

    // Any other 12-digit account has no configuration in this emulator.
    for other in ["123456789012", "999999999999"] {
        let (status, page) = list(&router, &format!("?AwsAccountId={other}")).await;
        assert_eq!(
            status, 200,
            "case AwsAccountId={other}: expected HTTP 200, got {status} with {page}"
        );
        assert_eq!(
            *listed(&page),
            json!([]),
            "case AwsAccountId={other}: expected no definitions for another account, got {:?}",
            listed(&page)
        );
        assert_eq!(
            page["UpdatedAt"],
            Value::Null,
            "case AwsAccountId={other}: expected no UpdatedAt for another account, got {:?}",
            page["UpdatedAt"]
        );
    }

    // This operation never issues a `NextToken`, so any token a caller sends is not one it
    // got from here.
    for token in ["", "not-a-cursor", "6162630a"] {
        let (status, error) = list(&router, &format!("?NextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case NextToken={token:?}: expected HTTP 400 for a token this service did not issue, got {status} with {error}"
        );
    }
}
