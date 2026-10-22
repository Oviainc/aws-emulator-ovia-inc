// SPDX-License-Identifier: CC0-1.0

//! How this service STATES a refused request.
//!
//! The rule under test: a `@pattern` is reported as the expression a value must
//! satisfy, WITHOUT the `^` / `$` the model spells it with. Smithy's `@pattern` is
//! an unanchored find, so those characters are the model's way of making the match
//! total — they are not part of the character class a client may send, and a
//! message quoting them reports a pattern the caller never sent. The rest of the
//! AWS validation report is unchanged: the `N validation error(s) detected:`
//! envelope over `Value '<value>' at '<member path>' failed to satisfy constraint:
//! <requirement>`.
//!
//! A `@sensitive` member is the one exception to the echo: its value is left out,
//! so a rejected secret never reaches a client's logs through an error body.
//!
//! Every case drives the real wire path through the harness the
//! `deployment_parameters` suite owns.

use serde_json::{json, Value};
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::state::{
    TAG_CHARACTER_CLASS, TAG_KEY_MAX_LEN, TAG_VALUE_MAX_LEN,
};

use crate::deployment_parameters::{emulator, error_code, message, provision, put, tag};

/// `Catalog`'s `@pattern`, unanchored. Model value: `^[a-zA-Z_-]+$`
/// (`smithy_query(query="shape",shape="Catalog")`).
const CATALOG_PATTERN: &str = "[a-zA-Z_-]+";
/// `ResourceId`'s `@pattern`, unanchored — the shape `productId` and
/// `agreementId` both target. Model value: `^[A-Za-z0-9_/-]+$`.
const RESOURCE_ID_PATTERN: &str = "[A-Za-z0-9_/-]+";
/// `DeploymentParameterName`'s `@pattern`, unanchored. Model value:
/// `^[a-zA-Z0-9/_+=.@-]+$`.
const PARAMETER_NAME_PATTERN: &str = "[a-zA-Z0-9/_+=.@-]+";
/// `ClientToken`'s `@pattern`, unanchored. Model value:
/// `^[a-zA-Z0-9/_+=.:@-]+$`.
const CLIENT_TOKEN_PATTERN: &str = "[a-zA-Z0-9/_+=.:@-]+";

#[tokio::test]
async fn a_pattern_violation_quotes_the_pattern_without_the_models_anchors() {
    let (router, engine) = emulator();
    // A target the account holds, so nothing below is refused for naming no
    // product or agreement instead of for its pattern.
    provision(&engine, "AWSMarketplace", "agmt-held", &["prod-held"]);

    // (case, catalog, productId, body, the member path the report names, the
    // rejected value, the pattern the report must quote)
    let rows: Vec<(&str, &str, &str, Value, &str, String, &str)> = vec![
        (
            "a catalog carrying a character its pattern excludes",
            "Not-A-Real-Catalog!",
            "prod-held",
            json!({
                "agreementId": "agmt-held",
                "deploymentParameter": { "name": "N", "secretString": "s" },
            }),
            "catalog",
            "Not-A-Real-Catalog!".to_string(),
            CATALOG_PATTERN,
        ),
        (
            "a catalog carrying a digit, which its pattern excludes",
            "AWSMarketplace1",
            "prod-held",
            json!({
                "agreementId": "agmt-held",
                "deploymentParameter": { "name": "N", "secretString": "s" },
            }),
            "catalog",
            "AWSMarketplace1".to_string(),
            CATALOG_PATTERN,
        ),
        (
            "a product id carrying a bang",
            "AWSMarketplace",
            "prod!held",
            json!({
                "agreementId": "agmt-held",
                "deploymentParameter": { "name": "N", "secretString": "s" },
            }),
            "productId",
            "prod!held".to_string(),
            RESOURCE_ID_PATTERN,
        ),
        (
            "an agreement id carrying a space",
            "AWSMarketplace",
            "prod-held",
            json!({
                "agreementId": "agmt held",
                "deploymentParameter": { "name": "N", "secretString": "s" },
            }),
            "agreementId",
            "agmt held".to_string(),
            RESOURCE_ID_PATTERN,
        ),
        (
            "a parameter name carrying a colon, which only tag keys admit",
            "AWSMarketplace",
            "prod-held",
            json!({
                "agreementId": "agmt-held",
                "deploymentParameter": { "name": "Db:Password", "secretString": "s" },
            }),
            "deploymentParameter.name",
            "Db:Password".to_string(),
            PARAMETER_NAME_PATTERN,
        ),
        (
            "an idempotency token of the right length carrying a bang",
            "AWSMarketplace",
            "prod-held",
            json!({
                "agreementId": "agmt-held",
                "deploymentParameter": { "name": "N", "secretString": "s" },
                "clientToken": "0123456789abcdef0123456789abcde!",
            }),
            "clientToken",
            "0123456789abcdef0123456789abcde!".to_string(),
            CLIENT_TOKEN_PATTERN,
        ),
    ];

    for (case, catalog, product_id, body, path, value, pattern) in rows {
        let (status, answer) = put(&router, catalog, product_id, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a pattern violation must be 400, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        let reported = message(&answer);
        let expected = format!(
            "1 validation error detected: Value '{value}' at '{path}' failed to satisfy \
             constraint: Member must satisfy regular expression pattern: {pattern}"
        );
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the AWS validation report quoting the unanchored pattern, \
             got {reported:?}"
        );
        assert!(
            !reported.contains('^') && !reported.contains('$'),
            "case {case:?}: a reported pattern carries no anchors, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_bound_violation_states_the_bound_and_a_sensitive_value_is_not_echoed() {
    let (router, engine) = emulator();
    provision(&engine, "AWSMarketplace", "agmt-held", &["prod-held"]);

    // A `@length` violation is unaffected by the pattern rule: it states the
    // bound the model declares. `catalog` and `productId` are `length 1..64`,
    // `clientToken` `length 32..64`.
    let over_long_catalog = "c".repeat(65);
    let bounded: Vec<(&str, &str, &str, Value, String)> = vec![
        (
            "a catalog one character past its maximum",
            &over_long_catalog,
            "prod-held",
            json!({
                "agreementId": "agmt-held",
                "deploymentParameter": { "name": "N", "secretString": "s" },
            }),
            format!(
                "1 validation error detected: Value '{over_long_catalog}' at 'catalog' failed to \
                 satisfy constraint: Member must have length less than or equal to 64"
            ),
        ),
        (
            "an idempotency token shorter than its minimum",
            "AWSMarketplace",
            "prod-held",
            json!({
                "agreementId": "agmt-held",
                "deploymentParameter": { "name": "N", "secretString": "s" },
                "clientToken": "too-short",
            }),
            "1 validation error detected: Value 'too-short' at 'clientToken' failed to satisfy \
             constraint: Member must have length greater than or equal to 32"
                .to_string(),
        ),
    ];

    for (case, catalog, product_id, body, expected) in bounded {
        let (status, answer) = put(&router, catalog, product_id, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 refusal, got {status} {answer:?}"
        );
        let reported = message(&answer);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the bound the model declares, got {reported:?}"
        );
    }

    // `SecretString` is the model's only `@sensitive` shape: a violation on it
    // names the member and the constraint and echoes NOTHING of the value. The
    // bound tested here is its MINIMUM (`length 1..15000`): a value past the
    // MAXIMUM never reaches this refusal at all — the front door answers it with
    // HTTP 403 `ForbiddenException`, which the `front_door_forbidden` suite pins.
    let (status, answer) = put(
        &router,
        "AWSMarketplace",
        "prod-held",
        &json!({
            "agreementId": "agmt-held",
            "deploymentParameter": { "name": "N", "secretString": "" },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a secret short of its minimum must be 400, got {status} {answer:?}"
    );
    let reported = message(&answer);
    assert_eq!(
        reported,
        "1 validation error detected: Value at 'deploymentParameter.secretString' failed to \
         satisfy constraint: Member must have length greater than or equal to 1",
        "a @sensitive member must be refused without its value, got {reported:?}"
    );
    assert!(
        !reported.contains('^') && !reported.contains('$'),
        "a reported bound carries no anchors, got {reported:?}"
    );
}

#[tokio::test]
async fn the_tag_character_class_this_service_states_carries_no_anchors() {
    let (router, engine) = emulator();
    provision(&engine, "AWSMarketplace", "agmt-tags", &["prod-tags"]);
    let (status, created) = put(
        &router,
        "AWSMarketplace",
        "prod-tags",
        &json!({
            "agreementId": "agmt-tags",
            "deploymentParameter": { "name": "Tagged", "secretString": "s" },
        }),
    )
    .await;
    assert_eq!(status, 200, "the setup Put must succeed, got {created:?}");
    let arn = created
        .get("resourceArn")
        .and_then(Value::as_str)
        .expect("a minted resourceArn")
        .to_string();

    // A tag key or value outside the modelled `TagKey` / `TagValue` is refused by
    // this service's own tag check (the centralized walk does not report map-key
    // constraints), which states the same character class — so it is quoted
    // unanchored there too.
    let rows: Vec<(&str, Vec<(String, String)>)> = vec![
        (
            "a key carrying a bang",
            vec![("bad!".to_string(), "v".to_string())],
        ),
        (
            "a value carrying a space",
            vec![("k".to_string(), "bad value".to_string())],
        ),
        (
            "a key one character past its maximum",
            vec![("k".repeat(TAG_KEY_MAX_LEN + 1), "v".to_string())],
        ),
        (
            "a value one character past its maximum",
            vec![("k".to_string(), "v".repeat(TAG_VALUE_MAX_LEN + 1))],
        ),
    ];

    for (case, pairs) in rows {
        let tags = pairs.into_iter().collect();
        let (status, answer) = tag(&router, &arn, &tags).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 refusal, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        let reported = message(&answer);
        assert!(
            !reported.contains('^') && !reported.contains('$'),
            "case {case:?}: a stated character class carries no anchors, got {reported:?}"
        );
    }

    // The constant the refusals quote is the class itself.
    assert!(
        !TAG_CHARACTER_CLASS.contains('^') && !TAG_CHARACTER_CLASS.contains('$'),
        "the tag character class carries no anchors, got {TAG_CHARACTER_CLASS:?}"
    );
}
