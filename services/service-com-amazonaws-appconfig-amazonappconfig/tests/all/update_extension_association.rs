// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateExtensionAssociation`, the
//! `PATCH /extensionassociations/{ExtensionAssociationId}` editor of an association this
//! account created.
//!
//! Every case drives a real HTTP request through the router the emulator's front door
//! uses, so what is asserted is what an AppConfig client sees on the wire. The rules under
//! test:
//!
//! * the update is IN PLACE on the association the id addresses: `Parameters` is the one
//!   member the request can carry, so supplying it REPLACES the whole parameter-value map
//!   and omitting it keeps the values the association already holds, while the id, the
//!   association's ARN, the extension version ARN and number it binds and the resource ARN
//!   it is bound to are all kept;
//! * the answer is the stored record, so `GetExtensionAssociation` reports exactly what the
//!   update answered, and a second update supersedes the first — last write wins;
//! * the values are checked against the declaration of the extension VERSION the
//!   association binds: a name the version does not declare, and a missing value for a
//!   parameter it marks required, are each refused HTTP 400 `BadRequestException`, with the
//!   association left exactly as it was;
//! * an id that addresses no association this account holds is refused HTTP 404
//!   `ResourceNotFoundException`.
//!
//! The tables span what those rules generalise over rather than one association: which end
//! of the catalogue the bound extension comes from (AWS-authored and created by this
//! account), how many values the update supplies against how many the association held
//! (growing, shrinking, replacing in place, an empty map, and no member at all), the form
//! of a value (a plain word, an ARN, a value carrying punctuation and non-ASCII text), and
//! which of the three resource kinds the association binds.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::ops::types::ACTION_POINT_VALUES;
use service_com_amazonaws_appconfig_amazonappconfig::state::AWS_AUTHORED_EXTENSIONS;

use crate::appconfig_workflow::{
    create_application, create_environment, create_profile, json_call, router, Answer,
};
use crate::extension_associations::{associate, get_association};
use crate::extensions::{actions, create_extension, minted_id};
use crate::resource_tags::{arn, pct};

/// `PATCH /extensionassociations/{ExtensionAssociationId}`. The id is bound to an
/// `@httpLabel`; `Parameters` is the only other member of the request and travels in the
/// body. `pub(crate)` because the refusal-message suite drives the same front door.
pub(crate) async fn update_association(
    router: &MultiServiceRouter,
    id: &str,
    body: &Value,
) -> Answer {
    json_call(
        router,
        "PATCH",
        &format!("/extensionassociations/{}", pct(id)),
        body,
    )
    .await
}

/// The `ExtensionAssociation` members every reader of one must report, as one comparable
/// value: absent and present are both represented, so two answers are compared whole
/// rather than member by member.
fn facts(body: &Value) -> BTreeMap<&'static str, Value> {
    [
        "Id",
        "Arn",
        "ExtensionArn",
        "ResourceArn",
        "ExtensionVersionNumber",
        "Parameters",
    ]
    .into_iter()
    .map(|member| (member, body.get(member).cloned().unwrap_or(Value::Null)))
    .collect()
}

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

/// The AWS-authored extension that declares a parameter, and the name of that parameter,
/// read from the published catalogue rather than written out here, so a case runs against
/// whatever the catalogue declares.
fn authored_with_parameter() -> (&'static str, &'static str) {
    AWS_AUTHORED_EXTENSIONS
        .iter()
        .find_map(|authored| {
            authored
                .parameters
                .first()
                .map(|parameter| (authored.name, parameter.name))
        })
        .expect("the published catalogue declares at least one extension with a parameter")
}

/// The AWS-authored extension that declares no parameter at all.
fn authored_without_parameter() -> &'static str {
    AWS_AUTHORED_EXTENSIONS
        .iter()
        .find(|authored| authored.parameters.is_empty())
        .map(|authored| authored.name)
        .expect("the published catalogue declares at least one extension with no parameter")
}

/// One application, with an environment and a configuration profile under it, and the
/// three ARNs an extension may be associated to.
struct Tree {
    application_arn: String,
    environment_arn: String,
    profile_arn: String,
}

async fn tree(router: &MultiServiceRouter, name: &str) -> Tree {
    let application = create_application(router, &format!("{name}-app")).await;
    let environment = create_environment(router, &application, &format!("{name}-env")).await;
    let profile = create_profile(router, &application, &format!("{name}-profile")).await;
    Tree {
        application_arn: arn(&format!("application/{application}")),
        environment_arn: arn(&format!("application/{application}/environment/{environment}")),
        profile_arn: arn(&format!(
            "application/{application}/configurationprofile/{profile}"
        )),
    }
}

/// An extension this account created, declaring one required parameter and one optional
/// one. Answers its minted id.
async fn created_extension(router: &MultiServiceRouter, name: &str) -> String {
    let answer = create_extension(
        router,
        &json!({
            "Name": name,
            "Actions": actions(&[ACTION_POINT_VALUES[0]]),
            "Parameters": {
                "mandatory": { "Required": true, "Dynamic": false },
                "optional": { "Required": false, "Dynamic": false },
            },
        }),
        None,
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

/// Create one association and answer the body the create reported.
async fn create_association(
    router: &MultiServiceRouter,
    case: &str,
    extension: &str,
    resource_arn: &str,
    parameters: &Value,
) -> Value {
    let mut body = Map::new();
    body.insert("ExtensionIdentifier".to_string(), Value::from(extension));
    body.insert("ResourceIdentifier".to_string(), Value::from(resource_arn));
    if !parameters.is_null() {
        body.insert("Parameters".to_string(), parameters.clone());
    }
    let answer = associate(router, &Value::Object(body)).await;
    assert_eq!(
        answer.status,
        201,
        "case {case:?}: CreateExtensionAssociation expected 201, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer.json()
}

/// The `Id` of an association body.
fn association_id(case: &str, body: &Value) -> String {
    body.get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no Id in {body}"))
        .to_string()
}

/// One row of the update table.
struct Row {
    case: &'static str,
    /// The `ExtensionIdentifier` the association binds, resolved against the fixtures.
    extension: fn(&str) -> String,
    /// The resource the association binds.
    resource: fn(&Tree) -> String,
    /// The values the association is created with; `Value::Null` means the create carries
    /// no `Parameters` member at all.
    created_with: fn() -> Value,
    /// The `Parameters` the update carries; `Value::Null` means it carries none.
    update_with: fn() -> Value,
    /// The values the association must hold after the update.
    expected: fn() -> Value,
}

/// `Parameters` replaces the stored map when the update carries it, and is kept when it
/// does not; everything else the association reports is untouched, and
/// `GetExtensionAssociation` reports what the update answered.
#[tokio::test]
async fn update_extension_association_replaces_only_the_parameter_values() {
    let table: Vec<Row> = vec![
        Row {
            case: "an AWS-authored extension: the one declared value is replaced",
            extension: |_| authored_with_parameter().0.to_string(),
            resource: |tree| tree.application_arn.clone(),
            created_with: || json!({ authored_with_parameter().1: arn("queue/first") }),
            update_with: || json!({ authored_with_parameter().1: arn("queue/second") }),
            expected: || json!({ authored_with_parameter().1: arn("queue/second") }),
        },
        Row {
            case: "a created extension: the update grows the map from one value to two",
            extension: |extension_id| extension_id.to_string(),
            resource: |tree| tree.environment_arn.clone(),
            created_with: || json!({ "mandatory": "before" }),
            update_with: || json!({ "mandatory": "after", "optional": "added" }),
            expected: || json!({ "mandatory": "after", "optional": "added" }),
        },
        Row {
            case: "a created extension: the update shrinks the map to the required value",
            extension: |extension_id| extension_id.to_string(),
            resource: |tree| tree.profile_arn.clone(),
            created_with: || json!({ "mandatory": "before", "optional": "dropped" }),
            update_with: || json!({ "mandatory": "before" }),
            expected: || json!({ "mandatory": "before" }),
        },
        Row {
            // A value is an opaque `ParameterValue`, so punctuation and non-ASCII text
            // round-trip byte for byte.
            case: "a created extension: a value carrying punctuation and non-ASCII text",
            extension: |extension_id| extension_id.to_string(),
            resource: |tree| tree.application_arn.clone(),
            created_with: || json!({ "mandatory": "plain" }),
            update_with: || json!({ "mandatory": "a/b:c-d_e.f 1%2", "optional": "größe=ø" }),
            expected: || json!({ "mandatory": "a/b:c-d_e.f 1%2", "optional": "größe=ø" }),
        },
        Row {
            case: "an AWS-authored extension declaring none: an empty map stays empty",
            extension: |_| authored_without_parameter().to_string(),
            resource: |tree| tree.profile_arn.clone(),
            created_with: || Value::Null,
            update_with: || json!({}),
            expected: || json!({}),
        },
        Row {
            case: "a created extension: an update carrying no Parameters keeps the values",
            extension: |extension_id| extension_id.to_string(),
            resource: |tree| tree.environment_arn.clone(),
            created_with: || json!({ "mandatory": "kept", "optional": "also kept" }),
            update_with: || Value::Null,
            expected: || json!({ "mandatory": "kept", "optional": "also kept" }),
        },
    ];

    let router = router();
    let tree = tree(&router, "association-update").await;
    let extension_id = created_extension(&router, "association-update-fixture").await;

    for row in &table {
        let case = row.case;
        let extension = (row.extension)(&extension_id);
        let resource_arn = (row.resource)(&tree);
        let created = create_association(
            &router,
            case,
            &extension,
            &resource_arn,
            &(row.created_with)(),
        )
        .await;
        let id = association_id(case, &created);

        let update_body = match (row.update_with)() {
            Value::Null => Value::Object(Map::new()),
            parameters => json!({ "Parameters": parameters }),
        };
        let answer = update_association(&router, &id, &update_body).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: UpdateExtensionAssociation expected 200, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let updated = answer.json();

        assert_eq!(
            updated.get("Parameters"),
            Some(&(row.expected)()),
            "case {case:?}: expected Parameters {}, got {updated}",
            (row.expected)()
        );
        // Nothing but the values changes: every other member is the one the create minted
        // or resolved.
        for member in ["Id", "Arn", "ExtensionArn", "ResourceArn", "ExtensionVersionNumber"] {
            assert_eq!(
                updated.get(member),
                created.get(member),
                "case {case:?}: expected {member} to be unchanged at {:?}, got {:?}",
                created.get(member),
                updated.get(member)
            );
        }
        assert_eq!(
            updated.get("ResourceArn").and_then(Value::as_str),
            Some(resource_arn.as_str()),
            "case {case:?}: expected the bound resource's ARN, got {updated}"
        );

        // The answer is the stored record, so the reader reports the same association.
        let read = get_association(&router, &id).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: GetExtensionAssociation expected 200, got {} / {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            facts(&read.json()),
            facts(&updated),
            "case {case:?}: expected the reader to report the updated association, got {}",
            read.json()
        );
    }
}

/// A sequence of updates of one association: each one supersedes the last, a repeat of the
/// same update changes nothing, and the association keeps its identity throughout.
#[tokio::test]
async fn the_latest_update_is_what_every_reader_reports() {
    // Each step is the whole map the update carries, so the sequence covers replacing a
    // value, adding one, dropping one, emptying the map back to the required value and
    // repeating a step verbatim.
    let steps: Vec<Value> = vec![
        json!({ "mandatory": "one" }),
        json!({ "mandatory": "two", "optional": "two-optional" }),
        json!({ "mandatory": "two", "optional": "two-optional" }),
        json!({ "mandatory": "three" }),
        json!({ "mandatory": "four", "optional": "four-optional" }),
    ];

    let router = router();
    let tree = tree(&router, "association-sequence").await;
    let extension_id = created_extension(&router, "association-sequence-fixture").await;
    let created = create_association(
        &router,
        "the sequence fixture",
        &extension_id,
        &tree.application_arn,
        &json!({ "mandatory": "zero" }),
    )
    .await;
    let id = association_id("the sequence fixture", &created);

    for (index, step) in steps.iter().enumerate() {
        let case = format!("step {index}: {step}");
        let answer = update_association(&router, &id, &json!({ "Parameters": step })).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: UpdateExtensionAssociation expected 200, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let updated = answer.json();
        assert_eq!(
            updated.get("Parameters"),
            Some(step),
            "case {case:?}: expected the values this step supplied, got {updated}"
        );
        assert_eq!(
            updated.get("Id").and_then(Value::as_str),
            Some(id.as_str()),
            "case {case:?}: expected the association to keep its id, got {updated}"
        );
        let read = get_association(&router, &id).await;
        assert_eq!(
            read.json().get("Parameters"),
            Some(step),
            "case {case:?}: expected the reader to report this step's values, got {}",
            read.json()
        );
    }
}

/// One row of the refusal table.
struct Refusal {
    case: &'static str,
    /// The id the update addresses, resolved from the stored association's id.
    id: fn(&str) -> String,
    parameters: fn() -> Value,
    status: u16,
    code: &'static str,
    /// A fragment the refusal's `Message` must carry.
    message_carries: fn() -> String,
}

/// An update this service will not store is refused with the modelled error, and changes
/// nothing.
#[tokio::test]
async fn an_update_this_service_will_not_store_is_refused_and_changes_nothing() {
    let table: Vec<Refusal> = vec![
        Refusal {
            case: "a parameter name the extension version does not declare",
            id: |id| id.to_string(),
            parameters: || json!({ "mandatory": "kept", "undeclared": "nope" }),
            status: 400,
            code: "BadRequestException",
            message_carries: || "declares no parameter named undeclared".to_string(),
        },
        Refusal {
            case: "a map omitting the value the version marks required",
            id: |id| id.to_string(),
            parameters: || json!({ "optional": "only this one" }),
            status: 400,
            code: "BadRequestException",
            message_carries: || "Parameter mandatory  is required for extension".to_string(),
        },
        Refusal {
            case: "an empty map against a version that declares a required parameter",
            id: |id| id.to_string(),
            parameters: || json!({}),
            status: 400,
            code: "BadRequestException",
            message_carries: || "Parameter mandatory  is required for extension".to_string(),
        },
        Refusal {
            // `Id` is `^[a-z0-9]{4,7}$`, so this addresses an association that could
            // exist and does not — the modelled 404 rather than a constraint refusal.
            case: "an id this account holds no association under",
            id: |_| "zzz999".to_string(),
            parameters: || json!({ "mandatory": "whatever" }),
            status: 404,
            code: "ResourceNotFoundException",
            message_carries: || "ExtensionAssociation could not be found".to_string(),
        },
    ];

    let router = router();
    let tree = tree(&router, "association-refusal").await;
    let extension_id = created_extension(&router, "association-refusal-fixture").await;
    let stored_parameters = json!({ "mandatory": "untouched", "optional": "untouched too" });
    let created = create_association(
        &router,
        "the refusal fixture",
        &extension_id,
        &tree.environment_arn,
        &stored_parameters,
    )
    .await;
    let id = association_id("the refusal fixture", &created);

    for row in &table {
        let case = row.case;
        let answer = update_association(
            &router,
            &(row.id)(&id),
            &json!({ "Parameters": (row.parameters)() }),
        )
        .await;
        assert_eq!(
            answer.status, row.status,
            "case {case:?}: expected HTTP {}, got {} / {}",
            row.status,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            code(&answer),
            row.code,
            "case {case:?}: expected {}, got {:?}",
            row.code,
            code(&answer)
        );
        let carried = (row.message_carries)();
        assert!(
            message(&answer).contains(&carried),
            "case {case:?}: expected a Message carrying {carried:?}, got {:?}",
            message(&answer)
        );

        // Nothing was written: the association still reports what it held.
        let read = get_association(&router, &id).await;
        assert_eq!(
            read.json().get("Parameters"),
            Some(&stored_parameters),
            "case {case:?}: expected the refused update to change nothing, got {}",
            read.json()
        );
        assert_eq!(
            facts(&read.json()),
            facts(&created),
            "case {case:?}: expected the association to be exactly as created, got {}",
            read.json()
        );
    }
}
