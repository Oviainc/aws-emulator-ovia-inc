// SPDX-License-Identifier: CC0-1.0

//! `CreateExtensionAssociation` and the `GetExtensionAssociation` reader that proves what
//! it persisted.
//!
//! The rules under test:
//!
//! * an association BINDS one version of one extension to one application, environment or
//!   configuration profile: the create resolves both ends against this service's state,
//!   mints the association's id and ARN, stores the parameter values it was given, and
//!   answers the stored record, which `GetExtensionAssociation` then reports unchanged;
//! * the extension end is resolved over the whole catalogue the account can see, so an
//!   AWS-authored extension associates exactly as one this account created does, and the
//!   version is the one the request pins or the extension's highest;
//! * the parameter values are checked against the extension version's own declaration: a
//!   required parameter must be given and a name the version does not declare is refused,
//!   with nothing stored either way;
//! * an identifier that addresses no extension, and one that addresses no associable
//!   resource, are refused with the modelled 404.
//!
//! The tables span what those rules generalise over: which end of the catalogue the
//! extension comes from, which of the three resource kinds is bound and in which
//! identifier form (ARN, and an application's id), how many parameters the extension
//! declares and how many values are supplied, whether a version is pinned, and whether
//! tags are assigned. Every assertion names its case and prints the actual value.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::ops::types::ACTION_POINT_VALUES;
use service_com_amazonaws_appconfig_amazonappconfig::state::AWS_AUTHORED_EXTENSIONS;

use crate::appconfig_workflow::{
    create_application, create_environment, create_profile, is_modelled_id, json_call, router,
    Answer,
};
use crate::extensions::{actions, create_extension, get_extension, minted_id};
use crate::resource_tags::{arn, list_tags, pct};

/// `POST /extensionassociations`. `pub(crate)` because the `ListExtensionAssociations`
/// suite creates its fixtures through the same front door.
pub(crate) async fn associate(router: &MultiServiceRouter, body: &Value) -> Answer {
    json_call(router, "POST", "/extensionassociations", body).await
}

/// `GET /extensionassociations/{ExtensionAssociationId}`. `pub(crate)` because the
/// `ListExtensionAssociations` suite reads its fixtures back through the same reader.
pub(crate) async fn get_association(router: &MultiServiceRouter, id: &str) -> Answer {
    json_call(
        router,
        "GET",
        &format!("/extensionassociations/{}", pct(id)),
        &Value::Null,
    )
    .await
}

/// The `ExtensionAssociation` members a reader must report, as one comparable value.
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

/// One application, with an environment and a configuration profile under it, and the
/// three ARNs an extension may be associated to.
struct Tree {
    application: String,
    application_arn: String,
    environment_arn: String,
    profile_arn: String,
}

async fn tree(router: &MultiServiceRouter) -> Tree {
    let application = create_application(router, "associated-app").await;
    let environment = create_environment(router, &application, "associated-env").await;
    let profile = create_profile(router, &application, "associated-profile").await;
    Tree {
        application_arn: arn(&format!("application/{application}")),
        environment_arn: arn(&format!("application/{application}/environment/{environment}")),
        profile_arn: arn(&format!(
            "application/{application}/configurationprofile/{profile}"
        )),
        application,
    }
}

/// An extension this account created, declaring one required parameter and one optional
/// one, at two versions. Answers `(id, name)`.
async fn created_extension(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({
        "Name": name,
        "Actions": actions(&[ACTION_POINT_VALUES[0]]),
        "Parameters": {
            "mandatory": { "Required": true, "Dynamic": false },
            "optional": { "Required": false, "Dynamic": false },
        },
    });
    let created = create_extension(router, &body, None).await;
    assert_eq!(
        created.status,
        201,
        "case {name:?}: CreateExtension expected 201, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    minted_id(name, &created.json())
}

/// One row of the association table.
struct Row {
    case: &'static str,
    /// The `ExtensionIdentifier`, resolved at run time from the fixtures.
    extension: fn(&Fixtures) -> String,
    /// The `ResourceIdentifier` the association names.
    resource: fn(&Tree) -> String,
    /// The ARN the association must report for that resource.
    resource_arn: fn(&Tree) -> String,
    version: Option<i32>,
    parameters: Option<Value>,
    tags: Option<Value>,
    /// The version number the association must report.
    expected_version: i32,
}

/// What the rows resolve their identifiers against.
struct Fixtures {
    created_id: String,
    created_name: String,
}

/// The AWS-authored extension the rows associate: the one that declares a parameter, so a
/// row exercises a required value, and the one that declares none. Each is answered as the
/// published ID a caller passes as an `ExtensionIdentifier`, which is also the id an
/// applied extension reports.
fn authored_with_parameter() -> &'static str {
    AWS_AUTHORED_EXTENSIONS
        .iter()
        .find(|authored| !authored.parameters.is_empty())
        .map(|authored| authored.id)
        .expect("the published catalogue declares at least one extension with a parameter")
}

/// The parameter name that extension declares.
fn authored_parameter() -> &'static str {
    AWS_AUTHORED_EXTENSIONS
        .iter()
        .find(|authored| !authored.parameters.is_empty())
        .and_then(|authored| authored.parameters.first())
        .map(|parameter| parameter.name)
        .expect("the published catalogue declares at least one parameter")
}

/// The AWS-authored extension that declares no parameter at all. `pub(crate)` because the
/// `DeleteExtensionAssociation` suite associates the same one, so both resolve it from the
/// published catalogue rather than naming an extension.
pub(crate) fn authored_without_parameter() -> &'static str {
    AWS_AUTHORED_EXTENSIONS
        .iter()
        .find(|authored| authored.parameters.is_empty())
        .map(|authored| authored.id)
        .expect("the published catalogue declares at least one extension with no parameter")
}

/// The create answers the record it stored, and `GetExtensionAssociation` reports that
/// same record back.
#[tokio::test]
async fn create_extension_association_persists_the_binding_and_get_reports_it() {
    let table: Vec<Row> = vec![
        Row {
            case: "an AWS-authored extension on an application, by ARN",
            extension: |_| authored_with_parameter().to_string(),
            resource: |tree| tree.application_arn.clone(),
            resource_arn: |tree| tree.application_arn.clone(),
            version: None,
            parameters: Some(json!({
                authored_parameter(): arn("queue/lifecycle-events"),
            })),
            tags: None,
            expected_version: 1,
        },
        Row {
            case: "an AWS-authored extension on an environment, version pinned",
            extension: |_| authored_with_parameter().to_string(),
            resource: |tree| tree.environment_arn.clone(),
            resource_arn: |tree| tree.environment_arn.clone(),
            version: Some(1),
            parameters: Some(json!({
                authored_parameter(): arn("queue/other-events"),
            })),
            tags: Some(json!({ "team": "platform" })),
            expected_version: 1,
        },
        Row {
            case: "an AWS-authored extension declaring no parameter, on a profile",
            extension: |_| authored_without_parameter().to_string(),
            resource: |tree| tree.profile_arn.clone(),
            resource_arn: |tree| tree.profile_arn.clone(),
            version: None,
            parameters: None,
            tags: None,
            expected_version: 1,
        },
        Row {
            case: "an extension this account created, by id, every parameter supplied",
            extension: |fixtures| fixtures.created_id.clone(),
            resource: |tree| tree.profile_arn.clone(),
            resource_arn: |tree| tree.profile_arn.clone(),
            version: None,
            parameters: Some(json!({ "mandatory": "yes", "optional": "also set" })),
            tags: Some(json!({ "only": "tag" })),
            expected_version: 1,
        },
        Row {
            case: "an extension this account created, by name, only the required parameter",
            extension: |fixtures| fixtures.created_name.clone(),
            resource: |tree| tree.environment_arn.clone(),
            resource_arn: |tree| tree.environment_arn.clone(),
            version: None,
            parameters: Some(json!({ "mandatory": "yes" })),
            tags: None,
            expected_version: 1,
        },
        Row {
            // `ResourceIdentifier` is an `Identifier`, the shape every "ID or name" member
            // of this service targets, so an application's own id addresses it and the
            // association reports that application's ARN.
            case: "an application named by its id rather than its ARN",
            extension: |_| authored_without_parameter().to_string(),
            resource: |tree| tree.application.clone(),
            resource_arn: |tree| tree.application_arn.clone(),
            version: None,
            parameters: None,
            tags: None,
            expected_version: 1,
        },
    ];

    let router = router();
    let tree = tree(&router).await;
    let created_name = "association-fixture".to_string();
    let fixtures = Fixtures {
        created_id: created_extension(&router, &created_name).await,
        created_name,
    };

    let mut minted: Vec<String> = Vec::new();
    for row in &table {
        let case = row.case;
        let mut body = Map::new();
        body.insert(
            "ExtensionIdentifier".to_string(),
            Value::from((row.extension)(&fixtures)),
        );
        body.insert(
            "ResourceIdentifier".to_string(),
            Value::from((row.resource)(&tree)),
        );
        if let Some(version) = row.version {
            body.insert("ExtensionVersionNumber".to_string(), Value::from(version));
        }
        if let Some(parameters) = &row.parameters {
            body.insert("Parameters".to_string(), parameters.clone());
        }
        if let Some(tags) = &row.tags {
            body.insert("Tags".to_string(), tags.clone());
        }

        let answer = associate(&router, &Value::Object(body)).await;
        assert_eq!(
            answer.status,
            201,
            "case {case:?}: CreateExtensionAssociation expected 201, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let created = answer.json();

        let id = created
            .get("Id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: the create answered no Id: {created}"))
            .to_string();
        assert!(
            is_modelled_id(&id),
            "case {case:?}: expected a minted id matching ^[a-z0-9]{{4,7}}$, got {id:?}"
        );
        assert!(
            !minted.contains(&id),
            "case {case:?}: every association gets its own id, got {id:?} twice"
        );
        minted.push(id.clone());

        assert_eq!(
            created.get("Arn").and_then(Value::as_str),
            Some(arn(&format!("extensionassociation/{id}")).as_str()),
            "case {case:?}: expected the association's own ARN, got {created}"
        );
        assert_eq!(
            created.get("ResourceArn").and_then(Value::as_str),
            Some((row.resource_arn)(&tree).as_str()),
            "case {case:?}: expected the bound resource's ARN, got {created}"
        );
        assert_eq!(
            created.get("ExtensionVersionNumber").and_then(Value::as_i64),
            Some(i64::from(row.expected_version)),
            "case {case:?}: expected version {}, got {created}",
            row.expected_version
        );
        // The extension end is the ARN of that extension VERSION, which is what the
        // extension's own reader reports for it.
        let extension_identifier = (row.extension)(&fixtures);
        let read_extension = crate::extensions::get_extension(
            &router,
            &extension_identifier,
            Some(row.expected_version),
        )
        .await;
        assert_eq!(
            created.get("ExtensionArn").cloned(),
            read_extension.json().get("Arn").cloned(),
            "case {case:?}: expected the extension version's own ARN, got {created}"
        );
        let expected_parameters = row
            .parameters
            .clone()
            .unwrap_or_else(|| Value::Object(Map::new()));
        assert_eq!(
            created.get("Parameters").cloned(),
            Some(expected_parameters),
            "case {case:?}: expected the supplied parameter values, got {created}"
        );

        // The reader reports the stored record, member for member.
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
            facts(&created),
            "case {case:?}: the reader must report what the create stored, got {}",
            read.json()
        );

        // The tags the create was asked to assign are the ones the tagging reader reports
        // for the association's ARN.
        let reported = list_tags(&router, &arn(&format!("extensionassociation/{id}"))).await;
        assert_eq!(
            reported.status,
            200,
            "case {case:?}: ListTagsForResource expected 200, got {} / {}",
            reported.status,
            String::from_utf8_lossy(&reported.body)
        );
        assert_eq!(
            reported.json().get("Tags").cloned(),
            Some(row.tags.clone().unwrap_or_else(|| Value::Object(Map::new()))),
            "case {case:?}: expected the assigned tags, got {}",
            reported.json()
        );
    }

    // Every association is still readable after the whole table ran, so one create did not
    // overwrite another.
    for id in &minted {
        let read = get_association(&router, id).await;
        assert_eq!(
            read.status,
            200,
            "association {id:?} must still be readable, got {} / {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
    }
}

/// A deployment records the associations in force when it was started as its
/// `AppliedExtensions`, and an extension version an association still binds cannot be
/// deleted.
///
/// Rows differ along WHICH level the association binds (the application, the environment,
/// the configuration profile, and a sibling environment the deployment does not run in),
/// so the applied list is the union of the three levels the deployment runs and nothing
/// else.
#[tokio::test]
async fn a_deployment_applies_the_associations_in_force_when_it_started() {
    let router = router();
    let application = create_application(&router, "applied-app").await;
    let environment = create_environment(&router, &application, "applied-env").await;
    let other_environment = create_environment(&router, &application, "other-env").await;
    let profile = create_profile(&router, &application, "applied-profile").await;
    let other_profile = create_profile(&router, &application, "other-profile").await;

    // Something to roll out, so the deployment has a configuration version.
    let hosted = crate::appconfig_workflow::send(
        &router,
        "POST",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        "application/json",
        br#"{"enabled":true}"#.to_vec(),
        &[],
    )
    .await;
    assert_eq!(
        hosted.status,
        201,
        "the configuration to deploy must be hosted first, got {} / {}",
        hosted.status,
        String::from_utf8_lossy(&hosted.body)
    );

    let authored = authored_without_parameter();
    // One association per level the deployment runs, plus two the deployment must NOT
    // apply: they bind siblings it does not run.
    let applied_rows = [
        ("the application", arn(&format!("application/{application}"))),
        (
            "the environment",
            arn(&format!("application/{application}/environment/{environment}")),
        ),
        (
            "the configuration profile",
            arn(&format!(
                "application/{application}/configurationprofile/{profile}"
            )),
        ),
    ];
    let unapplied_rows = [
        (
            "another environment",
            arn(&format!(
                "application/{application}/environment/{other_environment}"
            )),
        ),
        (
            "another configuration profile",
            arn(&format!(
                "application/{application}/configurationprofile/{other_profile}"
            )),
        ),
    ];

    let mut expected: Vec<String> = Vec::new();
    for (case, resource) in applied_rows.iter().chain(unapplied_rows.iter()) {
        let answer = associate(
            &router,
            &json!({
                "ExtensionIdentifier": authored,
                "ResourceIdentifier": resource,
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
        if applied_rows.iter().any(|(name, _)| name == case) {
            expected.push(
                answer
                    .string("Id")
                    .unwrap_or_else(|| panic!("case {case:?}: the create answered no Id")),
            );
        }
    }
    expected.sort();

    let deployment = json_call(
        &router,
        "POST",
        &format!("/applications/{application}/environments/{environment}/deployments"),
        &json!({
            "DeploymentStrategyId": "AppConfig.AllAtOnce",
            "ConfigurationProfileId": profile,
            "ConfigurationVersion": "1",
        }),
    )
    .await;
    assert_eq!(
        deployment.status,
        201,
        "StartDeployment expected 201, got {} / {}",
        deployment.status,
        String::from_utf8_lossy(&deployment.body)
    );
    let body = deployment.json();
    let mut reported: Vec<String> = body
        .get("AppliedExtensions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the deployment must report AppliedExtensions, got {body}"))
        .iter()
        .map(|applied| {
            applied
                .get("ExtensionAssociationId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("an applied extension names its association, got {applied}")
                })
                .to_string()
        })
        .collect();
    reported.sort();
    assert_eq!(
        reported, expected,
        "expected exactly the associations bound to the three levels this deployment runs"
    );
    for applied in body
        .get("AppliedExtensions")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
    {
        assert_eq!(
            applied.get("ExtensionId").and_then(Value::as_str),
            Some(authored),
            "an applied extension names the extension it ran, got {applied}"
        );
        assert_eq!(
            applied.get("VersionNumber").and_then(Value::as_i64),
            Some(1),
            "an applied extension names the version it ran, got {applied}"
        );
    }

    // An association created AFTER the deployment started changes nothing about it: the
    // record reports what was applied then.
    let later = associate(
        &router,
        &json!({
            "ExtensionIdentifier": authored,
            "ResourceIdentifier": arn(&format!("application/{application}")),
        }),
    )
    .await;
    assert_eq!(
        later.status,
        201,
        "a later association must still be created, got {} / {}",
        later.status,
        String::from_utf8_lossy(&later.body)
    );
    let number = body
        .get("DeploymentNumber")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("the deployment must report its number, got {body}"));
    let read = json_call(
        &router,
        "GET",
        &format!(
            "/applications/{application}/environments/{environment}/deployments/{number}"
        ),
        &Value::Null,
    )
    .await;
    let mut read_ids: Vec<String> = read
        .json()
        .get("AppliedExtensions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|applied| {
            applied
                .get("ExtensionAssociationId")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    read_ids.sort();
    assert_eq!(
        read_ids, expected,
        "a later association must not change what an earlier deployment applied"
    );
}

/// An extension version an association still binds cannot be deleted: "You must delete
/// all associations to an extension before you delete the extension."
///
/// Rows differ along which identifier form names the extension in the delete, and the
/// refusal is lifted for a version no association binds.
#[tokio::test]
async fn an_extension_version_an_association_binds_cannot_be_deleted() {
    let router = router();
    let tree = tree(&router).await;
    let name = "still-associated";
    let id = created_extension(&router, name).await;

    let associated = associate(
        &router,
        &json!({
            "ExtensionIdentifier": id,
            "ResourceIdentifier": tree.application_arn,
            "Parameters": { "mandatory": "yes" },
        }),
    )
    .await;
    assert_eq!(
        associated.status,
        201,
        "CreateExtensionAssociation expected 201, got {} / {}",
        associated.status,
        String::from_utf8_lossy(&associated.body)
    );

    for (case, identifier) in [
        ("by id", id.clone()),
        ("by name", name.to_string()),
        ("by ARN", arn(&format!("extension/{id}/1"))),
    ] {
        let answer = json_call(
            &router,
            "DELETE",
            &format!("/extensions/{}", pct(&identifier)),
            &Value::Null,
        )
        .await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected 400 while an association binds the version, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            code(&answer),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {}",
            answer.error_code()
        );
    }

    // A SECOND version nothing binds deletes normally, so the refusal is about the version
    // the association names rather than about the extension as a whole. `VersionNumber` is
    // bound to `@httpQuery("version")`.
    let second = create_extension(
        &router,
        &json!({
            "Name": name,
            "Actions": actions(&[ACTION_POINT_VALUES[1]]),
        }),
        Some(1),
    )
    .await;
    assert_eq!(
        second.status,
        201,
        "a second version must be created, got {} / {}",
        second.status,
        String::from_utf8_lossy(&second.body)
    );
    let deleted = json_call(
        &router,
        "DELETE",
        &format!("/extensions/{}?version=2", pct(&id)),
        &Value::Null,
    )
    .await;
    assert_eq!(
        deleted.status,
        204,
        "a version no association binds must delete, got {} / {}",
        deleted.status,
        String::from_utf8_lossy(&deleted.body)
    );

    // …and version 1, which the association still binds, is still refused and still there.
    let refused = json_call(
        &router,
        "DELETE",
        &format!("/extensions/{}?version=1", pct(&id)),
        &Value::Null,
    )
    .await;
    assert_eq!(
        refused.status,
        400,
        "the bound version must still be refused, got {} / {}",
        refused.status,
        String::from_utf8_lossy(&refused.body)
    );
    let read = crate::extensions::get_extension(&router, &id, Some(1)).await;
    assert_eq!(
        read.status,
        200,
        "the bound version must still be readable, got {} / {}",
        read.status,
        String::from_utf8_lossy(&read.body)
    );
}

///
/// Rows differ along WHICH end is wrong (the extension, the resource), HOW the resource
/// ARN misses (a resource kind no extension associates to, an id nothing holds) and WHICH
/// parameter rule is broken (a required value missing, a name the version never declared).
#[tokio::test]
async fn an_association_this_service_cannot_bind_is_refused() {
    let router = router();
    let tree = tree(&router).await;
    let created_name = "refusal-fixture";
    let created_id = created_extension(&router, created_name).await;

    let authored = authored_with_parameter();
    let parameter = authored_parameter();
    let queue = arn("queue/lifecycle-events");

    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "an extension nobody created",
            json!({
                "ExtensionIdentifier": "no-such-extension",
                "ResourceIdentifier": tree.application_arn,
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a version the extension does not hold",
            json!({
                "ExtensionIdentifier": authored,
                "ExtensionVersionNumber": 7,
                "ResourceIdentifier": tree.application_arn,
                "Parameters": { parameter: queue },
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a resource ARN nothing holds",
            json!({
                "ExtensionIdentifier": authored,
                "ResourceIdentifier": arn("application/nosuchapp"),
                "Parameters": { parameter: queue },
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a resource kind no extension associates to",
            json!({
                "ExtensionIdentifier": authored,
                "ResourceIdentifier": arn("deploymentstrategy/AppConfig.AllAtOnce"),
                "Parameters": { parameter: queue },
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a required parameter left out",
            json!({
                "ExtensionIdentifier": authored,
                "ResourceIdentifier": tree.application_arn,
            }),
            400,
            "BadRequestException",
        ),
        (
            "a parameter the version does not declare",
            json!({
                "ExtensionIdentifier": authored,
                "ResourceIdentifier": tree.application_arn,
                "Parameters": { parameter: queue.clone(), "notAParameter": "x" },
            }),
            400,
            "BadRequestException",
        ),
        (
            "a required parameter of a created extension left out",
            json!({
                "ExtensionIdentifier": created_id,
                "ResourceIdentifier": tree.profile_arn,
                "Parameters": { "optional": "only" },
            }),
            400,
            "BadRequestException",
        ),
    ];

    for (case, body, status, expected_code) in cases {
        let answer = associate(&router, &body).await;
        assert_eq!(
            answer.status,
            status,
            "case {case:?}: expected {status}, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            code(&answer),
            expected_code,
            "case {case:?}: expected {expected_code}, got {}",
            answer.error_code()
        );
    }

    // An id no create minted is a 404 from the reader, which is also what every refused
    // request above left behind: nothing.
    for id in ["abcd", "zzzzzzz"] {
        let read = get_association(&router, id).await;
        assert_eq!(
            read.status,
            404,
            "case {id:?}: expected 404 for an association nobody created, got {} / {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            code(&read),
            "ResourceNotFoundException",
            "case {id:?}: expected ResourceNotFoundException, got {}",
            read.error_code()
        );
    }

    // The ends the refusals named are untouched: the one legal association still works.
    let answer = associate(
        &router,
        &json!({
            "ExtensionIdentifier": authored,
            "ResourceIdentifier": tree.application_arn,
            "Parameters": { parameter: queue },
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "after the refusals a well-formed association must still be created, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}

/// The extension VERSION an association binds is the one the request pins — by number, or
/// by naming that version's own ARN as the `ExtensionIdentifier` — and the extension's
/// HIGHEST version when the request pins none: "the version number of the extension. If
/// not specified, AppConfig uses the maximum version of the extension"
/// (`smithy_query(query="doc",shape="CreateExtensionAssociationRequest",member="ExtensionVersionNumber")`).
///
/// The rows differ along how the version is selected, over an extension that holds MORE
/// than one version — which is what makes "the maximum version" distinguishable from
/// "version 1" — and along which resource kind the association binds.
#[tokio::test]
async fn the_version_an_association_binds_is_the_one_pinned_or_the_extensions_highest() {
    let router = router();
    let tree = tree(&router).await;
    let name = "two-version-extension";
    let extension_id = created_extension(&router, name).await;

    // One more version under the same name: `LatestVersionNumber` is the optimistic lock on
    // the name's current latest version, so naming 1 appends version 2.
    let appended = create_extension(
        &router,
        &json!({
            "Name": name,
            "Actions": actions(&[ACTION_POINT_VALUES[0]]),
            "Parameters": {
                "mandatory": { "Required": true, "Dynamic": false },
                "optional": { "Required": false, "Dynamic": false },
            },
        }),
        Some(1),
    )
    .await;
    assert_eq!(
        appended.status,
        201,
        "a second version of {name:?} must be created, got {} / {}",
        appended.status,
        String::from_utf8_lossy(&appended.body)
    );
    let highest = appended
        .json()
        .get("VersionNumber")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("the appended version answered no VersionNumber"));
    assert!(
        highest > 1,
        "the fixture must hold more than one version, got {highest}"
    );
    let first_version_arn = get_extension(&router, &extension_id, Some(1))
        .await
        .string("Arn")
        .unwrap_or_else(|| panic!("version 1 of {name:?} must report an Arn"));

    // (case, the `ExtensionIdentifier`, the version the request names, the resource, the
    // version the association must bind)
    let rows: Vec<(&str, String, Option<i64>, String, i64)> = vec![
        (
            "no version named binds the highest",
            extension_id.clone(),
            None,
            tree.application_arn.clone(),
            highest,
        ),
        (
            "version 1 named explicitly binds version 1",
            extension_id.clone(),
            Some(1),
            tree.environment_arn.clone(),
            1,
        ),
        (
            "the first version's own ARN pins that version",
            first_version_arn.clone(),
            None,
            tree.profile_arn.clone(),
            1,
        ),
        (
            "the highest named explicitly binds the highest",
            extension_id.clone(),
            Some(highest),
            tree.profile_arn.clone(),
            highest,
        ),
    ];

    for (case, extension, version, resource, expected_version) in &rows {
        let mut body = Map::new();
        body.insert(
            "ExtensionIdentifier".to_string(),
            Value::from(extension.clone()),
        );
        body.insert(
            "ResourceIdentifier".to_string(),
            Value::from(resource.clone()),
        );
        if let Some(version) = version {
            body.insert("ExtensionVersionNumber".to_string(), Value::from(*version));
        }
        body.insert("Parameters".to_string(), json!({ "mandatory": "yes" }));

        let created = associate(&router, &Value::Object(body)).await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: CreateExtensionAssociation expected 201, got {} / {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let created_body = created.json();
        assert_eq!(
            created_body
                .get("ExtensionVersionNumber")
                .and_then(Value::as_i64),
            Some(*expected_version),
            "case {case:?}: expected version {expected_version}, got {created_body}"
        );
        // The extension end is the ARN that extension VERSION carries, which is what the
        // extension's own reader reports for it.
        let version_arn = get_extension(&router, &extension_id, Some(*expected_version as i32))
            .await
            .string("Arn");
        assert_eq!(
            created_body.get("ExtensionArn").and_then(Value::as_str),
            version_arn.as_deref(),
            "case {case:?}: expected the bound version's own ARN, got {created_body}"
        );
        assert_eq!(
            created_body.get("ResourceArn").and_then(Value::as_str),
            Some(resource.as_str()),
            "case {case:?}: expected the bound resource's ARN, got {created_body}"
        );

        // What the create answered is what the reader reports, so the pinned version was
        // persisted rather than recomputed per read.
        let id = created_body
            .get("Id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: the create answered no Id: {created_body}"));
        let read = get_association(&router, id).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: GetExtensionAssociation expected 200, got {} / {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            facts(&read.json()),
            facts(&created_body),
            "case {case:?}: the reader must report what the create stored, got {}",
            read.json()
        );
    }
}

