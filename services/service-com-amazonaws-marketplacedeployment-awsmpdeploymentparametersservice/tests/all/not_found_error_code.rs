// SPDX-License-Identifier: CC0-1.0

//! The not-found failure this service emits for a resource ARN it holds no record
//! for, over the whole class of ARNs that name nothing.
//!
//! The rule under test: EVERY operation of this service that resolves a
//! caller-supplied resource ARN answers an ARN OF THIS SERVICE'S VENDOR that names
//! no stored deployment parameter with HTTP 404 and the wire error code
//! `NotFoundException` — the code this service's recorded responses carry, which is
//! NOT its modelled `ResourceNotFoundException` shape name — and never with a
//! success carrying an empty tag map, which would mask a bad ARN as "no tags yet".
//!
//! A value that is NOT a resource ARN of this service's vendor never reaches this
//! refusal at all: it is refused one step earlier as a bad request — HTTP 400 with
//! the tagging front-end's `BadRequestException` code — which the
//! `resource_arn_validation` suite owns. The two suites partition the space of
//! caller-supplied ARNs between them, and the two codes are what a client tells a
//! permanently malformed input from a resource that is simply not there by.
//!
//! The three operations that bind `resourceArn` as an `@httpLabel`
//! (`smithy_query(query="digest")`: `GET /tags/{resourceArn}`,
//! `POST /tags/{resourceArn}`, `DELETE /tags/{resourceArn}`) are each driven over
//! the same table of ARN forms, through the production `MultiServiceRouter` and
//! restJson1 serialization, so what is asserted is what a signed SDK request
//! observes: the status, the body's `__type` and the `x-amzn-errortype` header a
//! client branches on.
//!
//! `PutDeploymentParameter`'s own not-found path — a `(catalog, productId,
//! agreementId)` TARGET the account does not hold — is a different refusal with a
//! different code, pinned by the `put_deployment_parameter_target` suite; it is not
//! touched here.
//!
//! The harness (router, signed request, per-operation callers) is the one the
//! `deployment_parameters` suite owns.

use serde_json::{json, Value};
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::scaffolded::{
    NOT_FOUND_CODE, NOT_FOUND_TEXT,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::deployment_parameters::{
    emulator, error_code, pct, provision, put, request, ACCOUNT, REGION, SIGNING_NAME,
};

/// The wire error code this service answers an unknown resource ARN with.
const EXPECTED_CODE: &str = NOT_FOUND_CODE;
/// The status it answers it with — the same 404 the modelled
/// `ResourceNotFoundException` carries (`httpError 404`); only the code differs.
const EXPECTED_STATUS: u16 = 404;
/// The one text the service states for every such refusal, as its recorded
/// not-found responses carry it. Written out here rather than read from the crate,
/// so the assertion does not merely compare the implementation with itself.
const EXPECTED_MESSAGE: &str = "Resource was not found";

/// The catalog, agreement and product every setup write below targets.
const CATALOG: &str = "AWSMarketplace";
const AGREEMENT: &str = "agmt-present";
const PRODUCT: &str = "prod-present";

/// Status, parsed body and the `x-amzn-errortype` header of one signed call.
///
/// The suite's shared `call` folds the header into the body's `__type`; this one
/// keeps the two apart, because the header is what a restJson1 client resolves the
/// error shape from and it must carry the same code the body does.
async fn call_reading_header(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value, Option<String>) {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let header = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    (status, parsed, header)
}

/// The three operations that resolve a caller-supplied `resourceArn`, as the method
/// and the request they send for one ARN.
fn tag_operations(resource_arn: &str) -> Vec<(&'static str, &'static str, String, Option<Value>)> {
    let path = format!("/tags/{}", pct(resource_arn));
    vec![
        ("ListTagsForResource", "GET", path.clone(), None),
        (
            "TagResource",
            "POST",
            path.clone(),
            Some(json!({ "tags": { "owner": "integration-test" } })),
        ),
        (
            "UntagResource",
            "DELETE",
            format!("{path}?tagKeys=owner"),
            None,
        ),
    ]
}

/// One emulator holding exactly one deployment parameter, and the ARN the service
/// minted for it.
async fn emulator_with_one_parameter() -> (MultiServiceRouter, String) {
    let (router, engine) = emulator();
    provision(&engine, CATALOG, AGREEMENT, &[PRODUCT]);
    let (status, body) = put(
        &router,
        CATALOG,
        PRODUCT,
        &json!({
            "agreementId": AGREEMENT,
            "deploymentParameter": { "name": "Present", "secretString": "s" },
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
async fn every_tag_operation_answers_an_unknown_resource_arn_with_not_found_exception() {
    // The crate's own constants are the recorded ones, so every row below is
    // asserted against the recording rather than against the implementation.
    assert_eq!(
        NOT_FOUND_CODE, "NotFoundException",
        "the service's not-found code must be the recorded one, got {NOT_FOUND_CODE:?}"
    );
    assert_eq!(
        NOT_FOUND_TEXT, EXPECTED_MESSAGE,
        "the service's not-found text must be the recorded one, got {NOT_FOUND_TEXT:?}"
    );
    // A record that DOES exist, so a 404 below is about the ARN and not about an
    // empty store, and so rows can be derived from a live ARN.
    let (router, live) = emulator_with_one_parameter().await;

    // The class the rule generalises over: every form of ARN OF THIS SERVICE'S
    // VENDOR that names no stored deployment parameter. The rows differ along the
    // ARN's partition, region, account, resource template and length — not merely
    // along the resource id. A value of ANOTHER vendor, or one that is no ARN at
    // all, belongs to the input-validation rule instead (the
    // `resource_arn_validation` suite).
    let unknown: Vec<(&str, String)> = vec![
        (
            // The resource template of the model's own `DeploymentParameter`
            // resource example: a form this emulator never mints, so it can only
            // ever be unknown.
            "the model's DeploymentParameter resource template",
            format!(
                "arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:DeploymentParameter:catalogs/AWSMarketplace/products/prod-1a2b3c4d/dp-9f8e7d6c"
            ),
        ),
        (
            // The template this emulator DOES mint, with an id it never minted.
            "a minted-shape ARN of an id that was never minted",
            format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:deployment-parameter/dp-ffffffffffffffff"),
        ),
        (
            "the live ARN in another region",
            live.replace(REGION, "eu-central-1"),
        ),
        (
            "the live ARN in another account",
            live.replace(ACCOUNT, "000000000001"),
        ),
        (
            "the live ARN in another partition",
            live.replace("arn:aws:", "arn:aws-cn:"),
        ),
        (
            "the live ARN with its resource segment truncated to its prefix",
            live.trim_end_matches(|c: char| c != '/').to_string(),
        ),
        (
            "the live ARN with one trailing character added",
            format!("{live}0"),
        ),
        (
            "a 512-character resource segment of this vendor",
            format!(
                "arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:deployment-parameter/{}",
                "a".repeat(512)
            ),
        ),
        (
            "this vendor's ARN with an empty region and account",
            format!("arn:aws:{SIGNING_NAME}:::deployment-parameter/dp-0000000000000001"),
        ),
    ];

    for (case, resource_arn) in unknown {
        assert_ne!(
            resource_arn, live,
            "case {case:?}: the row must not name the stored record"
        );
        for (operation, method, uri, body) in tag_operations(&resource_arn) {
            let (status, answered, header) =
                call_reading_header(&router, method, &uri, body.as_ref()).await;
            assert_eq!(
                status, EXPECTED_STATUS,
                "case {case:?}/{operation}: expected HTTP {EXPECTED_STATUS}, got {status} {answered:?}"
            );
            assert_eq!(
                error_code(&answered),
                EXPECTED_CODE,
                "case {case:?}/{operation}: expected code {EXPECTED_CODE}, got {answered:?}"
            );
            assert_eq!(
                header.as_deref(),
                Some(EXPECTED_CODE),
                "case {case:?}/{operation}: expected x-amzn-errortype {EXPECTED_CODE}, got {header:?}"
            );
            // The one answer AWS never gives: a success that masks a bad ARN as an
            // empty tag map.
            assert!(
                answered.get("tags").is_none(),
                "case {case:?}/{operation}: a not-found answer carries no tags, got {answered:?}"
            );
            // ONE constant text for every row: the service's not-found message
            // carries nothing from the request, so it cannot vary with the ARN. The
            // text is the one the service's recorded not-found responses carry.
            assert_eq!(
                answered.get("message").and_then(Value::as_str),
                Some(EXPECTED_MESSAGE),
                "case {case:?}/{operation}: expected the constant {EXPECTED_MESSAGE:?}, got {answered:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_not_found_code_does_not_displace_the_stored_record_or_the_other_errors() {
    let (router, live) = emulator_with_one_parameter().await;

    // The ARN that IS stored still reads back 200 through the same route, so the
    // 404 above is the store's answer about the ARN, not a blanket refusal.
    let (status, body, header) =
        call_reading_header(&router, "GET", &format!("/tags/{}", pct(&live)), None).await;
    assert_eq!(
        status, 200,
        "the stored record must still read back, got {status} {body:?}"
    );
    assert_eq!(
        header, None,
        "a success carries no x-amzn-errortype, got {header:?}"
    );

    // A stored ARN with a tag value outside the modelled `TagValue` bounds is still
    // the modelled `ValidationException`: only the not-found code changed.
    let refused = json!({ "tags": { "owner": "not a tag value" } });
    let (status, body, _) = call_reading_header(
        &router,
        "POST",
        &format!("/tags/{}", pct(&live)),
        Some(&refused),
    )
    .await;
    assert_eq!(
        status, 400,
        "a tag value outside the modelled bounds must be 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "expected ValidationException, got {body:?}"
    );
}
