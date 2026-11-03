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
//! 3. A create that supplies NO parameter-value member at all against a version declaring
//!    at least one required parameter singles out no parameter: it is refused with the
//!    condition — the version has required parameters and nothing was supplied — naming
//!    the extension by the name it answers to and the version whose declaration was
//!    checked. A version declaring nothing required accepts such a request.
//!
//! The tables span what those rules generalise over: the operation reaching the refusal
//! (the update and the create for rule 2, the update and the reader for rule 1), the length
//! and the character mix of the id that addresses nothing, which end of the catalogue the
//! extension comes from (AWS-authored, where the id IS the published name, and created by
//! this account, where the minted id and the chosen name differ), how many parameters the
//! version declares required, which of them the map omits, which version of the extension
//! the association binds, whether the offending map is empty or carries the values the
//! version does not require, and — for rule 3 — whether the member is absent or present
//! and empty, which is what chooses between the two sentences.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::ops::types::ACTION_POINT_VALUES;
use service_com_amazonaws_appconfig_amazonappconfig::state::{
    AWS_AUTHORED_EXTENSIONS, AWS_AUTHORED_EXTENSION_VERSION,
};

use crate::appconfig_workflow::{
    account, create_application, is_modelled_id, json_call, router, Answer,
};
use crate::extension_associations::{associate, authored_without_parameter, get_association};
use crate::extensions::{actions, create_extension, minted_id};
use crate::resource_tags::{arn, pct};
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

/// The AWS-authored extension that declares a required parameter — its published id, its
/// published name and that parameter's name — read from the published catalogue rather
/// than written out here. The id and the name are BOTH needed: the per-parameter refusal
/// names the extension by id, and the one about the whole request names it by name.
fn authored_with_required_parameter() -> (&'static str, &'static str, &'static str) {
    AWS_AUTHORED_EXTENSIONS
        .iter()
        .find_map(|authored| {
            authored
                .parameters
                .iter()
                .find(|parameter| parameter.required)
                .map(|parameter| (authored.id, authored.name, parameter.name))
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
    let (authored_id, authored_name, authored_parameter) = authored_with_required_parameter();

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
            // An AWS-authored extension: the catalogue publishes its id and its name
            // separately, and this sentence names the extension by its id.
            case: format!("the published extension {authored_name}"),
            extension_id: authored_id.to_string(),
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

/// The message AppConfig answers a request that supplies no parameter values at all with:
/// the condition — the version declares required parameters and nothing was supplied — the
/// extension named by the name it answers to, and the version whose declaration was
/// checked. No parameter is singled out and the sentence carries no trailing stop.
fn expected_nothing_supplied(extension_name: &str, version: i32) -> String {
    format!(
        "Extension {extension_name} version {version} has required parameters, but no \
         parameters have been supplied"
    )
}

/// `POST /extensionassociations` carrying NO `Parameters` member at all — the member is
/// optional in the model, so a request omitting it is well formed and reaches the handler.
async fn associate_without_parameters(
    router: &MultiServiceRouter,
    extension: &str,
    pinned: Option<i32>,
    resource_arn: &str,
) -> Answer {
    let mut body = Map::new();
    body.insert("ExtensionIdentifier".to_string(), Value::from(extension));
    body.insert("ResourceIdentifier".to_string(), Value::from(resource_arn));
    if let Some(version) = pinned {
        body.insert("ExtensionVersionNumber".to_string(), Value::from(version));
    }
    associate(router, &Value::Object(body)).await
}

/// How many associations this account holds for `extension`, read through
/// `ListExtensionAssociations`, so a refusal that leaked one shows up as a count that grew.
async fn associations_held(router: &MultiServiceRouter, case: &str, extension: &str) -> usize {
    let answer = json_call(
        router,
        "GET",
        &format!(
            "/extensionassociations?extension_identifier={}",
            pct(extension)
        ),
        &Value::Null,
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: ListExtensionAssociations expected 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .json()
        .get("Items")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default()
}

/// One create that supplies no parameter-value member at all.
struct NothingSupplied {
    case: String,
    /// The identifier the request addresses the extension by — an id or a name, which the
    /// message does not depend on.
    identifier: String,
    /// The name the refusal must name the extension by.
    extension_name: String,
    /// The version the request pins, or `None` to leave the create to take the highest.
    pinned: Option<i32>,
    /// The version number the refusal must name.
    version: i32,
    /// A parameter that version marks required, which this sentence must NOT single out.
    required: String,
}

/// A create supplying no parameter values at all against a version that declares at least
/// one required parameter is refused with the CONDITION — naming the extension and the
/// version, singling out no parameter — while a request that does supply a map and omits a
/// required value keeps the per-parameter sentence, and a version declaring nothing
/// required accepts a request that supplies nothing. Nothing is written either way.
#[tokio::test]
async fn a_create_supplying_no_parameters_is_refused_naming_the_extension_and_version() {
    let router = router();
    let application = create_application(&router, "nothing-supplied").await;
    let resource_arn = arn(&format!("application/{application}"));

    // One extension declaring a required parameter beside an optional one; one declaring
    // two required; one at TWO versions whose FIRST version requires nothing and whose
    // second does, so the version's own declaration is what decides; and AppConfig's own
    // published extension, whose id is its name.
    let one_required_name = "nothing-supplied-one-required";
    let one_required = created_extension(
        &router,
        one_required_name,
        declaration(&["mandatory"], &["optional"]),
        None,
    )
    .await;
    let two_required_name = "nothing-supplied-two-required";
    let two_required = created_extension(
        &router,
        two_required_name,
        declaration(&["alpha", "zulu"], &[]),
        None,
    )
    .await;
    let versioned_name = "nothing-supplied-versioned-9f3b5c9b7b01";
    let versioned = created_extension(&router, versioned_name, declaration(&[], &["optional"]), None)
        .await;
    let versioned_again =
        created_extension(&router, versioned_name, declaration(&["needed"], &[]), Some(1)).await;
    assert_eq!(
        versioned, versioned_again,
        "a second version keeps the extension's id, got {versioned_again:?}"
    );
    let (authored_id, authored_name, authored_parameter) = authored_with_required_parameter();

    let table: Vec<NothingSupplied> = vec![
        NothingSupplied {
            case: "one required parameter, the extension addressed by its id".to_string(),
            identifier: one_required.clone(),
            extension_name: one_required_name.to_string(),
            pinned: None,
            version: 1,
            required: "mandatory".to_string(),
        },
        NothingSupplied {
            case: "the same extension addressed by its name, with the version pinned"
                .to_string(),
            identifier: one_required_name.to_string(),
            extension_name: one_required_name.to_string(),
            pinned: Some(1),
            version: 1,
            required: "mandatory".to_string(),
        },
        NothingSupplied {
            case: "two required parameters: neither is singled out".to_string(),
            identifier: two_required.clone(),
            extension_name: two_required_name.to_string(),
            pinned: None,
            version: 1,
            required: "alpha".to_string(),
        },
        NothingSupplied {
            case: "version 2, whose own declaration requires a value".to_string(),
            identifier: versioned.clone(),
            extension_name: versioned_name.to_string(),
            pinned: Some(2),
            version: 2,
            required: "needed".to_string(),
        },
        NothingSupplied {
            case: format!("the published extension {authored_name}"),
            identifier: authored_name.to_string(),
            extension_name: authored_name.to_string(),
            pinned: None,
            version: AWS_AUTHORED_EXTENSION_VERSION,
            required: authored_parameter.to_string(),
        },
    ];

    for row in &table {
        let case = &row.case;
        let held = associations_held(&router, case, &row.identifier).await;
        let answer =
            associate_without_parameters(&router, &row.identifier, row.pinned, &resource_arn).await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected 400, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            code(&answer),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {:?}",
            code(&answer)
        );
        let expected = expected_nothing_supplied(&row.extension_name, row.version);
        let actual = message(&answer);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected message {expected:?}, got {actual:?}"
        );
        // The sentence is about the request, so it singles out no parameter at all.
        assert!(
            !actual.contains("Parameter "),
            "case {case:?}: a request supplying nothing names no parameter, got {actual:?}"
        );
        if !row.extension_name.contains(row.required.as_str()) {
            assert!(
                !actual.contains(row.required.as_str()),
                "case {case:?}: this sentence must not name the required parameter {:?}, got \
                 {actual:?}",
                row.required
            );
        }
        assert_eq!(
            associations_held(&router, case, &row.identifier).await,
            held,
            "case {case:?}: the refused create must leak no association"
        );
    }

    // The member being ABSENT is what chooses that sentence: the same request carrying an
    // empty map is present-but-incomplete, which is the per-parameter refusal.
    for (case, identifier, extension_id, missing) in [
        (
            "an empty map against one required parameter",
            one_required.clone(),
            one_required.clone(),
            "mandatory",
        ),
        (
            "an empty map against two required parameters",
            two_required.clone(),
            two_required.clone(),
            "alpha",
        ),
        (
            "an empty map against the published extension",
            authored_name.to_string(),
            authored_id.to_string(),
            authored_parameter,
        ),
    ] {
        let answer = associate(
            &router,
            &json!({
                "ExtensionIdentifier": identifier,
                "ResourceIdentifier": resource_arn,
                "Parameters": {},
            }),
        )
        .await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected 400, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let expected = expected_required(missing, &extension_id, 1);
        let actual = message(&answer);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected message {expected:?}, got {actual:?}"
        );
    }

    // A version declaring nothing required accepts a request that supplies nothing, and
    // the association it mints reports the empty map every reader then reports.
    for (case, identifier, pinned, expected_version) in [
        (
            "version 1, which declares only an optional parameter",
            versioned.clone(),
            Some(1),
            1,
        ),
        (
            "the published extension that declares no parameter at all",
            authored_without_parameter().to_string(),
            None,
            AWS_AUTHORED_EXTENSION_VERSION,
        ),
    ] {
        let answer =
            associate_without_parameters(&router, &identifier, pinned, &resource_arn).await;
        assert_eq!(
            answer.status,
            201,
            "case {case:?}: expected 201, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let created = answer.json();
        assert_eq!(
            created.get("Parameters"),
            Some(&json!({})),
            "case {case:?}: expected an empty parameter map, got {created}"
        );
        assert_eq!(
            created.get("ExtensionVersionNumber"),
            Some(&json!(expected_version)),
            "case {case:?}: expected version {expected_version}, got {created}"
        );
        let id = created
            .get("Id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: the create answered no Id: {created}"));
        let read = get_association(&router, id).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: GetExtensionAssociation expected 200, got {} / {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            read.json().get("Parameters"),
            Some(&json!({})),
            "case {case:?}: expected the reader to report the empty map, got {}",
            read.json()
        );
    }
}
