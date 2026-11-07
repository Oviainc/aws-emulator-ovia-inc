// SPDX-License-Identifier: CC0-1.0

//! The Data Catalog's own security configuration: `PutDataCatalogEncryptionSettings`
//! and `GetDataCatalogEncryptionSettings`.
//!
//! The rule under test: a catalog's encryption settings are ONE record per catalog that
//! a write replaces whole and a read reports, with each setting a stored record does not
//! name reported at that setting's own default — encryption at rest `DISABLED`, and
//! connection passwords returned as stored. A read reports, so it never changes what a
//! later read sees, and a `CatalogId` this emulator holds no catalog under is refused by
//! both operations rather than served or created.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! of the two settings the write names, the encryption mode selected, whether a key or a
//! service role comes with it, and whether the request names its `CatalogId` or lets it
//! default. The recorded case — enforcing server-side encryption before tables are
//! created — is one row among them.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, error_message, ok, ACCOUNT_ID};

/// The published values of the modelled `CatalogEncryptionMode` enum, which are the only
/// modes `EncryptionAtRest$CatalogEncryptionMode` accepts.
///
/// Source: `smithy_query(query="shape", shape="CatalogEncryptionMode")` on this crate's
/// model, whose `values` are exactly these three.
const PUBLISHED_ENCRYPTION_MODES: [&str; 3] = ["DISABLED", "SSE-KMS", "SSE-KMS-WITH-SERVICE-ROLE"];

/// A KMS key ARN of this emulator's account, the form a client names a customer managed
/// key by.
const KEY_ARN: &str = "arn:aws:kms:us-east-1:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab";

/// An IAM role ARN matching the modelled `IAMRoleArn` pattern
/// (`^arn:aws(-(cn|us-gov|iso(-[bef])?))?:iam::[0-9]{12}:role/.+$`), which is what
/// `EncryptionAtRest$CatalogEncryptionServiceRole` targets.
const SERVICE_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/GlueCatalogEncryption";

/// The settings a catalog nothing has configured reports: both of them, each at its own
/// default, beside the catalog-search encryption state every read carries.
fn default_settings() -> Value {
    as_reported(json!({
        "EncryptionAtRest": { "CatalogEncryptionMode": "DISABLED" },
        "ConnectionPasswordEncryption": { "ReturnConnectionPasswordEncrypted": false }
    }))
}

/// The published settings `configured` as a READ reports them: with
/// `EncryptedCatalogSearchStatus` beside them.
///
/// The Data Catalog reports the state of encrypted catalog search with its encryption
/// settings, and no operation of this service turns that on — the published
/// `DataCatalogEncryptionSettings` carries no member for it — so every catalog reports it
/// `NOT_ENABLED`, as a value rather than as an absence.
fn as_reported(mut configured: Value) -> Value {
    configured["EncryptedCatalogSearchStatus"] = json!("NOT_ENABLED");
    configured
}

/// The `DataCatalogEncryptionSettings` body a read reports, for the catalog `request`
/// addresses.
async fn get_settings(router: &MultiServiceRouter, request: &Value) -> Value {
    ok(router, "GetDataCatalogEncryptionSettings", request).await
        ["DataCatalogEncryptionSettings"]
        .clone()
}

/// Write `settings` for the catalog `catalog` names (`None` lets the `CatalogId` default)
/// and assert the write answered 200 with an empty body.
async fn put_settings(
    router: &MultiServiceRouter,
    case: &str,
    settings: &Value,
    catalog: Option<&str>,
) {
    let mut request = json!({ "DataCatalogEncryptionSettings": settings });
    if let Some(catalog_id) = catalog {
        request["CatalogId"] = json!(catalog_id);
    }
    let answer = ok(router, "PutDataCatalogEncryptionSettings", &request).await;
    assert_eq!(
        answer,
        json!({}),
        "case {case}: PutDataCatalogEncryptionSettings reports no members, got {answer}"
    );
}

/// One row of the round-trip table: the case name, the settings the write carries, and
/// the settings the read must report for them.
type SettingsRow = (&'static str, Value, Value);

/// The rows the round-trip rule is asserted over: every published mode, each of the two
/// settings alone and both together, with and without a key and a service role.
fn round_trip_rows() -> Vec<SettingsRow> {
    vec![
        (
            "an empty settings structure",
            json!({}),
            default_settings(),
        ),
        (
            "encryption at rest turned off explicitly",
            json!({ "EncryptionAtRest": { "CatalogEncryptionMode": "DISABLED" } }),
            as_reported(json!({
                "EncryptionAtRest": { "CatalogEncryptionMode": "DISABLED" },
                "ConnectionPasswordEncryption": { "ReturnConnectionPasswordEncrypted": false }
            })),
        ),
        (
            // The recorded case: catalog encryption enforced under a customer key
            // before any table is created.
            "encryption at rest under a KMS key alias",
            json!({
                "EncryptionAtRest": {
                    "CatalogEncryptionMode": "SSE-KMS",
                    "SseAwsKmsKeyId": "alias/aws/glue"
                }
            }),
            as_reported(json!({
                "EncryptionAtRest": {
                    "CatalogEncryptionMode": "SSE-KMS",
                    "SseAwsKmsKeyId": "alias/aws/glue"
                },
                "ConnectionPasswordEncryption": { "ReturnConnectionPasswordEncrypted": false }
            })),
        ),
        (
            "encryption at rest under a key ARN and a service role",
            json!({
                "EncryptionAtRest": {
                    "CatalogEncryptionMode": "SSE-KMS-WITH-SERVICE-ROLE",
                    "SseAwsKmsKeyId": KEY_ARN,
                    "CatalogEncryptionServiceRole": SERVICE_ROLE_ARN
                }
            }),
            as_reported(json!({
                "EncryptionAtRest": {
                    "CatalogEncryptionMode": "SSE-KMS-WITH-SERVICE-ROLE",
                    "SseAwsKmsKeyId": KEY_ARN,
                    "CatalogEncryptionServiceRole": SERVICE_ROLE_ARN
                },
                "ConnectionPasswordEncryption": { "ReturnConnectionPasswordEncrypted": false }
            })),
        ),
        (
            "connection password encryption alone, enabled under a key",
            json!({
                "ConnectionPasswordEncryption": {
                    "ReturnConnectionPasswordEncrypted": true,
                    "AwsKmsKeyId": "alias/glue-connections"
                }
            }),
            as_reported(json!({
                "EncryptionAtRest": { "CatalogEncryptionMode": "DISABLED" },
                "ConnectionPasswordEncryption": {
                    "ReturnConnectionPasswordEncrypted": true,
                    "AwsKmsKeyId": "alias/glue-connections"
                }
            })),
        ),
        (
            "connection password encryption alone, disabled but keyed",
            json!({
                "ConnectionPasswordEncryption": {
                    "ReturnConnectionPasswordEncrypted": false,
                    "AwsKmsKeyId": KEY_ARN
                }
            }),
            as_reported(json!({
                "EncryptionAtRest": { "CatalogEncryptionMode": "DISABLED" },
                "ConnectionPasswordEncryption": {
                    "ReturnConnectionPasswordEncrypted": false,
                    "AwsKmsKeyId": KEY_ARN
                }
            })),
        ),
        (
            "both settings at once",
            json!({
                "EncryptionAtRest": {
                    "CatalogEncryptionMode": "SSE-KMS",
                    "SseAwsKmsKeyId": KEY_ARN
                },
                "ConnectionPasswordEncryption": {
                    "ReturnConnectionPasswordEncrypted": true,
                    "AwsKmsKeyId": KEY_ARN
                }
            }),
            as_reported(json!({
                "EncryptionAtRest": {
                    "CatalogEncryptionMode": "SSE-KMS",
                    "SseAwsKmsKeyId": KEY_ARN
                },
                "ConnectionPasswordEncryption": {
                    "ReturnConnectionPasswordEncrypted": true,
                    "AwsKmsKeyId": KEY_ARN
                }
            })),
        ),
    ]
}

/// A catalog nothing has configured reports both settings at their defaults, and the
/// read reports rather than writes: repeated reads, with and without the `CatalogId`
/// named, answer the same thing and leave the next read alone.
#[tokio::test]
async fn an_unconfigured_catalog_reports_both_settings_at_their_defaults() {
    let router = emulator();

    // (case, the request body)
    let cases: Vec<(&str, Value)> = vec![
        ("the catalog id left to default", json!({})),
        (
            "the catalog id named explicitly",
            json!({ "CatalogId": ACCOUNT_ID }),
        ),
    ];

    for (case, request) in &cases {
        let first = get_settings(&router, request).await;
        assert_eq!(
            first,
            default_settings(),
            "case {case}: expected {}, got {first}",
            default_settings()
        );
        // Reporting, not mutating: a second read of the same catalog answers the same,
        // and so does a read of the OTHER spelling of the same catalog id.
        let second = get_settings(&router, request).await;
        assert_eq!(
            second, first,
            "case {case}: a read must not change what a later read sees; got {first} then \
             {second}"
        );
    }

    let after_reads = get_settings(&router, &json!({})).await;
    assert_eq!(
        after_reads,
        default_settings(),
        "case after four reads: the defaults must still be reported, got {after_reads}"
    );
}

/// Every settings structure a write carries is reported back by the read, with each
/// setting the write did not name reported at that setting's default.
#[tokio::test]
async fn a_written_settings_record_is_reported_back_with_defaults_filled_in() {
    for (case, sent, expected) in round_trip_rows() {
        let router = emulator();
        put_settings(&router, case, &sent, None).await;

        let reported = get_settings(&router, &json!({})).await;
        assert_eq!(
            reported, expected,
            "case {case}: expected {expected}, got {reported}"
        );

        // Stored, not computed per call: a second read answers the same record.
        let again = get_settings(&router, &json!({})).await;
        assert_eq!(
            again, reported,
            "case {case}: two reads of one catalog must agree; got {reported} then {again}"
        );
    }
}

/// Every published `CatalogEncryptionMode` round-trips, including one the rest of this
/// suite does not cite, and a mode outside the published list is refused.
#[tokio::test]
async fn every_published_encryption_mode_round_trips_and_an_unpublished_one_is_refused() {
    for mode in PUBLISHED_ENCRYPTION_MODES {
        let router = emulator();
        put_settings(
            &router,
            mode,
            &json!({ "EncryptionAtRest": { "CatalogEncryptionMode": mode } }),
            None,
        )
        .await;
        let reported = get_settings(&router, &json!({})).await;
        assert_eq!(
            reported["EncryptionAtRest"]["CatalogEncryptionMode"],
            json!(mode),
            "case {mode}: expected the mode written back, got {reported}"
        );
    }

    // One step past the published list: a value no `CatalogEncryptionMode` names is
    // refused with a 400, and the catalog's stored settings are left alone.
    for unpublished in ["SSE-S3", "sse-kms", "ENABLED", ""] {
        let router = emulator();
        put_settings(
            &router,
            "a configured catalog",
            &json!({
                "EncryptionAtRest": {
                    "CatalogEncryptionMode": "SSE-KMS",
                    "SseAwsKmsKeyId": KEY_ARN
                }
            }),
            None,
        )
        .await;

        let (status, body) = call(
            &router,
            "PutDataCatalogEncryptionSettings",
            &json!({
                "DataCatalogEncryptionSettings": {
                    "EncryptionAtRest": { "CatalogEncryptionMode": unpublished }
                }
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {unpublished:?}: a mode outside the published list must be refused with \
             400, got {status} with {body}"
        );

        let reported = get_settings(&router, &json!({})).await;
        assert_eq!(
            reported["EncryptionAtRest"]["CatalogEncryptionMode"],
            json!("SSE-KMS"),
            "case {unpublished:?}: a refused write must leave the stored settings alone, got \
             {reported}"
        );
    }
}

/// A write replaces the whole record: a setting the second write does not name reads
/// back at its default, not at what the first write left there.
#[tokio::test]
async fn a_second_write_replaces_the_record_rather_than_merging_into_it() {
    // (case, the first settings written, the second, what the read must report)
    let cases: Vec<(&str, Value, Value, Value)> = vec![
        (
            "both settings, then encryption at rest alone",
            json!({
                "EncryptionAtRest": { "CatalogEncryptionMode": "SSE-KMS", "SseAwsKmsKeyId": KEY_ARN },
                "ConnectionPasswordEncryption": {
                    "ReturnConnectionPasswordEncrypted": true,
                    "AwsKmsKeyId": KEY_ARN
                }
            }),
            json!({ "EncryptionAtRest": { "CatalogEncryptionMode": "DISABLED" } }),
            as_reported(json!({
                "EncryptionAtRest": { "CatalogEncryptionMode": "DISABLED" },
                "ConnectionPasswordEncryption": { "ReturnConnectionPasswordEncrypted": false }
            })),
        ),
        (
            "encryption at rest, then password encryption alone",
            json!({
                "EncryptionAtRest": {
                    "CatalogEncryptionMode": "SSE-KMS-WITH-SERVICE-ROLE",
                    "SseAwsKmsKeyId": KEY_ARN,
                    "CatalogEncryptionServiceRole": SERVICE_ROLE_ARN
                }
            }),
            json!({ "ConnectionPasswordEncryption": { "ReturnConnectionPasswordEncrypted": true } }),
            as_reported(json!({
                "EncryptionAtRest": { "CatalogEncryptionMode": "DISABLED" },
                "ConnectionPasswordEncryption": { "ReturnConnectionPasswordEncrypted": true }
            })),
        ),
        (
            "a configured catalog, then an empty structure",
            json!({
                "EncryptionAtRest": { "CatalogEncryptionMode": "SSE-KMS", "SseAwsKmsKeyId": KEY_ARN },
                "ConnectionPasswordEncryption": { "ReturnConnectionPasswordEncrypted": true }
            }),
            json!({}),
            default_settings(),
        ),
        (
            "the same setting rewritten under a new key",
            json!({
                "EncryptionAtRest": { "CatalogEncryptionMode": "SSE-KMS", "SseAwsKmsKeyId": "alias/first" }
            }),
            json!({
                "EncryptionAtRest": { "CatalogEncryptionMode": "SSE-KMS", "SseAwsKmsKeyId": "alias/second" }
            }),
            as_reported(json!({
                "EncryptionAtRest": { "CatalogEncryptionMode": "SSE-KMS", "SseAwsKmsKeyId": "alias/second" },
                "ConnectionPasswordEncryption": { "ReturnConnectionPasswordEncrypted": false }
            })),
        ),
    ];

    for (case, first, second, expected) in &cases {
        let router = emulator();
        put_settings(&router, case, first, None).await;
        put_settings(&router, case, second, None).await;

        let reported = get_settings(&router, &json!({})).await;
        assert_eq!(
            reported, *expected,
            "case {case}: expected {expected}, got {reported}"
        );
    }
}

/// The `CatalogId` a request names and the one it lets default address the SAME catalog,
/// so a write under one spelling is reported by a read under the other.
#[tokio::test]
async fn the_named_catalog_id_and_the_default_address_one_catalog() {
    // (case, the CatalogId the write names, the CatalogId the read names)
    let cases: Vec<(&str, Option<&str>, Value)> = vec![
        ("written named, read defaulted", Some(ACCOUNT_ID), json!({})),
        (
            "written defaulted, read named",
            None,
            json!({ "CatalogId": ACCOUNT_ID }),
        ),
        (
            "written named, read named",
            Some(ACCOUNT_ID),
            json!({ "CatalogId": ACCOUNT_ID }),
        ),
    ];

    for (case, written_catalog, read_request) in &cases {
        let router = emulator();
        let settings = json!({
            "EncryptionAtRest": { "CatalogEncryptionMode": "SSE-KMS", "SseAwsKmsKeyId": KEY_ARN },
            "ConnectionPasswordEncryption": { "ReturnConnectionPasswordEncrypted": true }
        });
        put_settings(&router, case, &settings, *written_catalog).await;

        let reported = get_settings(&router, read_request).await;
        assert_eq!(
            reported["EncryptionAtRest"],
            settings["EncryptionAtRest"],
            "case {case}: expected the written encryption-at-rest setting, got {reported}"
        );
        assert_eq!(
            reported["ConnectionPasswordEncryption"]["ReturnConnectionPasswordEncrypted"],
            json!(true),
            "case {case}: expected the written password setting, got {reported}"
        );
    }
}

/// A `CatalogId` this emulator holds no Data Catalog under belongs to another account,
/// so both operations refuse it with HTTP 400 `AccessDeniedException` naming the catalog
/// ARN the request addressed — and the write leaves no catalog behind, so the default
/// catalog's own settings are untouched.
#[tokio::test]
async fn a_catalog_id_this_account_does_not_hold_is_refused_by_both_operations() {
    // Ids that are not this emulator's account: another twelve-digit account, a shorter
    // one, and a name-shaped id.
    let foreign_ids = ["210987654321", "1234", "other-catalog"];

    for foreign in foreign_ids {
        let router = emulator();
        put_settings(
            &router,
            foreign,
            &json!({ "EncryptionAtRest": { "CatalogEncryptionMode": "SSE-KMS", "SseAwsKmsKeyId": KEY_ARN } }),
            None,
        )
        .await;

        let requests: Vec<(&str, Value)> = vec![
            (
                "GetDataCatalogEncryptionSettings",
                json!({ "CatalogId": foreign }),
            ),
            (
                "PutDataCatalogEncryptionSettings",
                json!({
                    "CatalogId": foreign,
                    "DataCatalogEncryptionSettings": {
                        "EncryptionAtRest": { "CatalogEncryptionMode": "DISABLED" }
                    }
                }),
            ),
        ];

        for (operation, request) in &requests {
            let (status, body) = call(&router, operation, request).await;
            assert_eq!(
                status, 400,
                "case {foreign:?} / {operation}: expected 400, got {status} with {body}"
            );
            assert!(
                error_code(&body).contains("AccessDeniedException"),
                "case {foreign:?} / {operation}: expected AccessDeniedException, got {body}"
            );
            let message = error_message(&body);
            assert!(
                message.contains(foreign),
                "case {foreign:?} / {operation}: the refusal is built from the request, so it \
                 must name the CatalogId it refused, got {message:?}"
            );
        }

        // The refused calls stored nothing: this account's catalog still reports what
        // its own write set.
        let reported = get_settings(&router, &json!({})).await;
        assert_eq!(
            reported["EncryptionAtRest"]["CatalogEncryptionMode"],
            json!("SSE-KMS"),
            "case {foreign:?}: a refused call must leave this account's catalog alone, got \
             {reported}"
        );
    }
}

/// The settings record is the catalog's own: configuring it leaves the catalog's
/// databases and tables readable, and creating resources afterwards does not disturb the
/// record the write stored.
#[tokio::test]
async fn the_settings_record_lives_beside_the_catalog_resources() {
    let router = emulator();
    let settings = json!({
        "EncryptionAtRest": { "CatalogEncryptionMode": "SSE-KMS", "SseAwsKmsKeyId": KEY_ARN },
        "ConnectionPasswordEncryption": { "ReturnConnectionPasswordEncrypted": true }
    });
    put_settings(&router, "enforced before any table", &settings, None).await;

    ok(
        &router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": "encryption_settings_db" } }),
    )
    .await;
    ok(
        &router,
        "CreateTable",
        &json!({
            "DatabaseName": "encryption_settings_db",
            "TableInput": { "Name": "encryption_settings_tbl" }
        }),
    )
    .await;

    let table = ok(
        &router,
        "GetTable",
        &json!({ "DatabaseName": "encryption_settings_db", "Name": "encryption_settings_tbl" }),
    )
    .await;
    assert_eq!(
        table["Table"]["Name"],
        json!("encryption_settings_tbl"),
        "a table created under enforced catalog encryption must read back, got {table}"
    );

    let reported = get_settings(&router, &json!({})).await;
    let expected = as_reported(settings);
    assert_eq!(
        reported, expected,
        "creating catalog resources must not change the settings record; expected \
         {expected}, got {reported}"
    );
}
