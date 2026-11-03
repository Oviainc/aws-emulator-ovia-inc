// SPDX-License-Identifier: CC0-1.0

//! What `EnableControl` and `UpdateEnabledControl` ACCEPT, end to end over HTTP.
//!
//! Drives the calls through the same
//! [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter) the production
//! emulator dispatches with, so what these tests exercise is the wire behaviour a client
//! sees — status codes, error codes and JSON bodies — not the handlers' Rust signatures.
//!
//! The rule under test, asserted over TABLES of inputs rather than one case: an enablement
//! is accepted only when it names a control of the service's catalog that a caller may
//! enable, applied to an AWS Organizations organizational unit, configured only by
//! parameters that control declares. Every other request is refused — 404
//! `ResourceNotFoundException` for a control ARN naming no catalog control, 400
//! `ValidationException` for everything else — and persists nothing, so neither the store
//! nor the enablement quota is touched by a refusal.
//!
//! The controls, which of them a caller may enable and the parameter keys each declares are
//! read off `state::CONTROL_CATALOG` — the catalog `EnableControl` resolves a
//! `controlIdentifier` against — so every case covers entries beyond the one an issue
//! happened to cite, and no row is written out by hand against a value this emulator could
//! drift from. A control is named by either ARN form AWS publishes for it, so the accepting
//! side drives both.

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    CatalogControl, CONTROL_CATALOG,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::enabled_control_lifecycle::catalog_arn;
use crate::landing_zone_lifecycle::{call, emulator, past_the_settle_span, set_up_account, Answer};

/// The AWS Control Tower ARN of a control, whose resource segment is the control's name.
fn control_arn(name: &str) -> String {
    format!("arn:aws:controltower:us-west-2::control/{name}")
}

/// An organizational unit ARN, in the form the AWS Organizations documentation publishes.
/// `suffix` keeps the rows of one case on distinct targets.
fn ou(suffix: &str) -> String {
    format!("arn:aws:organizations::123456789012:ou/o-exampleorg/ou-cl22-{suffix}")
}

/// POST `/enable-control`, omitting `parameters` when the caller does.
async fn enable(
    router: &MultiServiceRouter,
    control_identifier: &str,
    target_identifier: &str,
    parameters: Option<serde_json::Value>,
) -> Answer {
    let mut body = serde_json::json!({
        "controlIdentifier": control_identifier,
        "targetIdentifier": target_identifier,
    });
    if let Some(parameters) = parameters {
        body["parameters"] = parameters;
    }
    call(router, "/enable-control", body).await
}

/// POST `/update-enabled-control`. `parameters` is a required member, so every call
/// carries a (possibly empty) list.
async fn update(
    router: &MultiServiceRouter,
    enabled_control_identifier: &str,
    parameters: serde_json::Value,
) -> Answer {
    call(
        router,
        "/update-enabled-control",
        serde_json::json!({
            "enabledControlIdentifier": enabled_control_identifier,
            "parameters": parameters,
        }),
    )
    .await
}

/// POST `/get-enabled-control` and hand back its `enabledControlDetails` member.
async fn enabled_control_details(
    router: &MultiServiceRouter,
    identifier: &str,
) -> (Answer, serde_json::Value) {
    let answer = call(
        router,
        "/get-enabled-control",
        serde_json::json!({ "enabledControlIdentifier": identifier }),
    )
    .await;
    let details = answer.get("enabledControlDetails").clone();
    (answer, details)
}

/// A member of a JSON object, as a `&str`.
fn member<'a>(value: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    value.get(name).and_then(serde_json::Value::as_str)
}

/// The configured `parameters` of an `EnabledControlDetails`, as a list — an absent member
/// reads as the empty list, which is what "nothing is configured" means.
fn configured_parameters(details: &serde_json::Value) -> serde_json::Value {
    details
        .get("parameters")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]))
}

/// A fresh emulator with the account set up, which is what lets [`listed`] report an empty
/// store: Control Tower serves the enablement reads for an account it has been set up in,
/// and these cases are about what `EnableControl` accepts rather than about that
/// precondition.
async fn emulator_for_enablement() -> MultiServiceRouter {
    let router = emulator();
    set_up_account(&router).await;
    router
}

/// Every enablement `ListEnabledControls` reports, so a case can assert a refusal wrote
/// nothing.
async fn listed(router: &MultiServiceRouter) -> Vec<serde_json::Value> {
    let answer = call(router, "/list-enabled-controls", serde_json::json!({})).await;
    assert_eq!(
        answer.status, 200,
        "list-enabled-controls: expected 200, got {} body {:?}",
        answer.status, answer.body
    );
    answer
        .get("enabledControls")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Assert that `refused` is the status and error code the rule names, and that it minted
/// nothing: a refusal reports neither an `EnabledControl` ARN nor an operation.
fn assert_refused(case: &str, refused: &Answer, status: u16, code: &str) {
    assert_eq!(
        (refused.status, refused.error_code.as_deref()),
        (status, Some(code)),
        "case {case:?}: expected {status} {code}, got {:?} body {:?}",
        (refused.status, refused.error_code.as_deref()),
        refused.body
    );
    assert!(
        refused.body.get("arn").is_none() && refused.body.get("operationIdentifier").is_none(),
        "case {case:?}: a refused enablement must mint nothing, got {:?}",
        refused.body
    );
}

/// The catalog entries a caller may enable.
fn enablable() -> Vec<&'static CatalogControl> {
    CONTROL_CATALOG
        .iter()
        .filter(|control| control.enablable)
        .collect()
}

/// The catalog entries a caller may NOT enable.
fn mandatory() -> Vec<&'static CatalogControl> {
    CONTROL_CATALOG
        .iter()
        .filter(|control| !control.enablable)
        .collect()
}

/// A control name the catalog does NOT hold, built from `seed` by replacing its last
/// character — so the rows track the catalog rather than hard-coding a value that could one
/// day become a real entry.
fn uncatalogued_name(seed: &str) -> String {
    let replacement = if seed.ends_with('X') { 'Y' } else { 'X' };
    let mut name: String = seed.chars().take(seed.chars().count() - 1).collect();
    name.push(replacement);
    assert!(
        !CONTROL_CATALOG.iter().any(|control| control.name == name),
        "the uncatalogued name {name:?} must not be a catalog entry"
    );
    name
}

/// A 26-character control catalog id the catalog does NOT hold, built from `seed` the same
/// way.
fn uncatalogued_catalog_id(seed: &str) -> String {
    let replacement = if seed.ends_with('z') { 'y' } else { 'z' };
    let mut id: String = seed.chars().take(25).collect();
    id.push(replacement);
    assert!(
        !CONTROL_CATALOG
            .iter()
            .any(|control| control.catalog_id == id),
        "the uncatalogued id {id:?} must not be a catalog entry"
    );
    id
}

/// A parameter key no control declares, built from `seed` by suffixing it.
fn undeclared_key(seed: &str) -> String {
    let key = format!("{seed}NotDeclared");
    assert!(
        !CONTROL_CATALOG
            .iter()
            .any(|control| control.declares_parameter(&key)),
        "the undeclared key {key:?} must not be a declared parameter"
    );
    key
}

/// RULE (the accepting side): every control the catalog publishes as a caller's to enable
/// enables on an organizational unit, under either ARN form AWS names it by, with the
/// parameters it declares — and what was accepted is what a later read reports.
#[tokio::test]
async fn every_enablable_catalog_control_enables_under_either_arn_form() {
    let router = emulator();

    // One row per enablable control per identifier form, each with the parameters that
    // control declares (an empty list for a control that declares none), on its own target.
    let mut accepted: Vec<(String, String, String, String, serde_json::Value)> = Vec::new();
    for (index, control) in enablable().into_iter().enumerate() {
        for (form_index, (form, identifier)) in [
            ("control ARN", control_arn(control.name)),
            ("control catalog ARN", catalog_arn(control.catalog_id)),
        ]
        .into_iter()
        .enumerate()
        {
            let case = format!("{} by its {form}", control.name);
            let target = ou(&format!("c{index}{form_index}"));
            // The declared keys, each carrying a value of a different document kind, so
            // the accepted set spans the `Document` a value may be.
            let parameters = serde_json::Value::Array(
                control
                    .parameters
                    .iter()
                    .enumerate()
                    .map(|(position, key)| {
                        let value = match position % 3 {
                            0 => serde_json::json!(["us-west-2", "eu-west-1"]),
                            1 => serde_json::json!("arn:aws:iam::123456789012:role/r"),
                            _ => serde_json::json!({ "inner": { "deep": [1, 2, 3] } }),
                        };
                        serde_json::json!({ "key": key, "value": value })
                    })
                    .collect(),
            );

            let answer = enable(&router, &identifier, &target, Some(parameters.clone())).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200, got {} body {:?}",
                answer.status, answer.body
            );
            accepted.push((case, answer.string("arn"), identifier, target, parameters));
        }
    }
    assert!(
        accepted.len() >= 2 * enablable().len(),
        "expected one row per enablable catalog control per ARN form, got {}",
        accepted.len()
    );

    // The enables are asynchronous; the read below is about what was PERSISTED, so wait for
    // the clock to make them due first.
    tokio::time::sleep(past_the_settle_span()).await;

    for (case, arn, identifier, target, parameters) in accepted {
        let (read, details) = enabled_control_details(&router, &arn).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: get expected 200, got {} body {:?}",
            read.status, read.body
        );
        assert_eq!(
            (
                member(&details, "arn"),
                member(&details, "controlIdentifier"),
                member(&details, "targetIdentifier"),
            ),
            (
                Some(arn.as_str()),
                Some(identifier.as_str()),
                Some(target.as_str()),
            ),
            "case {case:?}: expected the enablement the call accepted, got {details:?}"
        );
        assert_eq!(
            configured_parameters(&details),
            parameters,
            "case {case:?}: expected the declared parameters it was given, got {details:?}"
        );
    }
}

/// RULE: a `controlIdentifier` that names no control of the catalog enables nothing — a
/// control ARN whose control the catalog does not hold is 404 `ResourceNotFoundException`,
/// and a value that is not a control reference at all is 400 `ValidationException` — and
/// either way the store is untouched.
#[tokio::test]
async fn a_control_identifier_naming_no_catalog_control_is_refused() {
    let first = CONTROL_CATALOG
        .first()
        .expect("the catalog publishes at least one control");

    let cases: Vec<(&str, String, u16, &str)> = vec![
        (
            "a control ARN whose name the catalog does not hold",
            control_arn(&uncatalogued_name(first.name)),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a control catalog ARN whose id the catalog does not hold",
            catalog_arn(&uncatalogued_catalog_id(first.catalog_id)),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a control ARN in another partition and region, naming no catalog control",
            format!(
                "arn:aws-us-gov:controltower:us-gov-west-1::control/{}",
                uncatalogued_name(first.name)
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an ARN of another service",
            "arn:aws:s3:::my-example-bucket-name".to_string(),
            400,
            "ValidationException",
        ),
        (
            "an IAM role ARN",
            "arn:aws:iam::123456789012:role/not-a-control".to_string(),
            400,
            "ValidationException",
        ),
        (
            "a controltower ARN naming another kind of resource",
            "arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2".to_string(),
            400,
            "ValidationException",
        ),
        (
            "a control ARN naming an account, which no control belongs to",
            format!(
                "arn:aws:controltower:us-west-2:123456789012:control/{}",
                first.name
            ),
            400,
            "ValidationException",
        ),
        (
            "a control catalog ARN naming a region, which that global catalog does not",
            format!("arn:aws:controlcatalog:us-west-2::control/{}", first.catalog_id),
            400,
            "ValidationException",
        ),
        (
            "a control ARN whose resource is not of the published control-name form",
            control_arn("aws-gr-encrypted-volumes"),
            400,
            "ValidationException",
        ),
        (
            "a control catalog ARN whose id is not of the published length",
            catalog_arn("5kvme4m5d2b4d7dcf0fqkdmx"),
            400,
            "ValidationException",
        ),
    ];

    for (case, identifier, status, code) in cases {
        let router = emulator_for_enablement().await;
        let refused = enable(&router, &identifier, &ou("nf"), None).await;
        assert_refused(case, &refused, status, code);
        assert_eq!(
            listed(&router).await,
            Vec::<serde_json::Value>::new(),
            "case {case:?}: a refused enablement must persist nothing"
        );
    }
}

/// RULE: only part of the catalog is a caller's to enable — the mandatory controls AWS
/// Control Tower applies itself are named by a `controlIdentifier` but are refused with 400
/// `ValidationException`, under either ARN form, and persist nothing.
#[tokio::test]
async fn a_mandatory_catalog_control_is_not_a_callers_to_enable() {
    let mandatory = mandatory();
    assert!(
        !mandatory.is_empty(),
        "the catalog must publish controls a caller may not enable"
    );

    let router = emulator_for_enablement().await;
    for (index, control) in mandatory.into_iter().enumerate() {
        for (form_index, (form, identifier)) in [
            ("control ARN", control_arn(control.name)),
            ("control catalog ARN", catalog_arn(control.catalog_id)),
        ]
        .into_iter()
        .enumerate()
        {
            let case = format!("{} by its {form}", control.name);
            let refused = enable(
                &router,
                &identifier,
                &ou(&format!("m{index}{form_index}")),
                None,
            )
            .await;
            assert_refused(&case, &refused, 400, "ValidationException");
        }
    }
    assert_eq!(
        listed(&router).await,
        Vec::<serde_json::Value>::new(),
        "a refused enablement must persist nothing"
    );
}

/// RULE: `targetIdentifier` is "the ARN of the organizational unit", so a value that is not
/// an AWS Organizations organizational unit ARN is refused with 400 `ValidationException`
/// and persists nothing — whatever other resource it names.
#[tokio::test]
async fn a_target_that_is_not_an_organizational_unit_is_refused() {
    let enablable = control_arn(
        enablable()
            .first()
            .expect("the catalog publishes a control a caller may enable")
            .name,
    );

    let cases: Vec<(&str, String)> = vec![
        (
            "an Organizations ACCOUNT, which is a target a listing narrows by but not one a \
             control is enabled on",
            "arn:aws:organizations::123456789012:account/o-exampleorg/123456789012".to_string(),
        ),
        (
            "an Organizations ROOT",
            "arn:aws:organizations::123456789012:root/o-exampleorg/r-abcd".to_string(),
        ),
        (
            "an Organizations organization",
            "arn:aws:organizations::123456789012:organization/o-exampleorg".to_string(),
        ),
        (
            "an IAM role",
            "arn:aws:iam::123456789012:role/not-a-target".to_string(),
        ),
        (
            "an S3 bucket",
            "arn:aws:s3:::my-example-bucket-name".to_string(),
        ),
        (
            "an organizational unit ARN carrying a region, which an Organizations ARN does not",
            "arn:aws:organizations:us-west-2:123456789012:ou/o-exampleorg/ou-cl22-1a".to_string(),
        ),
        (
            "an organizational unit ARN whose account is not a 12-digit account id",
            "arn:aws:organizations::12345:ou/o-exampleorg/ou-cl22-1a".to_string(),
        ),
        (
            "an organizational unit ARN with no organization in its resource",
            "arn:aws:organizations::123456789012:ou/ou-cl22-1a".to_string(),
        ),
        (
            "an organizational unit ARN whose unit id is not of the published form",
            "arn:aws:organizations::123456789012:ou/o-exampleorg/unit-cl22-1a".to_string(),
        ),
        (
            "the enabled-control ARN form rather than a target",
            format!(
                "arn:aws:controltower:us-west-2:{}:enabledcontrol/00000000-0000-4000-8000-000000000000",
                engine::emulator_account_id()
            ),
        ),
    ];

    for (case, target) in cases {
        let router = emulator_for_enablement().await;
        let refused = enable(&router, &enablable, &target, None).await;
        assert_refused(case, &refused, 400, "ValidationException");
        assert_eq!(
            listed(&router).await,
            Vec::<serde_json::Value>::new(),
            "case {case:?}: a refused enablement must persist nothing"
        );
    }
}

/// RULE: a `parameters` list configures the control the request names, so a key that
/// control does not declare — and any parameter at all for a control that declares none —
/// is refused with 400 `ValidationException` and persists nothing, while a subset of the
/// declared keys is accepted.
#[tokio::test]
async fn parameters_the_named_control_does_not_declare_are_refused() {
    // Every enablable catalog entry: the ones that declare nothing are refused any
    // parameter, and the ones that declare keys are refused the keys outside their set.
    for (index, control) in enablable().into_iter().enumerate() {
        let identifier = control_arn(control.name);
        let router = emulator_for_enablement().await;

        let undeclared = undeclared_key(control.name.trim_start_matches("AWS-GR_"));
        let case = format!("{}: a key it does not declare", control.name);
        let refused = enable(
            &router,
            &identifier,
            &ou(&format!("p{index}a")),
            Some(serde_json::json!([{ "key": undeclared, "value": true }])),
        )
        .await;
        assert_refused(&case, &refused, 400, "ValidationException");
        assert_eq!(
            listed(&router).await,
            Vec::<serde_json::Value>::new(),
            "case {case:?}: a refused enablement must persist nothing"
        );

        if let Some(declared) = control.parameters.first() {
            // A declared key differing in case names no parameter either: a key is matched
            // exactly.
            let case = format!("{}: a declared key in another case", control.name);
            let refused = enable(
                &router,
                &identifier,
                &ou(&format!("p{index}b")),
                Some(serde_json::json!([{ "key": declared.to_lowercase(), "value": true }])),
            )
            .await;
            assert_refused(&case, &refused, 400, "ValidationException");

            // One declared key twice is not one configuration.
            let case = format!("{}: a declared key given twice", control.name);
            let refused = enable(
                &router,
                &identifier,
                &ou(&format!("p{index}c")),
                Some(serde_json::json!([
                    { "key": declared, "value": "first" },
                    { "key": declared, "value": "second" }
                ])),
            )
            .await;
            assert_refused(&case, &refused, 400, "ValidationException");

            // A subset of the declared keys configures the control, so the refusals above
            // are about the KEYS rather than about parameters as such.
            let case = format!("{}: one declared key", control.name);
            let accepted = enable(
                &router,
                &identifier,
                &ou(&format!("p{index}d")),
                Some(serde_json::json!([{ "key": declared, "value": true }])),
            )
            .await;
            assert_eq!(
                accepted.status, 200,
                "case {case:?}: expected 200, got {} body {:?}",
                accepted.status, accepted.body
            );
        }

        assert_eq!(
            listed(&router).await.len(),
            usize::from(!control.parameters.is_empty()),
            "case {:?}: only the accepted enablement may be stored",
            control.name
        );
    }
}

/// RULE: the parameter rule is the control's, wherever a configuration is supplied, so
/// `UpdateEnabledControl` refuses a key the control the enablement applies does not declare
/// — with 400 `ValidationException`, leaving the configured parameters alone — and accepts
/// one it does.
#[tokio::test]
async fn an_update_naming_a_parameter_the_control_does_not_declare_is_refused() {
    let parameterized = CONTROL_CATALOG
        .iter()
        .find(|control| control.enablable && !control.parameters.is_empty())
        .expect("the catalog publishes an enablable control with parameters");
    let declared = parameterized.parameters[0];

    let router = emulator();
    let enabled = enable(
        &router,
        &control_arn(parameterized.name),
        &ou("ud"),
        Some(serde_json::json!([{ "key": declared, "value": "configured" }])),
    )
    .await;
    assert_eq!(
        enabled.status, 200,
        "the enablement expected 200, got {} body {:?}",
        enabled.status, enabled.body
    );
    let arn = enabled.string("arn");
    tokio::time::sleep(past_the_settle_span()).await;

    let configured = serde_json::json!([{ "key": declared, "value": "configured" }]);
    let undeclared = undeclared_key("Update");
    for (case, parameters) in [
        (
            "a key the control does not declare",
            serde_json::json!([{ "key": undeclared, "value": true }]),
        ),
        (
            "a declared key in another case",
            serde_json::json!([{ "key": declared.to_lowercase(), "value": true }]),
        ),
        (
            "a declared key beside an undeclared one",
            serde_json::json!([
                { "key": declared, "value": "changed" },
                { "key": undeclared, "value": true }
            ]),
        ),
    ] {
        let refused = update(&router, &arn, parameters).await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected 400 ValidationException, got {:?} body {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
        let (_, details) = enabled_control_details(&router, &arn).await;
        assert_eq!(
            configured_parameters(&details),
            configured,
            "case {case:?}: a refused update must write nothing, got {details:?}"
        );
    }

    // The same call with a declared key is accepted, so the refusals are about the key
    // rather than about updating at all.
    let accepted = update(
        &router,
        &arn,
        serde_json::json!([{ "key": declared, "value": "changed" }]),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "an update naming a declared key expected 200, got {} body {:?}",
        accepted.status, accepted.body
    );
    tokio::time::sleep(past_the_settle_span()).await;
    let (_, details) = enabled_control_details(&router, &arn).await;
    assert_eq!(
        configured_parameters(&details),
        serde_json::json!([{ "key": declared, "value": "changed" }]),
        "expected the configuration the accepted update carried, got {details:?}"
    );
}
