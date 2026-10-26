// SPDX-License-Identifier: CC0-1.0

//! `DeleteExtensionAssociation`.
//!
//! The rule under test, in one sentence: a delete that answers its modelled 204 has taken
//! the addressed extension association out of this service's state — so every later call
//! naming that id (the reader, the writer, the tagging reader, the listing, a second
//! delete) is refused with `ResourceNotFoundException` (HTTP 404) or stops reporting it,
//! while the extension the association defined and every other association stay exactly
//! as they were.
//!
//! Every case drives real HTTP requests through the router the emulator's front door uses,
//! so what is asserted is what an AppConfig client sees on the wire.
//!
//! The table spans what the rule generalises over rather than one recorded association:
//! which end of the extension catalogue the association defines (an AWS-authored
//! extension, one this account created, addressed by id and by name), which of the three
//! associable resource kinds is bound and in which identifier form (ARN, and an
//! application's own id), whether a version is pinned, how many parameter values were
//! supplied, and whether tags were assigned. A second table spans the forms of a
//! well-formed `ExtensionAssociationId` (the model's `Id`, `^[a-z0-9]{4,7}$`, at both
//! published length bounds) that this account never minted.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::ops::types::ACTION_POINT_VALUES;

use crate::appconfig_workflow::{
    create_application, create_environment, create_profile, json_call, router, send, Answer,
};
use crate::extension_associations::{associate, authored_without_parameter, get_association};
use crate::extensions::{actions, create_extension, get_extension, minted_id};
use crate::resource_deletes::{assert_deleted, assert_not_found};
use crate::resource_tags::{arn, list_tags, pct};

/// `DELETE /extensionassociations/{ExtensionAssociationId}` — no body either way.
async fn delete_association(router: &MultiServiceRouter, id: &str) -> Answer {
    send(
        router,
        "DELETE",
        &format!("/extensionassociations/{}", pct(id)),
        "application/json",
        Vec::new(),
        &[],
    )
    .await
}

/// `PATCH /extensionassociations/{ExtensionAssociationId}`, the writer that must miss on a
/// deleted association too.
async fn update_association(router: &MultiServiceRouter, id: &str) -> Answer {
    json_call(
        router,
        "PATCH",
        &format!("/extensionassociations/{}", pct(id)),
        &json!({ "Parameters": {} }),
    )
    .await
}

/// The `Id` of every association `ListExtensionAssociations` reports for this account.
async fn listed_ids(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    // `MaxResults` has a documented range of 1..50 and this suite stores fewer than that,
    // so one unpaged listing reports the whole account.
    let answer = json_call(router, "GET", "/extensionassociations", &Value::Null).await;
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
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            item.get("Id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a listed association must carry an Id, got {item}")
                })
                .to_string()
        })
        .collect()
}

/// One application, with an environment and a configuration profile under it, and the
/// three ARNs an extension may be associated to.
struct Tree {
    application: String,
    application_arn: String,
    environment_arn: String,
    profile_arn: String,
}

async fn tree(router: &MultiServiceRouter, name: &str) -> Tree {
    let application = create_application(router, name).await;
    let environment = create_environment(router, &application, &format!("{name}-env")).await;
    let profile = create_profile(router, &application, &format!("{name}-profile")).await;
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
/// one, at two versions so a row can pin either. Answers its minted id.
async fn created_extension(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({
        "Name": name,
        "Actions": actions(&[ACTION_POINT_VALUES[0]]),
        "Parameters": {
            "mandatory": { "Required": true, "Dynamic": false },
            "optional": { "Required": false, "Dynamic": false },
        },
    });
    let first = create_extension(router, &body, None).await;
    assert_eq!(
        first.status,
        201,
        "case {name:?}: CreateExtension expected 201, got {} / {}",
        first.status,
        String::from_utf8_lossy(&first.body)
    );
    let id = minted_id(name, &first.json());
    // `LatestVersionNumber` is the optimistic lock on the name, so passing the version it
    // is at appends another version to the same extension.
    let second = create_extension(router, &body, Some(1)).await;
    assert_eq!(
        second.status,
        201,
        "case {name:?}: CreateExtension (version 2) expected 201, got {} / {}",
        second.status,
        String::from_utf8_lossy(&second.body)
    );
    id
}

/// What the rows resolve their extension identifiers against.
struct Fixtures {
    created_id: String,
    created_name: String,
}

/// One row of the delete table: an association to create, then to delete.
struct Row {
    case: &'static str,
    /// The `ExtensionIdentifier` the association defines, resolved at run time.
    extension: fn(&Fixtures) -> String,
    /// The `ResourceIdentifier` the association binds.
    resource: fn(&Tree) -> String,
    version: Option<i32>,
    parameters: Option<Value>,
    tags: Option<Value>,
}

/// An association created from a row, with what a delete of it must leave alone.
struct Created {
    case: &'static str,
    id: String,
    association_arn: String,
    extension_identifier: String,
    tags: Option<Value>,
}

/// The whole table, created once so that each delete is asserted against the associations
/// that must survive it.
async fn create_table(
    router: &MultiServiceRouter,
    table: &[Row],
    tree: &Tree,
    fixtures: &Fixtures,
) -> Vec<Created> {
    let mut created = Vec::new();
    for row in table {
        let case = row.case;
        let extension_identifier = (row.extension)(fixtures);
        let mut body = Map::new();
        body.insert(
            "ExtensionIdentifier".to_string(),
            Value::from(extension_identifier.clone()),
        );
        body.insert(
            "ResourceIdentifier".to_string(),
            Value::from((row.resource)(tree)),
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

        let answer = associate(router, &Value::Object(body)).await;
        assert_eq!(
            answer.status,
            201,
            "case {case:?}: CreateExtensionAssociation expected 201, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let created_body = answer.json();
        let id = created_body
            .get("Id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!("case {case:?}: the create answered no Id: {created_body}")
            })
            .to_string();
        let association_arn = created_body
            .get("Arn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!("case {case:?}: the create answered no Arn: {created_body}")
            })
            .to_string();
        created.push(Created {
            case,
            id,
            association_arn,
            extension_identifier,
            tags: row.tags.clone(),
        });
    }
    created
}

/// Each delete takes exactly the association it addressed out of state, and nothing else.
#[tokio::test]
async fn deleting_an_association_takes_it_out_of_state_and_leaves_the_rest() {
    let table: Vec<Row> = vec![
        Row {
            case: "an AWS-authored extension on an application, by ARN",
            extension: |_| authored_without_parameter().to_string(),
            resource: |tree| tree.application_arn.clone(),
            version: None,
            parameters: None,
            tags: None,
        },
        Row {
            case: "an AWS-authored extension on an environment, tagged",
            extension: |_| authored_without_parameter().to_string(),
            resource: |tree| tree.environment_arn.clone(),
            version: Some(1),
            parameters: None,
            tags: Some(json!({ "team": "platform", "tier": "test" })),
        },
        Row {
            case: "an AWS-authored extension on a configuration profile",
            extension: |_| authored_without_parameter().to_string(),
            resource: |tree| tree.profile_arn.clone(),
            version: None,
            parameters: None,
            tags: None,
        },
        Row {
            case: "an account extension by id, on an application named by its id, tagged",
            extension: |fixtures| fixtures.created_id.clone(),
            resource: |tree| tree.application.clone(),
            version: None,
            parameters: Some(json!({ "mandatory": "yes", "optional": "also set" })),
            tags: Some(json!({ "only": "tag" })),
        },
        Row {
            case: "an account extension by name, version 1 pinned, one parameter",
            extension: |fixtures| fixtures.created_name.clone(),
            resource: |tree| tree.environment_arn.clone(),
            version: Some(1),
            parameters: Some(json!({ "mandatory": "yes" })),
            tags: None,
        },
        Row {
            case: "an account extension by id, version 2 pinned, one parameter",
            extension: |fixtures| fixtures.created_id.clone(),
            resource: |tree| tree.profile_arn.clone(),
            version: Some(2),
            parameters: Some(json!({ "mandatory": "yes" })),
            tags: None,
        },
    ];

    let router = router();
    let tree = tree(&router, "deleted-association-app").await;
    let created_name = "deleted-association-extension".to_string();
    let fixtures = Fixtures {
        created_id: created_extension(&router, &created_name).await,
        created_name,
    };
    let created = create_table(&router, &table, &tree, &fixtures).await;

    let mut remaining: Vec<String> = created.iter().map(|row| row.id.clone()).collect();
    for row in &created {
        let case = row.case;

        // The association is there to begin with, with whatever tags it was created with.
        let before = get_association(&router, &row.id).await;
        assert_eq!(
            before.status,
            200,
            "case {case:?}: GetExtensionAssociation must report the association before the \
             delete, got {} / {}",
            before.status,
            String::from_utf8_lossy(&before.body)
        );
        if let Some(tags) = &row.tags {
            let tagged = list_tags(&router, &row.association_arn).await;
            assert_eq!(
                tagged.json().get("Tags"),
                Some(tags),
                "case {case:?}: expected the tags the create assigned, got {}",
                tagged.json()
            );
        }

        let deleted = delete_association(&router, &row.id).await;
        assert_deleted(case, "DeleteExtensionAssociation", &deleted);

        // Every later call naming the id misses, because each of them reads the map the
        // delete took the record out of.
        assert_not_found(
            case,
            "GetExtensionAssociation",
            &get_association(&router, &row.id).await,
        );
        assert_not_found(
            case,
            "UpdateExtensionAssociation",
            &update_association(&router, &row.id).await,
        );
        assert_not_found(
            case,
            "ListTagsForResource",
            &list_tags(&router, &row.association_arn).await,
        );
        assert_not_found(
            case,
            "a second DeleteExtensionAssociation",
            &delete_association(&router, &row.id).await,
        );

        // "This action doesn't delete extensions defined in the association": the
        // extension this association defined still reads.
        let extension = get_extension(&router, &row.extension_identifier, None).await;
        assert_eq!(
            extension.status,
            200,
            "case {case:?}: GetExtension({}) must still answer 200 after the association \
             delete, got {} / {}",
            row.extension_identifier,
            extension.status,
            String::from_utf8_lossy(&extension.body)
        );

        // The listing reports exactly the associations that have not been deleted yet.
        remaining.retain(|id| id != &row.id);
        let mut listed = listed_ids(&router, case).await;
        listed.sort();
        let mut expected = remaining.clone();
        expected.sort();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the listing to report {expected:?}, got {listed:?}"
        );
    }
}

/// An id this account never minted is refused, and the refusal removes nothing.
#[tokio::test]
async fn an_id_this_account_never_minted_is_refused_and_removes_nothing() {
    // The model's `Id` is `^[a-z0-9]{4,7}$`, so the table walks both published length
    // bounds and the alphabet's two halves.
    let table = [
        ("the shortest well-formed id", "abcd"),
        ("the longest well-formed id", "abcdefg"),
        ("digits only", "0000"),
        ("letters and digits", "a1b2c3d"),
    ];

    let router = router();
    let tree = tree(&router, "surviving-association-app").await;
    let association = json!({
        "ExtensionIdentifier": authored_without_parameter(),
        "ResourceIdentifier": tree.application_arn,
    });
    let created = associate(&router, &association).await;
    assert_eq!(
        created.status,
        201,
        "CreateExtensionAssociation expected 201, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let survivor = created
        .json()
        .get("Id")
        .and_then(Value::as_str)
        .expect("the create must answer an Id")
        .to_string();

    for (case, id) in table {
        assert_ne!(
            id, survivor,
            "case {case:?}: this id must be one the account never minted, got the stored {survivor:?}"
        );
        assert_not_found(
            case,
            "DeleteExtensionAssociation",
            &delete_association(&router, id).await,
        );
        let listed = listed_ids(&router, case).await;
        assert_eq!(
            listed,
            vec![survivor.clone()],
            "case {case:?}: a refused delete removes nothing, expected {:?}, got {listed:?}",
            vec![survivor.clone()]
        );
    }
}

/// Deleting the last association of an extension version releases the `DeleteExtension`
/// that version was blocking: the association really left this service's state.
#[tokio::test]
async fn deleting_the_last_association_releases_the_extension_delete_it_blocked() {
    let router = router();
    let tree = tree(&router, "released-extension-app").await;
    let name = "released-extension";
    let body = json!({
        "Name": name,
        "Actions": actions(&[ACTION_POINT_VALUES[0]]),
    });
    let created = create_extension(&router, &body, None).await;
    assert_eq!(
        created.status,
        201,
        "CreateExtension expected 201, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let extension_id = minted_id(name, &created.json());

    let association = associate(
        &router,
        &json!({
            "ExtensionIdentifier": extension_id,
            "ResourceIdentifier": tree.profile_arn,
        }),
    )
    .await;
    assert_eq!(
        association.status,
        201,
        "CreateExtensionAssociation expected 201, got {} / {}",
        association.status,
        String::from_utf8_lossy(&association.body)
    );
    let association_id = association
        .json()
        .get("Id")
        .and_then(Value::as_str)
        .expect("the create must answer an Id")
        .to_string();

    let case = "an extension whose only association binds its only version";
    let blocked = send(
        &router,
        "DELETE",
        &format!("/extensions/{}", pct(&extension_id)),
        "application/json",
        Vec::new(),
        &[],
    )
    .await;
    assert_eq!(
        blocked.status,
        400,
        "case {case:?}: DeleteExtension must be refused while an association binds the \
         version, got {} / {}",
        blocked.status,
        String::from_utf8_lossy(&blocked.body)
    );

    assert_deleted(
        case,
        "DeleteExtensionAssociation",
        &delete_association(&router, &association_id).await,
    );

    let released = send(
        &router,
        "DELETE",
        &format!("/extensions/{}", pct(&extension_id)),
        "application/json",
        Vec::new(),
        &[],
    )
    .await;
    assert_deleted(case, "DeleteExtension", &released);
    assert_not_found(
        case,
        "GetExtension",
        &get_extension(&router, &extension_id, None).await,
    );
}
