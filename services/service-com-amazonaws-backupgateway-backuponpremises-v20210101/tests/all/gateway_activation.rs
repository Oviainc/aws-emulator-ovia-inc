// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `CreateGateway` does with the `ActivationKey` a client
//! sends it.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the
//! `MultiServiceRouter` — the harness of `gateway_lifecycle` — so what is asserted is the
//! wire behaviour a client sees, and the only way a later request can see what an earlier
//! one wrote is the shared store.
//!
//! The rule under test: an activation key is minted for a deployed appliance OUTSIDE this
//! API (no operation of this model issues one), so the service holds nothing to resolve
//! the key against and `CreateGateway` mints a gateway for EVERY `ActivationKey` the
//! modelled constraints accept — len 1..50, pattern `^[0-9a-zA-Z\-]+$`
//! (`smithy_query(query="operation",operation="CreateGateway")`) — persisting the key on
//! the record and reporting a generated `GatewayArn`, while a key those constraints
//! refuse is answered with HTTP 400 `ValidationException` and writes nothing.
//!
//! The tables span the class the rule generalises over: keys differing in length (both
//! published bounds, and one past each of them), case, digit-only and hyphen-only
//! spelling, hyphen placement, and — on the refusal side — every class of character the
//! pattern excludes.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::BACKUP_VM;

use crate::gateway_lifecycle::{build_engine, build_router, call, create_body, gateway_count, REGION};

/// One `CreateGateway` the model accepts.
#[derive(Debug)]
struct Accepted {
    case: &'static str,
    activation_key: String,
    display_name: &'static str,
}

fn accepted(case: &'static str, activation_key: String, display_name: &'static str) -> Accepted {
    Accepted {
        case,
        activation_key,
        display_name,
    }
}

/// The keys the modelled constraints accept.
///
/// The hyphenated-group row is the form a deployed gateway appliance prints: the sibling
/// storage-gateway model records `29AV1-3OFV9-VVIUB-NKT0I-LRO6V` as the `ActivationKey`
/// of its `ActivateGateway` example
/// (`smithy_query(query="operation",operation="ActivateGateway",
/// model_path="services/service-com-amazonaws-storagegateway-storagegateway-20130630/\
/// smithy-model/com-amazonaws-storagegateway-storagegateway-20130630.json")`).
fn accepted_rows() -> Vec<Accepted> {
    vec![
        // The modelled length minimum.
        accepted("one character", "k".to_string(), "gw-one-char-key"),
        accepted("two characters", "k9".to_string(), "gw-two-char-key"),
        accepted("digits only", "0123456789".to_string(), "gw-digit-key"),
        accepted(
            "the five hyphenated groups an appliance prints",
            "29AV1-3OFV9-VVIUB-NKT0I-LRO6V".to_string(),
            "gw-appliance-key",
        ),
        accepted(
            "mixed case and hyphens",
            "aB3-dE6".to_string(),
            "gw-mixed-key",
        ),
        accepted(
            "hyphens at both ends",
            "-ABCDEF123456-".to_string(),
            "gw-edge-hyphen-key",
        ),
        accepted("hyphens only", "---".to_string(), "gw-hyphen-key"),
        accepted(
            "lower case and hyphens",
            "activation-key-0001".to_string(),
            "gw-lower-key",
        ),
        // The modelled length maximum.
        accepted(
            "at the 50-character maximum",
            "K".repeat(50),
            "gw-longest-key",
        ),
    ]
}

/// The keys the modelled constraints refuse: one past each published length bound, and
/// one per class of character the pattern excludes.
fn refused_rows() -> Vec<(&'static str, String)> {
    vec![
        ("empty, one below the length minimum", String::new()),
        (
            "51 characters, one past the length maximum",
            "K".repeat(51),
        ),
        ("an underscore", "key_1".to_string()),
        ("a space", "key 1".to_string()),
        ("a dot", "key.1".to_string()),
        ("a non-ASCII letter", "kéy".to_string()),
        ("a plus sign", "key+1".to_string()),
    ]
}

/// Every activation key the model accepts mints its own gateway, which reads back.
#[tokio::test]
async fn every_activation_key_the_model_accepts_mints_a_gateway() {
    let engine = build_engine();
    let router = build_router(&engine);
    let rows = accepted_rows();

    let mut arns = BTreeSet::new();
    for row in &rows {
        let created = call(
            &router,
            "CreateGateway",
            &create_body(row.display_name, &row.activation_key, &BTreeMap::new()),
        )
        .await;
        assert_eq!(
            created.status,
            200,
            "case {:?}: a well-formed ActivationKey must create a gateway, got {} {:?}",
            row.case,
            created.status,
            created.body
        );
        let arn = created.gateway_arn().to_string();
        assert!(
            arn.starts_with(&format!("arn:aws:backup-gateway:{REGION}:"))
                && arn.contains(":gateway/"),
            "case {:?}: the create must report a generated gateway ARN, got {arn:?}",
            row.case
        );
        assert!(
            arns.insert(arn.clone()),
            "case {:?}: the ARN {arn:?} was already handed to another gateway",
            row.case
        );

        // The round trip: the record the create persisted is what a read reports.
        let fetched = call(&router, "GetGateway", &json!({ "GatewayArn": arn })).await;
        assert_eq!(
            (
                fetched.status,
                fetched.str_at("/Gateway/GatewayArn"),
                fetched.str_at("/Gateway/GatewayDisplayName"),
                fetched.str_at("/Gateway/GatewayType"),
            ),
            (
                200,
                Some(arn.as_str()),
                Some(row.display_name),
                Some(BACKUP_VM)
            ),
            "case {:?}: the created gateway must read back through GetGateway, got {} {:?}",
            row.case,
            fetched.status,
            fetched.body
        );

        // The key identifies the appliance, and no member of the reported `Gateway`
        // carries it, so a read repeats neither the member nor its value.
        let reported = fetched
            .body
            .get("Gateway")
            .and_then(|gateway| gateway.as_object())
            .unwrap_or_else(|| panic!("case {:?}: no Gateway object in {:?}", row.case, fetched.body));
        assert!(
            !reported.contains_key("ActivationKey"),
            "case {:?}: the reported gateway must carry no ActivationKey member, got {reported:?}",
            row.case
        );
        assert!(
            reported
                .values()
                .all(|value| value.as_str() != Some(row.activation_key.as_str())),
            "case {:?}: no member of the reported gateway may repeat the activation key, \
             got {reported:?}",
            row.case
        );
    }

    // Every create is one record in the store, and the listing reports exactly them.
    assert_eq!(
        gateway_count(&engine),
        rows.len(),
        "the store must hold one gateway per accepted activation key, got {}",
        gateway_count(&engine)
    );
    let listed = call(&router, "ListGateways", &json!({})).await;
    assert_eq!(
        (
            listed.status,
            listed.gateway_arns().into_iter().collect::<BTreeSet<_>>()
        ),
        (200, arns.clone()),
        "ListGateways must report exactly the gateways the creates minted, got {} {:?}",
        listed.status,
        listed.body
    );
}

/// An activation key outside the modelled constraints is refused, and writes nothing.
#[tokio::test]
async fn an_activation_key_outside_the_modelled_constraints_is_refused() {
    let engine = build_engine();
    let router = build_router(&engine);

    for (case, key) in &refused_rows() {
        let refused = call(
            &router,
            "CreateGateway",
            &create_body("gw-malformed-key", key, &BTreeMap::new()),
        )
        .await;
        assert_eq!(
            (refused.status, refused.code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: an ActivationKey outside the modelled constraints must be \
             refused with 400 ValidationException, got {} {:?}",
            refused.status,
            refused.body
        );
        assert!(
            refused.str_at("/GatewayArn").is_none(),
            "case {case:?}: a refused create must hand back no GatewayArn, got {:?}",
            refused.body
        );
    }

    assert_eq!(
        gateway_count(&engine),
        0,
        "a refused create must leave the store empty, got {}",
        gateway_count(&engine)
    );
    let listed = call(&router, "ListGateways", &json!({})).await;
    assert_eq!(
        (listed.status, listed.gateway_arns()),
        (200, Vec::<String>::new()),
        "a refused CreateGateway must leave no gateway for ListGateways to report, got {:?}",
        listed.body
    );
}
