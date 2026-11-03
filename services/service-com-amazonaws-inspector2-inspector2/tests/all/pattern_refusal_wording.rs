// SPDX-License-Identifier: CC0-1.0

//! How this service words the `ValidationException` it answers a value one of its modelled
//! `@pattern`s does not admit with.
//!
//! RULE: the clause names the member and the EXPRESSION every value of that member must
//! match — the model's `@pattern` with the whole-value anchors it wraps it in removed — and
//! states no value at all, so the refusal of a malformed value is the same sentence
//! whatever the caller submitted.
//!
//! Every request goes through the real `MultiServiceRouter` — routing, restJson1
//! deserialization, modelled-constraint validation, the handler, and restJson1
//! serialization — so what is asserted here is the wire behaviour a client sees.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::filters_and_enablement::{router, send_in};

/// The expression a `KmsKeyArn` refusal quotes: `UpdateEncryptionKey$kmsKeyId`'s `@pattern`
/// without its leading `^` and trailing `$`.
/// Source: `smithy_query(query="operation", operation="UpdateEncryptionKey")`.
const KMS_KEY_ARN: &str = "arn:aws(-(us-gov|cn))?:kms:([a-z0-9][-.a-z0-9]{0,62})?:[0-9]{12}?\
                           :key/(([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}\
                           -[0-9a-fA-F]{12})|(mrk-[0-9a-zA-Z]{32}))";

/// The expression an `AccountId` refusal quotes (`^\d{12}$` unanchored).
/// Source: `smithy_query(query="operation", operation="AssociateMember")`.
const ACCOUNT_ID: &str = r"\d{12}";

/// The expression a `ConnectorName` refusal quotes (`^[\p{L}\p{N}_-]+$` unanchored).
/// Source: `smithy_query(query="closure", operation="CreateConnector")`.
const CONNECTOR_NAME: &str = r"[\p{L}\p{N}_-]+";

/// The expression an `AwsConfigConnectorArn` refusal quotes — the nested
/// `providerDetail.azure.awsConfigConnectorArn` member's `@pattern`, unanchored.
/// Source: `smithy_query(query="closure", operation="CreateConnector")`.
const AWS_CONFIG_CONNECTOR_ARN: &str =
    r"arn:([^:]+):config:([^:]+):([^:]+):connector/([^/]+)/([^/]+)/([^/:\s]+)";

/// A `ScopeValue`, a `ConnectorName` and an `AzureRegion` every one of their own
/// `@pattern`s admits, so a `CreateConnector` row refuses exactly the member it means to.
/// Source: `smithy_query(query="closure", operation="CreateConnector")`.
const SCOPE_VALUE: &str = "0123abcd-4567-89ef-0123-456789abcdef";
const CONNECTOR: &str = "inspector-connector";
const AZURE_REGION: &str = "eastus";

/// The clause AWS states for a value outside `member`'s `@pattern`, in the framework's
/// one-error envelope: the member, the expression, and no value.
fn expected_message(member: &str, pattern: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: \
         Member must satisfy regular expression pattern: {pattern}"
    )
}

/// `CreateConnector`'s body with `name` and the nested `awsConfigConnectorArn` chosen by
/// the caller; every other member is one the model admits.
fn create_connector_body(name: &str, aws_config_connector_arn: &str) -> Value {
    json!({
        "name": name,
        "provider": "AZURE",
        "providerDetail": {
            "azure": {
                "awsConfigConnectorArn": aws_config_connector_arn,
                "scopeConfiguration": {
                    "vmScanning": {
                        "scopeType": "SUBSCRIPTION",
                        "scopeValues": [SCOPE_VALUE],
                    }
                },
                "azureRegions": [AZURE_REGION],
            }
        }
    })
}

/// One row: the case, the request to send, and the member whose `@pattern` it breaks
/// together with the expression that member's refusal must quote.
struct Row {
    case: &'static str,
    method: &'static str,
    uri: &'static str,
    body: Value,
    member: &'static str,
    pattern: &'static str,
    /// The value the row submits, which must NOT appear in the refusal.
    submitted: &'static str,
}

/// The rows span what the rule generalises over: the operation, the member, the expression,
/// the depth of the member path (a top-level member and one inside a union and a structure),
/// and the form of the offending value.
fn rows() -> Vec<Row> {
    let encryption_key = |case, kms_key_id: &'static str| Row {
        case,
        method: "PUT",
        uri: "/encryptionkey/update",
        body: json!({
            "kmsKeyId": kms_key_id,
            "resourceType": "AWS_LAMBDA_FUNCTION",
            "scanType": "CODE",
        }),
        member: "kmsKeyId",
        pattern: KMS_KEY_ARN,
        submitted: kms_key_id,
    };

    vec![
        encryption_key("a key id that is not an ARN at all", "not-a-valid-kms-arn"),
        encryption_key(
            "an alias ARN rather than a key ARN",
            "arn:aws:kms:us-east-1:111122223333:alias/inspector",
        ),
        encryption_key("an empty key id", ""),
        encryption_key(
            "an unmodelled partition",
            "arn:aws-iso:kms:us-east-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
        ),
        Row {
            case: "a twelve-character account id that is not digits",
            method: "POST",
            uri: "/members/associate",
            body: json!({ "accountId": "notanaccount" }),
            member: "accountId",
            pattern: ACCOUNT_ID,
            submitted: "notanaccount",
        },
        Row {
            case: "the same expression under another operation's member",
            method: "POST",
            uri: "/delegatedadminaccounts/enable",
            body: json!({ "delegatedAdminAccountId": "notanaccount" }),
            member: "delegatedAdminAccountId",
            pattern: ACCOUNT_ID,
            submitted: "notanaccount",
        },
        Row {
            case: "a connector name carrying a character the expression excludes",
            method: "POST",
            uri: "/connector/create",
            body: create_connector_body(
                "a name with spaces",
                "arn:aws:config:us-east-1:111122223333:connector/azure/tenant/one",
            ),
            member: "name",
            pattern: CONNECTOR_NAME,
            submitted: "a name with spaces",
        },
        Row {
            case: "a member nested inside a union and a structure",
            method: "POST",
            uri: "/connector/create",
            body: create_connector_body(CONNECTOR, "not-a-real-arn"),
            member: "providerDetail.azure.awsConfigConnectorArn",
            pattern: AWS_CONFIG_CONNECTOR_ARN,
            submitted: "not-a-real-arn",
        },
    ]
}

/// Send one row and return the status, the error type and the `message`.
async fn refusal_of(router: &MultiServiceRouter, row: &Row) -> (u16, Option<String>, Value) {
    send_in(router, row.method, "us-east-1", row.uri, Some(&row.body)).await
}

/// Every row is refused 400 `ValidationException` with the clause that names the member and
/// the unanchored expression — and names neither the value the request carried nor the
/// anchors the model wraps the expression in.
#[tokio::test]
async fn a_pattern_refusal_states_the_unanchored_expression_and_no_value() {
    let router = router();
    for row in rows() {
        let (status, error_type, body) = refusal_of(&router, &row).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected 400, got {status} / {body}",
            row.case
        );
        assert_eq!(
            error_type.as_deref(),
            Some("ValidationException"),
            "case {:?}: expected ValidationException, got {error_type:?} / {body}",
            row.case
        );
        let message = body["message"].as_str().unwrap_or_else(|| {
            panic!("case {:?}: the refusal carries no message: {body}", row.case)
        });
        assert_eq!(
            message,
            expected_message(row.member, row.pattern),
            "case {:?}: unexpected refusal wording, got {message:?}",
            row.case
        );
        assert!(
            !message.contains(&format!("'{}'", row.submitted)),
            "case {:?}: the refusal must not quote the submitted value, got {message:?}",
            row.case
        );
        let quoted = format!("pattern: {}", row.pattern);
        assert!(
            message.ends_with(&quoted) && !message.contains(&format!("pattern: ^{}", row.pattern)),
            "case {:?}: the quoted expression must carry no anchors, got {message:?}",
            row.case
        );
    }
}

/// The refusal does not vary with the value: every malformed `kmsKeyId` is refused with the
/// same sentence, so a client matching it matches for every bad input rather than for one.
#[tokio::test]
async fn the_refusal_is_the_same_sentence_for_every_rejected_value() {
    let router = router();
    // Rows differ along WHICH part of the expression the value breaks, and along length,
    // including a value far longer than the member's own ARN form.
    let values: [&str; 6] = [
        "",
        "x",
        "not-a-valid-kms-arn",
        "ARN:AWS:KMS:US-EAST-1:111122223333:KEY/1234abcd-12ab-34cd-56ef-1234567890ab",
        "arn:aws:kms:us-east-1:111122223333:key/not-a-key-id",
        "arn:aws:kms:us-east-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab/extra",
    ];

    let expected = expected_message("kmsKeyId", KMS_KEY_ARN);
    for value in values {
        let body = json!({
            "kmsKeyId": value,
            "resourceType": "AWS_ECR_REPOSITORY",
            "scanType": "PACKAGE",
        });
        let (status, _, response) =
            send_in(&router, "PUT", "us-east-1", "/encryptionkey/update", Some(&body)).await;
        assert_eq!(
            status, 400,
            "case {value:?}: expected 400, got {status} / {response}"
        );
        assert_eq!(
            response["message"].as_str(),
            Some(expected.as_str()),
            "case {value:?}: every rejected value must be refused with the same sentence, got \
             {response}"
        );
    }
}

/// A refusal of another constraint class keeps the framework's own wording, value and all:
/// only the `@pattern` class is reworded, and an absent required member is still stated as
/// `Value null at '<member>'`.
#[tokio::test]
async fn another_constraint_class_keeps_its_own_clause() {
    let router = router();

    // An absent required member: the deserializer's refusal, not the constraint walk's.
    let (status, error_type, body) = send_in(
        &router,
        "PUT",
        "us-east-1",
        "/encryptionkey/update",
        Some(&json!({ "resourceType": "AWS_LAMBDA_FUNCTION", "scanType": "CODE" })),
    )
    .await;
    assert_eq!(status, 400, "an absent kmsKeyId: expected 400, got {body}");
    assert_eq!(
        error_type.as_deref(),
        Some("ValidationException"),
        "an absent kmsKeyId: expected ValidationException, got {error_type:?} / {body}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(
            "1 validation error detected: Value null at 'kmsKeyId' failed to satisfy constraint: \
             Member must not be null"
        ),
        "an absent kmsKeyId: unexpected refusal wording, got {body}"
    );

    // A value outside a `@length` bound: the clause still quotes what was submitted.
    let too_long = "x".repeat(129);
    let (status, _, body) = send_in(
        &router,
        "POST",
        "us-east-1",
        "/filters/delete",
        Some(&json!({ "arn": too_long })),
    )
    .await;
    assert_eq!(status, 400, "a filter ARN over its bound: got {body}");
    assert_eq!(
        body["message"].as_str(),
        Some(
            format!(
                "1 validation error detected: Value '{too_long}' at 'arn' failed to satisfy \
                 constraint: Member must have length less than or equal to 128"
            )
            .as_str()
        ),
        "a filter ARN over its bound: unexpected refusal wording, got {body}"
    );
}
