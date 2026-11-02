// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteAnalysisTemplate`.
//!
//! The rules under test:
//!
//! 1. Deleting an analysis template removes exactly the addressed resource from the
//!    store: the reader answers 404 for it, the membership's listing no longer carries
//!    it, its tags are gone with it, every other template of that membership is
//!    untouched, and the name it held is free for a new template — which gets its own
//!    identifier rather than resurrecting the deleted one.
//! 2. An identifier the membership in the path holds no template under — never minted,
//!    already deleted, or belonging to another membership — is a `ResourceNotFoundException`
//!    that removes nothing, and an unknown membership reaches no template at all.
//!
//! Each table varies what those rules generalise over (the analysis format, the source
//! kind, the shape of the name, whether the template carries tags, and WHICH of a
//! membership's templates is deleted — first, middle or last in creation order), so the
//! recorded single-template case is one row among several. Requests go over the wire
//! through the same `MultiServiceRouter` the production router uses.

use serde_json::{json, Value};

use super::cleanrooms_analysis_templates::{
    artifacts_source, create_analysis_template, get_analysis_template, membership_id,
    synthetic_data_parameters, NAME_MAX_LEN,
};
use super::cleanrooms_collaboration_lifecycle::{account_id, error_code, error_message, Emulator};
use super::resource_tags::label;

/// Ids in the model's `UUID` form (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`)
/// that no create in these tests mints.
const NEVER_MINTED_IDS: [&str; 3] = [
    "00000000-0000-0000-0000-000000000000",
    "ffffffff-ffff-ffff-ffff-ffffffffffff",
    "deadbeef-0000-4000-8000-000000000001",
];

/// `DELETE /memberships/{membershipIdentifier}/analysistemplates/{analysisTemplateIdentifier}`.
async fn delete_analysis_template(
    emulator: &Emulator,
    membership: &str,
    template: &str,
) -> (u16, Value) {
    emulator
        .call(
            "DELETE",
            &format!("/memberships/{membership}/analysistemplates/{template}"),
            None,
        )
        .await
}

/// Every analysis template id `ListAnalysisTemplates` reports for `membership`, in the
/// order it pages them.
async fn listed_ids(emulator: &Emulator, case: &str, membership: &str) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let path = match &next_token {
            Some(token) => {
                format!("/memberships/{membership}/analysistemplates?nextToken={token}")
            }
            None => format!("/memberships/{membership}/analysistemplates"),
        };
        let (status, page) = emulator.call("GET", &path, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListAnalysisTemplates 200, got {status} with {page}"
        );
        let summaries = page["analysisTemplateSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no analysisTemplateSummaries in {page}"))
            .clone();
        for summary in &summaries {
            ids.push(
                summary["id"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: no summary id in {page}"))
                    .to_string(),
            );
        }
        next_token = page["nextToken"].as_str().map(str::to_string);
        if next_token.is_none() {
            return ids;
        }
    }
}

/// One row of the deletion table: the three templates a membership is filled with and
/// which of them this row deletes.
struct DeletionCase {
    label: &'static str,
    /// The create bodies, in creation order. The row deletes `target`.
    bodies: Vec<Value>,
    /// Index into `bodies` of the template this row deletes.
    target: usize,
}

fn deletion_cases() -> Vec<DeletionCase> {
    // A plain SQL template, used as the neighbour that must survive every row.
    let neighbour = |name: &str| {
        json!({
            "name": name,
            "format": "SQL",
            "source": { "text": format!("SELECT id FROM {name}") },
        })
    };
    vec![
        DeletionCase {
            label: "first of three: minimal SQL template, no tags",
            bodies: vec![
                json!({
                    "name": "minimal",
                    "format": "SQL",
                    "source": { "text": "SELECT id FROM sales" },
                }),
                neighbour("second"),
                neighbour("third"),
            ],
            target: 0,
        },
        DeletionCase {
            label: "middle of three: SQL template with a description and tags",
            bodies: vec![
                neighbour("first"),
                json!({
                    "name": "tagged analysis with spaces",
                    "description": "joins two configured tables",
                    "format": "SQL",
                    "source": {
                        "text": "SELECT s.id, c.segment FROM sales s JOIN consumers c ON s.id = c.id",
                    },
                    "tags": { "env": "test", "owner": "fixer" },
                }),
                neighbour("third"),
            ],
            target: 1,
        },
        DeletionCase {
            label: "last of three: PySpark artifacts template with synthetic data parameters",
            bodies: vec![
                neighbour("first"),
                neighbour("second"),
                json!({
                    "name": "pyspark_artifacts",
                    "format": "PYSPARK_1_0",
                    "source": artifacts_source(),
                    "schema": { "referencedTables": ["sales"] },
                    "syntheticDataParameters": synthetic_data_parameters(),
                    "tags": { "env": "prod" },
                }),
                target_tail(),
            ],
            target: 2,
        },
        DeletionCase {
            label: "name at the declared maximum length (128)",
            bodies: vec![
                neighbour("first"),
                json!({
                    "name": "n".repeat(NAME_MAX_LEN),
                    "format": "SQL",
                    "source": { "text": "SELECT 1" },
                }),
                neighbour("third"),
            ],
            target: 1,
        },
    ]
}

/// A fourth template for the row that deletes the third one, so "last in creation order"
/// is tested as well as "followed by another template".
fn target_tail() -> Value {
    json!({
        "name": "created_after_the_target",
        "format": "SQL",
        "source": { "text": "SELECT id FROM consumers" },
    })
}

#[tokio::test]
async fn deleting_an_analysis_template_removes_it_and_frees_its_name() {
    let emulator = Emulator::new();

    for (row, case) in deletion_cases().into_iter().enumerate() {
        let label_text = case.label;
        // One collaboration (and so one membership) per row: an account holds at most
        // one membership per collaboration, and a template's name is taken per
        // membership.
        let membership = membership_id(&emulator, &format!("delete-collab-{row}")).await;

        let mut created: Vec<Value> = Vec::new();
        for body in &case.bodies {
            let (status, response) =
                create_analysis_template(&emulator, &membership, body.clone()).await;
            assert_eq!(
                status, 200,
                "case {label_text:?}: expected CreateAnalysisTemplate 200 for {body}, got {status} with {response}"
            );
            created.push(response["analysisTemplate"].clone());
        }

        let ids: Vec<String> = created
            .iter()
            .map(|template| {
                template["id"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {label_text:?}: no analysisTemplate.id"))
                    .to_string()
            })
            .collect();
        let target_id = ids[case.target].clone();
        let target_arn = created[case.target]["arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label_text:?}: no analysisTemplate.arn"))
            .to_string();
        let target_name = created[case.target]["name"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label_text:?}: no analysisTemplate.name"))
            .to_string();

        let listed = listed_ids(&emulator, label_text, &membership).await;
        assert_eq!(
            listed, ids,
            "case {label_text:?}: expected the created templates in creation order {ids:?}, got {listed:?}"
        );

        let (status, body) = delete_analysis_template(&emulator, &membership, &target_id).await;
        assert_eq!(
            status, 204,
            "case {label_text:?}: expected DeleteAnalysisTemplate 204, got {status} with {body}"
        );
        // `DeleteAnalysisTemplateOutput` models no members, so the response carries no
        // data of its own.
        assert_eq!(
            body.as_object().map(serde_json::Map::len).unwrap_or_default(),
            0,
            "case {label_text:?}: the response models no members, got {body}"
        );

        // The reader reports it gone, with the composed 404 of the resource that was not
        // found.
        let (status, refused) = get_analysis_template(&emulator, &membership, &target_id).await;
        assert_eq!(
            status, 404,
            "case {label_text:?}: expected GetAnalysisTemplate 404 after the delete, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {label_text:?}: expected ResourceNotFoundException, got {refused}"
        );
        assert_eq!(
            error_message(&refused),
            format!("analysis template {target_id} not found in membership {membership}"),
            "case {label_text:?}: unexpected message, got {refused}"
        );

        // Every other template of the membership survives, byte for byte, and the
        // listing reports exactly those, still in creation order.
        let expected_remaining: Vec<String> = ids
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != case.target)
            .map(|(_, id)| id.clone())
            .collect();
        for (index, id) in ids.iter().enumerate() {
            if index == case.target {
                continue;
            }
            let (status, fetched) = get_analysis_template(&emulator, &membership, id).await;
            assert_eq!(
                status, 200,
                "case {label_text:?}: template {id} must survive the delete, got {status} with {fetched}"
            );
            assert_eq!(
                fetched["analysisTemplate"], created[index],
                "case {label_text:?}: template {id} must be unchanged, got {:?}",
                fetched["analysisTemplate"]
            );
        }
        let listed = listed_ids(&emulator, label_text, &membership).await;
        assert_eq!(
            listed, expected_remaining,
            "case {label_text:?}: expected the surviving templates {expected_remaining:?}, got {listed:?}"
        );

        // Tags are keyed by the resource's ARN, so the deleted template's tags go with
        // it while a survivor's stay. An analysis template's ARN is longer than the
        // `CleanroomsArn` the tagging operations bind (`len ..100`), so the entry is read
        // where those operations read it: the store.
        {
            let store = emulator.store();
            let store = store.lock();
            assert!(
                !store.tags.contains_key(&target_arn),
                "case {label_text:?}: the deleted template's tags must go with it, {} still holds {:?}",
                label(&target_arn),
                store.tags.get(&target_arn)
            );
            for (index, template) in created.iter().enumerate() {
                if index == case.target {
                    continue;
                }
                let arn = template["arn"].as_str().expect("a template arn");
                // `AnalysisTemplate` models no `tags` member, so what the resource
                // carries is what its create was given.
                let expected = case.bodies[index].get("tags").is_some();
                assert_eq!(
                    store.tags.contains_key(arn),
                    expected,
                    "case {label_text:?}: template {arn} must keep its own tags, got {:?}",
                    store.tags.get(arn)
                );
            }
        }

        // Deleting it again reports what a read of it reports: there is nothing there.
        let (status, refused) = delete_analysis_template(&emulator, &membership, &target_id).await;
        assert_eq!(
            status, 404,
            "case {label_text:?}: a second delete of the same template is a 404, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {label_text:?}: expected ResourceNotFoundException, got {refused}"
        );

        // The name the deleted template held is free again, and the new template is its
        // own resource rather than the deleted one coming back.
        let mut reused = case.bodies[case.target].clone();
        reused["tags"] = json!({ "env": "recreated" });
        let (status, recreated) =
            create_analysis_template(&emulator, &membership, reused.clone()).await;
        assert_eq!(
            status, 200,
            "case {label_text:?}: the deleted template's name {target_name:?} must be free, got {status} with {recreated}"
        );
        let new_id = recreated["analysisTemplate"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label_text:?}: no analysisTemplate.id in {recreated}"))
            .to_string();
        assert_ne!(
            new_id, target_id,
            "case {label_text:?}: the re-created template must be a new resource"
        );
        let (status, fetched) = get_analysis_template(&emulator, &membership, &new_id).await;
        assert_eq!(
            status, 200,
            "case {label_text:?}: expected GetAnalysisTemplate 200 for the re-created template, got {status} with {fetched}"
        );
        assert_eq!(
            fetched["analysisTemplate"]["name"].as_str(),
            Some(target_name.as_str()),
            "case {label_text:?}: expected the re-used name, got {:?}",
            fetched["analysisTemplate"]["name"]
        );
        let listed = listed_ids(&emulator, label_text, &membership).await;
        let mut expected = expected_remaining.clone();
        expected.push(new_id.clone());
        assert_eq!(
            listed, expected,
            "case {label_text:?}: the re-created template lists last, expected {expected:?}, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn deleting_an_analysis_template_refuses_an_identifier_its_membership_does_not_hold() {
    let emulator = Emulator::new();
    let membership = membership_id(&emulator, "delete-refusal-collab").await;
    let other_membership = membership_id(&emulator, "delete-refusal-collab-other").await;

    // A template of each membership: the one this test keeps reading back, and one the
    // other membership holds, which is not deletable through the first.
    let (status, mine) = create_analysis_template(
        &emulator,
        &membership,
        json!({
            "name": "kept",
            "format": "SQL",
            "source": { "text": "SELECT id FROM sales" },
        }),
    )
    .await;
    assert_eq!(status, 200, "setup: CreateAnalysisTemplate: {mine}");
    let kept_id = mine["analysisTemplate"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: no analysisTemplate.id in {mine}"))
        .to_string();

    let (status, theirs) = create_analysis_template(
        &emulator,
        &other_membership,
        json!({
            "name": "elsewhere",
            "format": "SQL",
            "source": { "text": "SELECT id FROM consumers" },
        }),
    )
    .await;
    assert_eq!(status, 200, "setup: CreateAnalysisTemplate elsewhere: {theirs}");
    let elsewhere_id = theirs["analysisTemplate"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: no analysisTemplate.id in {theirs}"))
        .to_string();

    // A template created and then deleted under this membership: the identifier WAS
    // minted, so the row proves the refusal is about what the store holds now.
    let (status, gone) = create_analysis_template(
        &emulator,
        &membership,
        json!({
            "name": "gone",
            "format": "SQL",
            "source": { "text": "SELECT 1" },
        }),
    )
    .await;
    assert_eq!(status, 200, "setup: CreateAnalysisTemplate gone: {gone}");
    let gone_id = gone["analysisTemplate"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: no analysisTemplate.id in {gone}"))
        .to_string();
    let (status, body) = delete_analysis_template(&emulator, &membership, &gone_id).await;
    assert_eq!(status, 204, "setup: expected the first delete 204, got {status} with {body}");

    let mut cases: Vec<(String, String)> = NEVER_MINTED_IDS
        .iter()
        .map(|id| (format!("never minted {id}"), id.to_string()))
        .collect();
    cases.push(("minted then deleted".to_string(), gone_id));
    cases.push((
        "held by another membership".to_string(),
        elsewhere_id.clone(),
    ));

    for (case, template_id) in cases {
        let (status, refused) = delete_analysis_template(&emulator, &membership, &template_id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected DeleteAnalysisTemplate 404, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {refused}"
        );
        assert_eq!(
            error_message(&refused),
            format!("analysis template {template_id} not found in membership {membership}"),
            "case {case:?}: unexpected message, got {refused}"
        );
        assert_eq!(
            refused["resourceId"].as_str(),
            Some(template_id.as_str()),
            "case {case:?}: expected the requested id as resourceId, got {:?}",
            refused.get("resourceId")
        );
        assert_eq!(
            refused["resourceType"].as_str(),
            Some("ANALYSIS_TEMPLATE"),
            "case {case:?}: expected resourceType ANALYSIS_TEMPLATE, got {:?}",
            refused.get("resourceType")
        );

        // The refusal removed nothing: both memberships' templates still read back.
        let (status, fetched) = get_analysis_template(&emulator, &membership, &kept_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the membership's own template must survive, got {status} with {fetched}"
        );
        let (status, fetched) =
            get_analysis_template(&emulator, &other_membership, &elsewhere_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the other membership's template must survive, got {status} with {fetched}"
        );
    }

    // An unknown membership reaches no template at all, and says so about the
    // membership rather than about the template.
    for unknown in NEVER_MINTED_IDS {
        let (status, refused) = delete_analysis_template(&emulator, unknown, &kept_id).await;
        assert_eq!(
            status, 404,
            "case unknown membership {unknown:?}: expected 404, got {status} with {refused}"
        );
        assert_eq!(
            error_message(&refused),
            format!("No such membership: {unknown} for account {}", account_id()),
            "case unknown membership {unknown:?}: unexpected message, got {refused}"
        );
        assert_eq!(
            refused["resourceType"].as_str(),
            Some("MEMBERSHIP"),
            "case unknown membership {unknown:?}: expected resourceType MEMBERSHIP, got {:?}",
            refused.get("resourceType")
        );
    }

    // An identifier outside the modelled form is refused before the handler, so nothing
    // in the store is consulted for it.
    for malformed in ["not-a-uuid", "0000000-0000-0000-0000-000000000000"] {
        let (status, refused) = delete_analysis_template(&emulator, &membership, malformed).await;
        assert_eq!(
            status, 400,
            "case malformed {malformed:?}: expected ValidationException 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case malformed {malformed:?}: expected ValidationException, got {refused}"
        );
    }

    // Everything this test created that it did not delete is still there.
    let (status, fetched) = get_analysis_template(&emulator, &membership, &kept_id).await;
    assert_eq!(
        status, 200,
        "after every refusal the kept template must still read back, got {status} with {fetched}"
    );
}
