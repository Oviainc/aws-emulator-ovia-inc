// SPDX-License-Identifier: CC0-1.0

//! `PutDeploymentParameter`'s documented create path is reachable by an ordinary
//! HTTP client, with no in-process help at all.
//!
//! The rule under test: the target a write resolves — the agreement of the
//! account, the product its URI names, inside a catalog this service serves — is
//! established out of band of this API, and a fresh store is already holding the
//! fixture set `state::DEFAULT_AGREEMENTS` in every catalog of
//! `state::SUPPORTED_CATALOGS`. So the create-or-update the operation documents is
//! served for those targets over the wire alone, while any other triple is the
//! modelled `ResourceNotFoundException` (HTTP 404). Not one case below seeds state
//! through the crate's Rust seam: every byte of state these assertions read was
//! either held from boot or written by a request over the wire.
//!
//! What follows from it, and is asserted here: the identity a write is keyed on is
//! still (catalog, productId, agreementId, name) — distinct targets are distinct
//! resources and a repeat for one identity updates in place — one agreement covers
//! every product it was recorded with, and the create -> read -> update -> untag
//! lifecycle runs end to end over HTTP.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 routing and serialization, the modelled-constraint walk and the
//! handlers — through the harness the `deployment_parameters` suite owns.

use serde_json::{json, Value};
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::state::{
    DEFAULT_AGREEMENTS, SUPPORTED_CATALOGS,
};

use crate::deployment_parameters::{
    emulator, error_code, expected_arn, holds_target, is_parameter_id, list_tags, message, put,
    stored_parameters, tag, tag_map, tags_of, untag, CATALOG, SECRET_MAX_LEN,
};

/// Every target a fresh store holds, as `(catalog, productId, agreementId)`.
///
/// Read off the crate's own fixture set rather than restated here, so the suite
/// cannot drift from what the emulator boots with.
fn default_targets() -> Vec<(&'static str, &'static str, &'static str)> {
    let mut targets = Vec::new();
    for catalog in SUPPORTED_CATALOGS {
        for (agreement_id, product_ids) in DEFAULT_AGREEMENTS {
            for product_id in *product_ids {
                targets.push((*catalog, *product_id, *agreement_id));
            }
        }
    }
    targets
}

/// A write body for `parameter`, carrying `secret` and optionally `tags`.
fn body(
    agreement_id: &str,
    parameter: &str,
    secret: &str,
    tags: Option<&[(&str, &str)]>,
) -> Value {
    let mut body = json!({
        "agreementId": agreement_id,
        "deploymentParameter": { "name": parameter, "secretString": secret },
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

/// Whether `candidate` satisfies the modelled `ResourceId`
/// (`smithy_query(query="shape",shape="ResourceId")`: `length 1..64`,
/// `^[A-Za-z0-9_/-]+$`), which both `productId` and `agreementId` target.
fn is_resource_id(candidate: &str) -> bool {
    (1..=64).contains(&candidate.len())
        && candidate
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'/' | b'-'))
}

#[tokio::test]
async fn the_default_target_set_names_only_identifiers_the_model_admits() {
    let (_router, engine) = emulator();
    let targets = default_targets();
    assert!(
        !targets.is_empty(),
        "a fresh store must hold at least one target, or the create path is \
         unreachable over the wire"
    );
    for (catalog, product_id, agreement_id) in &targets {
        assert!(
            is_resource_id(product_id) && is_resource_id(agreement_id),
            "target {catalog:?}/{product_id:?}/{agreement_id:?}: both identifiers must \
             satisfy the modelled ResourceId"
        );
        // Held from boot, with nothing in-process having recorded it.
        assert!(
            holds_target(&engine, catalog, product_id, agreement_id),
            "target {catalog:?}/{product_id:?}/{agreement_id:?}: a fresh store must hold it"
        );
    }
    // And a fresh store holds no deployment parameter: the fixtures are targets,
    // not records.
    let stored = stored_parameters(&engine);
    assert_eq!(
        stored, 0,
        "a fresh store must hold no deployment parameter, got {stored}"
    );
}

/// One request shape a target is written with: the rule holds whatever else the
/// request carries, so every target below is written in each of these.
struct Shape {
    case: &'static str,
    parameter: &'static str,
    secret: String,
    tags: Option<&'static [(&'static str, &'static str)]>,
}

/// The request-shape table. The rows differ along what the rule generalises over:
/// the parameter name's form, the size of the `@sensitive` payload (one character
/// up to `SecretString`'s modelled maximum) and whether the create carries tags.
fn shape_table() -> Vec<Shape> {
    vec![
        Shape {
            case: "a plain name with tags",
            parameter: "DbPassword",
            secret: "s3cr3t-value-1".to_string(),
            tags: Some(&[("env", "dev")]),
        },
        Shape {
            case: "a punctuated name, no tags",
            parameter: "api.key+v1=1@host/path_1-2",
            secret: "x".to_string(),
            tags: None,
        },
        Shape {
            case: "a one-character name and a secret at the modelled maximum",
            parameter: "d",
            secret: "y".repeat(SECRET_MAX_LEN),
            tags: None,
        },
    ]
}

#[tokio::test]
async fn every_default_target_is_written_over_http_with_no_in_process_help() {
    let (router, engine) = emulator();
    let mut minted: Vec<(String, String)> = Vec::new();

    for (catalog, product_id, agreement_id) in default_targets() {
        for shape in shape_table() {
            let case = format!("{catalog}/{product_id}/{agreement_id} with {}", shape.case);
            let request = body(agreement_id, shape.parameter, &shape.secret, shape.tags);
            let (status, created) = put(&router, catalog, product_id, &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a target held from boot must be written over HTTP alone, \
                 got {status} {created:?}"
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
                Some(agreement_id),
                "case {case:?}: expected the stored agreementId, got {created:?}"
            );

            // The round trip: the record is readable back through an implemented
            // read, over the same wire path.
            let (status, listed) = list_tags(&router, &arn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the written record must be readable, got {status} {listed:?}"
            );
            assert_eq!(
                tags_of(&listed),
                tag_map(shape.tags.unwrap_or(&[])),
                "case {case:?}: expected the create's tags from the record, got {listed:?}"
            );

            // A repeat for the SAME identity updates in place: one target, one
            // record, and `tags` is ignored on the update.
            let (status, again) = put(
                &router,
                catalog,
                product_id,
                &body(
                    agreement_id,
                    shape.parameter,
                    "rotated",
                    Some(&[("ignored", "on-update")]),
                ),
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
            assert_eq!(
                tags_of(&again),
                tag_map(shape.tags.unwrap_or(&[])),
                "case {case:?}: tags are applied on create only, got {again:?}"
            );

            minted.push((case, id));
        }
    }

    // One record per (target, name) identity, and every identifier distinct.
    let expected = default_targets().len() * shape_table().len();
    let stored = stored_parameters(&engine);
    assert_eq!(
        stored, expected,
        "expected one record per written identity, got {stored} for {expected} identities"
    );
    for (left, (case, id)) in minted.iter().enumerate() {
        for (other_case, other_id) in minted.iter().skip(left + 1) {
            assert_ne!(
                id, other_id,
                "cases {case:?} and {other_case:?}: distinct identities must mint distinct ids, \
                 both got {id:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_target_outside_the_default_set_is_refused_over_http_alone() {
    let (router, engine) = emulator();
    let held = default_targets();
    let (_, held_product, held_agreement) = held[0];

    // Each row is a triple a fresh store does not hold: the product, the
    // agreement, or both, outside the fixture set — and one row differing from a
    // held identifier only in case, since neither pattern makes them
    // case-insensitive.
    let rows: Vec<(&str, String, String)> = vec![
        (
            "neither identifier held",
            "prod-never-provisioned".to_string(),
            "agmt-never-provisioned".to_string(),
        ),
        (
            "a held agreement, a product it does not cover",
            "prod-never-provisioned".to_string(),
            held_agreement.to_string(),
        ),
        (
            "a held product, an agreement that does not exist",
            held_product.to_string(),
            "agmt-never-provisioned".to_string(),
        ),
        (
            "a held product upper-cased",
            held_product.to_uppercase(),
            held_agreement.to_string(),
        ),
        (
            "a held agreement upper-cased",
            held_product.to_string(),
            held_agreement.to_uppercase(),
        ),
    ];

    for (case, product_id, agreement_id) in rows {
        assert!(
            !holds_target(&engine, CATALOG, &product_id, &agreement_id),
            "case {case:?}: the row must name a target a fresh store does not hold"
        );
        let (status, answer) = put(
            &router,
            CATALOG,
            &product_id,
            &body(&agreement_id, "DbPassword", "s3cr3t-value-1", None),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: a target the account does not hold must be 404, \
             got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        let reported = message(&answer);
        assert!(
            reported.starts_with("ProductId:") || reported.starts_with("AgreementId:"),
            "case {case:?}: the refusal must name the unresolved identifier, got {reported:?}"
        );
    }

    let stored = stored_parameters(&engine);
    assert_eq!(
        stored, 0,
        "a refused write must store nothing, got {stored} stored deployment parameters"
    );
}

#[tokio::test]
async fn the_create_read_update_untag_lifecycle_runs_over_http_alone() {
    let (router, _engine) = emulator();
    let (catalog, product_id, agreement_id) = default_targets()[0];
    let case = "the lifecycle an HTTP client drives";

    // create
    let (status, created) = put(
        &router,
        catalog,
        product_id,
        &body(
            agreement_id,
            "DbPassword",
            "s3cr3t",
            Some(&[("env", "dev"), ("owner", "team")]),
        ),
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
async fn one_agreement_covers_every_product_it_was_recorded_with() {
    let (router, engine) = emulator();

    // A fixture agreement covering more than one product, read off the crate's own
    // set: each (product, agreement) pair of it is its own target and its own
    // resource.
    let Some((agreement_id, product_ids)) = DEFAULT_AGREEMENTS
        .iter()
        .find(|(_, product_ids)| product_ids.len() > 1)
    else {
        panic!("the fixture set must hold one agreement covering several products");
    };

    let mut ids: Vec<(&str, String)> = Vec::new();
    for product_id in *product_ids {
        assert!(
            holds_target(&engine, CATALOG, product_id, agreement_id),
            "the agreement must cover {product_id:?} from boot"
        );
        let (status, created) = put(
            &router,
            CATALOG,
            product_id,
            &body(agreement_id, "DbPassword", "s", None),
        )
        .await;
        assert_eq!(
            status, 200,
            "product {product_id:?}: a covered product must be written, got {status} {created:?}"
        );
        let id = created
            .get("deploymentParameterId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            is_parameter_id(&id),
            "product {product_id:?}: expected a minted parameter id, got {created:?}"
        );
        ids.push((*product_id, id));
    }

    assert_eq!(
        stored_parameters(&engine),
        product_ids.len(),
        "expected one record per covered product, got {}",
        stored_parameters(&engine)
    );
    for (left, (case, id)) in ids.iter().enumerate() {
        for (other_case, other_id) in ids.iter().skip(left + 1) {
            assert_ne!(
                id, other_id,
                "products {case:?} and {other_case:?}: distinct targets must mint distinct ids, \
                 both got {id:?}"
            );
        }
    }
}
