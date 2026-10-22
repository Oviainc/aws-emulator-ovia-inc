// SPDX-License-Identifier: CC0-1.0

//! The PARTITION the tagging surface's resource-ARN classification draws, and what it
//! does not depend on.
//!
//! The two sibling suites pin one class each: `resource_arn_validation` owns the values
//! this API cannot address (400 `BadRequestException`) and `not_found_error_code` owns
//! the addressable ARNs that name no record (404 `NotFoundException`). What is asserted
//! HERE is the property that makes those two answers useful to a client, which neither
//! suite can state on its own:
//!
//! 1. the three classes get three PAIRWISE DISTINCT answers — a caller can tell a
//!    permanently malformed input, a resource that is not there, and its own resource
//!    apart from the status and the code alone, on every operation that resolves a
//!    `resourceArn`;
//! 2. the class a value falls in depends on the VALUE ALONE: it does not move when the
//!    store gains the record the request was about, and a repeated call answers the same
//!    way (nothing is consumed by being asked).
//!
//! The rows differ along the member of each class: which form of unaddressable value,
//! which form of addressable-but-unknown ARN, and which of the three operations asks.
//!
//! The harness (router, signed request, per-operation callers) is the one the
//! `deployment_parameters` suite owns.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::deployment_parameters::{
    emulator, error_code, pct, provision, put, request, ACCOUNT, CATALOG, REGION, SIGNING_NAME,
};

/// The agreement and product the one stored record below is written against.
const AGREEMENT: &str = "agmt-partition";
const PRODUCT: &str = "prod-partition";

/// The answer each class carries: `(status, wire error code)`. A success carries no
/// error code at all, which is what the empty string stands for here. All three are
/// written out rather than read from the crate, so the assertions are not the
/// implementation compared with itself.
const UNADDRESSABLE: (u16, &str) = (400, "BadRequestException");
const UNKNOWN: (u16, &str) = (404, "NotFoundException");
const STORED: (u16, &str) = (200, "");

/// Status and body of one signed call, with the error code reduced to the empty string
/// on a success.
async fn classify(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, String, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    let code = if status < 400 {
        String::new()
    } else {
        error_code(&parsed)
    };
    (status, code, parsed)
}

/// The three operations that resolve a caller-supplied `resourceArn`, with the status
/// each answers on SUCCESS — `ListTagsForResource` reports a tag map (200), while
/// `TagResource` and `UntagResource` are modelled with `code: 204`
/// (`smithy_query(query="digest")`).
fn tag_operations(
    resource_arn: &str,
) -> Vec<(&'static str, &'static str, String, Option<Value>, u16)> {
    let path = format!("/tags/{}", pct(resource_arn));
    vec![
        ("ListTagsForResource", "GET", path.clone(), None, 200),
        (
            "TagResource",
            "POST",
            path.clone(),
            Some(json!({ "tags": { "owner": "integration-test" } })),
            204,
        ),
        (
            "UntagResource",
            "DELETE",
            format!("{path}?tagKeys=owner"),
            None,
            204,
        ),
    ]
}

/// One emulator holding exactly one deployment parameter, and the ARN minted for it.
async fn emulator_with_one_parameter() -> (MultiServiceRouter, String) {
    let (router, engine) = emulator();
    provision(&engine, CATALOG, AGREEMENT, &[PRODUCT]);
    let (status, body) = put(
        &router,
        CATALOG,
        PRODUCT,
        &json!({
            "agreementId": AGREEMENT,
            "deploymentParameter": { "name": "Partitioned", "secretString": "s" },
        }),
    )
    .await;
    assert_eq!(status, 200, "the setup Put must succeed, got {body:?}");
    let arn = body
        .get("resourceArn")
        .and_then(Value::as_str)
        .expect("a minted resourceArn")
        .to_string();
    (router, arn)
}

#[tokio::test]
async fn the_three_resource_arn_classes_get_three_distinct_answers() {
    assert_ne!(
        UNADDRESSABLE, UNKNOWN,
        "the two refusals must differ, or a client cannot tell them apart"
    );
    assert_ne!(
        UNADDRESSABLE.1, STORED.1,
        "a refusal must differ from a success"
    );

    let (router, live) = emulator_with_one_parameter().await;

    // (case, value, the class answer it must get)
    let rows: Vec<(&str, String, (u16, &str))> = vec![
        (
            "an ARN of another service's namespace",
            format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret/dp-0000000000000001"),
            UNADDRESSABLE,
        ),
        (
            "a value that is no ARN at all",
            "dp-0000000000000001".to_string(),
            UNADDRESSABLE,
        ),
        (
            "an ARN of this vendor carrying a character outside the resource-ARN class",
            format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:deployment parameter/dp-1"),
            UNADDRESSABLE,
        ),
        (
            "an addressable ARN whose id was never minted",
            format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:deployment-parameter/dp-ffffffff"),
            UNKNOWN,
        ),
        (
            "the stored ARN in another account",
            live.replace(ACCOUNT, "000000000001"),
            UNKNOWN,
        ),
        ("the stored record's own ARN", live.clone(), STORED),
    ];

    for (case, resource_arn, (expected_status, expected_code)) in rows {
        for (operation, method, uri, body, success_status) in tag_operations(&resource_arn) {
            // The same call twice: a read or a tag write of one ARN is answered the same
            // way the second time, so nothing about the classification is consumed.
            for attempt in 1..=2 {
                let (status, code, answered) =
                    classify(&router, method, &uri, body.as_ref()).await;
                let expected_status = if expected_code.is_empty() {
                    success_status
                } else {
                    expected_status
                };
                assert_eq!(
                    status, expected_status,
                    "case {case:?}/{operation} attempt {attempt}: expected HTTP \
                     {expected_status}, got {status} {answered:?}"
                );
                assert_eq!(
                    code, expected_code,
                    "case {case:?}/{operation} attempt {attempt}: expected code \
                     {expected_code:?}, got {code:?} in {answered:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_class_of_a_resource_arn_does_not_depend_on_what_the_store_holds() {
    // An EMPTY store first: the classification is a judgement of the value, so a value
    // this API cannot address is refused before any lookup, and an addressable one is
    // the store's not-found.
    let (router, engine) = emulator();
    let unaddressable = format!("arn:aws:launchwizard:{REGION}:{ACCOUNT}:deployment/dp-1");
    let to_be_created = format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:deployment-parameter/dp-0000000000000001");

    for (case, resource_arn, expected) in [
        ("unaddressable, empty store", &unaddressable, UNADDRESSABLE),
        ("addressable, empty store", &to_be_created, UNKNOWN),
    ] {
        let (status, code, answered) = classify(
            &router,
            "GET",
            &format!("/tags/{}", pct(resource_arn)),
            None,
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            expected,
            "case {case:?}: expected {expected:?}, got {status} {code:?} in {answered:?}"
        );
    }

    // Now create the parameter that ARN names. The emulator mints ids in order, so the
    // first write takes the id the row above asked for.
    provision(&engine, CATALOG, AGREEMENT, &[PRODUCT]);
    let (status, body) = put(
        &router,
        CATALOG,
        PRODUCT,
        &json!({
            "agreementId": AGREEMENT,
            "deploymentParameter": { "name": "Created", "secretString": "s" },
        }),
    )
    .await;
    assert_eq!(status, 200, "the Put must succeed, got {body:?}");
    assert_eq!(
        body.get("resourceArn").and_then(Value::as_str),
        Some(to_be_created.as_str()),
        "the first minted ARN must be the one the not-found row asked for, got {body:?}"
    );

    // The record now exists: that ARN's class moved from unknown to stored, and the
    // unaddressable value's did NOT move — a populated store cannot make a value this
    // API addresses nothing into a resource.
    let (status, code, answered) = classify(
        &router,
        "GET",
        &format!("/tags/{}", pct(&to_be_created)),
        None,
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        STORED,
        "case stored ARN after its create: expected {STORED:?}, got {status} {code:?} in \
         {answered:?}"
    );

    let (status, code, answered) = classify(
        &router,
        "GET",
        &format!("/tags/{}", pct(&unaddressable)),
        None,
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        UNADDRESSABLE,
        "case unaddressable, populated store: expected {UNADDRESSABLE:?}, got {status} \
         {code:?} in {answered:?}"
    );
}
