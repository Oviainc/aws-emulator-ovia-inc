// SPDX-License-Identifier: CC0-1.0

//! How this service judges the `resourceArn` a tagging operation carries.
//!
//! The rule under test: a caller-supplied resource ARN is validated on the INPUT,
//! before any resource is looked up — a value that is not a well-formed resource ARN
//! of this service's own vendor (its `aws.api#service$arnNamespace`, which
//! `state::ARN_NAMESPACE` holds) names nothing this API can address, so every
//! operation binding `resourceArn` answers it with the modelled
//! `ValidationException`: HTTP 400, a message naming the member and the vendor that
//! was expected, and `fieldName` = `resourceArn`. A not-found is reserved for an ARN
//! that IS one of this service's and names no stored record, which the
//! `not_found_error_code` suite pins.
//!
//! Why the two must not be confused: an SDK client branches on the error shape, and
//! restJson1 resolves it from `x-amzn-errortype`. A permanent, caller-side input bug
//! answered as "resource does not exist yet" invites a retry or a create, and the
//! modelled `ValidationException$fieldName` that tells the caller WHICH member was
//! wrong is never produced.
//!
//! The three operations that bind `resourceArn` as a required `@httpLabel`
//! (`smithy_query(query="operation",operation="ListTagsForResource,TagResource,UntagResource")`:
//! `GET /tags/{resourceArn}`, `POST /tags/{resourceArn}`,
//! `DELETE /tags/{resourceArn}`) are each driven over the same table, through the
//! production `MultiServiceRouter` and restJson1 serialization, so what is asserted
//! is what a signed SDK request observes.
//!
//! The harness (router, signed request, per-operation callers) is the one the
//! `deployment_parameters` suite owns.

use serde_json::{json, Value};
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::state::{
    ARN_NAMESPACE, RESOURCE_ARN_CHARACTER_CLASS, RESOURCE_ARN_MAX_LEN, RESOURCE_ARN_MEMBER,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::deployment_parameters::{
    emulator, error_code, field_name, message, pct, provision, put, request, tags_of, ACCOUNT,
    REGION, SIGNING_NAME,
};

/// The status an input refusal carries — the modelled `ValidationException`'s
/// `httpError`.
const EXPECTED_STATUS: u16 = 400;
/// The wire error code it carries: the modelled shape name, which is what a
/// restJson1 client resolves the typed error from.
const EXPECTED_CODE: &str = "ValidationException";
/// The member every refusal below is about, written out rather than read from the
/// crate so the assertion is not the implementation compared with itself.
const EXPECTED_FIELD: &str = "resourceArn";

/// The catalog, agreement and product the one stored record below is written under.
const CATALOG: &str = "AWSMarketplace";
const AGREEMENT: &str = "agmt-arn";
const PRODUCT: &str = "prod-arn";

/// Status, parsed body and the `x-amzn-errortype` header of one signed call.
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

/// The three operations that resolve a caller-supplied `resourceArn`, as the method,
/// URI and body each sends for one ARN.
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
/// minted for it — so a refusal below is about the ARN the request carried and not
/// about an empty store.
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
async fn a_resource_arn_that_is_not_of_this_vendor_is_refused_on_the_input() {
    assert_eq!(
        ARN_NAMESPACE, "aws-marketplace",
        "this service's ARN namespace is its own signing name, got {ARN_NAMESPACE:?}"
    );
    assert_eq!(
        RESOURCE_ARN_MEMBER, EXPECTED_FIELD,
        "the refusal names the member the request carries, got {RESOURCE_ARN_MEMBER:?}"
    );
    let (router, live) = emulator_with_one_parameter().await;

    // The class the rule generalises over: every value that is not a well-formed
    // resource ARN of this vendor. The rows differ along WHICH part of the form is
    // wrong — the vendor field, the `arn` prefix, the number of fields, an empty
    // field, the resource, the characters, the length — not merely along the
    // resource id. Each row is paired with whether the refusal must name the
    // expected VENDOR (the ARN parses as something, but not as one of ours) or the
    // member's own syntax (the value is outside the modelled `ResourceArn` shape).
    let over_long = format!(
        "arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:deployment-parameter/{}",
        "a".repeat(RESOURCE_ARN_MAX_LEN)
    );
    let refused: Vec<(&str, String, bool)> = vec![
        (
            // The recorded instance: an ARN of another AWS service's namespace.
            "an ARN in another service's namespace",
            format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret/dp-0000000000000001"),
            true,
        ),
        (
            "the live ARN with its vendor field replaced",
            live.replace(SIGNING_NAME, "marketplace"),
            true,
        ),
        (
            "the live ARN with its vendor field in another case",
            live.replace(SIGNING_NAME, "AWS-Marketplace"),
            true,
        ),
        (
            "the live ARN with an empty vendor field",
            live.replace(&format!(":{SIGNING_NAME}:"), "::"),
            true,
        ),
        (
            "an identifier that is not an ARN at all",
            "dp-0000000000000001".to_string(),
            true,
        ),
        (
            "an ARN-shaped value whose first field is not arn",
            live.replacen("arn:", "urn:", 1),
            true,
        ),
        (
            "an ARN that stops before its resource field",
            format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}"),
            true,
        ),
        (
            "an ARN of this vendor with an empty resource field",
            format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:"),
            true,
        ),
        (
            "an ARN of this vendor with an empty partition field",
            format!("arn::{SIGNING_NAME}:{REGION}:{ACCOUNT}:deployment-parameter/dp-1"),
            true,
        ),
        ("a single-character resource arn", "-".to_string(), true),
        (
            "a 512-character value that is no ARN",
            "a".repeat(512),
            true,
        ),
        (
            // Outside the modelled `ResourceArn` character class: a space is not in
            // `[a-zA-Z0-9:*/-]`.
            "an ARN of this vendor carrying a space",
            format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:deployment parameter/dp-1"),
            false,
        ),
        (
            // One character past the modelled `ResourceArn` maximum.
            "a value one character past the modelled maximum",
            format!("{}a", &over_long[..RESOURCE_ARN_MAX_LEN]),
            false,
        ),
    ];

    for (case, resource_arn, names_vendor) in refused {
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
            assert_eq!(
                field_name(&answered),
                EXPECTED_FIELD,
                "case {case:?}/{operation}: the refusal names the failing member, got {answered:?}"
            );
            let reported = message(&answered);
            assert!(
                reported.contains("resource arn"),
                "case {case:?}/{operation}: the message names the member, got {reported:?}"
            );
            if names_vendor {
                assert!(
                    reported.contains(ARN_NAMESPACE),
                    "case {case:?}/{operation}: the message names the expected vendor \
                     {ARN_NAMESPACE:?}, got {reported:?}"
                );
            } else {
                assert!(
                    reported.contains(RESOURCE_ARN_CHARACTER_CLASS)
                        && reported.contains(&RESOURCE_ARN_MAX_LEN.to_string()),
                    "case {case:?}/{operation}: the message states the member's own syntax, \
                     got {reported:?}"
                );
            }
            // A refusal is not a 404 and carries no tag map: the two answers are
            // what a client tells a caller-side bug from a missing resource by.
            assert!(
                answered.get("tags").is_none(),
                "case {case:?}/{operation}: a refusal carries no tags, got {answered:?}"
            );
        }
    }

    // Nothing above reached the store: the stored record still reads back, with the
    // empty tag map its create left.
    let (status, listed, _) =
        call_reading_header(&router, "GET", &format!("/tags/{}", pct(&live)), None).await;
    assert_eq!(
        status, 200,
        "the stored record must still read back, got {status} {listed:?}"
    );
    assert!(
        tags_of(&listed).is_empty(),
        "a refused TagResource must not have written a tag, got {listed:?}"
    );
}

#[tokio::test]
async fn a_resource_arn_of_this_vendor_is_judged_by_the_store_and_not_refused() {
    let (router, live) = emulator_with_one_parameter().await;

    // The complement of the table above: values that ARE well-formed resource ARNs
    // of this vendor. None is refused as invalid input — the one that names the
    // stored record succeeds, and the rest are the store's 404.
    let accepted: Vec<(&str, String, u16)> = vec![
        ("the stored record's own ARN", live.clone(), 200),
        (
            "an id of this vendor that was never minted",
            format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:deployment-parameter/dp-ffffffff"),
            404,
        ),
        (
            "the model's own DeploymentParameter resource template",
            format!(
                "arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:DeploymentParameter:catalogs/{CATALOG}/products/prod-1a2b3c4d/dp-9f8e7d6c"
            ),
            404,
        ),
        (
            "this vendor in another partition",
            live.replace("arn:aws:", "arn:aws-cn:"),
            404,
        ),
        (
            "this vendor with an empty region and account",
            format!("arn:aws:{SIGNING_NAME}:::deployment-parameter/dp-0000000000000001"),
            404,
        ),
        (
            "this vendor with a wildcard in its resource field",
            format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:deployment-parameter/*"),
            404,
        ),
        (
            // Exactly at the modelled `ResourceArn` maximum, which is inside it.
            "a value exactly at the modelled maximum length",
            {
                let prefix =
                    format!("arn:aws:{SIGNING_NAME}:{REGION}:{ACCOUNT}:deployment-parameter/");
                format!(
                    "{prefix}{}",
                    "a".repeat(RESOURCE_ARN_MAX_LEN - prefix.len())
                )
            },
            404,
        ),
    ];

    for (case, resource_arn, expected) in accepted {
        let (status, answered, _) = call_reading_header(
            &router,
            "GET",
            &format!("/tags/{}", pct(&resource_arn)),
            None,
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status} {answered:?}"
        );
        if expected != 200 {
            assert_eq!(
                error_code(&answered),
                "NotFoundException",
                "case {case:?}: an ARN of this vendor that names nothing is the store's \
                 not-found, got {answered:?}"
            );
        }
        assert_ne!(
            error_code(&answered),
            EXPECTED_CODE,
            "case {case:?}: a well-formed ARN of this vendor is not an input refusal, \
             got {answered:?}"
        );
    }
}
