// SPDX-License-Identifier: CC0-1.0

//! `PutDeploymentParameter` is TARGETED: it writes against an agreement of the
//! account, for the product its URI names, inside one catalog.
//!
//! The rule under test: that target is created out of band of this API — AWS
//! models no operation here that creates a product or an agreement — so the
//! operation RESOLVES the whole triple `(catalog, productId, agreementId)`
//! against the agreements the account holds before it mints or updates anything:
//! a triple the store holds is the create-or-update the operation documents, and
//! a triple it does not hold is the modelled `ResourceNotFoundException`
//! (HTTP 404) naming the segment that did not resolve — the agreement before the
//! product, which is the order the operation is targeted by (the
//! `unresolved_target_precedence` suite owns that precedence) — with nothing
//! written and no token consumed.
//!
//! The target is the whole triple, so two requests differing in any one of its
//! segments are two different resolutions: two resources when both resolve, and a
//! 404 when the one they differ in does not.
//!
//! An input outside the modelled constraints is refused BEFORE the target is
//! resolved, so such a request is a `ValidationException` (HTTP 400) whether or
//! not its target exists.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 routing and serialization, the modelled-constraint walk and the
//! handlers — through the harness the `deployment_parameters` suite owns.

use serde_json::{json, Value};
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::state::{
    DEFAULT_AGREEMENTS, MAX_PROVISIONED_TARGETS, MAX_TAGS_PER_RESOURCE,
};

use crate::deployment_parameters::{
    emulator, error_code, holds_target, is_parameter_id, list_tags, message, provision, put,
    recorded_targets, stored_parameters, tags_of, CATALOG, SECRET_MAX_LEN,
};

/// The modelled error a write against a target the account does not hold is
/// refused with (`smithy_query(query="operation",operation="PutDeploymentParameter")`
/// lists `ResourceNotFoundException`, `errorKind client`, `httpError 404`).
const NOT_FOUND_CODE: &str = "ResourceNotFoundException";
/// That error's modelled HTTP status.
const NOT_FOUND_STATUS: u16 = 404;

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
///
/// `token` is the row's own `clientToken`: the member is the request's
/// `@idempotencyToken`, so one token identifies one write and a shared one across
/// rows would be the modelled conflict rather than this rule.
fn request_shapes(agreement_id: &str, token: &str) -> Vec<(&'static str, Value)> {
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
                "deploymentParameter": { "name": "TaggedToTheCap", "secretString": "s3cr3t-value-1" },
                "tags": many_tags,
            }),
        ),
        (
            "a body carrying an idempotency token and an expiry",
            json!({
                "agreementId": agreement_id,
                "deploymentParameter": { "name": "Other.Name+1", "secretString": "s" },
                "clientToken": token,
                // The RFC 3339 form the member's `@timestampFormat("date-time")`
                // declares: 2030-01-01T00:00:00Z, far enough ahead that the row's
                // parameter stays live for the whole suite.
                "expirationDate": "2030-01-01T00:00:00Z",
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

/// A `clientToken` of this row's own: 34 characters of
/// `^[a-zA-Z0-9/_+=.:@-]+$`, inside `ClientToken`'s modelled `length 32..64`.
fn row_token(row: usize) -> String {
    format!("idempotency-token-for-row-{row:03}-0000")
}

/// Assert `answer` is the modelled refusal for an unresolved target, blaming
/// `member` — the segment of the triple the store could not resolve.
fn assert_target_not_found(case: &str, member: &str, status: u16, answer: &Value) {
    assert_eq!(
        status, NOT_FOUND_STATUS,
        "case {case:?}: a target the account does not hold must be \
         HTTP {NOT_FOUND_STATUS}, got {status} {answer:?}"
    );
    assert_eq!(
        error_code(answer),
        NOT_FOUND_CODE,
        "case {case:?}: expected {NOT_FOUND_CODE}, got {answer:?}"
    );
    let reported = message(answer);
    assert!(
        reported.starts_with(&format!("{member}:")),
        "case {case:?}: the refusal must name the unresolved {member}, got {reported:?}"
    );
    assert!(
        answer.get("resourceArn").is_none() && answer.get("deploymentParameterId").is_none(),
        "case {case:?}: a refused write must mint nothing, got {answer:?}"
    );
}

// Whether this service's store holds one target, and how much it holds, are read
// through the `deployment_parameters` harness's own accessors (`holds_target`,
// `stored_parameters`, `recorded_targets`), which every suite of this target
// shares.

#[tokio::test]
async fn a_put_is_404_until_its_target_is_held_and_is_served_once_it_is() {
    // --- nothing holds these targets: every shape is the modelled 404 ----------
    let (router, engine) = emulator();
    for (index, row) in target_table().into_iter().enumerate() {
        let case = row.case;
        assert!(
            !holds_target(&engine, CATALOG, row.product_id, row.agreement_id),
            "case {case:?}: the row must name a target the account does not hold"
        );
        let token = row_token(index);
        for (shape, body) in request_shapes(row.agreement_id, &token) {
            let (status, answer) = put(&router, CATALOG, row.product_id, &body).await;
            assert_target_not_found(
                &format!("{case} with {shape}"),
                "AgreementId",
                status,
                &answer,
            );
        }
    }
    // A refused write stores nothing and records no target: the refusal is the
    // whole answer.
    let stored = stored_parameters(&engine);
    assert_eq!(
        stored, 0,
        "a write against an unheld target must store nothing, got {stored} stored"
    );
    for row in target_table() {
        assert!(
            !holds_target(&engine, CATALOG, row.product_id, row.agreement_id),
            "case {:?}: a refused write must not record the target it named",
            row.case
        );
    }

    // --- the same targets, held out of band: every shape is served ------------
    let (router, engine) = emulator();
    for (index, row) in target_table().into_iter().enumerate() {
        let case = row.case;
        provision(&engine, CATALOG, row.agreement_id, &[row.product_id]);
        let token = row_token(index);
        for (shape, body) in request_shapes(row.agreement_id, &token) {
            let (status, answer) = put(&router, CATALOG, row.product_id, &body).await;
            assert_eq!(
                status, 200,
                "case {case:?} with {shape:?}: a target the account holds must be written, \
                 got {status} {answer:?}"
            );
            let id = answer
                .get("deploymentParameterId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert!(
                is_parameter_id(&id),
                "case {case:?} with {shape:?}: expected a minted ^dp-[a-zA-Z0-9]+$ id, \
                 got {answer:?}"
            );
            assert_eq!(
                answer.get("agreementId").and_then(Value::as_str),
                Some(row.agreement_id),
                "case {case:?} with {shape:?}: expected the stored agreementId, got {answer:?}"
            );

            // The round trip: the record is readable back through an implemented
            // read, over the same wire path.
            let arn = answer
                .get("resourceArn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let (status, listed) = list_tags(&router, &arn).await;
            assert_eq!(
                status, 200,
                "case {case:?} with {shape:?}: the written record must be readable, \
                 got {status} {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_refusal_names_the_agreement_before_the_product() {
    let (router, engine) = emulator();
    // Two agreements over overlapping products, so each row below differs from a
    // held target in exactly one segment and the suite can tell the two blames
    // apart: `prod-shared` is covered by both agreements, `prod-one` by the first
    // alone.
    provision(&engine, CATALOG, "agmt-one", &["prod-one", "prod-shared"]);
    provision(&engine, CATALOG, "agmt-two", &["prod-shared"]);

    // (case, productId, agreementId, the member the refusal must name). The
    // agreement is judged first — it is the target the operation is targeted by —
    // so an agreement the account does not hold is blamed on `AgreementId`,
    // whatever the product is; otherwise the agreement resolves and the product it
    // does not cover is what the refusal names.
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        (
            "neither identifier is held",
            "prod-unknown",
            "agmt-unknown",
            "AgreementId",
        ),
        (
            "a product nothing covers, under a held agreement",
            "prod-unknown",
            "agmt-one",
            "ProductId",
        ),
        (
            "a product differing from a covered one only in case",
            "PROD-ONE",
            "agmt-one",
            "ProductId",
        ),
        (
            "a covered product, under an agreement that does not exist",
            "prod-one",
            "agmt-unknown",
            "AgreementId",
        ),
        (
            "a covered product, under an agreement covering other products",
            "prod-one",
            "agmt-two",
            "ProductId",
        ),
        (
            "an agreement differing from a held one only in case",
            "prod-one",
            "AGMT-ONE",
            "AgreementId",
        ),
    ];

    for (case, product_id, agreement_id, member) in rows {
        let (status, answer) = put(
            &router,
            CATALOG,
            product_id,
            &json!({
                "agreementId": agreement_id,
                "deploymentParameter": { "name": "DbPassword", "secretString": "s3cr3t-value-1" },
            }),
        )
        .await;
        assert_target_not_found(case, member, status, &answer);
    }

    // The targets that DO resolve are still served, so the rows above were refused
    // for their own segment and not by a blanket refusal.
    for (case, product_id, agreement_id) in [
        ("the first agreement's own product", "prod-one", "agmt-one"),
        ("the shared product under the first", "prod-shared", "agmt-one"),
        ("the shared product under the second", "prod-shared", "agmt-two"),
    ] {
        let (status, answer) = put(
            &router,
            CATALOG,
            product_id,
            &json!({
                "agreementId": agreement_id,
                "deploymentParameter": { "name": "DbPassword", "secretString": "s" },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a target the account holds must be written, got {status} {answer:?}"
        );
    }
    let stored = stored_parameters(&engine);
    assert_eq!(
        stored, 3,
        "expected one record per resolved target, got {stored} stored"
    );
}

#[tokio::test]
async fn targets_differing_in_one_segment_are_different_resources() {
    let (router, engine) = emulator();

    // (case, productId, agreementId): each differs from the first target in ONE
    // segment — including by case alone, since neither identifier's modelled
    // pattern makes them case-insensitive. Every one of them is held out of band,
    // so what the rows observe is the identity rule and not the target check.
    let targets: Vec<(&str, &str, &str)> = vec![
        ("the first target", "prod-live", "agmt-live"),
        ("another product under the same agreement", "prod-second", "agmt-live"),
        ("the same product under another agreement", "prod-live", "agmt-second"),
        ("a product id differing only in case", "PROD-LIVE", "agmt-live"),
        ("an agreement id differing only in case", "prod-live", "AGMT-LIVE"),
    ];
    for (_, product_id, agreement_id) in &targets {
        provision(&engine, CATALOG, agreement_id, &[product_id]);
    }

    let mut ids: Vec<(&str, String)> = Vec::new();
    let mut first_arn = String::new();
    for (case, product_id, agreement_id) in &targets {
        let mut body = json!({
            "agreementId": agreement_id,
            "deploymentParameter": { "name": "DbPassword", "secretString": "s" },
        });
        if ids.is_empty() {
            body.as_object_mut()
                .expect("an object body")
                .insert("tags".to_string(), json!({ "env": "live" }));
        }
        let (status, answer) = put(&router, CATALOG, product_id, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the write to be served, got {status} {answer:?}"
        );
        let id = answer
            .get("deploymentParameterId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            is_parameter_id(&id),
            "case {case:?}: expected a minted parameter id, got {answer:?}"
        );
        if ids.is_empty() {
            first_arn = answer
                .get("resourceArn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
        }
        ids.push((case, id));
    }

    for (left, (case, id)) in ids.iter().enumerate() {
        for (other_case, other_id) in ids.iter().skip(left + 1) {
            assert_ne!(
                id, other_id,
                "cases {case:?} and {other_case:?}: targets differing in one segment must be \
                 their own resources, both got {id:?}"
            );
        }
    }

    // None of those writes touched the first record: its own tags are still what
    // its create left.
    let (status, listed) = list_tags(&router, &first_arn).await;
    assert_eq!(
        status, 200,
        "the first record must still be readable, got {status} {listed:?}"
    );
    assert_eq!(
        tags_of(&listed),
        crate::deployment_parameters::tag_map(&[("env", "live")]),
        "the first record's tags must be untouched by the near misses, got {listed:?}"
    );
}

#[tokio::test]
async fn a_modelled_constraint_is_refused_before_the_target_is_resolved() {
    let (router, engine) = emulator();
    provision(&engine, CATALOG, "agmt-held", &["prod-held"]);

    // Each row breaks one modelled constraint of the input. The pairs cover both
    // sides of the target store: a target the account holds, and one it does not —
    // the answer is the 400, which is what "before the target is resolved" means.
    // An oversized `@sensitive` payload is NOT here: it is refused one layer
    // earlier, with the front door's 403 (`front_door_forbidden`). Neither is a
    // `tags` map past the number of tags one resource holds: that count is the
    // tagging front end's own limit, refused with its own identity ahead of this
    // service's modelled validation (`tag_count_front_end`).
    let rows: Vec<(&str, &str, &str, Value)> = vec![
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
            "catalog naming no catalog this service serves, target not held",
            "PrivateMarketplace",
            "prod-unknown",
            json!({
                "agreementId": "agmt-unknown",
                "deploymentParameter": { "name": "N", "secretString": "s" },
            }),
        ),
        (
            "agreementId outside its pattern, target not held",
            CATALOG,
            "prod-unknown",
            json!({
                "agreementId": "bad id!",
                "deploymentParameter": { "name": "N", "secretString": "s" },
            }),
        ),
        (
            "a tag key in the namespace AWS reserves, target not held",
            CATALOG,
            "prod-unknown",
            json!({
                "agreementId": "agmt-unknown",
                "deploymentParameter": { "name": "N", "secretString": "s" },
                "tags": { "aws:created-by": "someone" },
            }),
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
        let reported = message(&answer);
        assert!(
            !reported.contains("does not exist"),
            "case {case:?}: the refusal must not read as an unresolved target, got {reported:?}"
        );
    }

    let held = stored_parameters(&engine);
    assert_eq!(
        held, 0,
        "a refused write must store nothing, got {held} stored deployment parameters"
    );
}

#[tokio::test]
async fn a_write_refused_for_its_target_consumes_no_idempotency_token() {
    let (router, engine) = emulator();
    // 36 characters of `^[a-zA-Z0-9/_+=.:@-]+$`, inside `ClientToken`'s len 32..64.
    let token = "0123456789abcdef0123456789abcdef0123";
    let body = json!({
        "agreementId": "agmt-later",
        "deploymentParameter": { "name": "DbPassword", "secretString": "s" },
        "clientToken": token,
    });

    // Refused because nothing holds its target, so the write never happens and the
    // token is never bound to one.
    let (status, answer) = put(&router, CATALOG, "prod-later", &body).await;
    assert_target_not_found("the token's first attempt", "AgreementId", status, &answer);
    let stored = stored_parameters(&engine);
    assert_eq!(
        stored, 0,
        "a write refused for its target must store nothing, got {stored} stored"
    );

    // Once the target is held out of band, the SAME token performs the write rather
    // than replaying or conflicting with it.
    provision(&engine, CATALOG, "agmt-later", &["prod-later"]);
    let (status, created) = put(&router, CATALOG, "prod-later", &body).await;
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
    let (status, replayed) = put(&router, CATALOG, "prod-later", &body).await;
    assert_eq!(
        status, 200,
        "the replay must succeed, got {status} {replayed:?}"
    );
    assert_eq!(
        replayed, created,
        "a clientToken replay must answer the identical output, got {replayed:?} against {created:?}"
    );
}

#[tokio::test]
async fn the_target_ledger_is_bounded_and_refuses_a_target_past_its_ceiling() {
    let (router, engine) = emulator();

    /// The i-th seeded target's product and agreement ids, inside `ResourceId`'s
    /// modelled `^[A-Za-z0-9_/-]+$` and `length 1..64`.
    fn target(i: usize) -> (String, String) {
        (format!("prod-ledger-{i:06}"), format!("agmt-ledger-{i:06}"))
    }
    let body = |agreement_id: &str| {
        json!({
            "agreementId": agreement_id,
            "deploymentParameter": { "name": "Ledger", "secretString": "s" },
            // An expiry that has already come, so each stored parameter is purged
            // on the very next request and hands its slot under the
            // deployment-parameter quota straight back: only the target ledger's
            // OWN ceiling bounds this run. In the RFC 3339 form the member's
            // `@timestampFormat("date-time")` declares.
            "expirationDate": "1970-01-01T00:00:01Z",
        })
    };

    // The ledger the out-of-band seam fills is capped, and the cap is a REFUSAL:
    // seeding past it records nothing and every target already held stays held, so
    // no stored parameter loses the target it was written against.
    let (first_product, first_agreement) = target(0);
    provision(&engine, CATALOG, &first_agreement, &[&first_product]);
    let (status, answer) = put(&router, CATALOG, &first_product, &body(&first_agreement)).await;
    assert_eq!(
        status, 200,
        "the first seeded target must be written, got {status} {answer:?}"
    );

    // Seed well past the ceiling, one fresh target at a time.
    for i in 1..=MAX_PROVISIONED_TARGETS {
        let (product_id, agreement_id) = target(i);
        provision(&engine, CATALOG, &agreement_id, &[&product_id]);
        let held = recorded_targets(&engine);
        assert!(
            held <= MAX_PROVISIONED_TARGETS,
            "target {i}: the ledger must stay inside its {MAX_PROVISIONED_TARGETS}-target \
             ceiling, got {held} recorded targets"
        );
    }

    let held = recorded_targets(&engine);
    assert_eq!(
        held, MAX_PROVISIONED_TARGETS,
        "expected the ledger exactly at its ceiling, got {held} recorded targets"
    );

    // The target seeded FIRST — the one a stored parameter names — is still held,
    // and a write against it is still served.
    assert!(
        holds_target(&engine, CATALOG, &first_product, &first_agreement),
        "a target already held must survive seeding past the ceiling"
    );
    let (status, answer) = put(&router, CATALOG, &first_product, &body(&first_agreement)).await;
    assert_eq!(
        status, 200,
        "a write against a target the ledger kept must be served, got {status} {answer:?}"
    );

    // The target seeded LAST did not fit, so the account does not hold it and a
    // write against it is the modelled refusal.
    let (last_product, last_agreement) = target(MAX_PROVISIONED_TARGETS);
    assert!(
        !holds_target(&engine, CATALOG, &last_product, &last_agreement),
        "a target seeded past the ceiling must not be recorded"
    );
    let (status, answer) = put(&router, CATALOG, &last_product, &body(&last_agreement)).await;
    assert_target_not_found(
        "a write against a target the ledger refused",
        "AgreementId",
        status,
        &answer,
    );

    // The fixture set a fresh store boots with is still there too: a ceiling that
    // refuses rather than evicts cannot displace it.
    for (agreement_id, product_ids) in DEFAULT_AGREEMENTS {
        for product_id in *product_ids {
            assert!(
                holds_target(&engine, CATALOG, product_id, agreement_id),
                "the boot fixture {product_id:?}/{agreement_id:?} must survive seeding \
                 past the ceiling"
            );
        }
    }

    // And the expiries really were materialised, so the parameter quota was never
    // what bounded the run.
    let stored = stored_parameters(&engine);
    assert!(
        stored <= 1,
        "every parameter past its expiration date must be purged, got {stored} stored"
    );
}
