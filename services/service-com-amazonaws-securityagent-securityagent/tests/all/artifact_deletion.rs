// SPDX-License-Identifier: CC0-1.0

//! Artifact DELETION in the SecurityAgent emulator, end to end over the wire:
//! `DeleteArtifact`, with `AddArtifact` seeding the store and `GetArtifact` reading it
//! back (the upload/read round trip itself lives in `artifact_storage.rs`).
//!
//! Every call goes through the real `MultiServiceRouter` the emulator boots (see
//! `crate::support`), so nothing here can pass against a handler that answers a canned
//! body: what a delete did must come back out of the shared store through a read before
//! a case counts.
//!
//! The rules this suite owns:
//!
//! * a delete removes EXACTLY the record the pair of ids resolves — a read of that id is
//!   then refused with the service's not-found sentence for an artifact, naming the id
//!   that was asked for, while every other artifact, of that space and of every other
//!   space, still reads back with the content it was uploaded with;
//! * the call's postcondition is that the named agent space holds no artifact under that
//!   id, so a pair that already resolves to no record (an id nothing minted, an id an
//!   earlier delete removed, an id of ANOTHER space) is a success that removes nothing —
//!   deleting an artifact twice answers as deleting it once does;
//! * an agent space the account does not hold is refused with the agent space's own
//!   not-found sentence, and an id no modelled constraint covers (an empty one) as a bad
//!   request naming the member; neither removes anything;
//! * the delete frees what the record held, so a space at its artifact quota accepts a
//!   further upload once one of its artifacts is deleted.
//!
//! Each case asserts a rule over a TABLE of inputs: the rows differ along WHICH artifact
//! of several is deleted, how many the space holds, the declared type, the form and the
//! size of the content, and HOW the pair a repeat delete names misses a record.

use serde_json::{json, Value};
use service_com_amazonaws_securityagent_securityagent::state::MAX_ARTIFACTS_PER_AGENT_SPACE;
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{call, emulator, member, ok};

/// The sentence a refusal of an agent space the account does not hold states: wholly
/// FIXED text, naming the resource class under the name the service's own records use
/// for an agent space and carrying nothing the request sent.
const AGENT_SPACE_NOT_FOUND: &str = "The specified agent instance does not exist.";

/// The sentence a refusal of an artifact the named agent space does not hold states:
/// the `artifactId` the request named is its ONLY variable part, so a refusal names the
/// id it refused.
fn artifact_not_found(artifact_id: &str) -> String {
    format!("No artifact was found for id: {artifact_id}")
}

/// One seeded artifact: what is uploaded, so a delete of one row can be told from the
/// rows that must survive it.
struct Seed {
    file_name: &'static str,
    /// An `ArtifactType` member — the catalog is the enum's value set
    /// (`smithy_query(query="shape",shape="ArtifactType",
    /// model_path="services/service-com-amazonaws-securityagent-securityagent/smithy-model/com-amazonaws-securityagent-securityagent.json")`:
    /// TXT, PNG, JPEG, MD, PDF, DOCX, DOC, JSON, YAML).
    artifact_type: &'static str,
    content: Vec<u8>,
}

/// The artifacts one space is seeded with: the rows differ along the declared type and
/// the form and size of the content (text, text outside ASCII, bytes that encode no text
/// at all, and no content at all).
fn seeds() -> Vec<Seed> {
    vec![
        Seed {
            file_name: "architecture.txt",
            artifact_type: "TXT",
            content: b"one database and one queue".to_vec(),
        },
        Seed {
            file_name: "design.md",
            artifact_type: "MD",
            content: b"# Design\n\n* a list\n".to_vec(),
        },
        Seed {
            file_name: "diagram.png",
            artifact_type: "PNG",
            content: vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0xff],
        },
        Seed {
            file_name: "notes-ünïcode.yaml",
            artifact_type: "YAML",
            content: "clé: valeur ✅\n".as_bytes().to_vec(),
        },
        Seed {
            file_name: "empty.json",
            artifact_type: "JSON",
            content: Vec::new(),
        },
    ]
}

/// The bytes of an upload as the JSON body carries a blob: base64, with the codec the
/// wire serializer uses (`aws_smithy_types::base64`).
fn blob(content: &[u8]) -> String {
    aws_smithy_types::base64::encode(content)
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
            "artifactContent": blob(content),
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

/// `GetArtifact`, as the whole answer it gave.
async fn get(
    router: &MultiServiceRouter,
    agent_space_id: &str,
    artifact_id: &str,
) -> crate::support::Answer {
    call(
        router,
        "GetArtifact",
        &json!({"agentSpaceId": agent_space_id, "artifactId": artifact_id}),
    )
    .await
}

/// `DeleteArtifact`, as the whole answer it gave.
async fn delete(
    router: &MultiServiceRouter,
    agent_space_id: &str,
    artifact_id: &str,
) -> crate::support::Answer {
    call(
        router,
        "DeleteArtifact",
        &json!({"agentSpaceId": agent_space_id, "artifactId": artifact_id}),
    )
    .await
}

/// `DeleteArtifact`, requiring the modelled success: HTTP 200 and an answer carrying no
/// member at all (`DeleteArtifactOutput` models none).
async fn delete_ok(
    router: &MultiServiceRouter,
    agent_space_id: &str,
    artifact_id: &str,
    case: &str,
) {
    let answer = delete(router, agent_space_id, artifact_id).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected DeleteArtifact to answer 200, got {} ({}) with {}",
        answer.status, answer.error_code, answer.body
    );
    let members: Vec<String> = match &answer.body {
        Value::Object(map) => map.keys().cloned().collect(),
        Value::Null => Vec::new(),
        other => panic!("case {case:?}: expected an object or an empty body, got {other}"),
    };
    assert!(
        members.is_empty(),
        "case {case:?}: DeleteArtifactOutput models no member, got {members:?}"
    );
}

/// The `message` of a refusal, which every modelled error of this service carries.
fn refusal_message(body: &Value, case: &str) -> String {
    member(body, "message")
        .or_else(|| member(body, "Message"))
        .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no message: {body}"))
}

/// A read of `artifact_id` in `agent_space_id` must answer 200 and report `content` and
/// `file_name` off the stored record.
async fn assert_reads_back(
    router: &MultiServiceRouter,
    agent_space_id: &str,
    artifact_id: &str,
    file_name: &str,
    content: &[u8],
    case: &str,
) {
    let answer = get(router, agent_space_id, artifact_id).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected {artifact_id:?} to still read back, got {} ({}) with {}",
        answer.status, answer.error_code, answer.body
    );
    assert_eq!(
        member(&answer.body, "fileName").as_deref(),
        Some(file_name),
        "case {case:?}: expected fileName {file_name:?}, got {}",
        answer.body
    );
    let contents = answer
        .body
        .get("artifact")
        .and_then(|artifact| artifact.get("contents"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the read carries no contents: {}", answer.body));
    let decoded = aws_smithy_types::base64::decode(contents)
        .unwrap_or_else(|error| panic!("case {case:?}: contents is not base64 ({error})"));
    assert_eq!(
        decoded, content,
        "case {case:?}: expected the uploaded {} bytes back, got {} bytes",
        content.len(),
        decoded.len()
    );
}

/// A read of `artifact_id` in `agent_space_id` must be refused as an artifact the space
/// does not hold, with the sentence naming that id.
async fn assert_gone(
    router: &MultiServiceRouter,
    agent_space_id: &str,
    artifact_id: &str,
    case: &str,
) {
    let answer = get(router, agent_space_id, artifact_id).await;
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "case {case:?}: expected a 404 ResourceNotFoundException reading {artifact_id:?}, \
         got {} ({}) with {}",
        answer.status,
        answer.error_code,
        answer.body
    );
    let actual = refusal_message(&answer.body, case);
    let expected = artifact_not_found(artifact_id);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?}, got {actual:?}"
    );
}

/// RULE: a delete removes exactly the record the pair of ids resolves — a read of the
/// deleted id is refused with the service's not-found sentence for an artifact, naming
/// that id, while every artifact the delete did not name (of the same space and of
/// another space) still reads back with the file name and the bytes it was uploaded
/// with.
#[tokio::test]
async fn a_deleted_artifact_is_gone_and_its_neighbours_are_untouched() {
    // The rows differ in WHICH of the seeded artifacts is deleted — so in the declared
    // type and the content form of the record removed — and the whole table is replayed
    // for each of them.
    for victim in 0..seeds().len() {
        let router = emulator();
        let case = format!("deleting seed {victim} of {}", seeds().len());
        let holder = create_agent_space(&router, "artifact-delete-holder", &case).await;
        let neighbour = create_agent_space(&router, "artifact-delete-neighbour", &case).await;

        // The same set of artifacts in two spaces, so a delete that reached across spaces
        // would show up as a missing neighbour record.
        let mut held = Vec::new();
        let mut elsewhere = Vec::new();
        for seed in seeds() {
            held.push(
                add_ok(
                    &router,
                    &holder,
                    seed.file_name,
                    seed.artifact_type,
                    &seed.content,
                    &case,
                )
                .await,
            );
            elsewhere.push(
                add_ok(
                    &router,
                    &neighbour,
                    seed.file_name,
                    seed.artifact_type,
                    &seed.content,
                    &case,
                )
                .await,
            );
        }

        let doomed = held[victim].clone();
        delete_ok(&router, &holder, &doomed, &case).await;
        assert_gone(&router, &holder, &doomed, &case).await;

        // Every other record of the holding space, and every record of the neighbour, is
        // exactly as it was uploaded.
        for (index, seed) in seeds().into_iter().enumerate() {
            if index != victim {
                assert_reads_back(
                    &router,
                    &holder,
                    &held[index],
                    seed.file_name,
                    &seed.content,
                    &format!("{case}: surviving seed {index}"),
                )
                .await;
            }
            assert_reads_back(
                &router,
                &neighbour,
                &elsewhere[index],
                seed.file_name,
                &seed.content,
                &format!("{case}: the neighbour's seed {index}"),
            )
            .await;
        }
    }
}

/// RULE: the call's postcondition is that the named agent space holds no artifact under
/// the named id, so a delete whose pair already resolves to no record — an id nothing
/// minted, an id an earlier delete removed, a mutation of a stored id, or an id that
/// names an artifact of ANOTHER agent space — is a success that removes nothing:
/// deleting an artifact twice answers as deleting it once does, and no other space's
/// artifact is touched.
#[tokio::test]
async fn a_delete_of_an_artifact_the_space_does_not_hold_succeeds_and_removes_nothing() {
    let router = emulator();
    let holder = create_agent_space(&router, "artifact-repeat-holder", "seed").await;
    let neighbour = create_agent_space(&router, "artifact-repeat-neighbour", "seed").await;

    let mut held = Vec::new();
    for seed in seeds() {
        held.push(
            add_ok(
                &router,
                &holder,
                seed.file_name,
                seed.artifact_type,
                &seed.content,
                "seed",
            )
            .await,
        );
    }
    let kept = held[0].clone();
    let neighbours = add_ok(
        &router,
        &neighbour,
        "neighbours.txt",
        "TXT",
        b"not the holder's",
        "seed",
    )
    .await;
    let gone = add_ok(&router, &holder, "doomed.txt", "TXT", b"doomed", "seed").await;
    delete_ok(&router, &holder, &gone, "the artifact deleted up front").await;

    // (case, the agentSpaceId the delete names, the artifactId it names) — the rows
    // differ along HOW the pair resolves to no record of that space.
    let never_minted = "ffffffffffffffffffffffffffffffff".to_string();
    let readable_label = "no-such-artifact".to_string();
    let other_script = "неизвестно".to_string();
    let suffixed = format!("{kept}0");
    let truncated = kept[..kept.len() - 1].to_string();
    let rows: Vec<(&str, String, String)> = vec![
        (
            "an artifactId nothing minted",
            holder.clone(),
            never_minted,
        ),
        (
            "an artifactId that is a readable label",
            holder.clone(),
            readable_label,
        ),
        (
            "an artifactId in another script",
            holder.clone(),
            other_script,
        ),
        ("an artifactId already deleted", holder.clone(), gone.clone()),
        (
            "the same artifactId deleted a third time",
            holder.clone(),
            gone.clone(),
        ),
        (
            "a stored artifactId with a suffix",
            holder.clone(),
            suffixed,
        ),
        (
            "a stored artifactId with its last character dropped",
            holder.clone(),
            truncated,
        ),
        (
            "another space's artifactId, through this space",
            holder.clone(),
            neighbours.clone(),
        ),
        (
            "this space's artifactId, through the neighbour",
            neighbour.clone(),
            kept.clone(),
        ),
    ];

    for (case, agent_space_id, artifact_id) in rows {
        delete_ok(&router, &agent_space_id, &artifact_id, case).await;

        // Nothing was removed: every seeded artifact of the holding space, and the
        // neighbour's own, still reads back, and the one deleted up front is still gone.
        for (index, seed) in seeds().into_iter().enumerate() {
            assert_reads_back(
                &router,
                &holder,
                &held[index],
                seed.file_name,
                &seed.content,
                &format!("{case}: seed {index} after the delete"),
            )
            .await;
        }
        assert_reads_back(
            &router,
            &neighbour,
            &neighbours,
            "neighbours.txt",
            b"not the holder's",
            &format!("{case}: the neighbour after the delete"),
        )
        .await;
        assert_gone(
            &router,
            &holder,
            &gone,
            &format!("{case}: the artifact deleted up front"),
        )
        .await;
    }
}

/// RULE: a delete the service cannot act on at all is refused and removes nothing — an
/// `agentSpaceId` the account does not hold is HTTP 404 `ResourceNotFoundException` with
/// the agent space's own not-found sentence, and an empty id of either member is HTTP
/// 400 `ValidationException` naming that member.
#[tokio::test]
async fn a_delete_the_service_cannot_act_on_is_refused() {
    let router = emulator();
    let holder = create_agent_space(&router, "artifact-refusal-holder", "seed").await;
    let kept = add_ok(&router, &holder, "kept.txt", "TXT", b"kept", "seed").await;

    // (case, the agentSpaceId the delete names, the artifactId it names, the status, the
    // error code, the message due) — the rows differ along WHICH member cannot be acted
    // on and how.
    let rows: Vec<(&str, String, String, u16, &str, String)> = vec![
        (
            "an agentSpaceId no create minted",
            "f".repeat(32),
            kept.clone(),
            404,
            "ResourceNotFoundException",
            AGENT_SPACE_NOT_FOUND.to_string(),
        ),
        (
            "an agentSpaceId that is a readable label",
            "no-such-space".to_string(),
            kept.clone(),
            404,
            "ResourceNotFoundException",
            AGENT_SPACE_NOT_FOUND.to_string(),
        ),
        (
            "the stored agentSpaceId with a suffix",
            format!("{holder}0"),
            kept.clone(),
            404,
            "ResourceNotFoundException",
            AGENT_SPACE_NOT_FOUND.to_string(),
        ),
        (
            "the empty agentSpaceId",
            String::new(),
            kept.clone(),
            400,
            "ValidationException",
            "1 validation error detected: value for 'agentSpaceId' failed to satisfy \
             constraint: member must not be empty"
                .to_string(),
        ),
        (
            "the empty artifactId",
            holder.clone(),
            String::new(),
            400,
            "ValidationException",
            "1 validation error detected: value for 'artifactId' failed to satisfy \
             constraint: member must not be empty"
                .to_string(),
        ),
        (
            "both ids empty",
            String::new(),
            String::new(),
            400,
            "ValidationException",
            "1 validation error detected: value for 'agentSpaceId' failed to satisfy \
             constraint: member must not be empty"
                .to_string(),
        ),
    ];

    for (case, agent_space_id, artifact_id, status, code, message) in rows {
        let answer = delete(&router, &agent_space_id, &artifact_id).await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (status, code),
            "case {case:?}: expected {status} {code}, got {} ({}) with {}",
            answer.status,
            answer.error_code,
            answer.body
        );
        let actual = refusal_message(&answer.body, case);
        assert_eq!(
            actual, message,
            "case {case:?}: expected {message:?}, got {actual:?}"
        );

        // The refusal removed nothing: the stored artifact still reads back.
        assert_reads_back(
            &router,
            &holder,
            &kept,
            "kept.txt",
            b"kept",
            &format!("{case}: the stored artifact after the refusal"),
        )
        .await;
    }
}

/// RULE: a delete frees what the removed record held, so a space holding the maximum
/// number of artifacts accepts a further upload once one of them is deleted — and only
/// as many further uploads as were deleted.
#[tokio::test]
async fn a_delete_frees_a_slot_of_the_artifact_quota() {
    let router = emulator();
    let full = create_agent_space(&router, "artifact-quota-delete", "seed").await;

    let mut stored = Vec::new();
    for index in 0..MAX_ARTIFACTS_PER_AGENT_SPACE {
        stored.push(
            add_ok(
                &router,
                &full,
                &format!("seed-{index}.txt"),
                "TXT",
                format!("seed {index}").as_bytes(),
                "filling the space",
            )
            .await,
        );
    }

    // The rows differ in how many artifacts are deleted before the next upload is tried:
    // the space must accept exactly as many uploads as were freed.
    for freed in [1usize, 3] {
        let case = format!("after freeing {freed} of {MAX_ARTIFACTS_PER_AGENT_SPACE}");
        let refused = call(
            &router,
            "AddArtifact",
            &json!({
                "agentSpaceId": full,
                "fileName": "one-too-many.txt",
                "artifactType": "TXT",
                "artifactContent": blob(b"one too many"),
            }),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (402, "ServiceQuotaExceededException"),
            "case {case:?}: expected the full space to refuse an upload, got {} ({}) \
             with {}",
            refused.status,
            refused.error_code,
            refused.body
        );

        for _ in 0..freed {
            let victim = stored.remove(0);
            delete_ok(&router, &full, &victim, &case).await;
        }
        for slot in 0..freed {
            stored.push(
                add_ok(
                    &router,
                    &full,
                    &format!("refill-{freed}-{slot}.txt"),
                    "TXT",
                    format!("refill {freed} {slot}").as_bytes(),
                    &case,
                )
                .await,
            );
        }
        assert_eq!(
            stored.len(),
            MAX_ARTIFACTS_PER_AGENT_SPACE,
            "case {case:?}: expected the space to be full again, holding {} artifacts",
            stored.len()
        );
    }
}
