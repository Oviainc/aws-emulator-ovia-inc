// SPDX-License-Identifier: CC0-1.0

//! What a batch metadata read of an agent space's artifacts IS, end to end over the
//! wire: `AddArtifact` uploads artifacts and `BatchGetArtifactMetadata` reports the
//! metadata of the ones a request names.
//!
//! Every call goes through the real `MultiServiceRouter` the emulator boots, as a
//! `restJson1` `POST /<OperationName>` with a JSON body signed for the `securityagent`
//! signing name; the harness is `crate::support`. So nothing in this file can pass
//! against a handler that answers a canned body: a value must come back out of the
//! shared store through a read before a case counts.
//!
//! The rules this suite owns:
//!
//! * a batch metadata read reports one item per requested `artifactId` the named agent
//!   space RESOLVES, in the order asked and each one off the stored record, carrying
//!   exactly the members the metadata shape declares and the same values a single read of
//!   that artifact reports;
//! * an id the space holds no artifact for — an id nothing was ever minted for, one that
//!   names ANOTHER space's artifact, or one a delete removed — contributes no item, and
//!   an id asked for twice contributes one;
//! * the read changes nothing, so a second identical call answers the same payload and a
//!   single read still works afterwards;
//! * a request whose ids cannot name anything is refused: an empty `agentSpaceId` or an
//!   empty entry of `artifactIds` as a bad request naming the member, an `agentSpaceId`
//!   the account does not hold with the agent space's own not-found sentence.
//!
//! Each case asserts a rule over a TABLE of inputs: the rows differ along how many ids
//! are asked for, their order against the upload order, how often an id repeats, and
//! which of them resolves to nothing and why.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{call, emulator, member, ok, Answer};

/// The sentence a refusal of an agent space the account does not hold states: wholly
/// FIXED text, naming the resource class under the name the service's own records use
/// for an agent space and carrying nothing the request sent.
const AGENT_SPACE_NOT_FOUND: &str = "The specified agent instance does not exist.";

/// Every member `ArtifactMetadataItem` declares, and nothing beside it
/// (`smithy_query(query="shape",shape="ArtifactMetadataItem",
/// model_path="services/service-com-amazonaws-securityagent-securityagent/smithy-model/com-amazonaws-securityagent-securityagent.json")`:
/// agentSpaceId, artifactId, fileName, updatedAt — all `@required`), so a member added to
/// the shape makes this list go stale.
const METADATA_MEMBERS: [&str; 4] = ["agentSpaceId", "artifactId", "fileName", "updatedAt"];

/// One request the service must refuse: what a caller sends, the status and error code
/// the refusal carries, and the member path it names when it is a bad request (`None`
/// when the refusal is a missing agent space, whose sentence names no member).
struct Refusal {
    case: &'static str,
    agent_space_id: String,
    artifact_ids: Vec<String>,
    status: u16,
    code: &'static str,
    path: Option<&'static str>,
}

/// One artifact this suite uploads into the agent space that holds them: the rows differ
/// along the declared type, the file name and the size of the content, so an item's
/// `fileName` can only come from the record it names.
struct Seed {
    label: &'static str,
    file_name: &'static str,
    /// An `ArtifactType` member (TXT, PNG, JPEG, MD, PDF, DOCX, DOC, JSON, YAML — see
    /// `smithy_query(query="shape",shape="ArtifactType")`).
    artifact_type: &'static str,
    content: Vec<u8>,
}

fn seeds() -> Vec<Seed> {
    vec![
        Seed {
            label: "text",
            file_name: "architecture-notes.txt",
            artifact_type: "TXT",
            content: b"one database, one queue".to_vec(),
        },
        Seed {
            label: "markdown",
            file_name: "design.md",
            artifact_type: "MD",
            content: b"# design\n\nthe agent reads this\n".to_vec(),
        },
        Seed {
            label: "json",
            file_name: "openapi.json",
            artifact_type: "JSON",
            content: br#"{"openapi":"3.0.0"}"#.to_vec(),
        },
        Seed {
            label: "binary",
            file_name: "diagram.png",
            artifact_type: "PNG",
            // The PNG signature, which encodes no text at all.
            content: vec![0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a],
        },
    ]
}

/// `CreateAgentSpace`, as the id of the record it minted.
async fn create_agent_space(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let created = ok(router, "CreateAgentSpace", &json!({"name": name}), case).await;
    member(&created, "agentSpaceId")
        .unwrap_or_else(|| panic!("case {case:?}: CreateAgentSpace returned no agentSpaceId"))
}

/// `AddArtifact`, requiring the modelled 201 and answering the minted `artifactId`.
async fn add_ok(
    router: &MultiServiceRouter,
    agent_space_id: &str,
    file_name: &str,
    artifact_type: &str,
    content: &[u8],
    case: &str,
) -> String {
    let answer = call(
        router,
        "AddArtifact",
        &json!({
            "agentSpaceId": agent_space_id,
            "fileName": file_name,
            "artifactType": artifact_type,
            "artifactContent": aws_smithy_types::base64::encode(content),
        }),
    )
    .await;
    // The operation's own success status (`smithy_query(query="operation",
    // operation="AddArtifact")`: http code 201), not the service's usual 200.
    assert_eq!(
        answer.status, 201,
        "case {case:?}: expected AddArtifact to answer 201, got {} ({}) with {}",
        answer.status, answer.error_code, answer.body
    );
    member(&answer.body, "artifactId")
        .unwrap_or_else(|| panic!("case {case:?}: AddArtifact returned no artifactId"))
}

/// `BatchGetArtifactMetadata`, as the whole answer it gave.
async fn batch(router: &MultiServiceRouter, agent_space_id: &str, ids: &[String]) -> Answer {
    call(
        router,
        "BatchGetArtifactMetadata",
        &json!({"agentSpaceId": agent_space_id, "artifactIds": ids}),
    )
    .await
}

/// The `artifactMetadataList` of a 200, which the model makes `@required`.
fn metadata_list(payload: &Value, case: &str) -> Vec<Value> {
    payload
        .get("artifactMetadataList")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the read carries no artifactMetadataList: {payload}")
        })
}

/// `BatchGetArtifactMetadata`, requiring 200 and answering the metadata list.
async fn batch_ok(
    router: &MultiServiceRouter,
    agent_space_id: &str,
    ids: &[String],
    case: &str,
) -> Vec<Value> {
    let answer = batch(router, agent_space_id, ids).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected BatchGetArtifactMetadata to answer 200, got {} ({}) \
         with {}",
        answer.status, answer.error_code, answer.body
    );
    metadata_list(&answer.body, case)
}

/// `GetArtifact`, requiring 200 — the single read a metadata item must agree with.
async fn get_ok(
    router: &MultiServiceRouter,
    agent_space_id: &str,
    artifact_id: &str,
    case: &str,
) -> Value {
    ok(
        router,
        "GetArtifact",
        &json!({"agentSpaceId": agent_space_id, "artifactId": artifact_id}),
        case,
    )
    .await
}

/// The `artifactId`s of a metadata list, in the order it reports them.
fn ids_of(items: &[Value], case: &str) -> Vec<String> {
    items
        .iter()
        .map(|item| {
            member(item, "artifactId")
                .unwrap_or_else(|| panic!("case {case:?}: a metadata item carries no artifactId: {item}"))
        })
        .collect()
}

/// RULE: a batch metadata read answers one item per requested `artifactId` the named
/// agent space resolves — in the order asked, each item carrying exactly the members the
/// metadata shape declares, taken off the stored record and equal to what a single read
/// of that artifact reports — while an id that resolves to nothing contributes no item
/// and an id asked for twice contributes one; the read writes nothing, so a second
/// identical call answers the same payload.
#[tokio::test]
async fn a_batch_metadata_read_reports_the_records_the_ids_resolve() {
    let router = emulator();
    let holder = create_agent_space(&router, "artifact-metadata-holder", "seed").await;
    let neighbour = create_agent_space(&router, "artifact-metadata-neighbour", "seed").await;

    // The artifacts of the space under read, in upload order, plus one next door that
    // belongs to another space.
    let mut uploaded: Vec<(&str, String, &str)> = Vec::new();
    for seed in seeds() {
        let artifact_id = add_ok(
            &router,
            &holder,
            seed.file_name,
            seed.artifact_type,
            &seed.content,
            seed.label,
        )
        .await;
        uploaded.push((seed.label, artifact_id, seed.file_name));
    }
    let next_door = add_ok(
        &router,
        &neighbour,
        "next-door.txt",
        "TXT",
        b"held next door",
        "seed",
    )
    .await;
    // An id of the right shape that nothing was ever minted for.
    let unminted = "ffffffffffffffffffffffffffffffff".to_string();

    let id_of = |label: &str| -> String {
        uploaded
            .iter()
            .find(|(seed_label, _, _)| *seed_label == label)
            .map(|(_, artifact_id, _)| artifact_id.clone())
            .unwrap_or_else(|| panic!("no artifact was uploaded for {label:?}"))
    };
    let file_name_of = |label: &str| -> &'static str {
        uploaded
            .iter()
            .find(|(seed_label, _, _)| *seed_label == label)
            .map(|(_, _, file_name)| *file_name)
            .unwrap_or_else(|| panic!("no artifact was uploaded for {label:?}"))
    };

    // (case, the ids the request carries, the labels the answer must report in order) —
    // the rows differ along how many ids are asked for, their order against the upload
    // order, how often an id repeats, and which of them resolves to nothing and why.
    let rows: Vec<(&str, Vec<String>, Vec<&str>)> = vec![
        ("one id", vec![id_of("text")], vec!["text"]),
        (
            "every artifact of the space, in upload order",
            vec![
                id_of("text"),
                id_of("markdown"),
                id_of("json"),
                id_of("binary"),
            ],
            vec!["text", "markdown", "json", "binary"],
        ),
        (
            "the same ids asked for in reverse",
            vec![
                id_of("binary"),
                id_of("json"),
                id_of("markdown"),
                id_of("text"),
            ],
            vec!["binary", "json", "markdown", "text"],
        ),
        (
            "a subset, out of upload order",
            vec![id_of("json"), id_of("text")],
            vec!["json", "text"],
        ),
        ("no ids at all", vec![], vec![]),
        (
            "an id nothing was minted for, beside a known one",
            vec![id_of("markdown"), unminted.clone()],
            vec!["markdown"],
        ),
        (
            "only ids nothing resolves",
            vec![unminted.clone(), format!("{unminted}-also-not-an-artifact")],
            vec![],
        ),
        (
            "another agent space's artifact, beside a known one",
            vec![next_door.clone(), id_of("binary")],
            vec!["binary"],
        ),
        (
            "the same id twice",
            vec![id_of("json"), id_of("json")],
            vec!["json"],
        ),
        (
            "a repeat around a known id",
            vec![id_of("text"), id_of("binary"), id_of("text")],
            vec!["text", "binary"],
        ),
    ];

    for (case, ids, expected_labels) in rows {
        let items = batch_ok(&router, &holder, &ids, case).await;

        let expected_ids: Vec<String> = expected_labels.iter().map(|l| id_of(l)).collect();
        let actual_ids = ids_of(&items, case);
        assert_eq!(
            actual_ids, expected_ids,
            "case {case:?}: expected the metadata of {expected_labels:?} in that order, \
             got {actual_ids:?}"
        );

        for (item, label) in items.iter().zip(expected_labels.iter()) {
            // Exactly the modelled members, so the metadata read reports the artifact's
            // metadata and nothing else — no content and no invented member.
            let keys: Vec<String> = item
                .as_object()
                .unwrap_or_else(|| panic!("case {case:?}: a metadata item is not an object: {item}"))
                .keys()
                .cloned()
                .collect();
            let mut expected_keys: Vec<String> =
                METADATA_MEMBERS.iter().map(|m| m.to_string()).collect();
            expected_keys.sort();
            let mut sorted = keys.clone();
            sorted.sort();
            assert_eq!(
                sorted, expected_keys,
                "case {case:?}: expected the item for {label:?} to carry exactly \
                 {expected_keys:?}, got {keys:?}"
            );

            assert_eq!(
                member(item, "agentSpaceId").as_deref(),
                Some(holder.as_str()),
                "case {case:?}: expected the item for {label:?} to name the space that \
                 holds it, got {item}"
            );
            assert_eq!(
                member(item, "fileName").as_deref(),
                Some(file_name_of(label)),
                "case {case:?}: expected the stored fileName of {label:?}, got {item}"
            );

            // The SAME stored record a single read reports: the metadata read is a
            // projection of it, not a second source of truth.
            let read = get_ok(&router, &holder, &id_of(label), case).await;
            for name in METADATA_MEMBERS {
                assert_eq!(
                    member(item, name),
                    member(&read, name),
                    "case {case:?}: expected {name} of {label:?} to match GetArtifact, \
                     got {item} against {read}"
                );
            }
            // `updatedAt` is a `@timestampFormat("date-time")` member, so it is an
            // RFC 3339 string on this service's restJson1 wire, never an epoch number.
            let updated_at = member(item, "updatedAt").unwrap_or_else(|| {
                panic!("case {case:?}: expected updatedAt as a date-time string, got {item}")
            });
            DateTime::from_str(&updated_at, Format::DateTime).unwrap_or_else(|error| {
                panic!(
                    "case {case:?}: updatedAt {updated_at:?} is not an RFC 3339 instant \
                     ({error})"
                )
            });
        }

        // A read changes nothing: the same request answers the same payload.
        let again = batch_ok(&router, &holder, &ids, case).await;
        assert_eq!(
            again, items,
            "case {case:?}: expected a second batch read to answer the same items"
        );
    }
}

/// RULE: the metadata a batch read reports follows the STORE — an artifact uploaded
/// after an earlier read appears, and one a delete removed stops appearing, while every
/// other requested id answers exactly as before.
#[tokio::test]
async fn a_batch_metadata_read_follows_the_store() {
    let router = emulator();
    let holder = create_agent_space(&router, "artifact-metadata-store", "seed").await;

    let kept = add_ok(&router, &holder, "kept.txt", "TXT", b"kept", "seed").await;
    let doomed = add_ok(&router, &holder, "doomed.md", "MD", b"# doomed\n", "seed").await;

    let before = batch_ok(
        &router,
        &holder,
        &[kept.clone(), doomed.clone()],
        "both stored",
    )
    .await;
    assert_eq!(
        ids_of(&before, "both stored"),
        vec![kept.clone(), doomed.clone()],
        "case {:?}: expected both stored artifacts, got {before:?}",
        "both stored"
    );

    // An upload after the first read is reported by the next one.
    let added = add_ok(&router, &holder, "added.json", "JSON", b"{}", "added later").await;
    let with_added = batch_ok(
        &router,
        &holder,
        &[kept.clone(), doomed.clone(), added.clone()],
        "added later",
    )
    .await;
    assert_eq!(
        ids_of(&with_added, "added later"),
        vec![kept.clone(), doomed.clone(), added.clone()],
        "case {:?}: expected the artifact uploaded after the first read to be reported, \
         got {with_added:?}",
        "added later"
    );

    // A delete removes the record, so its id resolves to nothing and contributes no item.
    let deleted = call(
        &router,
        "DeleteArtifact",
        &json!({"agentSpaceId": holder, "artifactId": doomed}),
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "expected DeleteArtifact to answer 200, got {} ({}) with {}",
        deleted.status, deleted.error_code, deleted.body
    );

    let after = batch_ok(
        &router,
        &holder,
        &[kept.clone(), doomed.clone(), added.clone()],
        "after the delete",
    )
    .await;
    assert_eq!(
        ids_of(&after, "after the delete"),
        vec![kept.clone(), added.clone()],
        "case {:?}: expected the deleted artifact to drop out of the answer, got {after:?}",
        "after the delete"
    );
}

/// RULE: a request whose ids cannot name anything is refused and reports nothing — an
/// empty `agentSpaceId` or an empty entry of `artifactIds` as a bad request (HTTP 400
/// `ValidationException`) naming the member that failed, and an `agentSpaceId` the
/// account does not hold with the agent space's own not-found sentence (HTTP 404).
#[tokio::test]
async fn a_batch_metadata_read_of_ids_that_name_nothing_is_refused() {
    let router = emulator();
    let holder = create_agent_space(&router, "artifact-metadata-refusals", "seed").await;
    let doomed = create_agent_space(&router, "artifact-metadata-doomed", "seed").await;
    let stored = add_ok(&router, &holder, "stored.txt", "TXT", b"stored", "seed").await;
    ok(
        &router,
        "DeleteAgentSpace",
        &json!({"agentSpaceId": doomed}),
        "deleting the doomed space",
    )
    .await;

    // The rows differ along WHICH id cannot name anything and where it sits in the
    // request.
    let rows: Vec<Refusal> = vec![
        Refusal {
            case: "an empty agentSpaceId",
            agent_space_id: String::new(),
            artifact_ids: vec![stored.clone()],
            status: 400,
            code: "ValidationException",
            path: Some("agentSpaceId"),
        },
        Refusal {
            case: "an empty agentSpaceId with no ids",
            agent_space_id: String::new(),
            artifact_ids: vec![],
            status: 400,
            code: "ValidationException",
            path: Some("agentSpaceId"),
        },
        Refusal {
            case: "an empty first artifactId",
            agent_space_id: holder.clone(),
            artifact_ids: vec![String::new(), stored.clone()],
            status: 400,
            code: "ValidationException",
            path: Some("artifactIds[0]"),
        },
        Refusal {
            case: "an empty second artifactId",
            agent_space_id: holder.clone(),
            artifact_ids: vec![stored.clone(), String::new()],
            status: 400,
            code: "ValidationException",
            path: Some("artifactIds[1]"),
        },
        Refusal {
            case: "an agentSpaceId the account never held",
            agent_space_id: "00000000000000000000000000000000".to_string(),
            artifact_ids: vec![stored.clone()],
            status: 404,
            code: "ResourceNotFoundException",
            path: None,
        },
        Refusal {
            case: "an agent space a delete removed",
            agent_space_id: doomed.clone(),
            artifact_ids: vec![stored.clone()],
            status: 404,
            code: "ResourceNotFoundException",
            path: None,
        },
    ];

    for row in rows {
        let Refusal {
            case,
            agent_space_id,
            artifact_ids,
            status,
            code,
            path,
        } = row;
        let answer = batch(&router, &agent_space_id, &artifact_ids).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (status, code),
            "case {case:?}: expected {status} {code}, got {} ({}) with {}",
            answer.status,
            answer.error_code,
            answer.body
        );
        assert!(
            answer.body.get("artifactMetadataList").is_none(),
            "case {case:?}: expected a refusal to report no metadata, got {}",
            answer.body
        );
        match path {
            // A bad request names the member that failed, both in the message and in the
            // refusal's field list.
            Some(path) => {
                let message = member(&answer.body, "message").unwrap_or_default();
                assert!(
                    message.contains(&format!("'{path}'")),
                    "case {case:?}: expected the refusal to name {path:?}, got {message:?}"
                );
                let fields = answer
                    .body
                    .get("fieldList")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let paths: Vec<String> = fields.iter().filter_map(|f| member(f, "path")).collect();
                assert_eq!(
                    paths,
                    vec![path.to_string()],
                    "case {case:?}: expected the field list to name {path:?}, got {paths:?}"
                );
            }
            // A missing agent space is the agent space's own fixed sentence, carrying
            // nothing the request sent.
            None => {
                let message = member(&answer.body, "message").unwrap_or_default();
                assert_eq!(
                    message, AGENT_SPACE_NOT_FOUND,
                    "case {case:?}: expected {AGENT_SPACE_NOT_FOUND:?}, got {message:?}"
                );
            }
        }
    }

    // Every refusal above left the store as it was: the stored artifact still reads back.
    let items = batch_ok(
        &router,
        &holder,
        std::slice::from_ref(&stored),
        "after the refusals",
    )
    .await;
    assert_eq!(
        ids_of(&items, "after the refusals"),
        vec![stored],
        "expected the stored artifact to still be reported after the refusals, got \
         {items:?}"
    );
}
