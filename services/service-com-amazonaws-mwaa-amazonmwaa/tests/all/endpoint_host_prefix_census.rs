// SPDX-License-Identifier: CC0-1.0

//! WHICH endpoint the model hosts each operation of this service on.
//!
//! The rule: the endpoint census the crate holds (`service::endpoints`) is the
//! classification the MODEL declares — for every operation the shipped Smithy model binds,
//! the host prefix the crate records is the one that operation's `@endpoint` trait carries,
//! and the set of classes the crate knows is the set of prefixes the model uses (three of
//! them, not two). The model is READ here rather than restated, so the census cannot drift
//! from it, and no operation is classified by the absence of another class.
//!
//! What that classification DECIDES — which layer refuses an environment name it cannot
//! resolve, and so which sentence the refusal carries — is asserted end to end over HTTP by
//! `tests/all/env_endpoint_not_found_wording.rs`, which reads the classification from this
//! same census.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use service_com_amazonaws_mwaa_amazonmwaa::service::endpoints::{
    self, Endpoint, OPERATION_ENDPOINTS,
};

/// The Smithy model this crate is built from, as shipped in the crate.
const MODEL: &str = include_str!("../../smithy-model/com-amazonaws-mwaa-amazonmwaa.json");

/// The trait that carries an operation's endpoint host prefix, by its shape id in the JSON
/// AST.
const ENDPOINT_TRAIT: &str = "smithy.api#endpoint";

/// Every operation's `@endpoint` host prefix, as the shipped model declares it: the short
/// shape name of each operation the model binds, against the `hostPrefix` of its endpoint
/// trait — `None` for an operation the model gives no such trait.
fn host_prefixes_in_the_model() -> BTreeMap<String, Option<String>> {
    let document: Value = serde_json::from_str(MODEL).expect("the shipped model parses");
    let shapes = document
        .get("shapes")
        .and_then(Value::as_object)
        .expect("the model holds shapes");
    shapes
        .iter()
        .filter(|(_, shape)| shape.get("type").and_then(Value::as_str) == Some("operation"))
        .map(|(id, shape)| {
            let name = id.rsplit('#').next().unwrap_or(id).to_string();
            let prefix = shape
                .pointer(&format!("/traits/{ENDPOINT_TRAIT}/hostPrefix"))
                .and_then(Value::as_str)
                .map(str::to_string);
            (name, prefix)
        })
        .collect()
}

#[test]
fn the_crates_endpoint_census_is_the_classification_the_model_declares() {
    let modelled = host_prefixes_in_the_model();
    let recorded: BTreeMap<String, Option<String>> = OPERATION_ENDPOINTS
        .iter()
        .map(|(name, endpoint)| {
            (
                (*name).to_string(),
                Some(endpoint.host_prefix().to_string()),
            )
        })
        .collect();

    assert_eq!(
        recorded.keys().collect::<Vec<_>>(),
        modelled.keys().collect::<Vec<_>>(),
        "the census must hold exactly the operations the model binds, got {:?}",
        recorded.keys().collect::<Vec<_>>()
    );

    for (operation, prefix) in &modelled {
        let answered = endpoints::endpoint_of(operation).map(Endpoint::host_prefix);
        assert_eq!(
            answered,
            prefix.as_deref(),
            "case {operation:?}: the model declares the host prefix {prefix:?}, got {answered:?}"
        );
    }

    // The classes themselves: the crate must know every prefix the model uses, so no
    // operation is classified by the absence of another class.
    let modelled_classes: BTreeSet<&str> = modelled.values().flatten().map(String::as_str).collect();
    let recorded_classes: BTreeSet<&str> = recorded.values().flatten().map(String::as_str).collect();
    assert_eq!(
        recorded_classes, modelled_classes,
        "the census must know every endpoint class the model declares, got {recorded_classes:?}"
    );

    // A name this model binds no operation of belongs to no class, so a routing key the
    // service does not serve cannot fall into one by default.
    assert_eq!(
        endpoints::endpoint_of("NoSuchOperation"),
        None,
        "an operation this model does not bind must resolve to no endpoint, got {:?}",
        endpoints::endpoint_of("NoSuchOperation")
    );
}
