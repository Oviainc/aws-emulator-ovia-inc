// SPDX-License-Identifier: CC0-1.0

//! WHICH segment of a deployment-parameter write's target is blamed when the
//! target does not resolve.
//!
//! The rule under test: a targeted write resolves its target in the order the
//! operation is targeted by — `PutDeploymentParameter` "creates or updates a
//! deployment parameter and is targeted by `catalog` and `agreementId`" for the
//! product its URI addresses — so the blamed member is the FIRST segment that
//! does not resolve in that order: `AgreementId` whenever the account holds no
//! such agreement in the catalog (which is the case for a request naming neither
//! a held agreement nor a held product), and `ProductId` only once the agreement
//! itself resolves and does not cover the product the request addresses.
//!
//! The refusal is the modelled `ResourceNotFoundException` (HTTP 404,
//! `smithy_query(query="operation",operation="PutDeploymentParameter")`), and its
//! message is one template: the fixed sentence
//! `"<Member>: Request references a resource which does not exist."`, whose only
//! variable part is the PascalCase name of that segment. No value from the
//! request — no id, no ARN — appears in it.
//!
//! Every case drives the real wire path through the harness the
//! `deployment_parameters` suite owns.

use serde_json::{json, Value};

use crate::deployment_parameters::{
    emulator, error_code, holds_target, message, provision, put, stored_parameters, CATALOG,
};

/// The modelled error and status an unresolved target carries.
const NOT_FOUND_CODE: &str = "ResourceNotFoundException";
const NOT_FOUND_STATUS: u16 = 404;

/// The one sentence the refusal states for the segment `member`.
fn not_found_sentence(member: &str) -> String {
    format!("{member}: Request references a resource which does not exist.")
}

/// One row: the target a request names, and the member its refusal must blame
/// (`None` when the whole target resolves and the write is served).
struct Row {
    case: &'static str,
    product_id: String,
    agreement_id: String,
    blamed: Option<&'static str>,
}

/// The agreements the account holds for the table below, as
/// `(agreementId, productIds)`. Written in the forms `ResourceId` admits
/// (`^[A-Za-z0-9_/-]+$`, `length 1..64`), including the one-character minimum and
/// the 64-character maximum, so the rule is exercised over the identifier forms it
/// generalises over and not only over one spelling.
fn held_agreements() -> Vec<(String, Vec<String>)> {
    vec![
        (
            "agmt-held-1".to_string(),
            vec!["prod-held-1".to_string(), "prod/shared/7".to_string()],
        ),
        (
            "agmt_held/2-x".to_string(),
            vec!["prod/shared/7".to_string()],
        ),
        ("a".to_string(), vec!["p".to_string()]),
        (
            "g".repeat(64),
            vec!["0".to_string(), "d".repeat(64)],
        ),
    ]
}

/// The table. Each row differs from a resolving target in exactly one way, so what
/// it observes is the precedence rule: which segment fails to resolve, and in which
/// form.
fn table() -> Vec<Row> {
    let long_agreement = "g".repeat(64);
    let long_product = "d".repeat(64);
    vec![
        // Nothing the account holds: the agreement is the segment resolved first,
        // so it is the one blamed.
        Row {
            case: "neither the product nor the agreement is held",
            product_id: "prod-unknown".to_string(),
            agreement_id: "agmt-unknown".to_string(),
            blamed: Some("AgreementId"),
        },
        Row {
            case: "a product a held agreement covers, under an unknown agreement",
            product_id: "prod-held-1".to_string(),
            agreement_id: "agmt-unknown".to_string(),
            blamed: Some("AgreementId"),
        },
        Row {
            case: "a product two held agreements cover, under an unknown agreement",
            product_id: "prod/shared/7".to_string(),
            agreement_id: "agmt-unknown".to_string(),
            blamed: Some("AgreementId"),
        },
        Row {
            case: "an agreement differing from a held one only in case",
            product_id: "prod-held-1".to_string(),
            agreement_id: "AGMT-HELD-1".to_string(),
            blamed: Some("AgreementId"),
        },
        Row {
            case: "a one-character agreement id the account does not hold",
            product_id: "p".to_string(),
            agreement_id: "b".to_string(),
            blamed: Some("AgreementId"),
        },
        Row {
            case: "a 64-character agreement id the account does not hold",
            product_id: long_product.clone(),
            agreement_id: "h".repeat(64),
            blamed: Some("AgreementId"),
        },
        // The agreement resolves: now the product is the segment that does not.
        Row {
            case: "a held agreement, a product nothing covers",
            product_id: "prod-unknown".to_string(),
            agreement_id: "agmt-held-1".to_string(),
            blamed: Some("ProductId"),
        },
        Row {
            case: "a held agreement, a product only another agreement covers",
            product_id: "prod-held-1".to_string(),
            agreement_id: "agmt_held/2-x".to_string(),
            blamed: Some("ProductId"),
        },
        Row {
            case: "a held agreement, a product differing from a covered one only in case",
            product_id: "PROD-HELD-1".to_string(),
            agreement_id: "agmt-held-1".to_string(),
            blamed: Some("ProductId"),
        },
        Row {
            case: "a one-character held agreement, a product it does not cover",
            product_id: "q".to_string(),
            agreement_id: "a".to_string(),
            blamed: Some("ProductId"),
        },
        Row {
            case: "a 64-character held agreement, a product it does not cover",
            product_id: "prod-unknown".to_string(),
            agreement_id: long_agreement.clone(),
            blamed: Some("ProductId"),
        },
        // The controls: every segment resolves, so the write is served.
        Row {
            case: "a held agreement covering the product addressed",
            product_id: "prod-held-1".to_string(),
            agreement_id: "agmt-held-1".to_string(),
            blamed: None,
        },
        Row {
            case: "a shared product under the second agreement that covers it",
            product_id: "prod/shared/7".to_string(),
            agreement_id: "agmt_held/2-x".to_string(),
            blamed: None,
        },
        Row {
            case: "the one-character target",
            product_id: "p".to_string(),
            agreement_id: "a".to_string(),
            blamed: None,
        },
        Row {
            case: "the 64-character target",
            product_id: long_product,
            agreement_id: long_agreement,
            blamed: None,
        },
    ]
}

#[tokio::test]
async fn the_refusal_blames_the_first_target_segment_that_does_not_resolve() {
    let (router, engine) = emulator();
    for (agreement_id, product_ids) in held_agreements() {
        let products: Vec<&str> = product_ids.iter().map(String::as_str).collect();
        provision(&engine, CATALOG, &agreement_id, &products);
    }

    let mut served = 0usize;
    for row in table() {
        let case = row.case;
        assert_eq!(
            holds_target(&engine, CATALOG, &row.product_id, &row.agreement_id),
            row.blamed.is_none(),
            "case {case:?}: the row must name a target the account holds exactly when it \
             expects to be served, got holds_target {}",
            holds_target(&engine, CATALOG, &row.product_id, &row.agreement_id)
        );
        let (status, answer) = put(
            &router,
            CATALOG,
            &row.product_id,
            &json!({
                "agreementId": row.agreement_id,
                "deploymentParameter": { "name": "DbPassword", "secretString": "s3cr3t-value-1" },
            }),
        )
        .await;

        let Some(member) = row.blamed else {
            assert_eq!(
                status, 200,
                "case {case:?}: a target the account holds must be written, got {status} {answer:?}"
            );
            served += 1;
            continue;
        };

        assert_eq!(
            status, NOT_FOUND_STATUS,
            "case {case:?}: expected HTTP {NOT_FOUND_STATUS}, got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            NOT_FOUND_CODE,
            "case {case:?}: expected {NOT_FOUND_CODE}, got {answer:?}"
        );
        let reported = message(&answer);
        assert_eq!(
            reported,
            not_found_sentence(member),
            "case {case:?}: expected the sentence for {member}, got {reported:?}"
        );
        // The template's only variable part is the member, so nothing the request
        // sent may appear in it. Checked for the identifiers long enough for the
        // check to mean anything: a one-character id is a letter the sentence's own
        // fixed text carries anyway, and the equality above already pins that text.
        let carried: Vec<&str> = [row.product_id.as_str(), row.agreement_id.as_str(), CATALOG]
            .into_iter()
            .filter(|value| value.len() > 2)
            .collect();
        for value in carried {
            assert!(
                !reported.contains(value),
                "case {case:?}: the refusal must carry no value from the request, \
                 got {reported:?} carrying {value:?}"
            );
        }
        assert!(
            answer.get("resourceArn").is_none() && answer.get("deploymentParameterId").is_none(),
            "case {case:?}: a refused write must mint nothing, got {answer:?}"
        );
    }

    let stored = stored_parameters(&engine);
    assert_eq!(
        stored, served,
        "expected one record per resolved target ({served}), got {stored} stored"
    );
}

#[tokio::test]
async fn the_blamed_segment_follows_what_the_account_holds_and_nothing_else() {
    // The same request, answered three ways as the store gains the segments it
    // names: the agreement first, then the product it covers. Nothing about the
    // request changes between the calls, so the blamed segment is a fact of what
    // the account holds.
    let (router, engine) = emulator();
    let body = json!({
        "agreementId": "agmt-staged",
        "deploymentParameter": { "name": "Staged", "secretString": "s" },
    });

    let (status, answer) = put(&router, CATALOG, "prod-staged", &body).await;
    assert_eq!(
        status, NOT_FOUND_STATUS,
        "nothing is held yet, so the write must be refused, got {status} {answer:?}"
    );
    assert_eq!(
        message(&answer),
        not_found_sentence("AgreementId"),
        "with no agreement held the refusal must blame the agreement, got {answer:?}"
    );

    // The agreement exists now, covering ANOTHER product: the agreement resolves,
    // so the product addressed is what does not.
    provision(&engine, CATALOG, "agmt-staged", &["prod-elsewhere"]);
    let (status, answer) = put(&router, CATALOG, "prod-staged", &body).await;
    assert_eq!(
        status, NOT_FOUND_STATUS,
        "the agreement covers another product, so the write must be refused, \
         got {status} {answer:?}"
    );
    assert_eq!(
        message(&answer),
        not_found_sentence("ProductId"),
        "with the agreement held the refusal must blame the product, got {answer:?}"
    );

    // And once that agreement covers the product too, the write is served.
    provision(&engine, CATALOG, "agmt-staged", &["prod-staged"]);
    let (status, answer) = put(&router, CATALOG, "prod-staged", &body).await;
    assert_eq!(
        status, 200,
        "the resolved target must be written, got {status} {answer:?}"
    );
    let arn = answer.get("resourceArn").and_then(Value::as_str);
    assert!(
        arn.is_some_and(|arn| !arn.is_empty()),
        "the served write must report the ARN it minted, got {answer:?}"
    );
    assert_eq!(
        stored_parameters(&engine),
        1,
        "exactly the served write may have stored a record"
    );
}
