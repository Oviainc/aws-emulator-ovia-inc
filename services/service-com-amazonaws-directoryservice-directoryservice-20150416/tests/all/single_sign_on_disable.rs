// SPDX-License-Identifier: CC0-1.0

//! Turning single sign-on OFF — `DisableSso` — driven end to end over HTTP, with
//! `DescribeDirectories` as the reader that proves what was persisted.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router (the harness is the sibling suite's, reached as
//! `crate::directory_lifecycle::<item>`). Nothing a RESPONSE claims is read from the state
//! struct, so an answer that is not computed from persisted state cannot pass.
//!
//! The rules under test, each asserted over a TABLE rather than one recorded call:
//!
//! * a disable asks for the state the directory is to be left in, so it is an IDEMPOTENT
//!   NO-OP: the operation models no "already off" failure and its output carries no
//!   members, so a directory whose single sign-on was never turned on — and which has no
//!   alias and therefore no access URL either — is answered 200 with an empty body, exactly
//!   as one that had it on, and the switch reads off afterwards in both cases;
//! * the switch is a property of ONE directory: a disable clears it on the directory the
//!   request named and leaves every other directory in the store alone, over a table of
//!   directories differing in create operation, type, size, edition and network type;
//! * the operation resolves the `DirectoryId` in the store and refuses an id no directory
//!   is stored under with `EntityDoesNotExistException`, worded with this service's shared
//!   sentence for a directory a call cannot reach;
//! * `UserName`/`Password` name an alternate directory account this emulator holds no
//!   credentials for, so a request supplying either is refused with
//!   `AuthenticationFailedException` and records nothing;
//! * no case ever answers the scaffold's `NotImplemented`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{
    call, create_directory, describe_one, minimal_create, ok, service, string, PASSWORD,
    PAST_ONE_SPAN,
};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// Well-formed `DirectoryId`s no directory is ever created under, as `(id, class)`.
///
/// The form comes from the model: `DirectoryId` is `^d-[0-9a-f]{10}$`
/// (`smithy_query(query="operation", operation="DisableSso")`), so every row here is a
/// value the boundary validator admits and the handler must resolve in the store.
const ABSENT_DIRECTORY_IDS: [(&str, &str); 4] = [
    ("d-0000000000", "all zeros, the low end of the alphabet"),
    ("d-ffffffffff", "all f, the high end of the alphabet"),
    ("d-abcdef0123", "every hex digit once"),
    ("d-5e4d3c2b1a", "digits and letters interleaved"),
];

/// Credential pairs the MODEL admits, as `(user_name, password, class)`.
///
/// Both members' constraints come from the model
/// (`smithy_query(query="operation", operation="DisableSso")`): `UserName` is
/// `^[a-zA-Z0-9._-]+$` with a minimum length of 1, `Password` (`ConnectPassword`) is 1..128
/// and `@sensitive`. Each row is inside those bounds and differs along which member is
/// present and how the value is formed, including each published length bound of
/// `Password`.
const CREDENTIAL_ROWS: [(Option<&str>, Option<&str>, &str); 5] = [
    (
        Some("ssoadmin"),
        Some("Alt-Account!1"),
        "both members, a plain user name",
    ),
    (
        Some("sso.admin_1-x"),
        Some("Alt-Account!2"),
        "both members, every punctuation character the pattern admits",
    ),
    (Some("a"), Some("b"), "both members at their minimum length"),
    (Some("ssoadmin"), None, "the user name alone"),
    (None, Some("Alt-Account!3"), "the password alone"),
];

/// The sentence this service states when it could not authenticate the account a call
/// carried: the text the modelled error shape's own documentation states for the class
/// (`smithy_query(query="doc", shape="AuthenticationFailedException")`).
const AUTHENTICATION_FAILED: &str = "An authentication error occurred.";

/// One row of the directory table: how the directory is created, and whether single
/// sign-on is turned ON before this case turns it off — the "never enabled" rows are the
/// idempotent no-op the recorded teardown path drives.
struct Row {
    case: &'static str,
    operation: &'static str,
    body: Value,
    alias: &'static str,
    enabled_first: bool,
}

/// Create the row's directory through whichever create operation it names.
async fn create(router: &MultiServiceRouter, row: &Row) -> String {
    match row.operation {
        "CreateMicrosoftAD" => create_microsoft_ad(router, row.body.clone(), row.case).await,
        _ => create_directory(router, row.body.clone(), row.case).await,
    }
}

/// Whether `DescribeDirectories` reports single sign-on as on for `directory_id`.
async fn sso_enabled(router: &MultiServiceRouter, directory_id: &str, case: &str) -> bool {
    let described = describe_one(router, directory_id, case).await;
    described["SsoEnabled"]
        .as_bool()
        .unwrap_or_else(|| panic!("case {case:?}: expected a boolean SsoEnabled, got {described}"))
}

/// `DisableSso` on `directory_id`, asserting it answers 200 with the empty modelled output.
async fn disable(router: &MultiServiceRouter, directory_id: &str, case: &str) {
    let answer = ok(
        router,
        "DisableSso",
        json!({ "DirectoryId": directory_id }),
        case,
    )
    .await;
    assert_eq!(
        answer,
        json!({}),
        "case {case:?}: DisableSso has no modelled output members, got {answer}"
    );
}

/// The `Message` of a refusal body, whichever casing the envelope carries it in.
fn message(parsed: &Value, case: &str) -> String {
    parsed["Message"]
        .as_str()
        .or_else(|| parsed["message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: expected a message, got {parsed}"))
        .to_string()
}

/// The whole diagnostic a refusal carries: the sentence, then the request-id suffix this
/// service appends to every refusal it words.
fn with_request_id(sentence: &str, parsed: &Value, case: &str) -> String {
    format!(
        "{sentence} : RequestId: {id}",
        id = string(parsed, "RequestId", case)
    )
}

/// The table: rows differing along everything the two creates persist, and along whether
/// the row's single sign-on was ever turned on.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a minimal Simple AD directory that never had single sign-on on",
            operation: "CreateDirectory",
            body: minimal_create("off-simple.example.com"),
            alias: "off-simple",
            enabled_first: false,
        },
        Row {
            case: "a large dual-stack Simple AD directory with single sign-on on",
            operation: "CreateDirectory",
            body: json!({
                "Name": "off-large.example.com",
                "Password": PASSWORD,
                "Size": "Large",
                "NetworkType": "Dual-stack"
            }),
            alias: "off-large",
            enabled_first: true,
        },
        Row {
            case: "a minimal Managed Microsoft AD directory that never had it on",
            operation: "CreateMicrosoftAD",
            body: minimal_microsoft_ad("off-corp.example.com"),
            alias: "off-corp",
            enabled_first: false,
        },
        Row {
            case: "a Standard-edition Managed Microsoft AD directory with it on",
            operation: "CreateMicrosoftAD",
            body: json!({
                "Name": "off-standard.example.com",
                "Password": PASSWORD,
                "Edition": "Standard",
                "VpcSettings": {
                    "VpcId": "vpc-0a1b2c3d",
                    "SubnetIds": ["subnet-0a1b2c3d", "subnet-1a2b3c4d"]
                }
            }),
            alias: "off-standard",
            enabled_first: true,
        },
    ]
}

#[tokio::test]
async fn a_disable_is_an_idempotent_no_op_whether_single_sign_on_was_on() {
    let router = service();
    let rows = rows();

    let mut ids: Vec<(&Row, String)> = Vec::new();
    for row in &rows {
        ids.push((row, create(&router, row).await));
    }
    // An alias is assigned only to a directory that has reached its serving stage, and only
    // the rows that turn single sign-on ON first need one at all: the rows that never had it
    // on carry no alias and therefore no access URL, which is the state the recorded
    // teardown path disables from.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    for (row, id) in &ids {
        if !row.enabled_first {
            continue;
        }
        ok(
            &router,
            "CreateAlias",
            json!({ "DirectoryId": id, "Alias": row.alias }),
            row.case,
        )
        .await;
        ok(&router, "EnableSso", json!({ "DirectoryId": id }), row.case).await;
        assert!(
            sso_enabled(&router, id, row.case).await,
            "case {:?}: expected the enable to be in force before the disable",
            row.case
        );
    }

    // Every row disables, and every row is then off — the rows that were on and the rows
    // that never were, which are answered the same 200 with the same empty body.
    for (row, id) in &ids {
        disable(&router, id, row.case).await;
        assert!(
            !sso_enabled(&router, id, row.case).await,
            "case {:?}: expected single sign-on to read off after the disable",
            row.case
        );
    }

    // A repeated disable asks for the state the directory is already in: it is not refused
    // and nothing changes.
    for (row, id) in &ids {
        disable(&router, id, row.case).await;
        assert!(
            !sso_enabled(&router, id, row.case).await,
            "case {:?}: expected a repeated disable to leave single sign-on off",
            row.case
        );
    }

    // And the switch still turns back on and off again for a directory that has its access
    // URL, so the disable cleared a value rather than wedging the record.
    for (row, id) in &ids {
        if !row.enabled_first {
            continue;
        }
        ok(&router, "EnableSso", json!({ "DirectoryId": id }), row.case).await;
        assert!(
            sso_enabled(&router, id, row.case).await,
            "case {:?}: expected the enable after a disable to be in force",
            row.case
        );
        disable(&router, id, row.case).await;
        assert!(
            !sso_enabled(&router, id, row.case).await,
            "case {:?}: expected the second disable to clear the switch again",
            row.case
        );
    }
}

#[tokio::test]
async fn a_disable_clears_the_switch_of_the_directory_it_names_alone() {
    let router = service();

    // Three directories, all with single sign-on on, so a disable of one is shown to leave
    // the others as they were.
    let mut ids = Vec::new();
    for index in 0..3 {
        let id = create_directory(
            &router,
            minimal_create(&format!("many{index}.example.com")),
            "per directory",
        )
        .await;
        ids.push(id);
    }
    tokio::time::sleep(PAST_ONE_SPAN).await;
    for (index, id) in ids.iter().enumerate() {
        ok(
            &router,
            "CreateAlias",
            json!({ "DirectoryId": id, "Alias": format!("many{index}") }),
            "per directory",
        )
        .await;
        ok(
            &router,
            "EnableSso",
            json!({ "DirectoryId": id }),
            "per directory",
        )
        .await;
    }

    for (index, id) in ids.iter().enumerate() {
        let case = format!("disabling directory {index}");
        disable(&router, id, &case).await;
        for (other, other_id) in ids.iter().enumerate() {
            let expected = other > index;
            let reported = sso_enabled(&router, other_id, &case).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: expected directory {other} to report SsoEnabled {expected}, \
                 got {reported}"
            );
        }
    }
}

#[tokio::test]
async fn the_directory_is_resolved_in_the_store() {
    let router = service();

    // One stored directory with single sign-on on, so a refusal cannot come from there
    // being nothing at all — and so a refused call can be shown to have changed nothing.
    let stored = create_directory(
        &router,
        minimal_create("resolved.example.com"),
        "the stored directory",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    ok(
        &router,
        "CreateAlias",
        json!({ "DirectoryId": &stored, "Alias": "off-resolved" }),
        "the stored directory",
    )
    .await;
    ok(
        &router,
        "EnableSso",
        json!({ "DirectoryId": &stored }),
        "the stored directory",
    )
    .await;

    for (absent, class) in ABSENT_DIRECTORY_IDS {
        let (status, parsed) = call(&router, "DisableSso", json!({ "DirectoryId": absent })).await;
        assert_eq!(
            status, 400,
            "case {class:?}: expected DisableSso to answer 400, got {status} with {parsed}"
        );
        assert_eq!(
            parsed["__type"],
            json!("EntityDoesNotExistException"),
            "case {class:?}: expected the modelled resolution refusal, got {parsed}"
        );
        assert_eq!(
            message(&parsed, class),
            with_request_id(
                &format!("Directory: '{absent}' does not exist."),
                &parsed,
                class
            ),
            "case {class:?}: expected the shared unresolved-directory sentence, got {parsed}"
        );
    }

    // An id the MODEL refuses never reaches that resolution, and no case answers the
    // scaffold's signal.
    for (id, class) in [
        ("d-ABCDEF0123", "upper case, outside the pattern's alphabet"),
        ("d-0a1b2c3d", "too few digits"),
        ("d-0a1b2c3d4e5", "too many digits"),
        ("", "empty"),
    ] {
        let (status, parsed) = call(&router, "DisableSso", json!({ "DirectoryId": id })).await;
        assert_eq!(
            status, 400,
            "case {class:?}: the model refuses {id:?}, got {status} with {parsed}"
        );
        let wire_type = parsed["__type"].as_str().unwrap_or_default().to_string();
        assert!(
            !wire_type.contains("EntityDoesNotExistException"),
            "case {class:?}: the input is refused before the directory is resolved, got \
             {parsed}"
        );
        assert!(
            !wire_type.contains("NotImplemented"),
            "case {class:?}: the operation must not answer the scaffold signal, got {parsed}"
        );
    }

    // The stored directory is untouched by every refusal above, and still disables.
    assert!(
        sso_enabled(&router, &stored, "the stored directory").await,
        "case \"the stored directory\": expected the refusals to have changed nothing"
    );
    disable(&router, &stored, "the stored directory").await;
    assert!(
        !sso_enabled(&router, &stored, "the stored directory").await,
        "case \"the stored directory\": expected the stored directory to disable"
    );
}

#[tokio::test]
async fn an_alternate_account_cannot_be_authenticated() {
    let router = service();

    // Two directories: one with single sign-on on and one that never had it on, so the
    // credential refusal is shown to be independent of the state the switch is in.
    let enabled = create_directory(
        &router,
        minimal_create("creds-on.example.com"),
        "the enabled directory",
    )
    .await;
    let never = create_directory(
        &router,
        minimal_create("creds-off.example.com"),
        "the never-enabled directory",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    ok(
        &router,
        "CreateAlias",
        json!({ "DirectoryId": &enabled, "Alias": "creds-on" }),
        "the enabled directory",
    )
    .await;
    ok(
        &router,
        "EnableSso",
        json!({ "DirectoryId": &enabled }),
        "the enabled directory",
    )
    .await;

    for (user_name, password, class) in CREDENTIAL_ROWS {
        for (directory_id, kind, was_on) in [
            (&enabled, "with single sign-on on", true),
            (&never, "that never had it on", false),
        ] {
            let case = format!("{class}, on a directory {kind}");
            let mut body = json!({ "DirectoryId": directory_id });
            if let Some(user_name) = user_name {
                body["UserName"] = json!(user_name);
            }
            if let Some(password) = password {
                body["Password"] = json!(password);
            }

            let (status, parsed) = call(&router, "DisableSso", body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected DisableSso to answer 400, got {status} with {parsed}"
            );
            assert_eq!(
                parsed["__type"],
                json!("AuthenticationFailedException"),
                "case {case:?}: expected the modelled authentication refusal, got {parsed}"
            );
            assert_eq!(
                message(&parsed, &case),
                with_request_id(AUTHENTICATION_FAILED, &parsed, &case),
                "case {case:?}: expected the authentication-failure sentence, got {parsed}"
            );
            // The refusal recorded nothing: the switch is exactly where it was.
            let reported = sso_enabled(&router, directory_id, &case).await;
            assert_eq!(
                reported, was_on,
                "case {case:?}: expected a refused disable to leave SsoEnabled {was_on}, got \
                 {reported}"
            );
        }
    }

    // The same call without an alternate account acts as the service's own account, so both
    // directories disable.
    for (directory_id, case) in [
        (&enabled, "the enabled directory"),
        (&never, "the never-enabled directory"),
    ] {
        disable(&router, directory_id, case).await;
        assert!(
            !sso_enabled(&router, directory_id, case).await,
            "case {case:?}: expected the disable with no credentials to be admitted"
        );
    }
}
