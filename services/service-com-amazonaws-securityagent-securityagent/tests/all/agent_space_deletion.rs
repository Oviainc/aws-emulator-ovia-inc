// SPDX-License-Identifier: CC0-1.0

//! What `DeleteAgentSpace` removes, end to end over the wire: a delete followed by the
//! reads that resolve each kind of record this service stores — `BatchGetAgentSpaces`,
//! `GetArtifact`, `BatchGetTargetDomains` and `GetApplication`.
//!
//! Every call goes through the real `MultiServiceRouter` the emulator boots, as a
//! `restJson1` `POST /<OperationName>` with a JSON body signed for the `securityagent`
//! signing name; the harness is `crate::support`. So nothing in this file can pass
//! against a handler that answers a canned body: what a delete did must come back out of
//! the shared store through a read before a case counts.
//!
//! `crate::agent_space_batch_read` owns what a READ answers after a delete. The two rules
//! this suite owns are about what a delete REACHES:
//!
//! * the call removes the one record its `agentSpaceId` names together with the records
//!   reachable only through it — the artifacts uploaded into it — and leaves every record
//!   that is registered independently of it (the other agent spaces, their artifacts, the
//!   target domains it was associated with, the account's applications) exactly as it was;
//! * it removes a RECORD, not an identity: a create after a delete is a new agent space
//!   under a new id, and the removed id is never resolved again.
//!
//! Each case asserts its rule over a TABLE of inputs: the rows differ along how many
//! artifacts the deleted space held, how many target domains it named, which of several
//! stored spaces is deleted, and what the create after the delete asked for.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{call, emulator, member, ok, Answer};

/// The sentence a refusal of an agent space the account does not hold states: wholly
/// FIXED text, naming the resource class under the name the service's own records use for
/// an agent space and carrying nothing the request sent.
const AGENT_SPACE_NOT_FOUND: &str = "The specified agent instance does not exist.";

/// The sentence a refusal of an artifact the named agent space does not hold states.
const ARTIFACT_NOT_FOUND: &str = "The specified artifact does not exist.";

/// How many spaces each row of the cascade case stores, so a delete that removed more
/// than the record it names shows up on the spaces beside it.
const STORED_SPACES: usize = 3;

/// How many artifacts each space the delete does NOT name holds.
const KEPT_ARTIFACTS: usize = 2;

/// `CreateAgentSpace`, as the id of the record it minted.
async fn create_space(router: &MultiServiceRouter, request: &Value, case: &str) -> String {
    let created = ok(router, "CreateAgentSpace", request, case).await;
    member(&created, "agentSpaceId").unwrap_or_else(|| {
        panic!("case {case:?}: CreateAgentSpace minted no agentSpaceId: {created}")
    })
}

/// `CreateTargetDomain`, as the id of the record it minted. A target domain is registered
/// on its own, not through an agent space.
async fn register_domain(router: &MultiServiceRouter, domain: &str, case: &str) -> String {
    let created = ok(
        router,
        "CreateTargetDomain",
        &json!({"targetDomainName": domain, "verificationMethod": "DNS_TXT"}),
        case,
    )
    .await;
    member(&created, "targetDomainId").unwrap_or_else(|| {
        panic!("case {case:?}: CreateTargetDomain minted no targetDomainId: {created}")
    })
}

/// `AddArtifact` into `space`, as the id of the artifact it minted.
///
/// The uploaded bytes are the file name's, so a read that answers another record's
/// content cannot pass for this one.
async fn add_artifact(
    router: &MultiServiceRouter,
    space: &str,
    file_name: &str,
    case: &str,
) -> String {
    let answer = call(
        router,
        "AddArtifact",
        &json!({
            "agentSpaceId": space,
            "fileName": file_name,
            // `TXT` is an `ArtifactType` member
            // (`smithy_query(query="shape",shape="ArtifactType")`); which member it is
            // does not matter here, and `crate::artifact_storage` spans the catalog.
            "artifactType": "TXT",
            "artifactContent": aws_smithy_types::base64::encode(file_name.as_bytes()),
        }),
    )
    .await;
    // `AddArtifact` carries its own success status
    // (`smithy_query(query="operation",operation="AddArtifact")`: http code 201).
    assert_eq!(
        answer.status, 201,
        "case {case:?}: expected AddArtifact to answer 201, got {} ({}) with {}",
        answer.status, answer.error_code, answer.body
    );
    member(&answer.body, "artifactId")
        .unwrap_or_else(|| panic!("case {case:?}: AddArtifact minted no artifactId: {}", answer.body))
}

/// `GetArtifact` for the pair of ids, as the whole answer it gave.
async fn get_artifact(router: &MultiServiceRouter, space: &str, artifact: &str) -> Answer {
    call(
        router,
        "GetArtifact",
        &json!({"agentSpaceId": space, "artifactId": artifact}),
    )
    .await
}

/// `DeleteAgentSpace`, as the whole answer it gave.
async fn delete_space(router: &MultiServiceRouter, space: &str) -> Answer {
    call(router, "DeleteAgentSpace", &json!({"agentSpaceId": space})).await
}

/// The ids a list member of a read carries, in the order reported.
fn ids_in(payload: &Value, list: &str, id_member: &str, case: &str) -> Vec<String> {
    payload
        .get(list)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {list} array, got {payload}"))
        .iter()
        .map(|entry| match entry {
            Value::String(id) => id.clone(),
            record => member(record, id_member).unwrap_or_else(|| {
                panic!("case {case:?}: a {list} entry carries no {id_member}: {record}")
            }),
        })
        .collect()
}

/// `BatchGetAgentSpaces` over `ids`, as the (resolved, unresolved) pair it reports.
async fn batch_get_spaces(
    router: &MultiServiceRouter,
    ids: &[String],
    case: &str,
) -> (Vec<String>, Vec<String>) {
    let payload = ok(
        router,
        "BatchGetAgentSpaces",
        &json!({"agentSpaceIds": ids}),
        case,
    )
    .await;
    (
        ids_in(&payload, "agentSpaces", "agentSpaceId", case),
        ids_in(&payload, "notFound", "agentSpaceId", case),
    )
}

/// `BatchGetTargetDomains` over `ids`, as the (resolved, unresolved) pair it reports.
async fn batch_get_domains(
    router: &MultiServiceRouter,
    ids: &[String],
    case: &str,
) -> (Vec<String>, Vec<String>) {
    let payload = ok(
        router,
        "BatchGetTargetDomains",
        &json!({"targetDomainIds": ids}),
        case,
    )
    .await;
    (
        ids_in(&payload, "targetDomains", "targetDomainId", case),
        ids_in(&payload, "notFound", "targetDomainId", case),
    )
}

/// RULE: a delete removes the one agent space its id names together with the records
/// reachable only through it — the artifacts uploaded into it — reports the id of the
/// record it removed, and leaves every record registered independently of that space (the
/// other agent spaces, their artifacts, the target domains it was associated with and the
/// account's applications) exactly as it was.
#[tokio::test]
async fn a_delete_removes_the_named_space_and_only_what_it_held() {
    // The rows differ along how many artifacts the deleted space held, how many target
    // domains it named, and WHICH of the stored spaces it is.
    let rows: Vec<(&str, usize, usize, usize)> = vec![
        ("the first space, holding nothing and naming no domain", 0, 0, 0),
        ("a space in the middle, holding one artifact", 1, 0, 1),
        ("the last space, holding three artifacts and naming one domain", 3, 1, 2),
        ("a space naming two domains and holding two artifacts", 2, 2, 1),
    ];

    for (case, held, domains, victim) in rows {
        let router = emulator();

        // Records of other kinds, registered independently of any agent space.
        let mut domain_ids: Vec<String> = Vec::new();
        for index in 0..domains {
            domain_ids.push(register_domain(&router, &format!("d{index}.example.com"), case).await);
        }
        let application = ok(&router, "CreateApplication", &json!({}), case).await;
        let application_id = member(&application, "applicationId").unwrap_or_else(|| {
            panic!("case {case:?}: CreateApplication minted no applicationId: {application}")
        });

        // The agent spaces: the one to be deleted names every registered domain, so a
        // delete that followed the association would take them with it.
        let mut space_ids: Vec<String> = Vec::new();
        for index in 0..STORED_SPACES {
            let request = if index == victim {
                json!({"name": format!("space-{index}"), "targetDomainIds": domain_ids})
            } else {
                json!({"name": format!("space-{index}")})
            };
            space_ids.push(create_space(&router, &request, case).await);
        }
        let deleted = space_ids[victim].clone();
        let kept_spaces: Vec<String> = space_ids
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != victim)
            .map(|(_, id)| id.clone())
            .collect();
        let survivor = kept_spaces[0].clone();

        let mut doomed_artifacts: Vec<String> = Vec::new();
        for index in 0..held {
            doomed_artifacts
                .push(add_artifact(&router, &deleted, &format!("doomed-{index}.txt"), case).await);
        }
        // Every artifact of a space this delete does not name: (space, artifact, file).
        let mut kept_artifacts: Vec<(String, String, String)> = Vec::new();
        for space in &kept_spaces {
            for copy in 0..KEPT_ARTIFACTS {
                let file = format!("kept-{space}-{copy}.txt");
                let artifact = add_artifact(&router, space, &file, case).await;
                kept_artifacts.push((space.clone(), artifact, file));
            }
        }

        let answer = delete_space(&router, &deleted).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected DeleteAgentSpace to answer 200, got {} ({}) with {}",
            answer.status, answer.error_code, answer.body
        );
        assert_eq!(
            member(&answer.body, "agentSpaceId").as_deref(),
            Some(deleted.as_str()),
            "case {case:?}: expected the delete to name the record it removed, got {}",
            answer.body
        );
        // Recorded, not modelled: the same value under the name the service's own records
        // use for it, as every other agent space response carries it.
        assert_eq!(
            member(&answer.body, "id").as_deref(),
            Some(deleted.as_str()),
            "case {case:?}: expected the removed record's id, got {}",
            answer.body
        );

        // The named space is gone and every other space is still resolved.
        let (found, missing) = batch_get_spaces(&router, &space_ids, case).await;
        assert_eq!(
            found, kept_spaces,
            "case {case:?}: expected only the spaces still stored {kept_spaces:?}, got {found:?}"
        );
        assert_eq!(
            missing,
            vec![deleted.clone()],
            "case {case:?}: expected only the deleted id {deleted:?} unresolved, got {missing:?}"
        );

        // The artifacts of the removed space are themselves gone: the space no longer
        // resolves, and asking a space that DOES resolve for one of those ids reports no
        // such artifact rather than serving a record the delete should have taken.
        for artifact in &doomed_artifacts {
            let through_deleted = get_artifact(&router, &deleted, artifact).await;
            assert_eq!(
                (through_deleted.status, through_deleted.error_code.as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: expected a read through the deleted space to be refused, got \
                 {} ({}) with {}",
                through_deleted.status,
                through_deleted.error_code,
                through_deleted.body
            );
            assert_eq!(
                member(&through_deleted.body, "message").as_deref(),
                Some(AGENT_SPACE_NOT_FOUND),
                "case {case:?}: expected the agent space not-found sentence, got {}",
                through_deleted.body
            );

            let through_survivor = get_artifact(&router, &survivor, artifact).await;
            assert_eq!(
                (through_survivor.status, through_survivor.error_code.as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: expected the artifact record to be gone, got {} ({}) with {}",
                through_survivor.status,
                through_survivor.error_code,
                through_survivor.body
            );
            assert_eq!(
                member(&through_survivor.body, "message").as_deref(),
                Some(ARTIFACT_NOT_FOUND),
                "case {case:?}: expected the artifact not-found sentence, got {}",
                through_survivor.body
            );
        }

        // Every artifact of every other space is untouched, and still reports its own
        // content.
        for (space, artifact, file) in &kept_artifacts {
            let answer = get_artifact(&router, space, artifact).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the artifact {file:?} of {space:?} to survive, got {} \
                 ({}) with {}",
                answer.status, answer.error_code, answer.body
            );
            assert_eq!(
                member(&answer.body, "fileName").as_deref(),
                Some(file.as_str()),
                "case {case:?}: expected the stored fileName {file:?}, got {}",
                answer.body
            );
            let contents = answer
                .body
                .get("artifact")
                .and_then(|artifact| artifact.get("contents"))
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the read carries no contents: {}", answer.body)
                });
            assert_eq!(
                aws_smithy_types::base64::decode(contents).ok(),
                Some(file.as_bytes().to_vec()),
                "case {case:?}: expected the bytes uploaded for {file:?}, got {contents:?}"
            );
        }

        // A target domain is a record of its own, registered independently of any agent
        // space, so the association the deleted space carried did not take it with it.
        if !domain_ids.is_empty() {
            let (domains_found, domains_missing) =
                batch_get_domains(&router, &domain_ids, case).await;
            assert_eq!(
                domains_found, domain_ids,
                "case {case:?}: expected every registered target domain {domain_ids:?} to \
                 survive the delete, got {domains_found:?}"
            );
            assert!(
                domains_missing.is_empty(),
                "case {case:?}: expected no target domain to go with the agent space, got \
                 {domains_missing:?}"
            );
        }

        // And so is an application: another resource kind of the same store.
        let read = call(
            &router,
            "GetApplication",
            &json!({"applicationId": application_id}),
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the application {application_id:?} to survive the \
             delete, got {} ({}) with {}",
            read.status, read.error_code, read.body
        );
    }
}

/// RULE: a delete removes a RECORD and not an identity — the id is the service's to mint —
/// so a create after a delete is a new agent space under an id no earlier call used, and
/// the removed id is never resolved again however many creates follow.
#[tokio::test]
async fn a_delete_frees_no_identity_for_the_next_create() {
    let router = emulator();

    // The rows differ along what the create after the delete asks for: the same request
    // again, a different name, every member at once, and a name in another script.
    let rows: Vec<(&str, Value)> = vec![
        ("the same request again", json!({"name": "recreated"})),
        (
            "a description beside the name",
            json!({"name": "described", "description": "the second record of this name"}),
        ),
        (
            "every member the create can carry",
            json!({
                "name": "full-space",
                "description": "re-created with every member",
                "awsResources": {"s3Buckets": ["arn:aws:s3:::sec-agent-recreated"]},
                "codeReviewSettings": {"controlsScanning": true, "generalPurposeScanning": false},
                "kmsKeyId": "alias/sec",
                "tags": {"env": "test"},
            }),
        ),
        ("a name in another script", json!({"name": "агент-пространство"})),
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    let mut deleted_ids: Vec<String> = Vec::new();

    for (case, request) in rows {
        let first = create_space(&router, &request, case).await;
        assert!(
            minted.insert(first.clone()),
            "case {case:?}: CreateAgentSpace reused the agentSpaceId {first:?}"
        );

        let answer = delete_space(&router, &first).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected DeleteAgentSpace to answer 200, got {} ({}) with {}",
            answer.status, answer.error_code, answer.body
        );
        deleted_ids.push(first.clone());

        let again = create_space(&router, &request, case).await;
        assert_ne!(
            again, first,
            "case {case:?}: expected the create after the delete to mint a new identity, got \
             the removed id {first:?} again"
        );
        assert!(
            minted.insert(again.clone()),
            "case {case:?}: CreateAgentSpace reused the agentSpaceId {again:?}"
        );

        // The new record is the one a read resolves; the removed id is not.
        let (found, missing) =
            batch_get_spaces(&router, &[first.clone(), again.clone()], case).await;
        assert_eq!(
            found,
            vec![again.clone()],
            "case {case:?}: expected only the re-created record {again:?} to resolve, got \
             {found:?}"
        );
        assert_eq!(
            missing,
            vec![first.clone()],
            "case {case:?}: expected the removed id {first:?} to stay unresolved, got {missing:?}"
        );
    }

    // Every id a delete removed stays unresolved, however many creates followed it.
    let (found, missing) = batch_get_spaces(&router, &deleted_ids, "every removed id").await;
    assert!(
        found.is_empty(),
        "expected no removed id to be resolved again, got {found:?}"
    );
    assert_eq!(
        missing, deleted_ids,
        "expected every removed id {deleted_ids:?} to be reported unresolved, got {missing:?}"
    );
}
