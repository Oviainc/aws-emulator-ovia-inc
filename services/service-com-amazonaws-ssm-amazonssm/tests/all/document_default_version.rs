// SPDX-License-Identifier: CC0-1.0

//! `UpdateDocumentDefaultVersion` end to end: the default version a read resolves to
//! moves to the version the call names, and only to a version the document holds.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` ([`router`], [`call`], [`call_in_region`], [`error_code`])
//! dispatches `POST /` with `X-Amz-Target: AmazonSSM.<Operation>` through the
//! multi-service router. Nothing reaches into the state struct, so what the call
//! persisted is read back only through `DescribeDocument` and `GetDocument`.
//!
//! The suites are tables. Each row is one input along the axis the rule under test
//! generalises over — document type, document format, how many versions the document
//! holds, which of them the call names (the first, a middle one, the latest), whether
//! that version carries a `VersionName`, how often the default moves, and which Region
//! the document was created in — rather than the single document a recorded unit
//! happened to switch.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call, call_in_region, error_code, error_message, router, REGION};

/// A JSON `Command` document, parameterised by the description its content declares, so
/// two versions of one document differ in content without differing in anything else.
fn command_json(description: &str) -> String {
    format!(
        r#"{{
  "schemaVersion": "2.2",
  "description": "{description}",
  "mainSteps": [
    {{
      "action": "aws:runShellScript",
      "name": "runShell",
      "inputs": {{ "runCommand": ["echo hi"] }}
    }}
  ]
}}"#
    )
}

/// A YAML `Automation` document, parameterised the same way.
fn automation_yaml(description: &str) -> String {
    format!(
        r#"schemaVersion: '0.3'
description: {description}
assumeRole: '{{{{ AutomationAssumeRole }}}}'
parameters:
  AutomationAssumeRole:
    type: String
mainSteps:
  - name: putBucketLogging
    action: 'aws:executeAwsApi'
    inputs:
      Service: s3
      Api: PutBucketLogging
"#
    )
}

/// A Change Calendar entry: `TEXT` content, which carries no document tree at all.
fn calendar_text(summary: &str) -> String {
    format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nSUMMARY:{summary}\r\nEND:VCALENDAR")
}

/// `CreateDocument`, asserting it succeeded.
async fn create_ok(router: &MultiServiceRouter, case: &str, body: &Value) {
    let (status, response) = call(router, "CreateDocument", body).await;
    assert_eq!(
        status, 200,
        "case {case}: CreateDocument must succeed, got {status} {response}"
    );
}

/// `UpdateDocument`, asserting it succeeded.
async fn update_ok(router: &MultiServiceRouter, case: &str, body: &Value) {
    let (status, response) = call(router, "UpdateDocument", body).await;
    assert_eq!(
        status, 200,
        "case {case}: UpdateDocument must succeed, got {status} {response}"
    );
}

/// `UpdateDocumentDefaultVersion` with `(name, document_version)`, returning
/// `(status, body)`.
async fn switch(router: &MultiServiceRouter, name: &str, document_version: &str) -> (u16, Value) {
    call(
        router,
        "UpdateDocumentDefaultVersion",
        &json!({ "Name": name, "DocumentVersion": document_version }),
    )
    .await
}

/// `UpdateDocumentDefaultVersion`, asserting it succeeded, and returning its
/// `Description`.
async fn switch_ok(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    document_version: &str,
) -> Value {
    let (status, response) = switch(router, name, document_version).await;
    assert_eq!(
        status, 200,
        "case {case}: UpdateDocumentDefaultVersion {name} {document_version} must succeed, \
         got {status} {response}"
    );
    response["Description"].clone()
}

/// `DescribeDocument`'s `Document` for one version selector, asserting the read
/// succeeded.
async fn describe_ok(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    document_version: Option<&str>,
) -> Value {
    let mut body = json!({ "Name": name });
    if let Some(version) = document_version {
        body["DocumentVersion"] = json!(version);
    }
    let (status, response) = call(router, "DescribeDocument", &body).await;
    assert_eq!(
        status, 200,
        "case {case}: DescribeDocument {name} {document_version:?} must succeed, \
         got {status} {response}"
    );
    response["Document"].clone()
}

/// `GetDocument` for one version selector, in the format the content was written in, so
/// a round-trip compares the content the create or the update supplied.
async fn get_ok(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    document_version: Option<&str>,
    document_format: Option<&str>,
) -> Value {
    let mut body = json!({ "Name": name });
    if let Some(version) = document_version {
        body["DocumentVersion"] = json!(version);
    }
    if let Some(format) = document_format {
        body["DocumentFormat"] = json!(format);
    }
    let (status, response) = call(router, "GetDocument", &body).await;
    assert_eq!(
        status, 200,
        "case {case}: GetDocument {name} {document_version:?} must succeed, \
         got {status} {response}"
    );
    response
}

/// One switch row: a document, the contents of the versions it holds, and which of those
/// versions the call makes the default.
struct Switched<'a> {
    case: &'a str,
    name: &'a str,
    /// `None` leaves `DocumentType` to the service's default.
    document_type: Option<&'a str>,
    /// `None` leaves `DocumentFormat` to the documented `JSON` default.
    format: Option<&'a str>,
    /// One content per version, oldest first; the first is the create's.
    contents: Vec<String>,
    /// The `VersionName` each version carries, `None` for a version created without one.
    version_names: Vec<Option<&'a str>>,
    /// The version number the call makes the default, 1-based.
    switch_to: usize,
}

/// Create the row's document and append a version per further content.
async fn seed(router: &MultiServiceRouter, row: &Switched<'_>) {
    let mut body = json!({ "Name": row.name, "Content": row.contents[0] });
    if let Some(document_type) = row.document_type {
        body["DocumentType"] = json!(document_type);
    }
    if let Some(format) = row.format {
        body["DocumentFormat"] = json!(format);
    }
    if let Some(version_name) = row.version_names[0] {
        body["VersionName"] = json!(version_name);
    }
    create_ok(router, row.case, &body).await;

    for (index, content) in row.contents.iter().enumerate().skip(1) {
        let mut body = json!({ "Name": row.name, "Content": content });
        if let Some(format) = row.format {
            body["DocumentFormat"] = json!(format);
        }
        if let Some(version_name) = row.version_names[index] {
            body["VersionName"] = json!(version_name);
        }
        update_ok(router, row.case, &body).await;
    }
}

/// The call moves the document's default version to the version it names: the report it
/// answers with, every later read that names no version, and the `$DEFAULT` selector all
/// resolve that version, while the document's latest version does not move.
#[tokio::test]
async fn the_default_version_moves_to_the_version_the_call_names() {
    let router = router();
    let rows = vec![
        Switched {
            case: "a JSON Command document switched to its latest version",
            name: "SwitchToLatestDocument",
            document_type: None,
            format: None,
            contents: vec![command_json("one"), command_json("two")],
            version_names: vec![None, None],
            switch_to: 2,
        },
        Switched {
            case: "a four-version document switched to a middle version",
            name: "SwitchToMiddleDocument",
            document_type: Some("Command"),
            format: Some("JSON"),
            contents: vec![
                command_json("first"),
                command_json("second"),
                command_json("third"),
                command_json("fourth"),
            ],
            version_names: vec![None, None, None, None],
            switch_to: 3,
        },
        Switched {
            case: "a document switched to the version it already defaults to",
            name: "SwitchToFirstDocument",
            document_type: Some("Command"),
            format: Some("JSON"),
            contents: vec![command_json("only one"), command_json("and two")],
            version_names: vec![None, None],
            switch_to: 1,
        },
        Switched {
            case: "a YAML Automation document whose versions carry version names",
            name: "SwitchAutomationDocument",
            document_type: Some("Automation"),
            format: Some("YAML"),
            contents: vec![
                automation_yaml("remediate one"),
                automation_yaml("remediate two"),
                automation_yaml("remediate three"),
            ],
            version_names: vec![Some("Release1.0"), Some("Release2.0"), Some("Release3.0")],
            switch_to: 2,
        },
        Switched {
            case: "TEXT content, which carries no document tree",
            name: "SwitchCalendarDocument",
            document_type: Some("ChangeCalendar"),
            format: Some("TEXT"),
            contents: vec![calendar_text("one"), calendar_text("two")],
            version_names: vec![None, Some("Window.2")],
            switch_to: 2,
        },
    ];

    for row in rows {
        let case = row.case;
        seed(&router, &row).await;
        let latest = row.contents.len().to_string();
        let target = row.switch_to.to_string();
        let target_name = row.version_names[row.switch_to - 1];

        // The report is of the document that moved: its name, the version that is now the
        // default and that version's own `VersionName`.
        let description = switch_ok(&router, case, row.name, &target).await;
        assert_eq!(
            description["Name"],
            json!(row.name),
            "case {case}: the report names the document that moved, got {description}"
        );
        assert_eq!(
            description["DefaultVersion"],
            json!(target),
            "case {case}: expected DefaultVersion {target}, got {description}"
        );
        assert_eq!(
            description["DefaultVersionName"],
            match target_name {
                Some(version_name) => json!(version_name),
                None => Value::Null,
            },
            "case {case}: expected DefaultVersionName {target_name:?}, got {description}"
        );

        // A read that names no version is served the new default version, and so is the
        // `$DEFAULT` selector; the latest version has not moved with it.
        for selector in [None, Some("$DEFAULT")] {
            let described = describe_ok(&router, case, row.name, selector).await;
            assert_eq!(
                described["DocumentVersion"],
                json!(target),
                "case {case}: {selector:?} resolves the new default version {target}, \
                 got {described}"
            );
            assert_eq!(
                described["DefaultVersion"],
                json!(target),
                "case {case}: {selector:?} reports DefaultVersion {target}, got {described}"
            );
            assert_eq!(
                described["LatestVersion"],
                json!(latest),
                "case {case}: the latest version stays {latest}, got {described}"
            );
        }
        let fetched = get_ok(&router, case, row.name, None, row.format).await;
        assert_eq!(
            fetched["Content"],
            json!(row.contents[row.switch_to - 1]),
            "case {case}: a read that names no version serves the new default version's \
             content, got {fetched}"
        );
        assert_eq!(
            fetched["DocumentVersion"],
            json!(target),
            "case {case}: a read that names no version serves version {target}, got {fetched}"
        );

        // Every version is still readable by number, and each reports the new default.
        for (index, content) in row.contents.iter().enumerate() {
            let version = (index + 1).to_string();
            let described = describe_ok(&router, case, row.name, Some(&version)).await;
            assert_eq!(
                described["DefaultVersion"],
                json!(target),
                "case {case}: version {version} reports DefaultVersion {target}, \
                 got {described}"
            );
            let fetched = get_ok(&router, case, row.name, Some(&version), row.format).await;
            assert_eq!(
                fetched["Content"],
                json!(content),
                "case {case}: version {version} still serves its own content, got {fetched}"
            );
        }
    }
}

/// The default version is whatever the LAST switch named: it moves forwards and
/// backwards, and a switch that names the version already in force changes nothing.
#[tokio::test]
async fn the_default_version_is_the_one_the_last_call_named() {
    let router = router();
    let name = "MovingDefaultDocument";
    let contents: Vec<String> = (1..=4)
        .map(|version| command_json(&format!("version {version}")))
        .collect();
    create_ok(
        &router,
        "the first version",
        &json!({ "Name": name, "Content": contents[0] }),
    )
    .await;
    for content in contents.iter().skip(1) {
        update_ok(
            &router,
            "an appended version",
            &json!({ "Name": name, "Content": content }),
        )
        .await;
    }

    // Each row moves the default to the version it names: up, down, past the one in
    // force, and onto the one already in force.
    for target in ["4", "2", "3", "3", "1", "4"] {
        let case = format!("the default moved to {target}");
        let description = switch_ok(&router, &case, name, target).await;
        assert_eq!(
            description["DefaultVersion"],
            json!(target),
            "case {case}: expected DefaultVersion {target}, got {description}"
        );
        let described = describe_ok(&router, &case, name, None).await;
        assert_eq!(
            described["DocumentVersion"],
            json!(target),
            "case {case}: a read that names no version resolves {target}, got {described}"
        );
        let index: usize = target.parse().expect("the target names a version number");
        let fetched = get_ok(&router, &case, name, None, Some("JSON")).await;
        assert_eq!(
            fetched["Content"],
            json!(contents[index - 1]),
            "case {case}: the default version's content is served, got {fetched}"
        );
        assert_eq!(
            described["LatestVersion"],
            json!("4"),
            "case {case}: the latest version never moves with the default, got {described}"
        );
    }
}

/// A number the document holds no version of names none to make the default: the call is
/// refused with `InvalidDocumentVersion` and the default stays where it was.
#[tokio::test]
async fn a_version_the_document_has_not_got_is_refused() {
    let router = router();
    let name = "RefusedVersionDocument";
    create_ok(
        &router,
        "the first version",
        &json!({ "Name": name, "Content": command_json("one") }),
    )
    .await;
    update_ok(
        &router,
        "the second version",
        &json!({ "Name": name, "Content": command_json("two") }),
    )
    .await;
    switch_ok(&router, "the default in force", name, "2").await;

    // Each row is a number this document holds no version of: one past the latest, far
    // past it, and one too large to count any version at all.
    for (case, requested) in [
        ("the version one past the latest", "3"),
        ("a version far past the latest", "97"),
        (
            "a number no version could count",
            "99999999999999999999999999",
        ),
    ] {
        let (status, response) = switch(&router, name, requested).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "InvalidDocumentVersion"),
            "case {case}: {requested} names no version, got {status} {response}"
        );
        // The refusal names what was asked for and the version the document holds, around
        // text that is otherwise fixed.
        assert_eq!(
            error_message(&response),
            format!("Document version: {requested} is invalid, latest version is 2"),
            "case {case}: got {response}"
        );
        let described = describe_ok(&router, case, name, None).await;
        assert_eq!(
            described["DefaultVersion"],
            json!("2"),
            "case {case}: a refused call moves no default, got {described}"
        );
    }
}

/// The version a switch names is a `DocumentVersionNumber` — "^(^[1-9][0-9]*$)$", which
/// admits neither of the two role selectors a read takes nor a number with a leading
/// zero — so a value outside that pattern is refused at the boundary and moves nothing.
#[tokio::test]
async fn a_value_outside_the_version_number_pattern_is_refused() {
    let router = router();
    let name = "VersionPatternDocument";
    create_ok(
        &router,
        "the first version",
        &json!({ "Name": name, "Content": command_json("one") }),
    )
    .await;
    update_ok(
        &router,
        "the second version",
        &json!({ "Name": name, "Content": command_json("two") }),
    )
    .await;

    for requested in ["$LATEST", "$DEFAULT", "0", "01", "-1", "1.0", "two", ""] {
        let (status, response) = switch(&router, name, requested).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case {requested:?}: isn't a document version number, got {status} {response}"
        );
        let described = describe_ok(&router, requested, name, None).await;
        assert_eq!(
            described["DefaultVersion"],
            json!("1"),
            "case {requested:?}: a refused call moves no default, got {described}"
        );
    }
}

/// The default version of a CUSTOM document is the account's to move: a name nothing
/// created here names no custom document, whether nothing holds it at all, the catalogue
/// Systems Manager publishes holds it, or another Region's endpoint holds it.
#[tokio::test]
async fn a_document_the_account_did_not_create_has_no_default_version_to_move() {
    let router = router();
    // A document created in the harness's Region, to be asked for through another one.
    let elsewhere = "OtherRegionDefaultDocument";
    create_ok(
        &router,
        "a document of this Region",
        &json!({ "Name": elsewhere, "Content": command_json("one") }),
    )
    .await;

    for (case, name) in [
        ("a document nothing created", "NoSuchDocumentAtAll"),
        // A document the catalogue publishes, which resolves for a READ without any
        // create (`crate::state::PUBLISHED_DOCUMENTS`) but is owned by Amazon.
        ("a document Systems Manager publishes", "AWS-RunShellScript"),
        ("a document Systems Manager publishes", "AWS-UpdateSSMAgent"),
    ] {
        let (status, response) = switch(&router, name, "1").await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "InvalidDocument"),
            "case {case}: {name} has no custom default version to move, got {status} {response}"
        );
        assert_eq!(
            error_message(&response),
            format!("Document with name {name} does not exist."),
            "case {case}: got {response}"
        );
    }

    // A document is reached through the endpoint it was created at: another Region's
    // endpoint holds none of that name, and the Region that does is untouched.
    for region in ["us-east-1", "eu-west-1"] {
        let (status, response) = call_in_region(
            &router,
            region,
            "UpdateDocumentDefaultVersion",
            &json!({ "Name": elsewhere, "DocumentVersion": "1" }),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "InvalidDocument"),
            "case {region}: another Region holds no such document, got {status} {response}"
        );
    }
    let described = describe_ok(&router, "the published document", "AWS-RunShellScript", None).await;
    assert_eq!(
        described["DefaultVersion"],
        json!("1"),
        "a published document still reports its one version as the default, got {described}"
    );
    let described = describe_ok(&router, REGION, elsewhere, None).await;
    assert_eq!(
        described["DefaultVersion"],
        json!("1"),
        "case {REGION}: the Region that holds the document is untouched, got {described}"
    );
}
