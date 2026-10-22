// SPDX-License-Identifier: CC0-1.0

//! `PutDeploymentParameter` is TARGETED: it writes against an agreement of the
//! account, for a product that agreement covers, inside one catalog.
//!
//! The rule under test: a request whose `(catalog, productId, agreementId)` target
//! the account does not hold is refused with the modelled
//! `ResourceNotFoundException` (HTTP 404) and stores nothing, however well-formed
//! its identifiers are and whatever else the request carries; the same request
//! against a target the account DOES hold is the ordinary create, readable back
//! through `ListTagsForResource`.
//!
//! The modelled input constraints are refused BEFORE the target is looked at, so a
//! request that breaks one is a `ValidationException` (HTTP 400) whether or not the
//! target exists.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 routing and serialization, the modelled-constraint walk and the
//! handlers — through the harness the `deployment_parameters` suite owns.

use serde_json::{json, Value};
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::state::{
    service_state, MAX_TAGS_PER_RESOURCE,
};

use crate::deployment_parameters::{
    emulator, error_code, is_parameter_id, list_tags, message, provision, put, tags_of, CATALOG,
    SECRET_MAX_LEN,
};

/// One target row: a product and an agreement, in the forms their own modelled
/// pattern admits (`ResourceId` is `^[A-Za-z0-9_/-]+$` len 1..64).
///
/// The catalog is NOT a row field: the model documents `catalog` as a fixed value,
/// so every row names [`CATALOG`] and the rule about any other catalog is the
/// `catalog_allowed_values` suite's.
struct Target {
    case: &'static str,
    product_id: &'static str,
    agreement_id: &'static str,
}

/// The target table. The rows differ along everything the rule generalises over:
/// the characters and the length of each identifier (down to the one-character
/// minimum and up to the 64-character maximum), and the shape of the product id
/// (plain, nested, numeric).
fn target_table() -> Vec<Target> {
    vec![
        Target {
            case: "plain product and agreement ids",
            product_id: "prod-abc123",
            agreement_id: "agmt-abc123",
        },
        Target {
            case: "a nested product id, an agreement id carrying _ and -",
            product_id: "prod/nested/id-7",
            agreement_id: "agmt_1/2-3",
        },
        Target {
            case: "single-character identifiers, the modelled minimum",
            product_id: "b",
            agreement_id: "c",
        },
        Target {
            case: "numeric identifiers",
            product_id: "0",
            agreement_id: "9",
        },
        Target {
            // 64 characters is `ResourceId`'s modelled maximum.
            case: "identifiers at the modelled 64-character maximum",
            product_id: "pppppppppppppppppppppppppppppppppppppppppppppppppppppppppppppppp",
            agreement_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        },
    ]
}

/// The request bodies one target is written with: the rule holds whatever else the
/// request carries, so each row is tried in every one of these shapes.
fn request_shapes(agreement_id: &str) -> Vec<(&'static str, Value)> {
    let many_tags: serde_json::Map<String, Value> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|i| (format!("key-{i:02}"), json!("v")))
        .collect();
    vec![
        (
            "the minimal body",
            json!({
                "agreementId": agreement_id,
                "deploymentParameter": { "name": "DbPassword", "secretString": "s3cr3t-value-1" },
            }),
        ),
        (
            "a body carrying tags exactly at the modelled cap",
            json!({
                "agreementId": agreement_id,
                "deploymentParameter": { "name": "DbPassword", "secretString": "s3cr3t-value-1" },
                "tags": many_tags,
            }),
        ),
        (
            "a body carrying an idempotency token and an expiry",
            json!({
                "agreementId": agreement_id,
                "deploymentParameter": { "name": "Other.Name+1", "secretString": "s" },
                "clientToken": "0123456789abcdef0123456789abcdef0123",
                "expirationDate": 1_893_456_000_i64,
            }),
        ),
        (
            "a body whose secret is at the modelled maximum",
            json!({
                "agreementId": agreement_id,
                "deploymentParameter": {
                    "name": "AtTheMaximum",
                    "secretString": "y".repeat(SECRET_MAX_LEN),
                },
            }),
        ),
    ]
}

#[tokio::test]
async fn a_put_aimed_at_a_target_the_account_does_not_hold_is_refused_and_stores_nothing() {
    let (router, engine) = emulator();

    // --- nothing is provisioned: every well-formed write is a 404 -------------
    for row in target_table() {
        let case = row.case;
        for (shape, body) in request_shapes(row.agreement_id) {
            let (status, answer) = put(&router, CATALOG, row.product_id, &body).await;
            assert_eq!(
                status, 404,
                "case {case:?} with {shape:?}: a target the account does not hold must be 404, \
                 got {status} {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "ResourceNotFoundException",
                "case {case:?} with {shape:?}: expected ResourceNotFoundException, got {answer:?}"
            );
        }
    }

    // Not one of those calls wrote a record: the refusal happens before anything
    // is minted.
    let held = service_state(&*engine)
        .expect("the shared engine holds this service's store")
        .lock()
        .parameters
        .len();
    assert_eq!(
        held, 0,
        "a refused write must store nothing, got {held} stored deployment parameters"
    );

    // --- the same requests against a target the account holds ----------------
    for row in target_table() {
        let case = row.case;
        provision(&engine, CATALOG, row.agreement_id, &[row.product_id]);
        let (status, created) = put(
            &router,
            CATALOG,
            row.product_id,
            &json!({
                "agreementId": row.agreement_id,
                "deploymentParameter": { "name": "DbPassword", "secretString": "s3cr3t-value-1" },
                "tags": { "env": "dev" },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a provisioned target must be written, got {status} {created:?}"
        );
        let id = created
            .get("deploymentParameterId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            is_parameter_id(&id),
            "case {case:?}: expected a minted ^dp-[a-zA-Z0-9]+$ id, got {created:?}"
        );
        assert_eq!(
            created.get("agreementId").and_then(Value::as_str),
            Some(row.agreement_id),
            "case {case:?}: expected the stored agreementId, got {created:?}"
        );

        // The round trip: the record is readable back through an implemented read.
        let arn = created
            .get("resourceArn")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let (status, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the written record must be readable, got {status} {listed:?}"
        );
        assert_eq!(
            tags_of(&listed),
            crate::deployment_parameters::tag_map(&[("env", "dev")]),
            "case {case:?}: expected the create's tags from the record, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_target_the_account_holds_only_partly_is_refused_on_the_member_it_got_wrong() {
    let (router, engine) = emulator();
    // Two agreements of one catalog, each covering its own product.
    provision(&engine, "AWSMarketplace", "agmt-live", &["prod-live"]);
    provision(&engine, "AWSMarketplace", "agmt-second", &["prod-second"]);
    // The same identifiers in another catalog name nothing: a catalog is part of
    // the target. (Such an agreement is seeded, not reachable through this API: a
    // request naming another catalog is refused before any lookup — see the
    // `catalog_allowed_values` suite — so what this proves is that the stored
    // target is scoped by its catalog.)
    provision(&engine, "PrivateMarketplace", "agmt-private", &["prod-private"]);

    // (case, catalog, productId, agreementId, the member the refusal names)
    let near_misses: Vec<(&str, &str, &str, &str, &str)> = vec![
        (
            "a product no agreement of this catalog covers",
            "AWSMarketplace",
            "prod-never-published",
            "agmt-live",
            "ProductId",
        ),
        (
            "a held product under an agreement that does not exist",
            "AWSMarketplace",
            "prod-live",
            "agmt-never-accepted",
            "AgreementId",
        ),
        (
            "a held product under an agreement that covers another product",
            "AWSMarketplace",
            "prod-live",
            "agmt-second",
            "AgreementId",
        ),
        (
            "a pair held in a DIFFERENT catalog",
            "AWSMarketplace",
            "prod-private",
            "agmt-private",
            "ProductId",
        ),
        (
            "a product id differing only in case",
            "AWSMarketplace",
            "PROD-LIVE",
            "agmt-live",
            "ProductId",
        ),
        (
            "an agreement id differing only in case",
            "AWSMarketplace",
            "prod-live",
            "AGMT-LIVE",
            "AgreementId",
        ),
    ];

    for (case, catalog, product_id, agreement_id, member) in near_misses {
        let body = json!({
            "agreementId": agreement_id,
            "deploymentParameter": { "name": "DbPassword", "secretString": "s" },
        });
        let (status, answer) = put(&router, catalog, product_id, &body).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected a 404, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        // The status and the code are what the model fixes; this service's own
        // sentence names which half of the target the request got wrong.
        let reported = message(&answer);
        assert!(
            reported.starts_with(member),
            "case {case:?}: expected the refusal to name {member}, got {reported:?}"
        );
    }

    // The target that IS held is still writable, so none of the above refusals is
    // about an empty store.
    let (status, created) = put(
        &router,
        "AWSMarketplace",
        "prod-live",
        &json!({
            "agreementId": "agmt-live",
            "deploymentParameter": { "name": "DbPassword", "secretString": "s" },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the held target must still be writable, got {status} {created:?}"
    );
}

#[tokio::test]
async fn a_modelled_constraint_is_refused_before_the_target_is_looked_at() {
    let (router, engine) = emulator();
    provision(&engine, "AWSMarketplace", "agmt-held", &["prod-held"]);

    let too_many: serde_json::Map<String, Value> = (0..=MAX_TAGS_PER_RESOURCE)
        .map(|i| (format!("key-{i:02}"), json!("v")))
        .collect();

    // Each row breaks one modelled constraint of the input. The pairs cover both
    // sides of the target check: a target the account holds, and one it does not.
    // An oversized `@sensitive` payload is NOT here: it is refused one layer
    // earlier, with the front door's 403 (`front_door_forbidden`).
    let rows: Vec<(&str, &str, &str, Value)> = vec![
        (
            "tags one past the modelled cap, target held",
            "AWSMarketplace",
            "prod-held",
            json!({
                "agreementId": "agmt-held",
                "deploymentParameter": { "name": "N", "secretString": "s" },
                "tags": too_many.clone(),
            }),
        ),
        (
            "tags one past the modelled cap, target not held",
            "AWSMarketplace",
            "prod-unknown",
            json!({
                "agreementId": "agmt-unknown",
                "deploymentParameter": { "name": "N", "secretString": "s" },
                "tags": too_many,
            }),
        ),
        (
            "catalog outside its pattern, target not held",
            "Not-A-Real-Catalog!",
            "prod-unknown",
            json!({
                "agreementId": "agmt-unknown",
                "deploymentParameter": { "name": "N", "secretString": "s" },
            }),
        ),
        (
            "agreementId outside its pattern, target not held",
            "AWSMarketplace",
            "prod-unknown",
            json!({
                "agreementId": "bad id!",
                "deploymentParameter": { "name": "N", "secretString": "s" },
            }),
        ),
    ];

    for (case, catalog, product_id, body) in rows {
        let (status, answer) = put(&router, catalog, product_id, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a modelled-constraint violation must be 400, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
    }
}

#[tokio::test]
async fn a_refused_write_consumes_no_idempotency_token() {
    let (router, engine) = emulator();
    // 36 characters of `^[a-zA-Z0-9/_+=.:@-]+$`, inside `ClientToken`'s len 32..64.
    let token = "0123456789abcdef0123456789abcdef0123";
    let body = json!({
        "agreementId": "agmt-later",
        "deploymentParameter": { "name": "DbPassword", "secretString": "s" },
        "clientToken": token,
    });

    let (status, answer) = put(&router, "AWSMarketplace", "prod-later", &body).await;
    assert_eq!(
        status, 404,
        "a write at a target the account does not hold must be 404, got {status} {answer:?}"
    );

    // The token was never bound, so once the target is provisioned the SAME token
    // performs the write rather than replaying or conflicting with it.
    provision(&engine, "AWSMarketplace", "agmt-later", &["prod-later"]);
    let (status, created) = put(&router, "AWSMarketplace", "prod-later", &body).await;
    assert_eq!(
        status, 200,
        "the retried write must succeed, got {status} {created:?}"
    );
    let id = created
        .get("deploymentParameterId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        is_parameter_id(&id),
        "the retried write must mint a parameter, got {created:?}"
    );

    // And it is now bound: the same token for the same identity replays it.
    let (status, replayed) = put(&router, "AWSMarketplace", "prod-later", &body).await;
    assert_eq!(
        status, 200,
        "the replay must succeed, got {status} {replayed:?}"
    );
    assert_eq!(
        replayed, created,
        "a clientToken replay must answer the identical output, got {replayed:?} against {created:?}"
    );
}
