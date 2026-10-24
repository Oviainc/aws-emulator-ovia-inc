// SPDX-License-Identifier: CC0-1.0

//! `PutDeploymentParameter` is reachable by an ordinary HTTP client, with no
//! in-process help at all.
//!
//! The rule under test: a write whose target — the agreement of the account, the
//! product its URI names, inside a catalog this service serves — is created out of
//! band of this API is served as the create-or-update the operation documents, and
//! the write RECORDS that target as held, so nothing a client cannot call is a
//! precondition of the call. Not one case below seeds state through the crate's
//! Rust seam: every byte of state these assertions read was written by a request
//! over the wire.
//!
//! What follows from it, and is asserted here: the identity a write is keyed on is
//! still (catalog, productId, agreementId, name) — distinct targets are distinct
//! resources and a repeat for one identity updates in place — one agreement covers
//! every product written under it, and the create -> read -> update -> untag
//! lifecycle runs end to end over HTTP. A request the service REFUSES records no
//! target, so the recording is the write's, not the request's. The ledger those
//! targets live in is BOUNDED and evicts the oldest target first, since every
//! segment of a target is request-supplied.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 routing and serialization, the modelled-constraint walk and the
//! handlers — through the harness the `deployment_parameters` suite owns.

use serde_json::{json, Value};
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::state::MAX_PROVISIONED_TARGETS;

use crate::deployment_parameters::{
    emulator, error_code, expected_arn, holds_target, is_parameter_id, list_tags, put,
    recorded_targets, stored_parameters, tag, tag_map, tags_of, untag, CATALOG, SECRET_MAX_LEN,
};

/// One target a client writes against: a product and an agreement, in the forms
/// their own modelled pattern admits (`ResourceId` is `^[A-Za-z0-9_/-]+$`,
/// `length 1..64`), plus the parameter name and secret the write carries.
///
/// The catalog is NOT a row field: the model documents `catalog` as a fixed value,
/// so every row names [`CATALOG`] and the rule about any other catalog belongs to
/// the `catalog_allowed_values` suite.
struct Row {
    case: &'static str,
    product_id: String,
    agreement_id: String,
    parameter: String,
    secret: String,
}

impl Row {
    fn new(
        case: &'static str,
        product_id: &str,
        agreement_id: &str,
        parameter: &str,
        secret: String,
    ) -> Self {
        Self {
            case,
            product_id: product_id.to_string(),
            agreement_id: agreement_id.to_string(),
            parameter: parameter.to_string(),
            secret,
        }
    }

    /// This row's request body, optionally carrying `tags`.
    fn body(&self, tags: Option<&[(&str, &str)]>) -> Value {
        let mut body = json!({
            "agreementId": self.agreement_id,
            "deploymentParameter": { "name": self.parameter, "secretString": self.secret },
        });
        if let Some(pairs) = tags {
            body.as_object_mut().expect("an object body").insert(
                "tags".to_string(),
                Value::Object(
                    pairs
                        .iter()
                        .map(|(key, value)| ((*key).to_string(), json!(value)))
                        .collect(),
                ),
            );
        }
        body
    }
}

/// The target table. The rows differ along everything the rule generalises over:
/// the characters and the length of each identifier (down to the one-character
/// modelled minimum and up to the 64-character maximum), the shape of the product
/// id (plain, nested, numeric) and the size of the payload the write carries (one
/// character up to `SecretString`'s modelled maximum).
fn target_table() -> Vec<Row> {
    vec![
        Row::new(
            "plain product and agreement ids",
            "prod-abc123",
            "agmt-abc123",
            "DbPassword",
            "s3cr3t-value-1".to_string(),
        ),
        Row::new(
            "a nested product id, an agreement id carrying _ and -",
            "prod/nested/id-7",
            "agmt_1/2-3",
            "api.key+v1=1@host/path_1-2",
            "x".to_string(),
        ),
        Row::new(
            "single-character identifiers, the modelled minimum",
            "b",
            "c",
            "d",
            "e".to_string(),
        ),
        Row::new(
            "numeric identifiers",
            "0",
            "9",
            "p",
            "rotate-me".to_string(),
        ),
        Row::new(
            // 64 characters is `ResourceId`'s modelled maximum, 15000 characters
            // `SecretString`'s.
            "identifiers at the modelled 64-character maximum, secret at its maximum",
            "pppppppppppppppppppppppppppppppppppppppppppppppppppppppppppppppp",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "AtTheMaximum",
            "y".repeat(SECRET_MAX_LEN),
        ),
    ]
}

// How much the store holds, and whether it holds one target, are read through the
// `deployment_parameters` harness's own accessors (`holds_target`,
// `stored_parameters`, `recorded_targets`), which every suite of this test target
// shares.

#[tokio::test]
async fn the_target_ledger_is_bounded_and_evicts_the_oldest_target() {
    let (router, engine) = emulator();

    /// The i-th target's product and agreement ids, inside `ResourceId`'s
    /// modelled `^[A-Za-z0-9_/-]+$` and `length 1..64`.
    fn target(i: usize) -> (String, String) {
        (format!("prod-ledger-{i:06}"), format!("agmt-ledger-{i:06}"))
    }

    // Every write names an `expirationDate` that has already come, so the
    // parameter it stores is purged on the very next request and hands its slot
    // under the deployment-parameter quota straight back: the parameter store
    // never grows, and only the ledger's OWN ceiling bounds the targets these
    // request-supplied identifiers record.
    let body = |agreement_id: &str| {
        json!({
            "agreementId": agreement_id,
            "deploymentParameter": { "name": "Ledger", "secretString": "s" },
            "expirationDate": 1_i64,
        })
    };

    let (first_product, first_agreement) = target(0);
    let (status, answer) = put(&router, CATALOG, &first_product, &body(&first_agreement)).await;
    assert_eq!(
        status, 200,
        "the first write must be served, got {status} {answer:?}"
    );
    assert!(
        holds_target(&engine, CATALOG, &first_product, &first_agreement),
        "the first write must record its target"
    );

    // Fill the ledger past its cap, one fresh target per write.
    for i in 1..=MAX_PROVISIONED_TARGETS {
        let (product_id, agreement_id) = target(i);
        let (status, answer) = put(&router, CATALOG, &product_id, &body(&agreement_id)).await;
        assert_eq!(
            status, 200,
            "target {i} must be served, got {status} {answer:?}"
        );
        let held = recorded_targets(&engine);
        assert!(
            held <= MAX_PROVISIONED_TARGETS,
            "target {i}: the ledger must stay inside its {MAX_PROVISIONED_TARGETS}-target \
             ceiling, got {held} recorded targets"
        );
    }

    // At the cap, with the OLDEST target evicted and the newest one held: the
    // store cannot grow with the number of distinct identifiers a caller sends.
    let held = recorded_targets(&engine);
    assert_eq!(
        held, MAX_PROVISIONED_TARGETS,
        "expected the ledger exactly at its ceiling, got {held} recorded targets"
    );
    assert!(
        !holds_target(&engine, CATALOG, &first_product, &first_agreement),
        "the oldest target must be evicted once the ledger is at its ceiling"
    );
    let (last_product, last_agreement) = target(MAX_PROVISIONED_TARGETS);
    assert!(
        holds_target(&engine, CATALOG, &last_product, &last_agreement),
        "the newest target must be the one the ledger kept"
    );

    // And the expiries really were materialised, so the parameter quota was never
    // what bounded the run.
    let stored = stored_parameters(&engine);
    assert!(
        stored <= 1,
        "every parameter past its expiration date must be purged, got {stored} stored"
    );

    // An evicted target is recorded again by the next write that names it, and
    // that write is served exactly as the first one was.
    let (status, again) = put(&router, CATALOG, &first_product, &body(&first_agreement)).await;
    assert_eq!(
        status, 200,
        "a write naming an evicted target must still be served, got {status} {again:?}"
    );
    assert!(
        holds_target(&engine, CATALOG, &first_product, &first_agreement),
        "a write naming an evicted target must record it again"
    );
}

#[tokio::test]
async fn a_write_against_an_out_of_band_target_is_served_and_records_that_target() {
    let (router, engine) = emulator();
    let mut minted: Vec<(String, String)> = Vec::new();

    for row in target_table() {
        let case = row.case;
        // Nothing has provisioned this target: the store is empty of it until the
        // write itself records it.
        assert!(
            !holds_target(&engine, CATALOG, &row.product_id, &row.agreement_id),
            "case {case:?}: the target must not be held before any write, \
             got it already recorded"
        );

        let (status, created) = put(
            &router,
            CATALOG,
            &row.product_id,
            &row.body(Some(&[("env", "dev")])),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a well-formed write must be served, got {status} {created:?}"
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
        let arn = created
            .get("resourceArn")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            arn,
            expected_arn(&id),
            "case {case:?}: expected the ARN of the minted id, got {created:?}"
        );
        assert_eq!(
            created.get("agreementId").and_then(Value::as_str),
            Some(row.agreement_id.as_str()),
            "case {case:?}: expected the stored agreementId, got {created:?}"
        );

        // The write recorded its target: the account now holds that agreement,
        // covering that product.
        assert!(
            holds_target(&engine, CATALOG, &row.product_id, &row.agreement_id),
            "case {case:?}: the write must record the target it named, got it unheld"
        );

        // The round trip: the record is readable back through an implemented read,
        // over the same wire path.
        let (status, listed) = list_tags(&router, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the written record must be readable, got {status} {listed:?}"
        );
        assert_eq!(
            tags_of(&listed),
            tag_map(&[("env", "dev")]),
            "case {case:?}: expected the create's tags from the record, got {listed:?}"
        );

        // A repeat for the SAME identity updates in place: one target, one record.
        let (status, again) = put(
            &router,
            CATALOG,
            &row.product_id,
            &row.body(Some(&[("ignored", "on-update")])),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the repeat write must be served, got {status} {again:?}"
        );
        assert_eq!(
            again.get("deploymentParameterId").and_then(Value::as_str),
            Some(id.as_str()),
            "case {case:?}: a repeat for one identity must report the same id, got {again:?}"
        );

        minted.push((case.to_string(), id));
    }

    // One record per distinct target, and every identifier distinct.
    assert_eq!(
        stored_parameters(&engine),
        target_table().len(),
        "expected one record per target in the table, got {} for {} targets",
        stored_parameters(&engine),
        target_table().len()
    );
    for (left, (case, id)) in minted.iter().enumerate() {
        for (other_case, other_id) in minted.iter().skip(left + 1) {
            assert_ne!(
                id, other_id,
                "cases {case:?} and {other_case:?}: distinct targets must mint distinct ids, \
                 both got {id:?}"
            );
        }
    }
}

#[tokio::test]
async fn one_agreement_covers_every_product_written_under_it() {
    let (router, engine) = emulator();

    // (case, productId, agreementId) — one agreement over two products, and one
    // product under two agreements: each pair is its own target.
    let pairs = [
        ("first product of the shared agreement", "prod-one", "agmt-shared"),
        ("second product of the shared agreement", "prod-two", "agmt-shared"),
        ("the shared product under another agreement", "prod-one", "agmt-other"),
    ];

    let mut ids: Vec<(&str, String)> = Vec::new();
    for (case, product_id, agreement_id) in pairs {
        let row = Row::new(case, product_id, agreement_id, "DbPassword", "s".to_string());
        let (status, created) = put(&router, CATALOG, product_id, &row.body(None)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a well-formed write must be served, got {status} {created:?}"
        );
        let id = created
            .get("deploymentParameterId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            is_parameter_id(&id),
            "case {case:?}: expected a minted parameter id, got {created:?}"
        );
        ids.push((case, id));
    }

    for (case, product_id, agreement_id) in pairs {
        assert!(
            holds_target(&engine, CATALOG, product_id, agreement_id),
            "case {case:?}: every target written against must be held afterwards"
        );
    }
    // The agreement widened rather than being replaced: it covers both products.
    assert!(
        holds_target(&engine, CATALOG, "prod-two", "agmt-shared")
            && holds_target(&engine, CATALOG, "prod-one", "agmt-shared"),
        "a second product under one agreement must widen it, not replace its coverage"
    );
    // Three targets, three records, three distinct ids.
    assert_eq!(
        stored_parameters(&engine),
        3,
        "expected one record per target, got {}",
        stored_parameters(&engine)
    );
    for (left, (case, id)) in ids.iter().enumerate() {
        for (other_case, other_id) in ids.iter().skip(left + 1) {
            assert_ne!(
                id, other_id,
                "cases {case:?} and {other_case:?}: distinct targets must mint distinct ids, \
                 both got {id:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_create_read_update_untag_lifecycle_runs_over_http_alone() {
    let (router, _engine) = emulator();
    let row = Row::new(
        "the lifecycle an HTTP client drives",
        "prod-lifecycle",
        "agmt-lifecycle",
        "DbPassword",
        "s3cr3t".to_string(),
    );
    let case = row.case;

    // create
    let (status, created) = put(
        &router,
        CATALOG,
        &row.product_id,
        &row.body(Some(&[("env", "dev"), ("owner", "team")])),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the create must be served, got {status} {created:?}"
    );
    let arn = created
        .get("resourceArn")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    // read
    let (status, listed) = list_tags(&router, &arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the read must be served, got {status} {listed:?}"
    );
    assert_eq!(
        tags_of(&listed),
        tag_map(&[("env", "dev"), ("owner", "team")]),
        "case {case:?}: expected the create's tags, got {listed:?}"
    );

    // update: one more tag, merged onto what the create left
    let (status, tagged) = tag(&router, &arn, &tag_map(&[("tier", "gold")])).await;
    assert_eq!(
        status, 204,
        "case {case:?}: TagResource must be served, got {status} {tagged:?}"
    );
    let (_, listed) = list_tags(&router, &arn).await;
    assert_eq!(
        tags_of(&listed),
        tag_map(&[("env", "dev"), ("owner", "team"), ("tier", "gold")]),
        "case {case:?}: expected the merged tag set, got {listed:?}"
    );

    // untag
    let (status, untagged) = untag(&router, &arn, &["owner".to_string()]).await;
    assert_eq!(
        status, 204,
        "case {case:?}: UntagResource must be served, got {status} {untagged:?}"
    );
    let (_, listed) = list_tags(&router, &arn).await;
    assert_eq!(
        tags_of(&listed),
        tag_map(&[("env", "dev"), ("tier", "gold")]),
        "case {case:?}: expected the removal to be persisted, got {listed:?}"
    );
}

#[tokio::test]
async fn a_refused_write_records_no_target_and_stores_nothing() {
    let (router, engine) = emulator();

    // Each row is refused by the service, so the target it names is never
    // recorded: the recording belongs to the write, not to the request.
    // (case, catalog, productId, body)
    let rows: Vec<(&str, &str, &str, Value)> = vec![
        (
            "a catalog that names no catalog this service serves",
            "PrivateMarketplace",
            "prod-refused-catalog",
            json!({
                "agreementId": "agmt-refused-catalog",
                "deploymentParameter": { "name": "N", "secretString": "s" },
            }),
        ),
        (
            "a tag key in the namespace AWS reserves",
            CATALOG,
            "prod-refused-tag",
            json!({
                "agreementId": "agmt-refused-tag",
                "deploymentParameter": { "name": "N", "secretString": "s" },
                "tags": { "aws:created-by": "someone" },
            }),
        ),
        (
            "a required member absent",
            CATALOG,
            "prod-refused-required",
            json!({ "agreementId": "agmt-refused-required" }),
        ),
    ];

    for (case, catalog, product_id, body) in rows {
        let (status, answer) = put(&router, catalog, product_id, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a request outside the modelled input must be 400, \
             got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        let agreement_id = body
            .get("agreementId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            !holds_target(&engine, catalog, product_id, agreement_id),
            "case {case:?}: a refused write must record no target"
        );
    }

    assert_eq!(
        stored_parameters(&engine),
        0,
        "a refused write must store nothing, got {} stored deployment parameters",
        stored_parameters(&engine)
    );
}

#[tokio::test]
async fn a_write_whose_idempotency_token_is_fresh_is_performed_then_replayed() {
    let (router, engine) = emulator();
    // 36 characters of `^[a-zA-Z0-9/_+=.:@-]+$`, inside `ClientToken`'s len 32..64.
    let token = "0123456789abcdef0123456789abcdef0123";
    let body = json!({
        "agreementId": "agmt-token",
        "deploymentParameter": { "name": "DbPassword", "secretString": "s" },
        "clientToken": token,
    });

    let (status, created) = put(&router, CATALOG, "prod-token", &body).await;
    assert_eq!(
        status, 200,
        "the first write must be served, got {status} {created:?}"
    );
    assert!(
        holds_target(&engine, CATALOG, "prod-token", "agmt-token"),
        "the write must record the target it named"
    );

    let (status, replayed) = put(&router, CATALOG, "prod-token", &body).await;
    assert_eq!(
        status, 200,
        "the replay must be served, got {status} {replayed:?}"
    );
    assert_eq!(
        replayed, created,
        "a clientToken replay must answer the identical output, got {replayed:?} against {created:?}"
    );
    assert_eq!(
        stored_parameters(&engine),
        1,
        "a replay must mint nothing, got {} stored deployment parameters",
        stored_parameters(&engine)
    );
}
