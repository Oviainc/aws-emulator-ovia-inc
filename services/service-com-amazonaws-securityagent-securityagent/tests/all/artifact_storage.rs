// SPDX-License-Identifier: CC0-1.0

//! What an artifact of an agent space IS, end to end over the wire: `AddArtifact`
//! uploads one and `GetArtifact` reads it back.
//!
//! Every call goes through the real `MultiServiceRouter` the emulator boots, as a
//! `restJson1` `POST /<OperationName>` with a JSON body signed for the `securityagent`
//! signing name; the harness is `crate::support`. So nothing in this file can pass
//! against a handler that answers a canned body: a value must come back out of the
//! shared store through a read before a case counts.
//!
//! The rules this suite owns:
//!
//! * an upload PERSISTS the file name, the declared type and the uploaded bytes under a
//!   freshly minted `artifactId`, and a read of that id reports that record — the bytes
//!   in the base64 the `contents` string carries them in, which a caller reverses to get
//!   exactly the bytes it uploaded;
//! * the id is the service's to mint, so two uploads of the SAME file are two distinct
//!   artifacts and neither overwrites the other;
//! * an artifact is reachable only through the agent space that holds it, so a read is
//!   refused for an id that space does not hold — including one that names another
//!   space's artifact — and for a space the account does not hold;
//! * an upload the service refuses persists nothing and leaves every stored artifact as
//!   it was;
//! * the artifact quota is PER agent space.
//!
//! Each case asserts a rule over a TABLE of inputs: the rows differ along the declared
//! type, the form and the size of the content (text, text outside ASCII, bytes that
//! encode no text at all, nothing at all, the size ceiling), the file name, how often
//! the same file is uploaded, and which id of the pair names nothing.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_securityagent_securityagent::state::{
    MAX_ARTIFACTS_PER_AGENT_SPACE, MAX_ARTIFACT_CONTENT_BYTES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{call, emulator, member, ok};

/// The sentence a refusal of an agent space the account does not hold states: wholly
/// FIXED text, naming the resource class under the name the service's own records use
/// for an agent space and carrying nothing the request sent.
const AGENT_SPACE_NOT_FOUND: &str = "The specified agent instance does not exist.";

/// The sentence a refusal of an artifact the named agent space does not hold states:
/// fixed text naming the resource class, as every other not-found sentence of this
/// service does.
const ARTIFACT_NOT_FOUND: &str = "The specified artifact does not exist.";

/// One upload the service must refuse: what a caller sends, and the status and error
/// code the refusal carries.
struct Refusal {
    case: &'static str,
    agent_space_id: String,
    file_name: String,
    artifact_type: &'static str,
    content: Vec<u8>,
    status: u16,
    code: &'static str,
}

/// One upload: what a caller sends, and the bytes a read of it must carry back.
struct Upload {
    case: &'static str,
    file_name: &'static str,
    /// An `ArtifactType` member (see [`uploads`] for the source of the catalog).
    artifact_type: &'static str,
    content: Vec<u8>,
}

/// The bytes of one upload, as the JSON body carries a blob and as the `contents`
/// string of a read carries them back: base64, with the codec the wire serializer uses
/// (`aws_smithy_types::base64`).
fn blob(content: &[u8]) -> String {
    aws_smithy_types::base64::encode(content)
}

/// The bytes a `contents` string stands for: what a caller gets by reversing the
/// encoding, which must be the bytes it uploaded.
fn decoded(contents: &str, case: &str) -> Vec<u8> {
    aws_smithy_types::base64::decode(contents).unwrap_or_else(|error| {
        panic!(
            "case {case:?}: contents {:?} is not base64 ({error})",
            head(contents)
        )
    })
}

/// At most the first 64 characters of `value`, so a failure names the actual value
/// without printing a whole artifact.
fn head(value: &str) -> String {
    value.chars().take(64).collect()
}

/// One upload per `ArtifactType` the model admits, plus the content forms the rule
/// generalises over: text, text outside ASCII, bytes that encode no text, no content at
/// all, and content at the published size ceiling.
///
/// The nine types are every member of the enum
/// (`smithy_query(query="shape",shape="ArtifactType",
/// model_path="services/service-com-amazonaws-securityagent-securityagent/smithy-model/com-amazonaws-securityagent-securityagent.json")`:
/// TXT, PNG, JPEG, MD, PDF, DOCX, DOC, JSON, YAML), so a member added to the enum makes
/// this table go stale.
fn uploads() -> Vec<Upload> {
    vec![
        Upload {
            case: "a text file",
            file_name: "architecture-notes.txt",
            artifact_type: "TXT",
            content: b"the service talks to one database and one queue".to_vec(),
        },
        Upload {
            case: "markdown carrying newlines",
            file_name: "design.md",
            artifact_type: "MD",
            content: b"# Design\n\n- one component\n- two trust boundaries\n".to_vec(),
        },
        Upload {
            case: "a JSON API specification",
            file_name: "api.json",
            artifact_type: "JSON",
            content: br#"{"openapi":"3.0.0","paths":{"/orders":{}}}"#.to_vec(),
        },
        Upload {
            case: "a YAML template",
            file_name: "stack.yaml",
            artifact_type: "YAML",
            content: b"Resources:\n  Bucket:\n    Type: AWS::S3::Bucket\n".to_vec(),
        },
        Upload {
            case: "text outside ASCII",
            file_name: "diagramme.txt",
            artifact_type: "TXT",
            content: "architecture — diagramme ✓ schéma".as_bytes().to_vec(),
        },
        Upload {
            case: "no content at all",
            file_name: "empty.txt",
            artifact_type: "TXT",
            content: Vec::new(),
        },
        Upload {
            case: "a PNG's bytes",
            file_name: "diagram.png",
            // The PNG signature (ISO 15948 §5.2) followed by an IHDR length and tag.
            artifact_type: "PNG",
            content: vec![
                0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, b'I',
                b'H', b'D', b'R', 0xff, 0xfe, 0x80, 0x01,
            ],
        },
        Upload {
            case: "a JPEG's bytes",
            file_name: "whiteboard.jpeg",
            // SOI + APP0 with the JFIF identifier.
            artifact_type: "JPEG",
            content: vec![
                0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0x00, 0x01, 0x02,
                0x80, 0xfd,
            ],
        },
        Upload {
            case: "a PDF with a binary tail",
            file_name: "threat-model.pdf",
            artifact_type: "PDF",
            content: {
                let mut content = b"%PDF-1.7\n".to_vec();
                content.extend_from_slice(&[0x80, 0x9f, 0xa0, 0xff, 0xfe]);
                content
            },
        },
        Upload {
            case: "a DOCX zip container",
            file_name: "report.docx",
            // The local file header a zip container opens with (PK\x03\x04).
            artifact_type: "DOCX",
            content: vec![b'P', b'K', 0x03, 0x04, 0x14, 0x00, 0x00, 0x08, 0xff, 0xc0],
        },
        Upload {
            case: "a legacy DOC with an OLE header",
            file_name: "legacy.doc",
            // The OLE compound-file signature.
            artifact_type: "DOC",
            content: vec![0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1, 0x00, 0x90],
        },
        Upload {
            case: "content at the size ceiling",
            file_name: "evidence-at-the-ceiling.txt",
            artifact_type: "TXT",
            content: vec![b'e'; MAX_ARTIFACT_CONTENT_BYTES],
        },
    ]
}

/// `CreateAgentSpace`, as the id of the record it minted.
async fn create_agent_space(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let created = ok(router, "CreateAgentSpace", &json!({"name": name}), case).await;
    member(&created, "agentSpaceId")
        .unwrap_or_else(|| panic!("case {case:?}: CreateAgentSpace returned no agentSpaceId"))
}

/// `AddArtifact`, as the whole answer it gave.
async fn add(
    router: &MultiServiceRouter,
    agent_space_id: &str,
    file_name: &str,
    artifact_type: &str,
    content: &[u8],
) -> crate::support::Answer {
    call(
        router,
        "AddArtifact",
        &json!({
            "agentSpaceId": agent_space_id,
            "fileName": file_name,
            "artifactType": artifact_type,
            "artifactContent": blob(content),
        }),
    )
    .await
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
    let answer = add(router, agent_space_id, file_name, artifact_type, content).await;
    // The operation's own success status (`smithy_query(query="operation",
    // operation="AddArtifact")`: http code 201), not the service's usual 200.
    assert_eq!(
        answer.status, 201,
        "case {case:?}: expected AddArtifact to answer 201, got {} ({}) with {}",
        answer.status, answer.error_code, answer.body
    );
    let artifact_id = member(&answer.body, "artifactId")
        .unwrap_or_else(|| panic!("case {case:?}: AddArtifact returned no artifactId"));
    assert!(
        !artifact_id.is_empty(),
        "case {case:?}: expected a minted artifactId, got an empty one"
    );
    artifact_id
}

/// `GetArtifact`, as the whole answer it gave.
async fn get(router: &MultiServiceRouter, agent_space_id: &str, artifact_id: &str) -> crate::support::Answer {
    call(
        router,
        "GetArtifact",
        &json!({"agentSpaceId": agent_space_id, "artifactId": artifact_id}),
    )
    .await
}

/// The `artifact` member of a read, which the model makes `@required`.
fn artifact_of(payload: &Value, case: &str) -> Value {
    payload
        .get("artifact")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the read carries no artifact member: {payload}"))
}

/// `updatedAt` is a `@timestampFormat("date-time")` member, so the wire form is an RFC
/// 3339 instant STRING, never an epoch number — and it must be the instant the record
/// was written rather than a placeholder.
fn assert_updated_at(payload: &Value, case: &str) {
    let value = payload
        .get("updatedAt")
        .unwrap_or_else(|| panic!("case {case:?}: the read carries no updatedAt: {payload}"));
    let text = value
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected updatedAt as a string, got {value}"));
    let instant = DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: updatedAt {text:?} is not an RFC 3339 date-time: {error}")
    });
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the test clock is after the epoch")
        .as_secs_f64();
    let seconds = instant.as_secs_f64();
    assert!(
        seconds > now - 3600.0 && seconds <= now + 1.0,
        "case {case:?}: updatedAt {text:?} is not the instant this artifact was written \
         (now {now})"
    );
}

/// RULE: an upload persists the file name, the declared type and the uploaded bytes
/// under an `artifactId` the service mints, and a read of that id in that agent space
/// reports that stored record — the content as the base64 of the bytes that were
/// uploaded, which a caller reverses to get those exact bytes back; a read changes
/// nothing, so a second read answers the same payload.
#[tokio::test]
async fn an_uploaded_artifact_reads_back_as_it_was_uploaded() {
    let router = emulator();
    let agent_space_id = create_agent_space(&router, "artifact-round-trip", "seed").await;

    let mut minted: Vec<String> = Vec::new();
    for upload in uploads() {
        let case = upload.case;
        let artifact_id = add_ok(
            &router,
            &agent_space_id,
            upload.file_name,
            upload.artifact_type,
            &upload.content,
            case,
        )
        .await;
        // The id is the service's to choose, so every upload is a distinct artifact.
        assert!(
            !minted.contains(&artifact_id),
            "case {case:?}: expected a fresh artifactId, got {artifact_id:?} again"
        );
        minted.push(artifact_id.clone());

        let answer = get(&router, &agent_space_id, &artifact_id).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected GetArtifact to answer 200, got {} ({})",
            answer.status, answer.error_code
        );
        let read = answer.body;
        assert_eq!(
            member(&read, "agentSpaceId").as_deref(),
            Some(agent_space_id.as_str()),
            "case {case:?}: expected the read to name the agent space that holds the \
             artifact, got {:?}",
            member(&read, "agentSpaceId")
        );
        assert_eq!(
            member(&read, "artifactId").as_deref(),
            Some(artifact_id.as_str()),
            "case {case:?}: expected the read to name the artifact {artifact_id:?}, got {:?}",
            member(&read, "artifactId")
        );
        assert_eq!(
            member(&read, "fileName").as_deref(),
            Some(upload.file_name),
            "case {case:?}: expected the stored fileName, got {:?}",
            member(&read, "fileName")
        );
        assert_updated_at(&read, case);

        let artifact = artifact_of(&read, case);
        assert_eq!(
            member(&artifact, "type").as_deref(),
            Some(upload.artifact_type),
            "case {case:?}: expected the declared artifact type, got {:?}",
            member(&artifact, "type")
        );
        let expected = blob(&upload.content);
        let actual = member(&artifact, "contents").unwrap_or_else(|| {
            panic!("case {case:?}: the artifact carries no contents: {artifact}")
        });
        assert!(
            actual == expected,
            "case {case:?}: expected the uploaded content back ({} characters starting \
             {:?}), got {} characters starting {:?}",
            expected.len(),
            head(&expected),
            actual.len(),
            head(&actual)
        );
        // And reversing the encoding yields the bytes that were uploaded, byte for byte.
        let round_tripped = decoded(&actual, case);
        assert!(
            round_tripped == upload.content,
            "case {case:?}: expected the {} uploaded bytes back, got {} bytes",
            upload.content.len(),
            round_tripped.len()
        );

        // A read does not change what a later caller sees.
        let again = get(&router, &agent_space_id, &artifact_id).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: expected a second read to answer 200, got {} ({})",
            again.status, again.error_code
        );
        assert_eq!(
            again.body, read,
            "case {case:?}: expected a second read to answer the same record"
        );
    }
}

/// RULE: the `artifactId` is minted per call, so uploading the SAME file again is
/// another artifact: each upload answers its own id, and every one of them still reads
/// back with the content it was uploaded with.
#[tokio::test]
async fn the_same_file_uploaded_again_is_another_artifact() {
    let router = emulator();
    let agent_space_id = create_agent_space(&router, "artifact-repeats", "seed").await;

    // The rows differ along how often the same file is uploaded, and in the content and
    // the type that is repeated.
    let rows: Vec<(&str, usize, &str, &str, &[u8])> = vec![
        ("one upload", 1, "once.txt", "TXT", b"uploaded once"),
        ("the same text twice", 2, "twice.md", "MD", b"# twice\n"),
        (
            "the same JSON four times",
            4,
            "four.json",
            "JSON",
            br#"{"uploads":4}"#,
        ),
        ("the same empty file three times", 3, "empty.txt", "TXT", b""),
    ];

    for (case, count, file_name, artifact_type, content) in rows {
        let mut ids: Vec<String> = Vec::new();
        for _ in 0..count {
            let artifact_id =
                add_ok(&router, &agent_space_id, file_name, artifact_type, content, case).await;
            assert!(
                !ids.contains(&artifact_id),
                "case {case:?}: expected every upload to mint its own id, got \
                 {artifact_id:?} twice"
            );
            ids.push(artifact_id);
        }
        // Every one of them is still its own stored record afterwards.
        for (index, artifact_id) in ids.iter().enumerate() {
            let answer = get(&router, &agent_space_id, artifact_id).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected upload {index} to read back, got {} ({})",
                answer.status, answer.error_code
            );
            let artifact = artifact_of(&answer.body, case);
            let expected = blob(content);
            assert_eq!(
                member(&artifact, "contents").as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected upload {index} to report its own content, got {artifact}"
            );
            assert_eq!(
                member(&answer.body, "fileName").as_deref(),
                Some(file_name),
                "case {case:?}: expected upload {index} to report its own fileName, got {}",
                answer.body
            );
        }
    }
}

/// RULE: an artifact is reachable only through the agent space that holds it, so a read
/// resolves the PAIR of ids: an `artifactId` that space holds no record for — including
/// one that names ANOTHER space's artifact, and one whose space a delete removed — is
/// refused with the service's fixed not-found sentence, an `agentSpaceId` the account
/// does not hold with the agent space's own, and an empty id of either kind as a bad
/// request.
#[tokio::test]
async fn an_artifact_is_reachable_only_through_the_space_that_holds_it() {
    let router = emulator();
    let holder = create_agent_space(&router, "artifact-holder", "seed").await;
    let neighbour = create_agent_space(&router, "artifact-neighbour", "seed").await;
    let doomed = create_agent_space(&router, "artifact-doomed", "seed").await;

    let held = add_ok(&router, &holder, "held.txt", "TXT", b"held here", "seed").await;
    let neighbours = add_ok(
        &router,
        &neighbour,
        "neighbour.txt",
        "TXT",
        b"held next door",
        "seed",
    )
    .await;
    let orphaned = add_ok(&router, &doomed, "doomed.txt", "TXT", b"held briefly", "seed").await;

    // The agent space goes, and the artifacts reachable only through it go with it.
    ok(
        &router,
        "DeleteAgentSpace",
        &json!({"agentSpaceId": doomed}),
        "deleting the space that holds an artifact",
    )
    .await;

    // The rows differ along WHICH id of the pair names nothing and in what form: an id
    // no upload minted (32 hex `f`s, a readable label, a name in another script, a
    // stored id with a suffix), the id of another space's artifact, an id whose space
    // was deleted, and the empty id of either member.
    let rows: Vec<(&str, String, String, u16, &str, &str)> = vec![
        (
            "the artifact of another agent space",
            holder.clone(),
            neighbours.clone(),
            404,
            "ResourceNotFoundException",
            ARTIFACT_NOT_FOUND,
        ),
        (
            "an artifactId of 32 hex f's",
            holder.clone(),
            "f".repeat(32),
            404,
            "ResourceNotFoundException",
            ARTIFACT_NOT_FOUND,
        ),
        (
            "an artifactId that is a readable label",
            holder.clone(),
            "no-such-artifact".to_string(),
            404,
            "ResourceNotFoundException",
            ARTIFACT_NOT_FOUND,
        ),
        (
            "an artifactId in another script",
            holder.clone(),
            "неизвестно".to_string(),
            404,
            "ResourceNotFoundException",
            ARTIFACT_NOT_FOUND,
        ),
        (
            "a stored artifactId with a suffix",
            holder.clone(),
            format!("{held}-suffix"),
            404,
            "ResourceNotFoundException",
            ARTIFACT_NOT_FOUND,
        ),
        (
            "an artifact whose agent space was deleted",
            doomed.clone(),
            orphaned.clone(),
            404,
            "ResourceNotFoundException",
            AGENT_SPACE_NOT_FOUND,
        ),
        (
            "an agentSpaceId no create minted",
            "f".repeat(32),
            held.clone(),
            404,
            "ResourceNotFoundException",
            AGENT_SPACE_NOT_FOUND,
        ),
        (
            "the empty agentSpaceId",
            String::new(),
            held.clone(),
            400,
            "ValidationException",
            "1 validation error detected: value for 'agentSpaceId' failed to satisfy \
             constraint: member must not be empty",
        ),
        (
            "the empty artifactId",
            holder.clone(),
            String::new(),
            400,
            "ValidationException",
            "1 validation error detected: value for 'artifactId' failed to satisfy \
             constraint: member must not be empty",
        ),
    ];

    for (case, agent_space_id, artifact_id, status, code, message) in rows {
        let answer = get(&router, &agent_space_id, &artifact_id).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected a {status} refusal, got {} ({}) with {}",
            answer.status, answer.error_code, answer.body
        );
        assert_eq!(
            answer.error_code, code,
            "case {case:?}: expected the {code} error code, got {:?} with {}",
            answer.error_code, answer.body
        );
        assert_eq!(
            member(&answer.body, "message").as_deref(),
            Some(message),
            "case {case:?}: expected the service's own sentence for the refusal, got {}",
            answer.body
        );
    }

    // Each artifact still reads back through the space that does hold it: a refused read
    // removed nothing.
    for (case, agent_space_id, artifact_id, content) in [
        ("the holder's artifact", &holder, &held, b"held here".to_vec()),
        (
            "the neighbour's artifact",
            &neighbour,
            &neighbours,
            b"held next door".to_vec(),
        ),
    ] {
        let answer = get(&router, agent_space_id, artifact_id).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected the stored artifact to read back, got {} ({})",
            answer.status, answer.error_code
        );
        let artifact = artifact_of(&answer.body, case);
        assert_eq!(
            member(&artifact, "contents").as_deref(),
            Some(blob(&content).as_str()),
            "case {case:?}: expected the stored content, got {artifact}"
        );
    }
}

/// RULE: an upload the service refuses persists NOTHING — it mints no artifact and
/// leaves every stored artifact as it was: an `agentSpaceId` the account does not hold
/// is refused with the agent space's own not-found sentence, and an input no modelled
/// constraint covers (an empty id, a blank file name, content past the size ceiling) as
/// a bad request naming the member.
#[tokio::test]
async fn a_refused_upload_persists_nothing() {
    let router = emulator();
    let agent_space_id = create_agent_space(&router, "artifact-refusals", "seed").await;
    let stored = add_ok(
        &router,
        &agent_space_id,
        "already-here.txt",
        "TXT",
        b"stored before the refusals",
        "seed",
    )
    .await;

    // The rows differ along which member is wrong and in what way: the agent space, the
    // file name (empty, spaces, a tab and a newline), the content size (one byte past
    // the ceiling) and the declared type (outside the enum, which the boundary refuses).
    let rows: Vec<Refusal> = vec![
        Refusal {
            case: "an agentSpaceId no create minted",
            agent_space_id: "f".repeat(32),
            file_name: "orphan.txt".to_string(),
            artifact_type: "TXT",
            content: b"nowhere to put this".to_vec(),
            status: 404,
            code: "ResourceNotFoundException",
        },
        Refusal {
            case: "the empty agentSpaceId",
            agent_space_id: String::new(),
            file_name: "orphan.txt".to_string(),
            artifact_type: "TXT",
            content: b"nowhere to put this".to_vec(),
            status: 400,
            code: "ValidationException",
        },
        Refusal {
            case: "an empty fileName",
            agent_space_id: agent_space_id.clone(),
            file_name: String::new(),
            artifact_type: "TXT",
            content: b"a file with no name".to_vec(),
            status: 400,
            code: "ValidationException",
        },
        Refusal {
            case: "a fileName of spaces",
            agent_space_id: agent_space_id.clone(),
            file_name: "   ".to_string(),
            artifact_type: "TXT",
            content: b"a file with no name".to_vec(),
            status: 400,
            code: "ValidationException",
        },
        Refusal {
            case: "a fileName of a tab and a newline",
            agent_space_id: agent_space_id.clone(),
            file_name: "\t\n".to_string(),
            artifact_type: "MD",
            content: b"a file with no name".to_vec(),
            status: 400,
            code: "ValidationException",
        },
        Refusal {
            case: "content one byte past the ceiling",
            agent_space_id: agent_space_id.clone(),
            file_name: "too-big.txt".to_string(),
            artifact_type: "TXT",
            content: vec![b'e'; MAX_ARTIFACT_CONTENT_BYTES + 1],
            status: 400,
            code: "ValidationException",
        },
        Refusal {
            case: "an artifactType outside the enum",
            agent_space_id: agent_space_id.clone(),
            file_name: "bad.exe".to_string(),
            artifact_type: "EXE",
            content: b"malware.exe".to_vec(),
            status: 400,
            code: "ValidationException",
        },
    ];

    for row in rows {
        let Refusal {
            case,
            agent_space_id: space,
            file_name,
            artifact_type,
            content,
            status,
            code,
        } = row;
        let answer = add(&router, &space, &file_name, artifact_type, &content).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected a {status} refusal, got {} ({})",
            answer.status, answer.error_code
        );
        assert_eq!(
            answer.error_code, code,
            "case {case:?}: expected the {code} error code, got {:?} with {}",
            answer.error_code, answer.body
        );
        // A refusal is not an upload: it names no artifact.
        assert!(
            member(&answer.body, "artifactId").is_none(),
            "case {case:?}: expected a refusal to mint no artifactId, got {}",
            answer.body
        );
        if status == 404 {
            assert_eq!(
                member(&answer.body, "message").as_deref(),
                Some(AGENT_SPACE_NOT_FOUND),
                "case {case:?}: expected the agent space's own not-found sentence, got {}",
                answer.body
            );
        }
    }

    // The artifact that was there before the refusals is untouched, and an upload after
    // them is still accepted and reads back.
    let answer = get(&router, &agent_space_id, &stored).await;
    assert_eq!(
        answer.status, 200,
        "expected the artifact stored before the refusals to read back, got {} ({})",
        answer.status, answer.error_code
    );
    let artifact = artifact_of(&answer.body, "after the refusals");
    assert_eq!(
        member(&artifact, "contents").as_deref(),
        Some(blob(b"stored before the refusals").as_str()),
        "expected the stored content to survive the refusals, got {artifact}"
    );
    let after = add_ok(
        &router,
        &agent_space_id,
        "after.txt",
        "TXT",
        b"stored after the refusals",
        "after the refusals",
    )
    .await;
    assert_ne!(
        after, stored,
        "expected the upload after the refusals to mint its own id"
    );
}

/// RULE: the artifact quota is PER agent space — a space holding the maximum refuses a
/// further upload with the service's `ServiceQuotaExceededException` (HTTP 402), while
/// every other space still accepts one.
#[tokio::test]
async fn the_artifact_quota_is_per_agent_space() {
    let router = emulator();
    let full = create_agent_space(&router, "artifact-quota-full", "seed").await;
    let spare = create_agent_space(&router, "artifact-quota-spare", "seed").await;

    for index in 0..MAX_ARTIFACTS_PER_AGENT_SPACE {
        add_ok(
            &router,
            &full,
            &format!("filler-{index}.txt"),
            "TXT",
            format!("filler {index}").as_bytes(),
            "filling the quota",
        )
        .await;
    }

    // The rows differ along the content and the type of the upload the full space
    // refuses: the quota is about the number of artifacts, not about what they carry.
    let rows: Vec<(&str, &str, &str, &[u8])> = vec![
        ("one more text file", "one-too-many.txt", "TXT", b"one too many"),
        ("one more empty file", "one-too-many-empty.md", "MD", b""),
        (
            "one more JSON document",
            "one-too-many.json",
            "JSON",
            br#"{"one":"too many"}"#,
        ),
    ];

    for (case, file_name, artifact_type, content) in rows {
        let answer = add(&router, &full, file_name, artifact_type, content).await;
        assert_eq!(
            answer.status, 402,
            "case {case:?}: expected the quota refusal's 402, got {} ({}) with {}",
            answer.status, answer.error_code, answer.body
        );
        assert_eq!(
            answer.error_code, "ServiceQuotaExceededException",
            "case {case:?}: expected the ServiceQuotaExceededException error code, got {:?} \
             with {}",
            answer.error_code, answer.body
        );
        assert!(
            member(&answer.body, "message")
                .is_some_and(|message| message.contains(&MAX_ARTIFACTS_PER_AGENT_SPACE.to_string())),
            "case {case:?}: expected the refusal to state the quota it exceeded, got {}",
            answer.body
        );

        // The quota is the full space's own: the neighbour still accepts the very same
        // upload, and it reads back there.
        let accepted = add_ok(&router, &spare, file_name, artifact_type, content, case).await;
        let read = get(&router, &spare, &accepted).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the neighbour's upload to read back, got {} ({})",
            read.status, read.error_code
        );
    }
}
