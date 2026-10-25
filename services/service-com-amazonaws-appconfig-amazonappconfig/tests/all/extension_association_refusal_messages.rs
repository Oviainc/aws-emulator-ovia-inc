// SPDX-License-Identifier: CC0-1.0

//! How AppConfig WORDS the two refusals an extension-association request can earn, over
//! the set of requests that earn them.
//!
//! Two rules are under test, each one level more general than any single request:
//!
//! 1. The `ResourceNotFoundException` for an `ExtensionAssociationId` that addresses no
//!    association names this resource kind with the single token AppConfig's own template
//!    carries for it (`ExtensionAssociation`, the one-word form the deployment strategy's
//!    template also uses), reports the account the lookup was scoped to and echoes no
//!    identifier at all — whichever operation reached the lookup and whatever well-formed
//!    id the request sent.
//! 2. The `BadRequestException` for a parameter-value map that omits a value an extension
//!    VERSION marks required names the omitted parameter, identifies the extension by its
//!    system-generated `Id` (never by the caller-chosen `Name`) and the version by its
//!    number, and states the condition — no value was supplied. Every operation that
//!    writes a `ParameterValueMap` words it the same way, and the refusal writes nothing.
//!
//! The tables span what those rules generalise over: the operation reaching the refusal
//! (the update and the create for rule 2, the update and the reader for rule 1), the length
//! and the character mix of the id that addresses nothing, which end of the catalogue the
//! extension comes from (AWS-authored, where the id IS the published name, and created by
//! this account, where the minted id and the chosen name differ), how many parameters the
//! version declares required, which of them the map omits, which version of the extension
//! the association binds, and whether the offending map is empty or carries the values the
//! version does not require.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::ops::types::ACTION_POINT_VALUES;
use service_com_amazonaws_appconfig_amazonappconfig::state::AWS_AUTHORED_EXTENSIONS;

use crate::appconfig_workflow::{account, create_application, is_modelled_id, router, Answer};
use crate::extension_associations::{associate, get_association};
use crate::extensions::{actions, create_extension, minted_id};
use crate::resource_tags::arn;
use crate::update_extension_association::update_association;

/// The error code of a refusal, without its namespace.
fn code(answer: &Answer) -> String {
    answer
        .error_code()
        .split('#')
        .next_back()
        .unwrap_or_default()
        .to_string()
}

/// The `Message` of a refusal.
fn message(answer: &Answer) -> String {
    answer.string("Message").unwrap_or_default()
}

/// The message AppConfig answers a lookup of an association no account holds with: the
/// kind as ONE token, and the account the lookup was scoped to.
fn expected_not_found() -> String {
    format!(
        "ExtensionAssociation could not be found for account {}.",
        account()
    )
}

/// The message AppConfig answers a parameter map that supplies no value for a required
/// parameter with. The fixed text carries TWO spaces after the parameter name and ends by
/// stating the condition; the variable parts are the omitted parameter, the extension's id
/// and the version number.
fn expected_required(missing: &str, extension_id: &str, version: i32) -> String {
    format!(
        "Parameter {missing}  is required for extension {extension_id} version {version}, \
         but a value has not been supplied."
    )
}

/// An extension this account created, carrying `parameters` at the version the create
/// opens. Answers the minted id, which is NOT the name the caller chose.
async fn created_extension(
    router: &MultiServiceRouter,
    name: &str,
    parameters: Value,
    latest_version_number: Option<i32>,
) -> String {
    let answer = create_extension(
        router,
        &json!({
            "Name": name,
            "Actions": actions(&[ACTION_POINT_VALUES[0]]),
            "Parameters": parameters,
        }),
        latest_version_number,
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case {name:?}: CreateExtension expected 201, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    minted_id(name, &answer.json())
}

/// A `ParameterMap` declaring every name in `required` required and every name in
/// `optional` optional.
fn declaration(required: &[&str], optional: &[&str]) -> Value {
    let mut map = Map::new();
    for name in required {
        map.insert(
            (*name).to_string(),
            json!({ "Required": true, "Dynamic": false }),
        );
    }
    for name in optional {
        map.insert(
            (*name).to_string(),
            json!({ "Required": false, "Dynamic": false }),
        );
    }
    Value::Object(map)
}

/// The AWS-authored extension that declares a required parameter, and that parameter's
/// name, read from the published catalogue rather than written out here.
fn authored_with_required_parameter() -> (&'static str, &'static str) {
    AWS_AUTHORED_EXTENSIONS
        .iter()
        .find_map(|authored| {
            authored
                .parameters
                .iter()
                .find(|parameter| parameter.required)
                .map(|parameter| (authored.name, parameter.name))
        })
        .expect("the published catalogue declares at least one required parameter")
}

/// Create one association and answer its id.
async fn create_association(
    router: &MultiServiceRouter,
    case: &str,
    extension: &str,
    version: i32,
    resource_arn: &str,
    parameters: &Value,
) -> String {
    let answer = associate(
        router,
        &json!({
            "ExtensionIdentifier": extension,
            "ExtensionVersionNumber": version,
            "ResourceIdentifier": resource_arn,
            "Parameters": parameters,
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case {case:?}: CreateExtensionAssociation expected 201, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .string("Id")
        .unwrap_or_else(|| panic!("case {case:?}: the create answered no Id: {}", answer.json()))
}

/// Every operation that fails the association lookup refuses it with this resource kind's
/// own one-token template, over ids that differ in length and character mix.
#[tokio::test]
async fn an_association_nothing_holds_is_refused_with_the_one_token_kind() {
    let router = router();
    let application = create_application(&router, "association-refusal-message").await;
    let resource_arn = arn(&format!("application/{application}"));
    let extension = created_extension(
        &router,
        "association-refusal-message-extension",
        declaration(&[], &["optional"]),
        None,
    )
    .await;
    let held = create_association(
        &router,
        "the fixture association",
        &extension,
        1,
        &resource_arn,
        &json!({ "optional": "held" }),
    )
    .await;

    // Ids that satisfy `Id`'s `^[a-z0-9]{4,7}$` (so each reaches the lookup rather than
    // being refused as malformed) and address no association: the shortest and the longest
    // the pattern allows, digits only, letters only, a mix, and the held association's own
    // id read backwards — a well-formed neighbour of a real id.
    let mut unknown_ids: Vec<String> = vec![
        "abcd".to_string(),
        "zzzzzzz".to_string(),
        // Digits only, and not a substring of the account the template DOES report, so
        // the withheld-identifier assertion below is about this id alone.
        "48217".to_string(),
        "9z8y7x6".to_string(),
        held.chars().rev().collect::<String>(),
    ];
    unknown_ids.retain(|candidate| candidate != &held);
    for unknown in &unknown_ids {
        assert!(
            is_modelled_id(unknown),
            "case {unknown:?}: this row must satisfy ^[a-z0-9]{{4,7}}$ to reach the lookup"
        );
    }

    let expected = expected_not_found();
    for unknown in &unknown_ids {
        // Both operations that reach this lookup, and both shapes of update request: one
        // carrying a parameter map and one carrying no member at all.
        let answers: Vec<(String, Answer)> = vec![
            (
                format!("UpdateExtensionAssociation with an empty map on {unknown:?}"),
                update_association(&router, unknown, &json!({ "Parameters": {} })).await,
            ),
            (
                format!("UpdateExtensionAssociation with values on {unknown:?}"),
                update_association(&router, unknown, &json!({ "Parameters": { "optional": "x" } }))
                    .await,
            ),
            (
                format!("UpdateExtensionAssociation with no member on {unknown:?}"),
                update_association(&router, unknown, &Value::Object(Map::new())).await,
            ),
            (
                format!("GetExtensionAssociation on {unknown:?}"),
                get_association(&router, unknown).await,
            ),
        ];

        for (case, answer) in &answers {
            assert_eq!(
                answer.status,
                404,
                "case {case:?}: expected 404, got {} / {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                code(answer),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {:?}",
                code(answer)
            );
            let actual = message(answer);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected message {expected:?}, got {actual:?}"
            );
            // The kind is ONE token, and the template echoes no identifier.
            assert!(
                !actual.contains("Extension Association"),
                "case {case:?}: the kind is the single token \"ExtensionAssociation\", got \
                 {actual:?}"
            );
            assert!(
                !actual.contains(unknown.as_str()),
                "case {case:?}: this template withholds the submitted id, got {actual:?}"
            );
        }
    }

    // None of those refusals touched the association that does exist.
    let read = get_association(&router, &held).await;
    assert_eq!(
        read.status,
        200,
        "the held association must still be readable, got {} / {}",
        read.status,
        String::from_utf8_lossy(&read.body)
    );
    assert_eq!(
        read.json().get("Parameters"),
        Some(&json!({ "optional": "held" })),
        "the held association must be untouched, got {}",
        read.json()
    );
}

/// One parameter map that omits a required value, and the extension version it is checked
/// against.
struct Omission {
    case: String,
    /// The `ExtensionIdentifier` the association binds, and the id the refusal must name.
    extension_id: String,
    /// The name the extension answers to, which the refusal must NOT name when it differs
    /// from the id.
    extension_name: String,
    version: i32,
    /// The values the association is created with, which satisfy the declaration.
    valid: Value,
    /// The values the refused request supplies.
    offending: Value,
    /// The parameter the refusal must name as missing.
    missing: String,
}

/// A map omitting a required value is refused with AWS's template — the omitted parameter,
/// the extension's ID and version, and the "no value was supplied" condition — on the
/// update and the create alike, and nothing is written.
#[tokio::test]
async fn a_missing_required_value_is_refused_naming_the_extension_by_id() {
    let router = router();
    let application = create_application(&router, "parameter-refusal-message").await;
    let resource_arn = arn(&format!("application/{application}"));

    // One extension at TWO versions, so the version number in the message varies and the
    // declaration checked is the version's own; one declaring two required parameters, so
    // the refusal names one of several; and AppConfig's own published extension, whose id
    // is its name.
    let versioned_name = "parameter-refusal-versioned";
    let versioned = created_extension(
        &router,
        versioned_name,
        declaration(&["mandatory"], &["optional"]),
        None,
    )
    .await;
    let versioned_again = created_extension(
        &router,
        versioned_name,
        declaration(&["needed"], &[]),
        Some(1),
    )
    .await;
    assert_eq!(
        versioned, versioned_again,
        "a second version keeps the extension's id, got {versioned_again:?}"
    );
    let two_required_name = "parameter-refusal-two-required";
    let two_required = created_extension(
        &router,
        two_required_name,
        declaration(&["alpha", "zulu"], &[]),
        None,
    )
    .await;
    let (authored_name, authored_parameter) = authored_with_required_parameter();

    let table: Vec<Omission> = vec![
        Omission {
            case: "version 1: the map carries only the optional value".to_string(),
            extension_id: versioned.clone(),
            extension_name: versioned_name.to_string(),
            version: 1,
            valid: json!({ "mandatory": "initial" }),
            offending: json!({ "optional": "opt-value" }),
            missing: "mandatory".to_string(),
        },
        Omission {
            case: "version 1: an empty map".to_string(),
            extension_id: versioned.clone(),
            extension_name: versioned_name.to_string(),
            version: 1,
            valid: json!({ "mandatory": "initial", "optional": "also" }),
            offending: json!({}),
            missing: "mandatory".to_string(),
        },
        Omission {
            case: "version 2: the version's own declaration is what is checked".to_string(),
            extension_id: versioned.clone(),
            extension_name: versioned_name.to_string(),
            version: 2,
            valid: json!({ "needed": "initial" }),
            offending: json!({}),
            missing: "needed".to_string(),
        },
        Omission {
            // Two required parameters, one supplied: the refusal names the omitted one.
            case: "two required, the later one supplied".to_string(),
            extension_id: two_required.clone(),
            extension_name: two_required_name.to_string(),
            version: 1,
            valid: json!({ "alpha": "a", "zulu": "z" }),
            offending: json!({ "zulu": "z" }),
            missing: "alpha".to_string(),
        },
        Omission {
            // Both omitted: the names are read sorted, so the refusal is stable.
            case: "two required, neither supplied".to_string(),
            extension_id: two_required.clone(),
            extension_name: two_required_name.to_string(),
            version: 1,
            valid: json!({ "alpha": "a", "zulu": "z" }),
            offending: json!({}),
            missing: "alpha".to_string(),
        },
        Omission {
            // An AWS-authored extension: its id IS its published name, so the message
            // carries that one token for both.
            case: format!("the published extension {authored_name}"),
            extension_id: authored_name.to_string(),
            extension_name: authored_name.to_string(),
            version: 1,
            valid: json!({ authored_parameter: arn("queue/lifecycle-events") }),
            offending: json!({}),
            missing: authored_parameter.to_string(),
        },
    ];

    for row in &table {
        let case = &row.case;
        let expected = expected_required(&row.missing, &row.extension_id, row.version);
        let id = create_association(
            &router,
            case,
            &row.extension_id,
            row.version,
            &resource_arn,
            &row.valid,
        )
        .await;

        // The update path.
        let updated = update_association(
            &router,
            &id,
            &json!({ "Parameters": row.offending.clone() }),
        )
        .await;
        // The create path, which renders the same sentence.
        let refused_create = associate(
            &router,
            &json!({
                "ExtensionIdentifier": row.extension_id,
                "ExtensionVersionNumber": row.version,
                "ResourceIdentifier": resource_arn,
                "Parameters": row.offending.clone(),
            }),
        )
        .await;

        for (path, answer) in [
            ("UpdateExtensionAssociation", &updated),
            ("CreateExtensionAssociation", &refused_create),
        ] {
            assert_eq!(
                answer.status,
                400,
                "case {case:?} via {path}: expected 400, got {} / {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                code(answer),
                "BadRequestException",
                "case {case:?} via {path}: expected BadRequestException, got {:?}",
                code(answer)
            );
            let actual = message(answer);
            assert_eq!(
                actual, expected,
                "case {case:?} via {path}: expected message {expected:?}, got {actual:?}"
            );
            if row.extension_name != row.extension_id {
                assert!(
                    !actual.contains(row.extension_name.as_str()),
                    "case {case:?} via {path}: the refusal identifies the extension by its id, \
                     not by the name {:?}, got {actual:?}",
                    row.extension_name
                );
            }
        }

        // The refused update wrote nothing: the association still holds the values it was
        // created with.
        let read = get_association(&router, &id).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: GetExtensionAssociation expected 200, got {} / {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            read.json().get("Parameters"),
            Some(&row.valid),
            "case {case:?}: expected the refused update to change nothing, got {}",
            read.json()
        );
    }
}
