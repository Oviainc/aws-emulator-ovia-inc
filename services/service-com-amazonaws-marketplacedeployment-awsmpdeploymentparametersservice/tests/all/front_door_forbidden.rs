// SPDX-License-Identifier: CC0-1.0

//! The front door refuses an oversized `@sensitive` payload before the service
//! sees the request.
//!
//! The rule under test: a request carrying a `@sensitive` member past the MAXIMUM
//! length its modelled shape declares is refused by the AWS front door — HTTP 403,
//! wire error code `ForbiddenException`, the constant message `"Forbidden"` — ahead
//! of the modelled constraint walk and of every judgement the service itself makes,
//! and nothing of the rejected value is echoed. A gateway drops a payload it will
//! not forward rather than describing it back to the caller, so the answer is that
//! opaque refusal and not the modelled 400 `ValidationException`; a client
//! branching on the code sees a non-retryable front-door refusal, which is what AWS
//! sends.
//!
//! What the rule does NOT cover, and this suite pins too: the same member at or
//! below its maximum is an ordinary write (readable back through an implemented
//! read), and a violation of its MINIMUM is the modelled `ValidationException`,
//! stated without echoing the value (`validation_messages`).
//!
//! The model's only `@sensitive` shape is `SecretString`
//! (`smithy_query(query="shape",shape="SecretString")`: `length 1..15000`,
//! `sensitive: true`), reached as `DeploymentParameterInput$secretString`, so
//! `PutDeploymentParameter` is the one operation the rule can be observed on.
//!
//! Every case drives the real wire path through the harness the
//! `deployment_parameters` suite owns.

use serde_json::{json, Value};
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::state::service_state;

use crate::deployment_parameters::{
    emulator, error_code, list_tags, message, provision, put, tags_of, CATALOG, SECRET_MAX_LEN,
};

/// How many deployment parameters the store holds.
fn held(engine: &engine::InMemoryEngine) -> usize {
    service_state(engine)
        .expect("the shared engine holds this service's store")
        .lock()
        .parameters
        .len()
}

#[tokio::test]
async fn an_oversized_sensitive_payload_is_refused_403_forbidden_and_stores_nothing() {
    let (router, engine) = emulator();
    provision(&engine, CATALOG, "agmt-held", &["prod-held"]);

    // The rows differ along what the rule generalises over: how far past the
    // maximum the payload reaches (one character, a little, an order of
    // magnitude), whether the target is one the account holds, and what ELSE the
    // request gets wrong — every one of which the walk or the handler would
    // otherwise answer differently (a 400 on the pattern, a 400 on the catalog, a
    // 404 on the target), so each row also pins that the front door answers FIRST.
    //
    // (case, catalog, productId, agreementId, parameter name, secret length)
    let rows: Vec<(&str, &str, &str, &str, String, usize)> = vec![
        (
            "one character past the maximum, target held",
            CATALOG,
            "prod-held",
            "agmt-held",
            "DbPassword".to_string(),
            SECRET_MAX_LEN + 1,
        ),
        (
            "one character past the maximum, target not held",
            CATALOG,
            "prod-unknown",
            "agmt-unknown",
            "DbPassword".to_string(),
            SECRET_MAX_LEN + 1,
        ),
        (
            "a thousand characters past the maximum",
            CATALOG,
            "prod-held",
            "agmt-held",
            "DbPassword".to_string(),
            SECRET_MAX_LEN + 1_000,
        ),
        (
            "twice the maximum",
            CATALOG,
            "prod-held",
            "agmt-held",
            "DbPassword".to_string(),
            SECRET_MAX_LEN * 2,
        ),
        (
            "oversized, and an agreementId outside its pattern",
            CATALOG,
            "prod-held",
            "bad id!",
            "DbPassword".to_string(),
            SECRET_MAX_LEN + 1,
        ),
        (
            "oversized, and a parameter name past its own maximum",
            CATALOG,
            "prod-held",
            "agmt-held",
            "n".repeat(401),
            SECRET_MAX_LEN + 1,
        ),
        (
            "oversized, and a catalog that names no catalog",
            "NotTheRealMarketplace",
            "prod-held",
            "agmt-held",
            "DbPassword".to_string(),
            SECRET_MAX_LEN + 1,
        ),
    ];

    for (case, catalog, product_id, agreement_id, name, length) in rows {
        // A distinctive secret, so an echo of any part of it is detectable.
        let secret = format!("topsecret-{}", "x".repeat(length - "topsecret-".len()));
        assert_eq!(
            secret.len(),
            length,
            "case {case:?}: the row's secret must be the length it names, got {}",
            secret.len()
        );
        let (status, answer) = put(
            &router,
            catalog,
            product_id,
            &json!({
                "agreementId": agreement_id,
                "deploymentParameter": { "name": name, "secretString": secret },
            }),
        )
        .await;
        assert_eq!(
            status, 403,
            "case {case:?}: an oversized sensitive payload must be refused 403, \
             got {status} {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ForbiddenException",
            "case {case:?}: expected ForbiddenException, got {answer:?}"
        );
        assert_eq!(
            message(&answer),
            "Forbidden",
            "case {case:?}: expected the front door's constant message, got {answer:?}"
        );
        let rendered = answer.to_string();
        assert!(
            !rendered.contains("topsecret-"),
            "case {case:?}: a refused secret must not be echoed, got {rendered:?}"
        );
        assert!(
            !rendered.contains("15000"),
            "case {case:?}: the front door names no member and no bound, got {rendered:?}"
        );
    }

    assert_eq!(
        held(&engine),
        0,
        "a refused request must store nothing, got {} stored deployment parameters",
        held(&engine)
    );
}

#[tokio::test]
async fn a_sensitive_payload_within_its_bounds_is_written_and_one_short_of_them_is_a_400() {
    let (router, engine) = emulator();
    provision(&engine, CATALOG, "agmt-bounds", &["prod-bounds"]);

    // Every length the model admits is an ordinary write, up to and INCLUDING the
    // maximum: the guard bites past the bound, not at it.
    let accepted: Vec<(&str, usize)> = vec![
        ("the one-character minimum", 1),
        ("a short secret", 14),
        ("a kilobyte", 1_024),
        ("one character short of the maximum", SECRET_MAX_LEN - 1),
        ("exactly the maximum", SECRET_MAX_LEN),
    ];

    for (case, length) in accepted {
        let (status, created) = put(
            &router,
            CATALOG,
            "prod-bounds",
            &json!({
                "agreementId": "agmt-bounds",
                "deploymentParameter": {
                    "name": format!("Secret{length}"),
                    "secretString": "y".repeat(length),
                },
                "tags": { "env": "dev" },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a secret inside its modelled bounds must be written, \
             got {status} {created:?}"
        );
        // The round trip: the record the write stored is readable back.
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

    // Below the MINIMUM is the modelled refusal, not the front door's: the service
    // states the member and the bound (without the value, which is `@sensitive`).
    let (status, answer) = put(
        &router,
        CATALOG,
        "prod-bounds",
        &json!({
            "agreementId": "agmt-bounds",
            "deploymentParameter": { "name": "Empty", "secretString": "" },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a secret short of its minimum must be the modelled 400, got {status} {answer:?}"
    );
    assert_eq!(
        error_code(&answer),
        "ValidationException",
        "expected ValidationException, got {answer:?}"
    );
    assert!(
        message(&answer).contains("deploymentParameter.secretString"),
        "the modelled refusal must name the member, got {answer:?}"
    );
}
