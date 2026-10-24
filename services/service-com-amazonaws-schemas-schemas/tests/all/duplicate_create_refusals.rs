// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ENVELOPE a duplicate create is refused with.
//!
//! RULE: a create whose name is already taken is refused with the error shape that
//! operation's OWN modelled error set declares — a conflict shape (409) where the
//! model binds one to the operation, and otherwise the client-refusal shape that set
//! does carry (`BadRequestException`, 400) — always under the already-exists condition
//! code and the service's sentence built from the name the request sent, leaving the
//! stored resource exactly as it was.
//!
//! The two creates this service implements sit on either side of that rule, which is
//! why both are rows here rather than only the one an issue cited.
//!
//! Every case drives the real wire path, through the crate's one harness in
//! `registry_schema_lifecycle.rs`; nothing here touches the state struct directly.

use serde_json::{json, Value};
use service_com_amazonaws_schemas_schemas::state::{
    CODE_MALFORMED_REQUEST, CODE_RESOURCE_ALREADY_EXIST,
};

use crate::registry_schema_lifecycle::{registry_uri, schema_uri, Answer, Emulator};

/// The registry the schema rows create their schemas in.
const HOST_REGISTRY: &str = "duplicate-create-host";

/// Which create a row is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Create {
    Registry,
    Schema,
}

/// One create operation and the refusal its own modelled error set gives a name that
/// is already taken.
///
/// Source for `status`/`shape`: the operation's modelled `errors` list —
/// `smithy_query(query="operation", operation="CreateRegistry")` declares
/// `ConflictException` (httpError 409), while
/// `smithy_query(query="operation", operation="CreateSchema")` declares
/// `BadRequestException` (400), `ForbiddenException` (403),
/// `InternalServerErrorException` (500) and `ServiceUnavailableException` (503) — and
/// no conflict shape at all.
#[derive(Debug)]
struct Row {
    create: Create,
    status: u16,
    shape: &'static str,
    /// The word the service's already-exists sentence names the resource kind with.
    resource: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            create: Create::Registry,
            status: 409,
            shape: "ConflictException",
            resource: "Registry",
        },
        Row {
            create: Create::Schema,
            status: 400,
            shape: "BadRequestException",
            resource: "Schema",
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

/// The URI the named create posts to.
fn create_uri(create: Create, name: &str) -> String {
    match create {
        Create::Registry => registry_uri(name),
        Create::Schema => schema_uri(HOST_REGISTRY, name),
    }
}

/// The body the FIRST (succeeding) create sends.
fn first_body(create: Create) -> Value {
    match create {
        Create::Registry => json!({"Description": "the original"}),
        Create::Schema => schema_body("original"),
    }
}

/// Bodies a SECOND create under the same name sends: the rule is about the NAME, so
/// it holds however the rest of the request differs from the stored resource — same
/// body, a different description, different tags, different content, and (for a
/// schema) a different `Type` with a document of that type.
fn second_bodies(create: Create) -> Vec<(&'static str, Value)> {
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
    }
}

/// What a read of the resource must still report after a refused duplicate: the
/// member every row can read back through the implemented Describe, and its value.
async fn describe(emulator: &Emulator, create: Create, name: &str) -> Answer {
    let described = emulator.call("GET", &create_uri(create, name), None).await;
    assert_eq!(
        described.status, 200,
        "case {create:?}/{name:?}: the stored resource must survive a refused duplicate, \
         got {} {:?}",
        described.status, described.body
    );
    described
}

/// Assert one refusal's whole envelope: status, the shape in the header and in the
/// restJson1 body, the condition code and the sentence.
fn assert_refusal(case: &str, row: &Row, name: &str, answer: &Answer) {
    assert_eq!(
        answer.status, row.status,
        "case {case:?}: expected HTTP {}, got {} {:?}",
        row.status, answer.status, answer.body
    );
    assert_eq!(
        answer.error_type(),
        row.shape,
        "case {case:?}: expected x-amzn-errortype {}, got {:?}",
        row.shape,
        answer.header("x-amzn-errortype")
    );
    assert_eq!(
        answer.error_code(),
        row.shape,
        "case {case:?}: expected the restJson1 __type {}, got {:?}",
        row.shape,
        answer.body
    );
    let code = answer.str_at("Code");
    assert_eq!(
        code, CODE_RESOURCE_ALREADY_EXIST,
        "case {case:?}: a duplicate create carries one condition code whatever the \
         shape, expected {CODE_RESOURCE_ALREADY_EXIST}, got {code:?}"
    );
    let message = answer.str_at("Message");
    assert_eq!(
        message,
        format!("{} with name {name} already exists.", row.resource),
        "case {case:?}: wrong already-exists sentence"
    );
}

/// RULE (above): the class of a duplicate create's refusal is the one its own
/// operation models — 409 for the create that declares a conflict shape, 400 for the
/// create that does not — under one already-exists condition code, and the stored
/// resource is untouched.
#[tokio::test]
async fn a_duplicate_create_is_refused_in_the_class_its_own_operation_models() {
    // Names differing in shape, length and alphabet: the rule generalises over the
    // name the request sent, not over any one name.
    let names: Vec<String> = vec![
        "dup".to_string(),
        "order.placed_v1".to_string(),
        "UPPER-and-lower-99".to_string(),
        "n".repeat(64),
    ];

    for row in rows() {
        for name in &names {
            for (variant, body) in second_bodies(row.create) {
                let case = format!("{:?} {name:?} recreated with {variant}", row.create);
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
                        &create_uri(row.create, name),
                        Some(first_body(row.create)),
                    )
                    .await;
                assert_eq!(
                    created.status, 201,
                    "case {case:?}: the first create must succeed, got {} {:?}",
                    created.status, created.body
                );
                let before = describe(&emulator, row.create, name).await;

                // Repeating the duplicate must not drift: the same refusal every
                // time, and never a different class on the second attempt.
                for attempt in 1..=2 {
                    let refused = emulator
                        .call("POST", &create_uri(row.create, name), Some(body.clone()))
                        .await;
                    assert_refusal(&format!("{case} (attempt {attempt})"), &row, name, &refused);
                }

                // Round trip: the read-back is byte-for-byte what it was before the
                // refused creates, so nothing the duplicate carried reached the store.
                let after = describe(&emulator, row.create, name).await;
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

/// RULE: `Code` carries the CONDITION, so the two conditions `CreateSchema` answers
/// 400 for — a name already taken and a body that is not a schema document — are
/// still told apart by it even though they share the shape and the status.
#[tokio::test]
async fn the_two_four_hundred_conditions_of_create_schema_differ_in_their_code() {
    let emulator = Emulator::new();
    emulator
        .call("POST", &registry_uri(HOST_REGISTRY), Some(json!({})))
        .await;
    let schema = "both-four-hundreds";
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

    // Rows: the condition, the body that provokes it, and the code it must carry.
    // The malformed row is sent under the SAME (taken) name, which is what shows the
    // request itself is judged before the store's contents are: `Content` and `Type`
    // are required members of this operation, so a body that is not a schema document
    // is refused as malformed whether or not the name is free.
    for (case, body, code) in [
        (
            "a name the registry already holds",
            schema_body("impostor"),
            CODE_RESOURCE_ALREADY_EXIST,
        ),
        (
            "content that is not a schema document",
            json!({"Content": "not json at all", "Type": "OpenApi3"}),
            CODE_MALFORMED_REQUEST,
        ),
        (
            "content that is JSON but not an object",
            json!({"Content": "[1,2,3]", "Type": "OpenApi3"}),
            CODE_MALFORMED_REQUEST,
        ),
    ] {
        let refused = emulator
            .call("POST", &schema_uri(HOST_REGISTRY, schema), Some(body))
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type(),
            "BadRequestException",
            "case {case:?}: expected x-amzn-errortype BadRequestException, got {:?}",
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
