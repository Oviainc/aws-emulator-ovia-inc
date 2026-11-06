// SPDX-License-Identifier: CC0-1.0

//! Single sign-on — `EnableSso` — driven end to end over HTTP, with
//! `DescribeDirectories` as the reader that proves what was persisted.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router (the harness is the sibling suite's, reached
//! as `crate::directory_lifecycle::<item>`). Nothing a RESPONSE claims is read from the
//! state struct, so an answer that is not computed from persisted state cannot pass.
//!
//! The rules under test, each asserted over a TABLE rather than one recorded call:
//!
//! * single sign-on is a property of ONE directory: an enable records it on the
//!   directory the request named and `DescribeDirectories` reports it as `SsoEnabled`,
//!   over a table of directories differing in create operation, type, size, edition and
//!   network type, with an untouched directory in the same store as the control — and a
//!   second enable of a directory already carrying it is not refused and changes
//!   nothing;
//! * an enable needs the directory's access URL, which exists only once an alias has
//!   been assigned: before `CreateAlias` the call is refused with `ClientException`
//!   whose diagnostic names the directory the request asked for, the refusal records
//!   nothing, and the identical call succeeds once the alias is there;
//! * `UserName`/`Password` are an alternate directory account's credentials and this
//!   emulator holds no directory account credentials, so a request supplying either is
//!   refused with `AuthenticationFailedException` — over a table of credential forms the
//!   model admits, on directories both with and without an alias, and with nothing
//!   recorded by the refusal;
//! * the operation resolves the `DirectoryId` in the store and refuses an id no
//!   directory is stored under with `EntityDoesNotExistException` — over a table of
//!   well-formed ids differing in digits and form — while an id the MODEL refuses is
//!   refused before that resolution happens at all, and no case ever answers the
//!   scaffold's `NotImplemented`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{
    call, create_directory, describe_one, minimal_create, ok, refused, service, string, PASSWORD,
    PAST_ONE_SPAN,
};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// Well-formed `DirectoryId`s no directory is ever created under, as `(id, class)`.
///
/// The form comes from the model: `DirectoryId` is `^d-[0-9a-f]{10}$`
/// (`smithy_query(query="operation", operation="EnableSso")`), so every row here is a
/// value the boundary validator admits and the handler must resolve in the store. The
/// rows are that shape with different digits, including both extremes of the alphabet.
const ABSENT_DIRECTORY_IDS: [(&str, &str); 4] = [
    ("d-0000000000", "all zeros, the low end of the alphabet"),
    ("d-ffffffffff", "all f, the high end of the alphabet"),
    ("d-abcdef0123", "every hex digit once"),
    ("d-5e4d3c2b1a", "digits and letters interleaved"),
];

/// Credential pairs the MODEL admits, as `(user_name, password, class)`.
///
/// Both members' constraints come from the model
/// (`smithy_query(query="operation", operation="EnableSso")`): `UserName` is
/// `^[a-zA-Z0-9._-]+$` with a minimum length of 1, `Password` (`ConnectPassword`) is
/// 1..128 and `@sensitive`. Each row is inside those bounds and differs along which
/// member is present and how the value is formed, including each published length bound
/// of `Password`.
const CREDENTIAL_ROWS: [(Option<&str>, Option<&str>, &str); 6] = [
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
    (
        Some("ssoadmin"),
        Some(LONGEST_PASSWORD),
        "both members, a password at the published maximum length",
    ),
    (Some("ssoadmin"), None, "the user name alone"),
    (None, Some("Alt-Account!3"), "the password alone"),
];

/// A `Password` at `ConnectPassword`'s published maximum length of 128.
const LONGEST_PASSWORD: &str = "Alt-Account!0123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123456789012345678";

/// The sentence this service states when it could not authenticate the account a call
/// carried: the text the modelled error shape's own documentation states for the class
/// (`smithy_query(query="doc", shape="AuthenticationFailedException")`).
const AUTHENTICATION_FAILED: &str = "An authentication error occurred.";

/// The `Message` of a refusal body, whichever casing the envelope carries it in: a
/// modelled error of this service reports `Message`, the boundary's `ValidationException`
/// reports `message`.
fn message(parsed: &Value, case: &str) -> String {
    parsed["Message"]
        .as_str()
        .or_else(|| parsed["message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: expected a message, got {parsed}"))
        .to_string()
}

/// The modelled `RequestId` a refusal of this service reports.
fn request_id(parsed: &Value, case: &str) -> String {
    string(parsed, "RequestId", case)
}

/// The whole diagnostic a refusal carries: the sentence, then the request-id suffix this
/// service appends to every refusal it words.
fn with_request_id(sentence: &str, reported: &str) -> String {
    format!("{sentence} : RequestId: {reported}")
}

/// One row of the directory table: how the directory is created, and whether this case
/// enables single sign-on on it or leaves it as the untouched control.
struct SsoCase {
    case: &'static str,
    operation: &'static str,
    body: Value,
    alias: &'static str,
    enabled: bool,
}

/// Create the row's directory through whichever create operation it names.
async fn create(router: &MultiServiceRouter, row: &SsoCase) -> String {
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

/// The `Alias` `DescribeDirectories` reports for `directory_id`.
async fn reported_alias(router: &MultiServiceRouter, directory_id: &str, case: &str) -> String {
    let described = describe_one(router, directory_id, case).await;
    string(&described, "Alias", case)
}

/// Assign `alias` to `directory_id`, asserting the reader then reports it.
async fn create_alias(router: &MultiServiceRouter, directory_id: &str, alias: &str, case: &str) {
    ok(
        router,
        "CreateAlias",
        json!({ "DirectoryId": directory_id, "Alias": alias }),
        case,
    )
    .await;
    let reported = reported_alias(router, directory_id, case).await;
    assert_eq!(
        reported, alias,
        "case {case:?}: expected the assigned alias to be reported, got {reported:?}"
    );
}

/// The directory table: rows differing along everything the two creates persist —
/// create operation, directory type, size, edition and network type — plus whether this
/// case enables single sign-on at all, so the control row proves the flag is
/// per-directory rather than per-store.
fn directory_rows() -> Vec<SsoCase> {
    vec![
        SsoCase {
            case: "a minimal Simple AD directory",
            operation: "CreateDirectory",
            body: minimal_create("simple.example.com"),
            alias: "sso-simple",
            enabled: true,
        },
        SsoCase {
            case: "a large Simple AD directory with a dual-stack network",
            operation: "CreateDirectory",
            body: json!({
                "Name": "large.example.com",
                "Password": PASSWORD,
                "Size": "Large",
                "NetworkType": "Dual-stack"
            }),
            alias: "sso-large",
            enabled: true,
        },
        SsoCase {
            case: "a minimal Managed Microsoft AD directory",
            operation: "CreateMicrosoftAD",
            body: minimal_microsoft_ad("corp.example.com"),
            alias: "sso-corp",
            enabled: true,
        },
        SsoCase {
            case: "a Standard-edition Managed Microsoft AD directory",
            operation: "CreateMicrosoftAD",
            body: json!({
                "Name": "standard.example.com",
                "Password": PASSWORD,
                "Edition": "Standard",
                "VpcSettings": {
                    "VpcId": "vpc-0a1b2c3d",
                    "SubnetIds": ["subnet-0a1b2c3d", "subnet-1a2b3c4d"]
                }
            }),
            alias: "sso-standard",
            enabled: true,
        },
        SsoCase {
            case: "the control directory, never enabled",
            operation: "CreateDirectory",
            body: minimal_create("control.example.com"),
            alias: "sso-control",
            enabled: false,
        },
    ]
}

#[tokio::test]
async fn single_sign_on_is_recorded_per_directory_and_reported_from_the_store() {
    let router = service();
    let rows = directory_rows();

    let mut ids: Vec<(&SsoCase, String)> = Vec::new();
    for row in &rows {
        let id = create(&router, row).await;
        // A freshly created directory has single sign-on off and no alias of its own:
        // its `Alias` is the directory id, which is how the missing access URL is
        // observable.
        assert!(
            !sso_enabled(&router, &id, row.case).await,
            "case {:?}: a freshly created directory reports SsoEnabled false",
            row.case
        );
        let alias = reported_alias(&router, &id, row.case).await;
        assert_eq!(
            alias, id,
            "case {:?}: expected Alias to default to the directory id, got {alias:?}",
            row.case
        );
        ids.push((row, id));
    }

    // Every row gets its access URL, the control included, so the only difference
    // between the control and the rest is the enable itself.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    for (row, id) in &ids {
        create_alias(&router, id, row.alias, row.case).await;
    }

    for (row, id) in &ids {
        if !row.enabled {
            continue;
        }
        let answer = ok(
            &router,
            "EnableSso",
            json!({ "DirectoryId": id }),
            row.case,
        )
        .await;
        assert_eq!(
            answer,
            json!({}),
            "case {:?}: EnableSso has no modelled output members, got {answer}",
            row.case
        );
    }

    for (row, id) in &ids {
        let reported = sso_enabled(&router, id, row.case).await;
        assert_eq!(
            reported, row.enabled,
            "case {:?}: expected SsoEnabled {}, got {reported}",
            row.case, row.enabled
        );
    }

    // A second enable asks for the state the directory is already in: the modelled
    // output has no members, nothing is refused, and the flag stays on. The control
    // directory in the same store is still off, and enabling it now turns on only its
    // own flag.
    for (row, id) in &ids {
        if !row.enabled {
            continue;
        }
        ok(
            &router,
            "EnableSso",
            json!({ "DirectoryId": id }),
            row.case,
        )
        .await;
        assert!(
            sso_enabled(&router, id, row.case).await,
            "case {:?}: a repeated enable leaves single sign-on on",
            row.case
        );
    }

    for (row, id) in &ids {
        if row.enabled {
            continue;
        }
        ok(
            &router,
            "EnableSso",
            json!({ "DirectoryId": id }),
            row.case,
        )
        .await;
        assert!(
            sso_enabled(&router, id, row.case).await,
            "case {:?}: the control directory enables like any other",
            row.case
        );
    }

    // Reading the directories back has not moved anything on: every row is on now.
    for (row, id) in &ids {
        assert!(
            sso_enabled(&router, id, row.case).await,
            "case {:?}: a repeated read reports single sign-on as on",
            row.case
        );
    }
}

#[tokio::test]
async fn enabling_single_sign_on_needs_the_directory_s_access_url() {
    let router = service();
    let rows = directory_rows();

    let mut ids: Vec<(&SsoCase, String)> = Vec::new();
    for row in &rows {
        let id = create(&router, row).await;
        ids.push((row, id));
    }
    tokio::time::sleep(PAST_ONE_SPAN).await;

    // No alias has been assigned to any of them, so none has an access URL and every
    // row is refused — with the diagnostic built from the id that row's request asked
    // for, so the refusal is about the directory the call named.
    for (row, id) in &ids {
        let (status, parsed) = call(&router, "EnableSso", json!({ "DirectoryId": id })).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected EnableSso to answer 400 before an alias exists, got \
             {status} with {parsed}",
            row.case
        );
        assert_eq!(
            parsed["__type"],
            json!("ClientException"),
            "case {:?}: expected the modelled client refusal, got {parsed}",
            row.case
        );
        let reported = request_id(&parsed, row.case);
        assert_eq!(
            message(&parsed, row.case),
            with_request_id(
                &format!("An alias is required before enabling SSO. DomainId={id}"),
                &reported
            ),
            "case {:?}: expected the missing-access-URL sentence, got {parsed}",
            row.case
        );
        // The refusal recorded nothing.
        assert!(
            !sso_enabled(&router, id, row.case).await,
            "case {:?}: a refused enable leaves single sign-on off",
            row.case
        );
    }

    // The refusal was about the missing access URL, so the identical call is admitted
    // once the one missing prerequisite is supplied.
    for (row, id) in &ids {
        create_alias(&router, id, row.alias, row.case).await;
        ok(
            &router,
            "EnableSso",
            json!({ "DirectoryId": id }),
            row.case,
        )
        .await;
        assert!(
            sso_enabled(&router, id, row.case).await,
            "case {:?}: the enable is admitted once the alias is assigned",
            row.case
        );
    }
}

#[tokio::test]
async fn an_alternate_account_cannot_be_authenticated() {
    let router = service();

    // Two directories: one that has its access URL already and one that has not, so the
    // credential refusal is shown to be independent of that prerequisite.
    let aliased = create_directory(
        &router,
        minimal_create("aliased.example.com"),
        "the aliased directory",
    )
    .await;
    let bare = create_directory(
        &router,
        minimal_create("bare.example.com"),
        "the directory with no alias",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    create_alias(&router, &aliased, "sso-aliased", "the aliased directory").await;

    for (user_name, password, class) in CREDENTIAL_ROWS {
        for (directory_id, kind) in [
            (&aliased, "with an alias"),
            (&bare, "without an alias"),
        ] {
            let case = format!("{class}, on a directory {kind}");
            let mut body = json!({ "DirectoryId": directory_id });
            if let Some(user_name) = user_name {
                body["UserName"] = json!(user_name);
            }
            if let Some(password) = password {
                body["Password"] = json!(password);
            }

            let (status, parsed) = call(&router, "EnableSso", body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected EnableSso to answer 400, got {status} with \
                 {parsed}"
            );
            assert_eq!(
                parsed["__type"],
                json!("AuthenticationFailedException"),
                "case {case:?}: expected the modelled authentication refusal, got {parsed}"
            );
            let reported = request_id(&parsed, &case);
            assert_eq!(
                message(&parsed, &case),
                with_request_id(AUTHENTICATION_FAILED, &reported),
                "case {case:?}: expected the authentication-failure sentence, got {parsed}"
            );
            // Neither member of the credential pair is echoed back: a refusal discloses
            // nothing the caller sent.
            let body_text = parsed.to_string();
            if let Some(password) = password {
                assert!(
                    !body_text.contains(password),
                    "case {case:?}: the refusal must not echo the password, got {parsed}"
                );
            }
            assert!(
                !sso_enabled(&router, directory_id, &case).await,
                "case {case:?}: a refused enable records nothing"
            );
        }
    }

    // The same call without an alternate account acts as the service's own account, so
    // the directory that has its access URL is enabled and the one that has not is
    // refused for the prerequisite instead.
    ok(
        &router,
        "EnableSso",
        json!({ "DirectoryId": &aliased }),
        "the aliased directory",
    )
    .await;
    assert!(
        sso_enabled(&router, &aliased, "the aliased directory").await,
        "case {:?}: an enable with no credentials is admitted",
        "the aliased directory"
    );
    refused(
        &router,
        "EnableSso",
        json!({ "DirectoryId": &bare }),
        "ClientException",
        "the directory with no alias",
    )
    .await;
}

#[tokio::test]
async fn the_directory_is_resolved_in_the_store() {
    let router = service();

    // One stored, aliased, enabled directory, so the store is not empty and a refusal
    // cannot come from there being nothing at all.
    let stored = create_directory(
        &router,
        minimal_create("resolved.example.com"),
        "the stored directory",
    )
    .await;
    tokio::time::sleep(PAST_ONE_SPAN).await;
    create_alias(&router, &stored, "sso-resolved", "the stored directory").await;

    for (absent, class) in ABSENT_DIRECTORY_IDS {
        let (status, parsed) = call(&router, "EnableSso", json!({ "DirectoryId": absent })).await;
        assert_eq!(
            status, 400,
            "case {class:?}: expected EnableSso to answer 400, got {status} with {parsed}"
        );
        assert_eq!(
            parsed["__type"],
            json!("EntityDoesNotExistException"),
            "case {class:?}: expected the modelled resolution refusal, got {parsed}"
        );
        let reported = request_id(&parsed, class);
        assert_eq!(
            message(&parsed, class),
            with_request_id(&format!("Directory: '{absent}' does not exist."), &reported),
            "case {class:?}: expected the shared unresolved-directory sentence, got {parsed}"
        );
    }

    // An id the MODEL refuses never reaches that resolution: the boundary validator
    // refuses it first, so the answer names neither the directory nor the scaffold.
    let malformed = [
        ("d-ABCDEF0123", "upper case, outside the pattern's alphabet"),
        ("d-0a1b2c3d", "too few digits"),
        ("d-0a1b2c3d4e5", "too many digits"),
        ("", "empty"),
    ];
    for (id, class) in malformed {
        let (status, parsed) = call(&router, "EnableSso", json!({ "DirectoryId": id })).await;
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
            "case {class:?}: the operation must not answer the scaffold signal, got \
             {parsed}"
        );
    }

    // And the stored directory still enables and reports back, so none of the refusals
    // above touched it.
    ok(
        &router,
        "EnableSso",
        json!({ "DirectoryId": &stored }),
        "the stored directory",
    )
    .await;
    assert!(
        sso_enabled(&router, &stored, "the stored directory").await,
        "case {:?}: the stored directory enables and reports back",
        "the stored directory"
    );
}
