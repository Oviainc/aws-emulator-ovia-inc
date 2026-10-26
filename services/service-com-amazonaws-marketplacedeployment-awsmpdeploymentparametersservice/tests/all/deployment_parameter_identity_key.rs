// SPDX-License-Identifier: CC0-1.0

//! A deployment parameter's IDENTITY is one value, used by every judgement that
//! asks which parameter a `PutDeploymentParameter` is about.
//!
//! The rule under test: a deployment parameter is keyed on
//! `(catalog, productId, agreementId, deploymentParameter.name)` — the model's doc
//! for `DeploymentParameterInput$name` says the name "is the identifier on which
//! deployment parameters are keyed for a given buyer and product. If this name
//! matches an existing deployment parameter, this request will update the existing
//! resource."
//! (`smithy_query(query="doc",shape="DeploymentParameterInput",member="name")`).
//! Two requests whose whole key is equal are about ONE resource and two whose key
//! differs in any one component are about two, and BOTH of the handler's identity
//! judgements answer from that one key:
//!
//! * the `@idempotencyToken` branch — a bound token aimed at the key it wrote
//!   REPLAYS that write, and the same token aimed at any other key is the modelled
//!   `ConflictException` (HTTP 409);
//! * the identity-index lookup — a fresh token for a key already stored UPDATES it
//!   in place and reports the same `deploymentParameterId`/`resourceArn`, and a key
//!   not stored mints a new resource.
//!
//! Each row below drives both judgements for one key that differs from the base key
//! along a different component (or none, or two at once), so a key the two judgements
//! disagreed about would show up as a replay turned into a conflict, or a conflict
//! turned into a replay.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 routing and serialization, the modelled-constraint walk and the
//! handlers — through the harness the `deployment_parameters` suite owns.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::state::service_state;

use crate::deployment_parameters::{
    emulator, error_code, is_parameter_id, provision, put, stored_parameters, CATALOG,
};

/// The base identity every row is compared against.
const BASE_PRODUCT: &str = "prod-base";
const BASE_AGREEMENT: &str = "agmt-base";
const BASE_NAME: &str = "BaseParameter";

/// One row: an identity, and whether it is the base identity itself.
struct Identity {
    case: &'static str,
    product_id: &'static str,
    agreement_id: &'static str,
    /// A `DeploymentParameterName` (`^[a-zA-Z0-9/_+=.@-]+$`, `length 1..400`).
    name: &'static str,
}

impl Identity {
    /// Whether this row names the base identity — i.e. whether the whole key is
    /// equal, which is the rule's own test for "the same parameter".
    fn is_base(&self) -> bool {
        self.product_id == BASE_PRODUCT
            && self.agreement_id == BASE_AGREEMENT
            && self.name == BASE_NAME
    }

    fn body(&self, secret: &str, client_token: &str) -> Value {
        json!({
            "agreementId": self.agreement_id,
            "deploymentParameter": { "name": self.name, "secretString": secret },
            "clientToken": client_token,
        })
    }
}

/// The identity table. The rows differ along everything the identity generalises
/// over: the base key itself, each single component changed on its own, two
/// components changed at once, and a name differing from the base only in case
/// (the key is the name's exact text, not a case-folded form).
///
/// The catalog is NOT a row field: the model documents `catalog` as a fixed value
/// (`smithy_query(query="doc",shape="PutDeploymentParameterRequest",member="catalog")`),
/// so every row names [`CATALOG`] and the rule about any other catalog belongs to
/// the `catalog_allowed_values` suite.
fn identity_table() -> Vec<Identity> {
    vec![
        Identity {
            case: "the base identity itself",
            product_id: BASE_PRODUCT,
            agreement_id: BASE_AGREEMENT,
            name: BASE_NAME,
        },
        Identity {
            case: "another parameter name",
            product_id: BASE_PRODUCT,
            agreement_id: BASE_AGREEMENT,
            name: "OtherParameter",
        },
        Identity {
            case: "another agreement",
            product_id: BASE_PRODUCT,
            agreement_id: "agmt-other",
            name: BASE_NAME,
        },
        Identity {
            case: "another product",
            product_id: "prod-other",
            agreement_id: BASE_AGREEMENT,
            name: BASE_NAME,
        },
        Identity {
            case: "another product and another name",
            product_id: "prod-other",
            agreement_id: BASE_AGREEMENT,
            name: "Other/Parameter_1",
        },
        Identity {
            case: "a name differing from the base only in case",
            product_id: BASE_PRODUCT,
            agreement_id: BASE_AGREEMENT,
            name: "baseparameter",
        },
    ]
}

/// The i-th distinct `clientToken`: 32 characters of `^[a-zA-Z0-9/_+=.:@-]+$`, the
/// modelled `ClientToken` minimum length.
fn token(i: usize) -> String {
    format!("token-{i:026}")
}

/// The `deploymentParameterId` of a successful answer.
fn minted_id(answer: &Value) -> String {
    answer
        .get("deploymentParameterId")
        .and_then(Value::as_str)
        .unwrap_or("<no deploymentParameterId>")
        .to_string()
}

#[tokio::test]
async fn one_identity_decides_both_the_token_replay_and_the_update_in_place() {
    let (router, engine) = emulator();
    // Every identity the table names is a target the account holds, so what the rows
    // observe is the identity rule and not an unresolved agreement.
    provision(&engine, CATALOG, BASE_AGREEMENT, &["prod-base", "prod-other"]);
    provision(&engine, CATALOG, "agmt-other", &["prod-base", "prod-other"]);

    // The write every row's key is compared against, and the token bound to it.
    let base_token = token(0);
    let (status, base) = put(
        &router,
        CATALOG,
        BASE_PRODUCT,
        &json!({
            "agreementId": BASE_AGREEMENT,
            "deploymentParameter": { "name": BASE_NAME, "secretString": "base" },
            "clientToken": base_token,
        }),
    )
    .await;
    assert_eq!(status, 200, "the base Put must succeed, got {status} {base:?}");
    let base_id = minted_id(&base);
    assert!(
        is_parameter_id(&base_id),
        "case \"base\": expected a modelled dp- identifier, got {base_id:?}"
    );

    let mut ids_by_identity: BTreeSet<String> = BTreeSet::new();
    ids_by_identity.insert(base_id.clone());

    for (row, identity) in identity_table().iter().enumerate() {
        let case = identity.case;
        let own_token = token(row * 3 + 1);

        // The identity-index judgement: a key already stored is an update in place,
        // a key not stored mints a new resource.
        let (status, written) = put(
            &router,
            CATALOG,
            identity.product_id,
            &identity.body("first", &own_token),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the write must succeed, got {status} {written:?}"
        );
        let written_id = minted_id(&written);
        assert!(
            is_parameter_id(&written_id),
            "case {case:?}: expected a modelled dp- identifier, got {written_id:?}"
        );
        if identity.is_base() {
            assert_eq!(
                written_id, base_id,
                "case {case:?}: the base key must resolve the base resource, got {written_id:?}"
            );
        } else {
            assert_ne!(
                written_id, base_id,
                "case {case:?}: a key differing from the base must be its own resource, \
                 got {written_id:?}"
            );
        }
        ids_by_identity.insert(written_id.clone());

        // The `@idempotencyToken` judgement, same key: a replay of that very write,
        // answered from the stored record and not from this request's content.
        let (status, replayed) = put(
            &router,
            CATALOG,
            identity.product_id,
            &identity.body("rotated", &own_token),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the replay must succeed, got {status} {replayed:?}"
        );
        assert_eq!(
            replayed, written,
            "case {case:?}: a clientToken replay must answer the identical output, \
             got {replayed:?} against {written:?}"
        );

        // The `@idempotencyToken` judgement, the base's token aimed at this key: a
        // replay when the whole key is the base's, and the modelled conflict when any
        // component of it differs.
        let (status, answer) = put(
            &router,
            CATALOG,
            identity.product_id,
            &identity.body("via the base token", &base_token),
        )
        .await;
        if identity.is_base() {
            assert_eq!(
                status, 200,
                "case {case:?}: the base token on the base key must replay, got {status} {answer:?}"
            );
            assert_eq!(
                minted_id(&answer),
                base_id,
                "case {case:?}: the replay must report the bound resource, got {answer:?}"
            );
        } else {
            assert_eq!(
                status, 409,
                "case {case:?}: the base token on another key must conflict, \
                 got {status} {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {answer:?}"
            );
            assert_eq!(
                answer.get("resourceId").and_then(Value::as_str),
                Some(base_id.as_str()),
                "case {case:?}: the conflict must name the parameter the token is bound to, \
                 got {answer:?}"
            );
        }

        // A FRESH token for the same key is the ordinary update-in-place path: the
        // index resolves the record the first write of this row stored.
        let (status, updated) = put(
            &router,
            CATALOG,
            identity.product_id,
            &identity.body("updated", &token(row * 3 + 2)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a fresh token must succeed, got {status} {updated:?}"
        );
        assert_eq!(
            minted_id(&updated),
            written_id,
            "case {case:?}: a fresh token for the same key must update in place, got {updated:?}"
        );
    }

    // One resource per distinct key, and the index agrees with every record it
    // points at: the identity a record reports is the identity it is stored under.
    assert_eq!(
        stored_parameters(&engine),
        ids_by_identity.len(),
        "expected one stored parameter per distinct identity, got {} for {} identities",
        stored_parameters(&engine),
        ids_by_identity.len()
    );
    let state = service_state(&*engine).expect("the shared engine holds this service's store");
    let store = state.lock();
    for (resource_arn, record) in &store.parameters {
        assert_eq!(
            store.by_name.get(&record.key()),
            Some(resource_arn),
            "case {resource_arn:?}: the index must hold this record under the identity it \
             reports, got {:?}",
            store.by_name.get(&record.key())
        );
    }
    assert_eq!(
        store.by_name.len(),
        store.parameters.len(),
        "expected one index entry per record, got {} against {}",
        store.by_name.len(),
        store.parameters.len()
    );
}
