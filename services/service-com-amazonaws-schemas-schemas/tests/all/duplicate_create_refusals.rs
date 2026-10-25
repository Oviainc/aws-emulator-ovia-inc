// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ENVELOPE a duplicate create is refused with.
//!
//! RULE: a create whose requested resource is already there is refused as a CONFLICT —
//! this service's conflict shape, HTTP 409 — under the already-exists condition code
//! and the service's already-exists sentence built from the identifier the request
//! sent, leaving the stored resource exactly as it was. The class follows the
//! CONDITION (the request is well-formed; what stops it is the stored resource), not
//! which shapes one operation's modelled `errors` list happens to enumerate.
//!
//! Every create this service implements is a row, so the rule is asserted where it
//! holds rather than only on the operation an issue cited.
//!
//! Every case drives the real wire path, through the crate's one harness in
//! `registry_schema_lifecycle.rs`; nothing here touches the state struct directly.

use serde_json::{json, Value};
use service_com_amazonaws_schemas_schemas::state::{
    CODE_MALFORMED_REQUEST, CODE_RESOURCE_ALREADY_EXIST,
};

use crate::discoverer_state::{discoverer_uri, source_arn, DISCOVERERS_URI};
use crate::registry_schema_lifecycle::{registry_uri, schema_uri, Answer, Emulator};

/// The registry the schema rows create their schemas in.
const HOST_REGISTRY: &str = "duplicate-create-host";

/// The status and shape every row expects.
///
/// Source: `smithy_query(query="shape", shape="ConflictException")` on this service's
/// model — `errorKind` client, `httpError` 409 — the shape the service binds to a
/// resource that already exists wherever it declares one.
const CONFLICT_STATUS: u16 = 409;
const CONFLICT_SHAPE: &str = "ConflictException";

/// Which create a row is about: every create operation this service implements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Create {
    Registry,
    Schema,
    Discoverer,
}

/// One create operation and how its already-exists sentence names the resource the
/// request asked for.
#[derive(Debug)]
struct Row {
    create: Create,
    /// The sentence the refusal must carry, built from the identifier the request sent
    /// (`token`, below) exactly as the service builds it.
    sentence: fn(&str) -> String,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            create: Create::Registry,
            sentence: |token| format!("Registry with name {token} already exists."),
        },
        Row {
            create: Create::Schema,
            sentence: |token| format!("Schema with name {token} already exists."),
        },
        Row {
            create: Create::Discoverer,
            // A discoverer is identified by the source it discovers, so its sentence
            // names that source rather than a name the caller chose.
            sentence: |token| format!("Discoverer for source {} already exists.", source_arn(token)),
        },
    ]
}

/// A body `CreateSchema` accepts, distinct per `marker`.
fn schema_body(marker: &str) -> Value {
    json!({
        "Content": format!(r#"{{"openapi":"3.0.0","info":{{"title":"{marker}","version":"1"}}}}"#),
        "Type": "OpenApi3",
    })
}

/// The URI the named create posts to. A discoverer's identity travels in the BODY, so
/// its route carries no identifier.
fn create_uri(create: Create, token: &str) -> String {
    match create {
        Create::Registry => registry_uri(token),
        Create::Schema => schema_uri(HOST_REGISTRY, token),
        Create::Discoverer => DISCOVERERS_URI.to_string(),
    }
}

/// The body the FIRST (succeeding) create sends.
fn first_body(create: Create, token: &str) -> Value {
    match create {
        Create::Registry => json!({"Description": "the original"}),
        Create::Schema => schema_body("original"),
        Create::Discoverer => json!({"SourceArn": source_arn(token)}),
    }
}

/// Bodies a SECOND create of the same resource sends: the rule is about the IDENTITY
/// the request asks for, so it holds however the rest of the request differs from the
/// stored resource — same body, a different description, different tags, different
/// content, and (for a schema) a different `Type` with a document of that type.
fn second_bodies(create: Create, token: &str) -> Vec<(&'static str, Value)> {
    match create {
        Create::Registry => vec![
            ("identical body", json!({"Description": "the original"})),
            ("no members at all", json!({})),
            ("a different description", json!({"Description": "the impostor"})),
            ("tags the stored resource has not", json!({"tags": {"owner": "someone"}})),
        ],
        Create::Schema => vec![
            ("identical body", schema_body("original")),
            ("different content", schema_body("impostor")),
            (
                "a different Type",
                json!({"Content": r#"{"$schema":"http://json-schema.org/draft-04/schema#"}"#,
                       "Type": "JSONSchemaDraft4"}),
            ),
            (
                "a description and tags",
                json!({"Content": r#"{"openapi":"3.0.0"}"#, "Type": "OpenApi3",
                       "Description": "the impostor", "tags": {"owner": "someone"}}),
            ),
        ],
        Create::Discoverer => vec![
            ("identical body", json!({"SourceArn": source_arn(token)})),
            (
                "a different description",
                json!({"SourceArn": source_arn(token), "Description": "the impostor"}),
            ),
            (
                "CrossAccount flipped",
                json!({"SourceArn": source_arn(token), "CrossAccount": false}),
            ),
            (
                "tags the stored resource has not",
                json!({"SourceArn": source_arn(token), "tags": {"owner": "someone"}}),
            ),
        ],
    }
}

/// The URI a read of the stored resource goes to, after the first create answered
/// `created`. A discoverer's id is MINTED by the service, so it is read back from the
/// create's answer rather than built from the request.
fn read_uri(create: Create, token: &str, created: &Answer) -> String {
    match create {
        Create::Registry | Create::Schema => create_uri(create, token),
        Create::Discoverer => discoverer_uri(&created.str_at("DiscovererId")),
    }
}

/// What a read of the resource must still report after a refused duplicate: the whole
/// body the implemented Describe answers with.
async fn describe(emulator: &Emulator, case: &str, uri: &str) -> Answer {
    let described = emulator.call("GET", uri, None).await;
    assert_eq!(
        described.status, 200,
        "case {case:?}: the stored resource must survive a refused duplicate, got {} {:?}",
        described.status, described.body
    );
    described
}

/// Assert one refusal's whole envelope: status, the shape in the header and in the
/// restJson1 body, the condition code and the sentence.
fn assert_refusal(case: &str, row: &Row, token: &str, answer: &Answer) {
    assert_eq!(
        answer.status, CONFLICT_STATUS,
        "case {case:?}: expected HTTP {CONFLICT_STATUS}, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_type(),
        CONFLICT_SHAPE,
        "case {case:?}: expected x-amzn-errortype {CONFLICT_SHAPE}, got {:?}",
        answer.header("x-amzn-errortype")
    );
    assert_eq!(
        answer.error_code(),
        CONFLICT_SHAPE,
        "case {case:?}: expected the restJson1 __type {CONFLICT_SHAPE}, got {:?}",
        answer.body
    );
    let code = answer.str_at("Code");
    assert_eq!(
        code, CODE_RESOURCE_ALREADY_EXIST,
        "case {case:?}: a duplicate create carries the already-exists condition code, \
         expected {CODE_RESOURCE_ALREADY_EXIST}, got {code:?}"
    );
    let message = answer.str_at("Message");
    assert_eq!(
        message,
        (row.sentence)(token),
        "case {case:?}: wrong already-exists sentence"
    );
}

/// RULE (above): every duplicate create this service serves is refused as a 409
/// conflict under one already-exists condition code, and the stored resource is
/// untouched.
#[tokio::test]
async fn a_duplicate_create_is_refused_as_a_conflict_whatever_the_resource() {
    // Identifiers differing in shape, length and alphabet: the rule generalises over
    // the identity the request sent, not over any one value.
    let tokens: Vec<String> = vec![
        "dup".to_string(),
        "order.placed_v1".to_string(),
        "UPPER-and-lower-99".to_string(),
        "n".repeat(64),
    ];

    for row in rows() {
        for token in &tokens {
            for (variant, body) in second_bodies(row.create, token) {
                let case = format!("{:?} {token:?} recreated with {variant}", row.create);
                let emulator = Emulator::new();
                if row.create == Create::Schema {
                    let host = emulator
                        .call("POST", &registry_uri(HOST_REGISTRY), Some(json!({})))
                        .await;
                    assert_eq!(
                        host.status, 201,
                        "case {case:?}: seeding the registry must succeed, got {} {:?}",
                        host.status, host.body
                    );
                }

                let created = emulator
                    .call(
                        "POST",
                        &create_uri(row.create, token),
                        Some(first_body(row.create, token)),
                    )
                    .await;
                assert_eq!(
                    created.status, 201,
                    "case {case:?}: the first create must succeed, got {} {:?}",
                    created.status, created.body
                );
                let uri = read_uri(row.create, token, &created);
                let before = describe(&emulator, &case, &uri).await;

                // Repeating the duplicate must not drift: the same refusal every
                // time, and never a different class on the second attempt.
                for attempt in 1..=2 {
                    let refused = emulator
                        .call("POST", &create_uri(row.create, token), Some(body.clone()))
                        .await;
                    assert_refusal(&format!("{case} (attempt {attempt})"), &row, token, &refused);
                }

                // Round trip: the read-back is byte-for-byte what it was before the
                // refused creates, so nothing the duplicate carried reached the store.
                let after = describe(&emulator, &case, &uri).await;
                assert_eq!(
                    after.body, before.body,
                    "case {case:?}: a refused duplicate must change nothing, \
                     was {:?}, now {:?}",
                    before.body, after.body
                );
                if row.create == Create::Schema {
                    assert_eq!(
                        after.str_at("SchemaVersion"),
                        "1",
                        "case {case:?}: a refused duplicate must mint no version, got {:?}",
                        after.body
                    );
                }
            }
        }
    }
}

/// RULE: the class follows the condition, so the two ways a `CreateSchema` can be
/// refused on the SAME taken name are told apart by status, shape and code — the name
/// already taken is the 409 conflict, while a body that is not a schema document is
/// the 400 malformed request, judged from the request alone before the store's
/// contents are read at all.
#[tokio::test]
async fn create_schema_separates_a_taken_name_from_a_malformed_body() {
    let emulator = Emulator::new();
    emulator
        .call("POST", &registry_uri(HOST_REGISTRY), Some(json!({})))
        .await;
    let schema = "both-refusals";
    let created = emulator
        .call(
            "POST",
            &schema_uri(HOST_REGISTRY, schema),
            Some(schema_body("original")),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "seeding the schema must succeed, got {} {:?}",
        created.status, created.body
    );

    // Rows: the condition, the body that provokes it, and the envelope it must carry.
    // Every row is sent under the SAME (taken) name, which is what shows the request
    // itself is judged before the store's contents are: `Content` and `Type` are
    // required members of this operation, so a body that is not a schema document is
    // refused as malformed whether or not the name is free.
    for (case, body, status, shape, code) in [
        (
            "a name the registry already holds",
            schema_body("impostor"),
            CONFLICT_STATUS,
            CONFLICT_SHAPE,
            CODE_RESOURCE_ALREADY_EXIST,
        ),
        (
            "content that is not a schema document",
            json!({"Content": "not json at all", "Type": "OpenApi3"}),
            400,
            "BadRequestException",
            CODE_MALFORMED_REQUEST,
        ),
        (
            "content that is JSON but not an object",
            json!({"Content": "[1,2,3]", "Type": "OpenApi3"}),
            400,
            "BadRequestException",
            CODE_MALFORMED_REQUEST,
        ),
    ] {
        let refused = emulator
            .call("POST", &schema_uri(HOST_REGISTRY, schema), Some(body))
            .await;
        assert_eq!(
            refused.status, status,
            "case {case:?}: expected {status}, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type(),
            shape,
            "case {case:?}: expected x-amzn-errortype {shape}, got {:?}",
            refused.header("x-amzn-errortype")
        );
        assert_eq!(
            refused.str_at("Code"),
            code,
            "case {case:?}: expected Code {code}, got {:?}",
            refused.body
        );
    }
}
