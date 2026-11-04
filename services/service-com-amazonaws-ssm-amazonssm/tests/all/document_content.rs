// SPDX-License-Identifier: CC0-1.0

//! `GetDocument` end to end: the content of an SSM document, in the format a read asks
//! for.
//!
//! The rule under test: `GetDocument` reports the content that was PERSISTED for the
//! document the request names — the one a caller created at this endpoint or the one
//! Systems Manager publishes — rendered in the `DocumentFormat` the request asks for
//! ("Returns the document in the specified format … JSON is the default format"), beside
//! that document's own stored metadata; a name no document is held under is
//! `InvalidDocument` and a version the document does not carry is
//! `InvalidDocumentVersion`.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` ([`router`], [`call`], [`call_in_region`]) dispatches `POST /`
//! with `X-Amz-Target: AmazonSSM.<Operation>` through the multi-service router. Nothing
//! reaches into the state struct, so a response that is not computed from persisted state
//! cannot pass: every read follows a write.
//!
//! The suites are tables. Their rows differ along what the rule generalises over — the
//! format the content was persisted in, the format the read asks for, the document type,
//! the alphabet the content is written in, the version selector, the name spelling and
//! the endpoint Region — rather than the single document a recorded unit happened to get.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call, call_in_region, error_code, router, ACCOUNT, REGION};

/// The transient window a created document spends in `Creating`
/// (`crate::state::DOCUMENT_TRANSITION_DELAY`), which the service stores as a due instant
/// rather than sleeping for.
const TRANSITION_DELAY: Duration =
    service_com_amazonaws_ssm_amazonssm::state::DOCUMENT_TRANSITION_DELAY;

/// The catalogue of documents Systems Manager publishes
/// (`crate::state::PUBLISHED_DOCUMENTS`), so the published suite can be checked to cover
/// every entry of it rather than the one the issue named.
const CATALOGUE: &[service_com_amazonaws_ssm_amazonssm::state::PublishedDocument] =
    service_com_amazonaws_ssm_amazonssm::state::PUBLISHED_DOCUMENTS;

/// The `DocumentFormat` values the model's enum declares
/// (`smithy_query(query="shape", shape="DocumentFormat")`).
const DOCUMENT_FORMATS: &[&str] = &["JSON", "TEXT", "YAML"];

/// A `Command` document written in JSON, whose description, parameter default and step
/// carry non-ASCII text, so a round trip that reports the persisted bytes is told apart
/// from one that re-encodes them.
const COMMAND_JSON: &str = r#"{
  "schemaVersion": "2.2",
  "description": "Écho ünïcode — 日本語 🚀",
  "parameters": {
    "Message": { "type": "String", "default": "こんにちは" }
  },
  "mainSteps": [
    {
      "action": "aws:runShellScript",
      "name": "runShell",
      "inputs": { "runCommand": ["echo hi"] }
    }
  ]
}"#;

/// An `Automation` document written in YAML — the same document tree a JSON content
/// declares, in the other format a document tree is written in.
const AUTOMATION_YAML: &str = r#"schemaVersion: '0.3'
description: Remediate a noncompliant bucket
assumeRole: '{{ AutomationAssumeRole }}'
parameters:
  AutomationAssumeRole:
    type: String
  BucketName:
    type: String
mainSteps:
  - name: putBucketLogging
    action: 'aws:executeAwsApi'
    inputs:
      Service: s3
      Api: PutBucketLogging
"#;

/// A `Session` document written in JSON with no parameters at all.
const SESSION_JSON: &str = r#"{
  "schemaVersion": "1.0",
  "description": "Start an interactive shell",
  "sessionType": "Standard_Stream",
  "inputs": { "idleSessionTimeout": "20" }
}"#;

/// A Change Calendar entry: `TEXT` content, which carries no document tree at all.
const CHANGE_CALENDAR_TEXT: &str =
    "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//emulator//test//EN\r\nEND:VCALENDAR";

/// The document tree [`COMMAND_JSON`] declares.
fn command_tree() -> Value {
    json!({
        "schemaVersion": "2.2",
        "description": "Écho ünïcode — 日本語 🚀",
        "parameters": { "Message": { "type": "String", "default": "こんにちは" } },
        "mainSteps": [
            {
                "action": "aws:runShellScript",
                "name": "runShell",
                "inputs": { "runCommand": ["echo hi"] }
            }
        ]
    })
}

/// The document tree [`AUTOMATION_YAML`] declares.
fn automation_tree() -> Value {
    json!({
        "schemaVersion": "0.3",
        "description": "Remediate a noncompliant bucket",
        "assumeRole": "{{ AutomationAssumeRole }}",
        "parameters": {
            "AutomationAssumeRole": { "type": "String" },
            "BucketName": { "type": "String" }
        },
        "mainSteps": [
            {
                "name": "putBucketLogging",
                "action": "aws:executeAwsApi",
                "inputs": { "Service": "s3", "Api": "PutBucketLogging" }
            }
        ]
    })
}

/// The document tree [`SESSION_JSON`] declares.
fn session_tree() -> Value {
    json!({
        "schemaVersion": "1.0",
        "description": "Start an interactive shell",
        "sessionType": "Standard_Stream",
        "inputs": { "idleSessionTimeout": "20" }
    })
}

/// `CreateDocument`, asserting it succeeded.
async fn create_ok(router: &MultiServiceRouter, case: &str, body: &Value) {
    let (status, response) = call(router, "CreateDocument", body).await;
    assert_eq!(
        status, 200,
        "case {case}: CreateDocument must succeed, got {status} {response}"
    );
}

/// Create one document of `document_type` in `format` from `content`.
async fn create_document(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    document_type: &str,
    format: &str,
    content: &str,
) {
    create_ok(
        router,
        case,
        &json!({
            "Name": name,
            "Content": content,
            "DocumentType": document_type,
            "DocumentFormat": format,
        }),
    )
    .await;
}

/// `GetDocument` with `body`, returning `(status, body)`.
async fn get(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "GetDocument", body).await
}

/// `GetDocument` with `body`, asserting the read succeeded.
async fn get_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let (status, response) = get(router, body).await;
    assert_eq!(
        status, 200,
        "case {case}: GetDocument {body} must succeed, got {status} {response}"
    );
    response
}

/// The `Content` a read reported, as a string.
fn content_of(case: &str, response: &Value) -> String {
    response["Content"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: GetDocument reports Content, got {response}"))
        .to_string()
}

/// One persisted document: the format it was created in, and the tree its content
/// declares (`None` for free text, which declares no tree).
struct Stored<'a> {
    case: &'a str,
    name: &'a str,
    document_type: &'a str,
    format: &'a str,
    content: &'a str,
    tree: Option<Value>,
}

/// Every form of persisted content, one row per `DocumentFormat` a document tree can be
/// written in plus the free-text format.
fn stored_rows() -> Vec<Stored<'static>> {
    vec![
        Stored {
            case: "a JSON Command document whose content is not ASCII",
            name: "EchoUnicode",
            document_type: "Command",
            format: "JSON",
            content: COMMAND_JSON,
            tree: Some(command_tree()),
        },
        Stored {
            case: "a YAML Automation document",
            name: "RemediateBucket",
            document_type: "Automation",
            format: "YAML",
            content: AUTOMATION_YAML,
            tree: Some(automation_tree()),
        },
        Stored {
            case: "a JSON Session document with no parameters",
            name: "StartShell",
            document_type: "Session",
            format: "JSON",
            content: SESSION_JSON,
            tree: Some(session_tree()),
        },
        Stored {
            case: "TEXT content, which carries no document tree",
            name: "MyChangeCalendar",
            document_type: "ChangeCalendar",
            format: "TEXT",
            content: CHANGE_CALENDAR_TEXT,
            tree: None,
        },
    ]
}

/// A read that asks for the format a document was persisted in reports the persisted
/// content byte for byte, beside that document's own stored metadata.
#[tokio::test]
async fn content_round_trips_in_the_format_it_was_persisted_in() {
    let router = router();
    let rows = stored_rows();
    for format in DOCUMENT_FORMATS {
        assert!(
            rows.iter().any(|row| row.format == *format),
            "the round-trip table must cover DocumentFormat {format}"
        );
    }

    for row in rows {
        let case = row.case;
        create_ok(
            &router,
            case,
            &json!({
                "Name": row.name,
                "Content": row.content,
                "DocumentType": row.document_type,
                "DocumentFormat": row.format,
                "DisplayName": "display name",
                "VersionName": "1.0",
            }),
        )
        .await;

        let read = get_ok(
            &router,
            case,
            &json!({ "Name": row.name, "DocumentFormat": row.format }),
        )
        .await;
        assert_eq!(
            content_of(case, &read),
            row.content,
            "case {case}: the content persisted must be reported byte for byte"
        );
        for (member, expected) in [
            ("Name", json!(row.name)),
            ("DocumentType", json!(row.document_type)),
            ("DocumentFormat", json!(row.format)),
            ("DocumentVersion", json!("1")),
            ("DisplayName", json!("display name")),
            ("VersionName", json!("1.0")),
        ] {
            assert_eq!(
                read[member], expected,
                "case {case}: expected {member} {expected}, got {read}"
            );
        }
        assert!(
            read["CreatedDate"].is_number(),
            "case {case}: CreatedDate must be the instant the document was created, got {read}"
        );
        assert_eq!(
            read["StatusInformation"],
            Value::Null,
            "case {case}: nothing failed, so there is no Status to explain, got {read}"
        );
        assert_eq!(
            read["ReviewStatus"],
            Value::Null,
            "case {case}: only a change template carries a review, got {read}"
        );
    }
}

/// A read that asks for no format gets the documented default — JSON — so a document
/// persisted as JSON is reported verbatim and one persisted as YAML is reported as the
/// SAME document tree written in JSON. Free text carries no tree, so it is reported as
/// persisted.
#[tokio::test]
async fn a_read_that_names_no_format_gets_the_default_format() {
    let router = router();
    for row in stored_rows() {
        let case = row.case;
        create_document(
            &router,
            case,
            row.name,
            row.document_type,
            row.format,
            row.content,
        )
        .await;

        let read = get_ok(&router, case, &json!({ "Name": row.name })).await;
        let content = content_of(case, &read);
        match row.tree {
            Some(tree) => {
                assert_eq!(
                    read["DocumentFormat"],
                    json!("JSON"),
                    "case {case}: a read naming no format reports the default format, got {read}"
                );
                let parsed: Value = serde_json::from_str(&content).unwrap_or_else(|why| {
                    panic!("case {case}: expected JSON content, got {content:?} ({why})")
                });
                assert_eq!(
                    parsed, tree,
                    "case {case}: expected the persisted document tree, got {parsed}"
                );
            }
            None => {
                assert_eq!(
                    content, row.content,
                    "case {case}: free text has no tree to re-render, so it is reported as persisted"
                );
                assert_eq!(
                    read["DocumentFormat"],
                    json!(row.format),
                    "case {case}: the format reported is the format of the content returned, got {read}"
                );
            }
        }
    }
}

/// A read that asks for the OTHER format a document tree can be written in gets the same
/// tree rendered in that format: the JSON of a YAML document and the YAML of a JSON one.
///
/// The YAML is checked by feeding it back through the service: a second document created
/// from it is read as JSON and must declare the tree the first document declared. So a
/// read that only echoed the persisted bytes, or that emitted something no document
/// parser reads, cannot pass.
#[tokio::test]
async fn a_read_renders_the_persisted_tree_in_the_format_it_asks_for() {
    let router = router();

    // YAML persisted, JSON asked for.
    let case = "a YAML Automation document read as JSON";
    create_document(
        &router,
        case,
        "RemediateBucket",
        "Automation",
        "YAML",
        AUTOMATION_YAML,
    )
    .await;
    let read = get_ok(
        &router,
        case,
        &json!({ "Name": "RemediateBucket", "DocumentFormat": "JSON" }),
    )
    .await;
    assert_eq!(
        read["DocumentFormat"],
        json!("JSON"),
        "case {case}: expected DocumentFormat JSON, got {read}"
    );
    let rendered = content_of(case, &read);
    let parsed: Value = serde_json::from_str(&rendered)
        .unwrap_or_else(|why| panic!("case {case}: expected JSON content, got {rendered:?} ({why})"));
    assert_eq!(
        parsed,
        automation_tree(),
        "case {case}: expected the tree the YAML declared, got {parsed}"
    );

    // JSON persisted, YAML asked for.
    let case = "a JSON Command document read as YAML";
    create_document(&router, case, "EchoUnicode", "Command", "JSON", COMMAND_JSON).await;
    let read = get_ok(
        &router,
        case,
        &json!({ "Name": "EchoUnicode", "DocumentFormat": "YAML" }),
    )
    .await;
    assert_eq!(
        read["DocumentFormat"],
        json!("YAML"),
        "case {case}: expected DocumentFormat YAML, got {read}"
    );
    let as_yaml = content_of(case, &read);
    assert_ne!(
        as_yaml, COMMAND_JSON,
        "case {case}: the content must be re-rendered, not echoed"
    );
    assert!(
        serde_json::from_str::<Value>(&as_yaml).is_err(),
        "case {case}: expected YAML rather than the persisted JSON, got {as_yaml:?}"
    );
    assert!(
        as_yaml.contains("日本語"),
        "case {case}: the content's own text must survive the rendering, got {as_yaml:?}"
    );
    // The YAML is a document the service itself reads back: create a second document from
    // it and the tree it declares must be the tree the first document declared.
    create_document(
        &router,
        case,
        "EchoUnicodeFromYaml",
        "Command",
        "YAML",
        &as_yaml,
    )
    .await;
    let reread = get_ok(
        &router,
        case,
        &json!({ "Name": "EchoUnicodeFromYaml", "DocumentFormat": "JSON" }),
    )
    .await;
    let rendered = content_of(case, &reread);
    let parsed: Value = serde_json::from_str(&rendered)
        .unwrap_or_else(|why| panic!("case {case}: expected JSON content, got {rendered:?} ({why})"));
    assert_eq!(
        parsed,
        command_tree(),
        "case {case}: the tree must survive JSON → YAML → JSON, got {parsed}"
    );

    // Free text, asked for in each format a tree is written in.
    let case = "TEXT content asked for in another format";
    create_document(
        &router,
        case,
        "MyChangeCalendar",
        "ChangeCalendar",
        "TEXT",
        CHANGE_CALENDAR_TEXT,
    )
    .await;
    for format in ["JSON", "YAML", "TEXT"] {
        let read = get_ok(
            &router,
            case,
            &json!({ "Name": "MyChangeCalendar", "DocumentFormat": format }),
        )
        .await;
        assert_eq!(
            content_of(case, &read),
            CHANGE_CALENDAR_TEXT,
            "case {case} ({format}): free text has no tree to re-render"
        );
        assert_eq!(
            read["DocumentFormat"],
            json!("TEXT"),
            "case {case} ({format}): the format reported is the format of the content returned, got {read}"
        );
    }
}

/// Every selector that names the version a document holds reports that version, and every
/// selector that names one it does not carry is refused.
#[tokio::test]
async fn a_version_selector_resolves_to_the_version_the_document_holds() {
    let router = router();
    let case = "a document holding its first version";
    create_ok(
        &router,
        case,
        &json!({
            "Name": "EchoUnicode",
            "Content": COMMAND_JSON,
            "DocumentType": "Command",
            "VersionName": "1.0",
        }),
    )
    .await;

    // The selectors the modelled `DocumentVersion` pattern admits: the two role
    // selectors and a positive integer (`^([$]LATEST|[$]DEFAULT|^[1-9][0-9]*$)$`).
    for selector in ["$LATEST", "$DEFAULT", "1"] {
        let read = get_ok(
            &router,
            case,
            &json!({ "Name": "EchoUnicode", "DocumentVersion": selector }),
        )
        .await;
        assert_eq!(
            read["DocumentVersion"],
            json!("1"),
            "case {case}: {selector} names the version the document holds, got {read}"
        );
        assert_eq!(
            content_of(case, &read),
            COMMAND_JSON,
            "case {case}: {selector} must report the persisted content"
        );
    }
    // A version name is "unique across all versions of a document", so the one the
    // create supplied names this version and any other names none.
    let read = get_ok(
        &router,
        case,
        &json!({ "Name": "EchoUnicode", "VersionName": "1.0" }),
    )
    .await;
    assert_eq!(
        read["VersionName"],
        json!("1.0"),
        "case {case}: expected the persisted VersionName, got {read}"
    );

    for (refused, expected_code, expected_message) in [
        (
            json!({ "Name": "EchoUnicode", "DocumentVersion": "2" }),
            "InvalidDocumentVersion",
            "Document version: 2 is invalid, latest version is 1",
        ),
        (
            json!({ "Name": "EchoUnicode", "DocumentVersion": "7" }),
            "InvalidDocumentVersion",
            "Document version: 7 is invalid, latest version is 1",
        ),
        (
            json!({ "Name": "EchoUnicode", "DocumentVersion": "42" }),
            "InvalidDocumentVersion",
            "Document version: 42 is invalid, latest version is 1",
        ),
        // A version NAME no version carries names no version of the document, and the
        // refusal names the version name it was asked for.
        (
            json!({ "Name": "EchoUnicode", "VersionName": "2.0" }),
            "InvalidDocumentVersion",
            "Document version name: 2.0 is invalid",
        ),
        (
            json!({ "Name": "EchoUnicode", "DocumentVersion": "1", "VersionName": "9.9" }),
            "InvalidDocumentVersion",
            "Document version name: 9.9 is invalid",
        ),
    ] {
        let (status, response) = get(&router, &refused).await;
        assert_eq!(
            status, 400,
            "case {case}: {refused} names no version of the document, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            expected_code,
            "case {case}: expected {refused} to be refused as {expected_code}, got {response}"
        );
        assert_eq!(
            response["Message"],
            json!(expected_message),
            "case {case}: the refusal must name what was asked for, got {response}"
        );
    }
}

/// The name a read resolves: a bare name, the document's ARN, and — for a document no
/// endpoint of this account holds under that name — the not-found refusal.
#[tokio::test]
async fn a_name_no_document_is_held_under_is_reported_as_no_document() {
    let router = router();
    let case = "a document created at this endpoint";
    create_document(
        &router,
        case,
        "EchoUnicode",
        "Command",
        "JSON",
        COMMAND_JSON,
    )
    .await;

    // "The name of the SSM document", which may be spelled as the document's ARN.
    for spelling in [
        "EchoUnicode".to_string(),
        format!("arn:aws:ssm:{REGION}:{ACCOUNT}:document/EchoUnicode"),
    ] {
        let read = get_ok(&router, case, &json!({ "Name": spelling })).await;
        assert_eq!(
            read["Name"],
            json!("EchoUnicode"),
            "case {case}: {spelling} names the created document, got {read}"
        );
        assert_eq!(
            content_of(case, &read),
            COMMAND_JSON,
            "case {case}: {spelling} must report the persisted content"
        );
    }

    let absent = [
        ("a name nothing was created under", "NoSuchDocument"),
        (
            "an ARN naming another account",
            "arn:aws:ssm:us-west-2:111122223333:document/EchoUnicode",
        ),
        (
            "an ARN naming another Region",
            "arn:aws:ssm:eu-west-1:000000000000:document/EchoUnicode",
        ),
        ("an ARN of another service", "arn:aws:s3:::my-bucket"),
    ];
    for (why, name) in absent {
        let (status, response) = get(&router, &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {why}: {name} names no document, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidDocument",
            "case {why}: {name} must be refused as no document, got {response}"
        );
        assert_eq!(
            response["Message"],
            json!(format!("Document with name {name} does not exist.")),
            "case {why}: the refusal must name the document asked for, got {response}"
        );
    }

    // A document is reachable only through the endpoint it was created at.
    let (status, response) =
        call_in_region(&router, "eu-west-1", "GetDocument", &json!({ "Name": "EchoUnicode" })).await;
    assert_eq!(
        status, 400,
        "case another Region's endpoint: the document was created in {REGION}, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "InvalidDocument",
        "case another Region's endpoint: expected InvalidDocument, got {response}"
    );
}

/// Every document Systems Manager publishes is readable in every account and Region
/// without anybody creating it, and its content is the catalogue's own body.
#[tokio::test]
async fn a_published_document_reports_the_published_content() {
    let router = router();
    assert!(
        CATALOGUE.len() > 1,
        "the published catalogue must hold more than the one document the issue named"
    );
    for published in CATALOGUE {
        let case = published.name;
        let read = get_ok(&router, case, &json!({ "Name": published.name })).await;
        assert_eq!(
            content_of(case, &read),
            published.content,
            "case {case}: a published document reports the published body"
        );
        for (member, expected) in [
            ("Name", json!(published.name)),
            ("DocumentType", json!(published.document_type)),
            ("DocumentFormat", json!("JSON")),
            ("DocumentVersion", json!("1")),
            // A published document is already registered, so it is never `Creating`.
            ("Status", json!("Active")),
        ] {
            assert_eq!(
                read[member], expected,
                "case {case}: expected {member} {expected}, got {read}"
            );
        }
    }
    // The same document resolves through another Region's endpoint, and nothing had to be
    // created there for it.
    let published = &CATALOGUE[0];
    let (status, response) = call_in_region(
        &router,
        "eu-west-1",
        "GetDocument",
        &json!({ "Name": published.name }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {}: a published document resolves at every endpoint, got {status} {response}",
        published.name
    );
    assert_eq!(
        content_of(published.name, &response),
        published.content,
        "case {}: the published body is the same at every endpoint",
        published.name
    );
}

/// A read reports the status the clock owes and changes nothing else: two reads of one
/// document agree, the document is still there afterwards, and a read of a name nothing
/// holds does not bring a document into being.
#[tokio::test]
async fn a_read_reports_the_owed_status_and_changes_nothing_else() {
    let router = router();
    let case = "a document read straight after it was created";
    create_document(
        &router,
        case,
        "EchoUnicode",
        "Command",
        "JSON",
        COMMAND_JSON,
    )
    .await;

    let read = get_ok(&router, case, &json!({ "Name": "EchoUnicode" })).await;
    assert_eq!(
        read["Status"],
        json!("Creating"),
        "case {case}: a document Systems Manager is still registering is Creating, got {read}"
    );
    let again = get_ok(&router, case, &json!({ "Name": "EchoUnicode" })).await;
    assert_eq!(
        again, read,
        "case {case}: two reads inside the transient window must agree, got {again}"
    );

    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(50)).await;
    let settled = get_ok(&router, case, &json!({ "Name": "EchoUnicode" })).await;
    assert_eq!(
        settled["Status"],
        json!("Active"),
        "case {case}: the move to Active was due, got {settled}"
    );
    assert_eq!(
        content_of(case, &settled),
        COMMAND_JSON,
        "case {case}: the content is unchanged by the status move"
    );

    // A read of a name nothing holds leaves the store as it was: the name is still free
    // for a create, and the document that IS held is still reported.
    let case = "a read of a name nothing holds";
    let (status, _) = get(&router, &json!({ "Name": "NoSuchDocument" })).await;
    assert_eq!(status, 400, "case {case}: expected the not-found refusal");
    create_document(
        &router,
        case,
        "NoSuchDocument",
        "Session",
        "JSON",
        SESSION_JSON,
    )
    .await;
    let (status, described) = call(
        &router,
        "DescribeDocument",
        &json!({ "Name": "EchoUnicode" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: the document created earlier is still held, got {status} {described}"
    );
    assert_eq!(
        described["Document"]["Name"],
        json!("EchoUnicode"),
        "case {case}: expected the created document, got {described}"
    );
}
