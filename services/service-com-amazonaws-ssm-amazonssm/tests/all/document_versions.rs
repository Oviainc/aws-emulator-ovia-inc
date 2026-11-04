// SPDX-License-Identifier: CC0-1.0

//! `UpdateDocument` end to end: an update appends a version, a read resolves one.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` ([`router`], [`call`], [`error_code`]) dispatches `POST /`
//! with `X-Amz-Target: AmazonSSM.<Operation>` through the multi-service router. Nothing
//! reaches into the state struct, so what an update persisted is read back only through
//! `DescribeDocument` and `GetDocument`.
//!
//! The suites are tables. Each row is one input along the axis the rule under test
//! generalises over — document type, document format, how many updates follow one
//! another, which version a selector names, which member the request supplies and which
//! it omits — rather than the single document a recorded unit happened to update.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::parameter_store::{call, error_code, router};

/// The transient window a version spends in `Creating`/`Updating`
/// (`crate::state::DOCUMENT_TRANSITION_DELAY`), which the service stores as a due instant
/// rather than sleeping for.
const TRANSITION_DELAY: Duration =
    service_com_amazonaws_ssm_amazonssm::state::DOCUMENT_TRANSITION_DELAY;

/// How many versions of one document this store keeps
/// (`crate::state::MAX_DOCUMENT_VERSIONS`), past which an update is refused with the
/// modelled `DocumentVersionLimitExceeded`.
const MAX_VERSIONS: usize = service_com_amazonaws_ssm_amazonssm::state::MAX_DOCUMENT_VERSIONS;

/// A JSON `Command` document, parameterised by the description its content declares, so
/// two rows of a table differ in content without differing in anything else.
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

/// `CreateDocument`, asserting it succeeded, and returning its `DocumentDescription`.
async fn create_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let (status, response) = call(router, "CreateDocument", body).await;
    assert_eq!(
        status, 200,
        "case {case}: CreateDocument must succeed, got {status} {response}"
    );
    response["DocumentDescription"].clone()
}

/// `UpdateDocument` with `body`, returning `(status, body)`.
async fn update(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "UpdateDocument", body).await
}

/// `UpdateDocument`, asserting it succeeded, and returning its `DocumentDescription`.
async fn update_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> Value {
    let (status, response) = update(router, body).await;
    assert_eq!(
        status, 200,
        "case {case}: UpdateDocument must succeed, got {status} {response}"
    );
    response["DocumentDescription"].clone()
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

/// `GetDocument` for one version selector and document format, asserting the read
/// succeeded.
///
/// "Returns the document in the specified format", so a round-trip that compares the
/// content a call supplied asks for the format it was written in; a request that names
/// none is served the documented `JSON` default, which re-renders the content.
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

/// One update-and-read row: how a document is created, the versions appended to it, and
/// the format each one is written in.
struct Appended<'a> {
    case: &'a str,
    name: &'a str,
    /// `None` leaves `DocumentType` to the service's default.
    document_type: Option<&'a str>,
    /// `None` leaves `DocumentFormat` to the documented `JSON` default.
    format: Option<&'a str>,
    /// The content of version 1, then of every version an update appends.
    contents: Vec<String>,
}

/// An update appends the NEXT version: the version it reports is one past the latest, the
/// document's `LatestVersion` moves with it, its `DefaultVersion` does not, and every
/// version is readable afterwards with the content and the derived members it persisted.
#[tokio::test]
async fn an_update_appends_the_next_version_and_every_version_stays_readable() {
    let router = router();
    let rows = vec![
        Appended {
            case: "a JSON Command document updated once",
            name: "AppendOnceDocument",
            document_type: None,
            format: None,
            contents: vec![command_json("version one"), command_json("version two")],
        },
        Appended {
            case: "a JSON Command document updated three times",
            name: "AppendThriceDocument",
            document_type: Some("Command"),
            format: Some("JSON"),
            contents: vec![
                command_json("first"),
                command_json("second"),
                command_json("third"),
                command_json("fourth"),
            ],
        },
        Appended {
            case: "a YAML Automation document",
            name: "AppendAutomationDocument",
            document_type: Some("Automation"),
            format: Some("YAML"),
            contents: vec![
                automation_yaml("remediate one"),
                automation_yaml("remediate two"),
            ],
        },
        Appended {
            case: "TEXT content, which carries no document tree",
            name: "AppendCalendarDocument",
            document_type: Some("ChangeCalendar"),
            format: Some("TEXT"),
            contents: vec![calendar_text("one"), calendar_text("two")],
        },
    ];

    for row in rows {
        let case = row.case;
        let mut body = json!({ "Name": row.name, "Content": row.contents[0] });
        if let Some(document_type) = row.document_type {
            body["DocumentType"] = json!(document_type);
        }
        if let Some(format) = row.format {
            body["DocumentFormat"] = json!(format);
        }
        let created = create_ok(&router, case, &body).await;
        assert_eq!(
            created["DocumentVersion"],
            json!("1"),
            "case {case}: a create mints version 1, got {created}"
        );

        for (index, content) in row.contents.iter().enumerate().skip(1) {
            let expected_version = (index + 1).to_string();
            let mut body = json!({ "Name": row.name, "Content": content });
            if let Some(format) = row.format {
                body["DocumentFormat"] = json!(format);
            }
            let updated = update_ok(&router, case, &body).await;
            for member in ["DocumentVersion", "LatestVersion"] {
                assert_eq!(
                    updated[member],
                    json!(expected_version),
                    "case {case}: expected {member} {expected_version}, got {updated}"
                );
            }
            // "The default version" moves only when `UpdateDocumentDefaultVersion` moves
            // it, so an appended version is the latest without becoming the default.
            assert_eq!(
                updated["DefaultVersion"],
                json!("1"),
                "case {case}: an update must not move DefaultVersion, got {updated}"
            );
            assert_eq!(
                updated["Status"],
                json!("Updating"),
                "case {case}: an appended version starts Updating, got {updated}"
            );
        }

        // Every version the document now holds reports the content it was given, and the
        // members derived from that content.
        let latest = row.contents.len().to_string();
        for (index, content) in row.contents.iter().enumerate() {
            let version = (index + 1).to_string();
            let described =
                describe_ok(&router, case, row.name, Some(version.as_str())).await;
            assert_eq!(
                described["DocumentVersion"],
                json!(version),
                "case {case}: version {version} reports itself, got {described}"
            );
            assert_eq!(
                described["LatestVersion"],
                json!(latest),
                "case {case}: version {version} reports the document's latest version \
                 {latest}, got {described}"
            );
            assert_eq!(
                described["Hash"],
                json!(content_hash(content)),
                "case {case}: version {version} reports the Sha256 of its own content, \
                 got {described}"
            );
            let fetched =
                get_ok(&router, case, row.name, Some(version.as_str()), row.format).await;
            assert_eq!(
                fetched["Content"],
                json!(content),
                "case {case}: version {version} reports the content it persisted, \
                 got {fetched}"
            );
        }

        // A read that names no version resolves the DEFAULT version, which is still 1.
        let described = describe_ok(&router, case, row.name, None).await;
        assert_eq!(
            described["DocumentVersion"],
            json!("1"),
            "case {case}: a read that names no version reports the default one, \
             got {described}"
        );
        let fetched = get_ok(&router, case, row.name, None, row.format).await;
        assert_eq!(
            fetched["Content"],
            json!(row.contents[0]),
            "case {case}: a read that names no version serves the default version's \
             content, got {fetched}"
        );
        // `$LATEST` resolves the version the last update appended.
        let fetched = get_ok(&router, case, row.name, Some("$LATEST"), row.format).await;
        assert_eq!(
            fetched["Content"],
            json!(row.contents[row.contents.len() - 1]),
            "case {case}: $LATEST serves the newest version's content, got {fetched}"
        );
    }
}

/// The hex Sha256 the service must report as a version's `Hash`
/// (`DocumentDescription$Hash`: "The Sha256 … hash created by the system when the
/// document was created").
fn content_hash(content: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(content.as_bytes()))
}

/// One duplicate-content row: a document, the contents its versions were given, and
/// which of those contents the refused update re-submits.
struct Duplicated<'a> {
    case: &'a str,
    name: &'a str,
    /// `None` leaves `DocumentType` to the service's default.
    document_type: Option<&'a str>,
    /// `None` leaves `DocumentFormat` to the documented `JSON` default.
    format: Option<&'a str>,
    contents: Vec<String>,
    /// The index in `contents` the refused update re-submits.
    duplicated: usize,
}

/// Content a version of the document already holds is not a new version of it: the update
/// is refused with `DuplicateDocumentContent` and appends nothing, whichever version holds
/// that content and whatever format it is written in.
#[tokio::test]
async fn content_a_version_already_holds_is_refused() {
    let router = router();
    let rows = vec![
        Duplicated {
            case: "the only version's content",
            name: "DuplicateOnlyVersion",
            document_type: None,
            format: None,
            contents: vec![command_json("only")],
            duplicated: 0,
        },
        Duplicated {
            case: "the latest version's content",
            name: "DuplicateLatestVersion",
            document_type: None,
            format: None,
            contents: vec![command_json("one"), command_json("two")],
            duplicated: 1,
        },
        Duplicated {
            case: "an older version's content",
            name: "DuplicateOlderVersion",
            document_type: None,
            format: None,
            contents: vec![
                command_json("one"),
                command_json("two"),
                command_json("three"),
            ],
            duplicated: 0,
        },
        Duplicated {
            case: "TEXT content with no document tree",
            name: "DuplicateCalendarVersion",
            document_type: Some("ChangeCalendar"),
            format: Some("TEXT"),
            contents: vec![calendar_text("one"), calendar_text("two")],
            duplicated: 0,
        },
        Duplicated {
            case: "YAML content",
            name: "DuplicateYamlVersion",
            document_type: Some("Automation"),
            format: Some("YAML"),
            contents: vec![automation_yaml("one"), automation_yaml("two")],
            duplicated: 1,
        },
    ];

    for row in rows {
        let Duplicated {
            case,
            name,
            document_type,
            format,
            contents,
            duplicated,
        } = row;
        let mut body = json!({ "Name": name, "Content": contents[0] });
        if let Some(document_type) = document_type {
            body["DocumentType"] = json!(document_type);
        }
        if let Some(format) = format {
            body["DocumentFormat"] = json!(format);
        }
        create_ok(&router, case, &body).await;
        for content in contents.iter().skip(1) {
            let mut body = json!({ "Name": name, "Content": content });
            if let Some(format) = format {
                body["DocumentFormat"] = json!(format);
            }
            update_ok(&router, case, &body).await;
        }

        let mut body = json!({ "Name": name, "Content": contents[duplicated] });
        if let Some(format) = format {
            body["DocumentFormat"] = json!(format);
        }
        let (status, response) = update(&router, &body).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "DuplicateDocumentContent"),
            "case {case}: content version {} already holds is refused, got {status} {response}",
            duplicated + 1
        );
        // Nothing was appended: the latest version is still the one the updates left.
        let described = describe_ok(&router, case, name, Some("$LATEST")).await;
        assert_eq!(
            described["LatestVersion"],
            json!(contents.len().to_string()),
            "case {case}: a refused update appends no version, got {described}"
        );
    }
}

/// "Currently, Systems Manager supports updating only the latest version of the
/// document": every selector that names the latest version is accepted, a selector that
/// names an older one is refused, and a selector that names no version of the document at
/// all is refused as an invalid version.
#[tokio::test]
async fn only_the_latest_version_can_be_updated() {
    let router = router();
    let name = "SelectUpdateTargetDocument";
    create_ok(
        &router,
        "the update target",
        &json!({ "Name": name, "Content": command_json("one") }),
    )
    .await;
    update_ok(
        &router,
        "the second version",
        &json!({ "Name": name, "Content": command_json("two") }),
    )
    .await;

    // Each accepted row appends a version, so the latest version moves 2 -> 3 -> 4 -> 5
    // and the third row names the then-latest version (4) by number.
    for (case, selector, appended) in [
        ("no selector at all", None, "3"),
        ("the $LATEST selector", Some("$LATEST"), "4"),
        ("the then-latest version's number", Some("4"), "5"),
    ] {
        let mut body = json!({
            "Name": name,
            "Content": command_json(&format!("version {appended}")),
        });
        if let Some(selector) = selector {
            body["DocumentVersion"] = json!(selector);
        }
        let updated = update_ok(&router, case, &body).await;
        assert_eq!(
            updated["DocumentVersion"],
            json!(appended),
            "case {case}: the update appends version {appended}, got {updated}"
        );
    }

    // The latest version is now 5 and the default is still 1, so each selector below
    // names a version this operation does not update, or none at all.
    for (case, selector, expected) in [
        (
            "the $DEFAULT selector, which names version 1",
            "$DEFAULT",
            "InvalidDocumentOperation",
        ),
        ("the first version's number", "1", "InvalidDocumentOperation"),
        (
            "a version between the first and the latest",
            "3",
            "InvalidDocumentOperation",
        ),
        (
            "a version the document has not got",
            "97",
            "InvalidDocumentVersion",
        ),
    ] {
        let (status, response) = update(
            &router,
            &json!({
                "Name": name,
                "DocumentVersion": selector,
                "Content": command_json("content nothing holds"),
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, expected),
            "case {case}: got {status} {response}"
        );
        let described = describe_ok(&router, case, name, Some("$LATEST")).await;
        assert_eq!(
            described["LatestVersion"],
            json!("5"),
            "case {case}: a refused update appends no version, got {described}"
        );
    }
}

/// An update is a write to a document this account created: a name nothing created names
/// no document, and a name Systems Manager publishes names one this account cannot
/// version.
#[tokio::test]
async fn a_document_the_account_did_not_create_is_not_updated() {
    let router = router();
    for (case, name, expected) in [
        ("a document nothing created", "NoSuchDocumentAtAll", "InvalidDocument"),
        // A document the catalogue publishes, which resolves for a read without any
        // create (`crate::state::PUBLISHED_DOCUMENTS`).
        ("a document Systems Manager publishes", "AWS-RunShellScript", "InvalidDocumentOperation"),
        ("another published document", "AWS-UpdateSSMAgent", "InvalidDocumentOperation"),
    ] {
        let (status, response) = update(
            &router,
            &json!({ "Name": name, "Content": command_json("mine now") }),
        )
        .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, expected),
            "case {case}: {name} must not take an update, got {status} {response}"
        );
    }
    // The published document is unchanged: it still reports its one version.
    let described = describe_ok(
        &router,
        "the published document after the refusal",
        "AWS-RunShellScript",
        None,
    )
    .await;
    assert_eq!(
        described["LatestVersion"],
        json!("1"),
        "a refused update leaves the published document alone, got {described}"
    );
}

/// A `VersionName` is "unique across all versions of a document": a name another version
/// already carries is refused, a fresh one is accepted, and a read by version name
/// resolves the version that carries it.
#[tokio::test]
async fn a_version_name_another_version_carries_is_refused() {
    let router = router();
    let name = "VersionNamedDocument";
    create_ok(
        &router,
        "the named first version",
        &json!({
            "Name": name,
            "Content": command_json("one"),
            "VersionName": "Release1.0",
        }),
    )
    .await;

    // A name version 1 already carries names no new version, whatever content comes with
    // it.
    let (status, response) = update(
        &router,
        &json!({
            "Name": name,
            "Content": command_json("two"),
            "VersionName": "Release1.0",
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "DuplicateDocumentVersionName"),
        "case the name version 1 carries: got {status} {response}"
    );

    // A fresh name is accepted, and then it is taken too.
    let updated = update_ok(
        &router,
        "a version name no version carries yet",
        &json!({
            "Name": name,
            "Content": command_json("two"),
            "VersionName": "Release2.0",
        }),
    )
    .await;
    assert_eq!(
        updated["VersionName"],
        json!("Release2.0"),
        "the appended version carries the name it was given, got {updated}"
    );
    let (status, response) = update(
        &router,
        &json!({
            "Name": name,
            "Content": command_json("three"),
            "VersionName": "Release2.0",
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "DuplicateDocumentVersionName"),
        "case the name version 2 carries: got {status} {response}"
    );

    // An update that names no version carries none: `VersionName` is optional.
    let updated = update_ok(
        &router,
        "an update that names no version",
        &json!({ "Name": name, "Content": command_json("three") }),
    )
    .await;
    assert_eq!(
        updated["VersionName"],
        Value::Null,
        "a version the update named nothing for carries no version name, got {updated}"
    );

    // A version name selects the version carrying it, and one no version carries selects
    // none.
    for (case, version_name, expected_version) in [
        ("version 1's name", "Release1.0", "1"),
        ("version 2's name", "Release2.0", "2"),
    ] {
        let (status, response) = call(
            &router,
            "DescribeDocument",
            &json!({ "Name": name, "VersionName": version_name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: a stored version name selects its version, got {status} {response}"
        );
        assert_eq!(
            response["Document"]["DocumentVersion"],
            json!(expected_version),
            "case {case}: expected version {expected_version}, got {response}"
        );
    }
    let (status, response) = call(
        &router,
        "DescribeDocument",
        &json!({ "Name": name, "VersionName": "Release9.9" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "InvalidDocumentVersion"),
        "a version name no version carries names no version, got {status} {response}"
    );
}

/// "If you don't specify a value for this parameter in your request, the existing value is
/// applied to the new document version" (`DisplayName`): an update that omits it keeps the
/// previous version's, and one that supplies it replaces it for the new version only.
#[tokio::test]
async fn an_omitted_display_name_is_inherited_and_a_supplied_one_replaces_it() {
    let router = router();
    // Each row: what the update supplies as DisplayName, and what the appended version
    // must report.
    let rows: &[(&str, Option<&str>, Option<&str>)] = &[
        ("an omitted display name", None, Some("First display name")),
        ("a supplied display name", Some("Second display name"), Some("Second display name")),
        ("an omitted display name after a supplied one", None, Some("Second display name")),
    ];
    let name = "DisplayNamedDocument";
    create_ok(
        &router,
        "the first version",
        &json!({
            "Name": name,
            "Content": command_json("one"),
            "DisplayName": "First display name",
        }),
    )
    .await;

    for (index, (case, supplied, expected)) in rows.iter().enumerate() {
        let mut body = json!({
            "Name": name,
            "Content": command_json(&format!("row {index}")),
        });
        if let Some(supplied) = supplied {
            body["DisplayName"] = json!(supplied);
        }
        let updated = update_ok(&router, case, &body).await;
        assert_eq!(
            updated["DisplayName"],
            json!(expected),
            "case {case}: expected DisplayName {expected:?}, got {updated}"
        );
        // The read of that version reports the same name the update did.
        let version = updated["DocumentVersion"].as_str().unwrap_or_default().to_string();
        let described = describe_ok(&router, case, name, Some(version.as_str())).await;
        assert_eq!(
            described["DisplayName"],
            json!(expected),
            "case {case}: the read of version {version} reports the stored DisplayName, \
             got {described}"
        );
    }
    // Version 1 still carries what the create gave it: a later version's display name is
    // that version's own.
    let described = describe_ok(&router, "version 1 after the updates", name, Some("1")).await;
    assert_eq!(
        described["DisplayName"],
        json!("First display name"),
        "an update must not rewrite an older version's DisplayName, got {described}"
    );
}

/// An appended version is `Updating` until the clock makes its move to `Active` due, and
/// `Active` after — materialised from the stored due instant, with nothing sleeping.
#[tokio::test]
async fn an_appended_version_becomes_active_once_the_transition_is_due() {
    let router = router();
    let name = "TransitioningVersionDocument";
    create_ok(
        &router,
        "the first version",
        &json!({ "Name": name, "Content": command_json("one") }),
    )
    .await;
    let started = std::time::Instant::now();
    let updated = update_ok(
        &router,
        "the appended version",
        &json!({ "Name": name, "Content": command_json("two") }),
    )
    .await;
    if started.elapsed() < TRANSITION_DELAY {
        assert_eq!(
            updated["Status"],
            json!("Updating"),
            "an appended version starts Updating, got {updated}"
        );
        let described = describe_ok(&router, "before the move is due", name, Some("2")).await;
        assert_eq!(
            described["Status"],
            json!("Updating"),
            "a read before the move is due still reports Updating, got {described}"
        );
    }
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(50)).await;
    let described = describe_ok(&router, "after the move is due", name, Some("2")).await;
    assert_eq!(
        described["Status"],
        json!("Active"),
        "the clock-owed move to Active is materialised, got {described}"
    );
    // The version the create minted made the same move, so both versions are Active.
    let described = describe_ok(&router, "version 1 after the move", name, Some("1")).await;
    assert_eq!(
        described["Status"],
        json!("Active"),
        "the created version is Active too, got {described}"
    );
}

/// One document's version history is bounded: the update that would append one version
/// past the bound is refused with `DocumentVersionLimitExceeded`, and the one at the bound
/// is accepted.
#[tokio::test]
async fn the_version_history_is_bounded() {
    let router = router();
    let name = "BoundedHistoryDocument";
    create_ok(
        &router,
        "the first version",
        &json!({ "Name": name, "Content": command_json("version 1") }),
    )
    .await;
    for version in 2..=MAX_VERSIONS {
        let case = format!("version {version}");
        let updated = update_ok(
            &router,
            &case,
            &json!({ "Name": name, "Content": command_json(&format!("version {version}")) }),
        )
        .await;
        assert_eq!(
            updated["DocumentVersion"],
            json!(version.to_string()),
            "case {case}: every version up to the bound is appended, got {updated}"
        );
    }
    let past = MAX_VERSIONS + 1;
    let (status, response) = update(
        &router,
        &json!({ "Name": name, "Content": command_json(&format!("version {past}")) }),
    )
    .await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "DocumentVersionLimitExceeded"),
        "an update one version past the bound is refused, got {status} {response}"
    );
    let described = describe_ok(&router, "after the refusal", name, Some("$LATEST")).await;
    assert_eq!(
        described["LatestVersion"],
        json!(MAX_VERSIONS.to_string()),
        "the refused update appends nothing, got {described}"
    );
}

/// Content that is not a document in the declared format, or that is too big, is refused
/// and appends nothing — the same rules a create is held to, applied to the version an
/// update would append.
#[tokio::test]
async fn content_an_update_cannot_persist_is_refused() {
    let router = router();
    let name = "RefusedContentDocument";
    create_ok(
        &router,
        "the first version",
        &json!({ "Name": name, "Content": command_json("one") }),
    )
    .await;

    // "The content of the document must not exceed 64KB" (`Content`), measured on the
    // content the update supplies.
    let oversize = format!(
        "{{\"schemaVersion\": \"2.2\", \"description\": \"{}\"}}",
        "p".repeat(64 * 1024)
    );
    // A schema version no Command document supports, and content that is not a document
    // at all in the declared format.
    for (case, content, expected) in [
        ("content past the size limit", oversize.as_str(), "MaxDocumentSizeExceeded"),
        ("content that is not a JSON document", "not a document", "InvalidDocumentContent"),
        (
            "a schema version the document type does not support",
            r#"{"schemaVersion": "9.9", "description": "unsupported"}"#,
            "InvalidDocumentSchemaVersion",
        ),
    ] {
        let (status, response) = update(&router, &json!({ "Name": name, "Content": content })).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, expected),
            "case {case}: got {status} {response}"
        );
        let described = describe_ok(&router, case, name, Some("$LATEST")).await;
        assert_eq!(
            described["LatestVersion"],
            json!("1"),
            "case {case}: a refused update appends no version, got {described}"
        );
    }
}
