// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `ImportDocumentationParts`'s `failOnWarnings` does to an
//! import that has nothing to warn about.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. [`import_body`], [`import_parts`] and
//! [`listed_entities`] are the `documentation_parts` suite's own callers, reused here.
//!
//! The RULE under test, over a SET of inputs rather than one recorded call: `failOnWarnings`
//! "specif[ies] whether to rollback the documentation importation (true) or not (false)
//! WHEN A WARNING IS ENCOUNTERED", so for a body that earns none it decides nothing — the
//! import is carried out, reports no warnings and leaves the same documentation set
//! whichever way the flag and the `mode` are set — and an entry the import cannot STORE
//! refuses the whole body either way, because that is an error rather than a warning. The
//! rows differ along the entity types the body documents, the number of entries, the
//! `mode`, the flag, and the one entry that cannot be stored.
//!
//! What AWS WARNS about on an import it does carry out, and the sentence it words that
//! warning with, is not asserted here: that is an open report of its own
//! (`issues/things-that-do-not-work/20261027T101902Z-apigateway-2-causes-of-the-request-validator-issue-remain-open.md`),
//! and this suite deliberately pins only the half of the flag that is sourced.

use serde_json::{json, Value};

use crate::authorizer_lifecycle::lambda_uri;
use crate::documentation_parts::{import_body, import_parts, listed_entities};
use crate::rest_api_lifecycle::{create_api, Gateway};

/// A documentation entry for `location` describing it with one property.
fn entry(location: Value, description: &str) -> (Value, Value) {
    (location, json!({ "description": description }))
}

/// The warnings an answered import reported, as plain strings; an absent member is no
/// warnings, because the service omits an empty list.
fn warnings(body: &Value, case: &str) -> Vec<String> {
    match &body["warnings"] {
        Value::Null => Vec::new(),
        Value::Array(reported) => reported
            .iter()
            .map(|warning| {
                warning
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case}: expected a string warning, got {warning:?}")
                    })
                    .to_string()
            })
            .collect(),
        other => panic!("case {case}: expected warnings to be a list, got {other:?}"),
    }
}

/// One import body: what it documents, and the records the API must hold for every entry
/// of it to name one.
struct Row {
    case: &'static str,
    /// The models the API holds before the import, so a `MODEL` location names one.
    models: &'static [&'static str],
    /// The authorizers the API holds before the import, for an `AUTHORIZER` location.
    authorizers: &'static [&'static str],
    entries: Vec<(Value, Value)>,
}

/// The bodies every case runs over. The rows differ along the entity types a location
/// names, along how many entries one body declares, and along an entity documented twice —
/// the dimensions an import generalises over. Each row holds the records its locations
/// NAME: an entry naming a model or an authorizer the API does not hold is the warning
/// case, which is the open report's subject, not this suite's.
fn table() -> Vec<Row> {
    vec![
        Row {
            case: "one entry for the api itself",
            models: &[],
            authorizers: &[],
            entries: vec![entry(json!({ "type": "API" }), "the api")],
        },
        Row {
            case: "entries for a resource, a method and a response",
            models: &[],
            authorizers: &[],
            entries: vec![
                entry(json!({ "type": "RESOURCE", "path": "/" }), "the root"),
                entry(
                    json!({ "type": "METHOD", "path": "/pets", "method": "GET" }),
                    "list the pets",
                ),
                entry(
                    json!({ "type": "RESPONSE", "path": "/pets", "method": "GET", "statusCode": "200" }),
                    "the pets",
                ),
            ],
        },
        Row {
            case: "entries naming a model and an authorizer the API holds",
            models: &["Order"],
            authorizers: &["guard"],
            entries: vec![
                entry(json!({ "type": "MODEL", "name": "Order" }), "an order"),
                entry(json!({ "type": "AUTHORIZER", "name": "guard" }), "the guard"),
            ],
        },
        Row {
            case: "one entity documented twice",
            models: &["Order"],
            authorizers: &[],
            entries: vec![
                entry(json!({ "type": "MODEL", "name": "Order" }), "first"),
                entry(json!({ "type": "MODEL", "name": "Order" }), "second"),
            ],
        },
    ]
}

/// `CreateModel` of `name`, asserting the modelled 201.
async fn create_model(gateway: &Gateway, api: &str, name: &str) {
    let answer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/models"),
            Some(json!({
                "name": name,
                "contentType": "application/json",
                "schema": json!({ "type": "object" }).to_string(),
            })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case CreateModel({name}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// `CreateAuthorizer` of `name`, asserting the modelled 201.
async fn create_authorizer(gateway: &Gateway, api: &str, name: &str) {
    let answer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/authorizers"),
            Some(json!({
                "name": name,
                "type": "TOKEN",
                "identitySource": "method.request.header.Auth",
                "authorizerUri": lambda_uri(name),
            })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case CreateAuthorizer({name}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// The `(location, content map)` pairs of `entries`, as the documentation set holds them:
/// one per ENTITY, the last entry for an entity winning, which is what `merge` means.
fn documented(entries: &[(Value, Value)]) -> Vec<(Value, Value)> {
    let mut held: Vec<(Value, Value)> = Vec::new();
    for (location, properties) in entries {
        match held
            .iter_mut()
            .find(|(documented, _)| documented == location)
        {
            Some((_, existing)) => *existing = properties.clone(),
            None => held.push((location.clone(), properties.clone())),
        }
    }
    held
}

#[tokio::test]
async fn fail_on_warnings_decides_nothing_for_a_body_that_warns_about_nothing() {
    for row in table() {
        // Every combination of the flag and the documented `mode` values: the outcome is
        // the same import, because nothing in these bodies warns.
        for fail_on_warnings in [false, true] {
            for mode in [None, Some("merge"), Some("overwrite")] {
                let case = format!(
                    "{} (failonwarnings {fail_on_warnings}, mode {mode:?})",
                    row.case
                );
                let gateway = Gateway::new();
                let (api, _root) = create_api(&gateway, "documentation-import-flag").await;
                // The records this row's locations NAME, so every entry of the body names
                // an entity the API holds and the body warns about nothing.
                for model in row.models {
                    create_model(&gateway, &api, model).await;
                }
                for authorizer in row.authorizers {
                    create_authorizer(&gateway, &api, authorizer).await;
                }

                let answer = import_parts(
                    &gateway,
                    &api,
                    mode,
                    fail_on_warnings,
                    &import_body(&row.entries),
                )
                .await;
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected the import to be carried out, got {} with {:?}",
                    answer.status, answer.body
                );
                let reported = warnings(&answer.body, &case);
                assert!(
                    reported.is_empty(),
                    "case {case}: expected no warnings, got {reported:?}"
                );
                let expected = documented(&row.entries);
                let stored = listed_entities(&gateway, &api, &case).await;
                assert_eq!(
                    stored, expected,
                    "case {case}: expected the imported documentation set {expected:?}, got \
                     {stored:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_entry_the_import_cannot_store_refuses_the_whole_body_either_way() {
    // An ERROR is not a warning: the flag does not decide it, so the same body is refused
    // with the modelled 400 whichever way the flag and the mode are set, and the
    // documentation set a previous import left is untouched.
    let unstorable: &[(&str, Value)] = &[
        (
            "a location type the model does not declare",
            json!({ "type": "NOT_AN_ENTITY" }),
        ),
        (
            "a location member the entity type does not make valid",
            json!({ "type": "API", "path": "/pets" }),
        ),
        (
            "a status code outside the declared pattern",
            json!({ "type": "RESPONSE", "path": "/", "method": "*", "statusCode": "600" }),
        ),
    ];
    for (case, location) in unstorable {
        for fail_on_warnings in [false, true] {
            for mode in [None, Some("merge"), Some("overwrite")] {
                let case = format!("{case} (failonwarnings {fail_on_warnings}, mode {mode:?})");
                let gateway = Gateway::new();
                let (api, _root) = create_api(&gateway, "documentation-import-errors").await;
                let standing = entry(json!({ "type": "API" }), "the api");
                let seeded = import_parts(
                    &gateway,
                    &api,
                    None,
                    false,
                    &import_body(std::slice::from_ref(&standing)),
                )
                .await;
                assert_eq!(
                    seeded.status, 200,
                    "case {case}: expected the seeded import to be carried out, got {} with \
                     {:?}",
                    seeded.status, seeded.body
                );

                let body = import_body(&[
                    entry(json!({ "type": "RESOURCE", "path": "/" }), "the root"),
                    entry(location.clone(), "the unstorable entry"),
                ]);
                let answer = import_parts(&gateway, &api, mode, fail_on_warnings, &body).await;
                assert_eq!(
                    answer.status, 400,
                    "case {case}: expected 400, got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error.as_deref(),
                    Some("BadRequestException"),
                    "case {case}: expected BadRequestException, got {:?}",
                    answer.error
                );
                let after = listed_entities(&gateway, &api, &case).await;
                assert_eq!(
                    after,
                    vec![standing.clone()],
                    "case {case}: expected the refused import to change nothing, got {after:?}"
                );
            }
        }
    }
}
