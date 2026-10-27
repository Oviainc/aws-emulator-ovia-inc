// SPDX-License-Identifier: CC0-1.0

//! The one `EncryptionContext` projection, driven through both operations that report a
//! Domain record.
//!
//! The rule: a Domain's `encryptionContext` is read out of the stored record — the
//! stored encryption type's wire value, and the key the record holds, reported for the
//! encryption type the model documents it on (`smithy_query(query="shape",
//! shape="EncryptionContext")`: `encryptionType` required, `kmsKeyArn` "Only present
//! when encryptionType is CUSTOMER_MANAGED_KEY") — so `CreateDomain` and `GetDomain`,
//! which both project the same record through `state::Domain::encryption_context`,
//! cannot describe one domain's encryption differently.
//!
//! The table's rows differ along what the rule generalises over: whether the create
//! named a key at all, the form of the key id, the key's Region relative to the
//! domain's, and which other optional members the create carried. Every row is asserted
//! on the create's response, on a first read, and on a second read, so the projection is
//! pinned at each of its two former call sites.
//!
//! The harness (`Emulator`, `string_at`) is the sibling suites': one way of driving this
//! service through signed restJson1 requests, not two.

use serde_json::{json, Value};

use super::domain_subscription_lifecycle::{string_at, Emulator};

/// The `EncryptionType` values this service records
/// (`smithy_query(query="shape", shape="EncryptionType")`): the type reported for a
/// domain is one of these, and which one follows whether the create named a key.
const CUSTOMER_MANAGED_KEY: &str = "CUSTOMER_MANAGED_KEY";
const AWS_OWNED_KEY: &str = "AWS_OWNED_KEY";

/// A `CreateWebAppConfiguration` (`ehrRole`, `idcInstanceId`, `idcRegion`): one of the
/// other optional members a create may carry, so a row can show that the encryption
/// projection does not depend on it.
fn web_app_setup() -> Value {
    json!({
        "ehrRole": "arn:aws:iam::000000000000:role/ehr-operations",
        "idcInstanceId": "ssoins-72230ed02aa4b4ea",
        "idcRegion": "us-east-1",
    })
}

/// One row: the case, the Region the request is signed for, and the `CreateDomain` body.
struct Case {
    case: &'static str,
    region: &'static str,
    request: Value,
}

/// The rows the rule is asserted over. `KmsKeyArn`'s pattern is
/// `^arn:aws:kms:[a-z0-9-]+:[0-9]{12}:key/[a-f0-9-]+$`
/// (`smithy_query(query="shape", shape="KmsKeyArn")`), which every key below follows;
/// the row the issue cited (a create with no key at all) is the first.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "no key named",
            region: "us-east-1",
            request: json!({ "name": "clinic-owned" }),
        },
        Case {
            case: "a key with a dashed id",
            region: "us-east-1",
            request: json!({
                "name": "clinic-dashed",
                "kmsKeyArn":
                    "arn:aws:kms:us-east-1:000000000000:key/abcdef01-2345-6789-abcd-ef0123456789",
            }),
        },
        Case {
            case: "a key whose id carries no dashes",
            region: "eu-west-2",
            request: json!({
                "name": "clinic-flat",
                "kmsKeyArn": "arn:aws:kms:eu-west-2:000000000000:key/0123456789abcdef",
            }),
        },
        Case {
            case: "a key in another Region than the domain",
            region: "us-west-2",
            request: json!({
                "name": "clinic-crossregion",
                "kmsKeyArn":
                    "arn:aws:kms:ap-southeast-2:210987654321:key/fedcba98-7654-3210-fedc-ba9876543210",
            }),
        },
        Case {
            case: "a key beside every other optional member",
            region: "eu-central-1",
            request: json!({
                "name": "clinic-everything",
                "kmsKeyArn":
                    "arn:aws:kms:eu-central-1:000000000000:key/11112222-3333-4444-5555-666677778888",
                "tags": { "ward": "cardiology" },
                "webAppSetupConfiguration": web_app_setup(),
            }),
        },
        Case {
            case: "no key, beside every other optional member",
            region: "ap-southeast-2",
            request: json!({
                "name": "clinic-owned-everything",
                "tags": { "ward": "oncology" },
                "webAppSetupConfiguration": web_app_setup(),
            }),
        },
    ]
}

/// The `encryptionContext` the stored record implies for `request`: the type follows
/// whether a key was named, and the key is reported only with the customer-managed type.
fn expected_context(request: &Value) -> Value {
    match request.get("kmsKeyArn") {
        Some(key) => json!({ "encryptionType": CUSTOMER_MANAGED_KEY, "kmsKeyArn": key }),
        None => json!({ "encryptionType": AWS_OWNED_KEY }),
    }
}

#[tokio::test]
async fn create_domain_and_get_domain_report_one_encryption_context_projection() {
    for Case {
        case,
        region,
        request,
    } in cases()
    {
        let emulator = Emulator::new(region);
        let created = emulator.create_domain(&request).await;
        let expected = expected_context(&request);
        assert_eq!(
            created.get("encryptionContext"),
            Some(&expected),
            "case {case:?}: CreateDomain must report the stored encryption context, \
             expected {expected:?}, got {created:?}"
        );

        // The same record, through the reader: the projection is single-homed, so each
        // read reports exactly what the create did — and reports it again unchanged.
        let domain_id = string_at(&created, "domainId");
        let path = format!("/domain/{domain_id}");
        for read in 1..=2 {
            let (status, fetched) = emulator.send("GET", &path, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: read {read} of GetDomain got {fetched:?}"
            );
            assert_eq!(
                fetched.get("encryptionContext"),
                Some(&expected),
                "case {case:?}: read {read} of GetDomain must report the stored \
                 encryption context, expected {expected:?}, got {fetched:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_reported_key_is_the_domains_own_not_another_domains() {
    // Two domains in one store, encrypted differently: each response projects the record
    // the request names, so neither domain's context leaks into the other's.
    let emulator = Emulator::new("us-east-1");
    let key = "arn:aws:kms:us-east-1:000000000000:key/aaaabbbb-cccc-dddd-eeee-ffff00001111";
    let with_key = emulator
        .create_domain(&json!({ "name": "clinic-cmk", "kmsKeyArn": key }))
        .await;
    let without_key = emulator
        .create_domain(&json!({ "name": "clinic-aws-owned" }))
        .await;

    for (case, created) in [("customer-managed", &with_key), ("service-owned", &without_key)] {
        let domain_id = string_at(created, "domainId");
        let (status, fetched) = emulator
            .send("GET", &format!("/domain/{domain_id}"), None)
            .await;
        assert_eq!(status, 200, "case {case:?}: GetDomain got {fetched:?}");
        assert_eq!(
            fetched.get("encryptionContext"),
            created.get("encryptionContext"),
            "case {case:?}: GetDomain must report this domain's own encryption context, \
             got {fetched:?}"
        );
        assert_eq!(
            fetched
                .get("encryptionContext")
                .and_then(|context| context.get("kmsKeyArn"))
                .and_then(Value::as_str),
            created.get("kmsKeyArn").and_then(Value::as_str),
            "case {case:?}: the context's key is the key the domain holds, got {fetched:?}"
        );
    }
}
