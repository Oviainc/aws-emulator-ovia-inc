// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteWebAppCustomization` — the operation that removes the
//! customization object `UpdateWebAppCustomization` assigned to a web app.
//!
//! Every test drives the real wire path through the shared [`crate::harness`] — `POST /`
//! with `X-Amz-Target: TransferService.<Op>` — so what is asserted is what an SDK sees,
//! not what a handler returns. Blob members travel base64-encoded on awsJson, so each
//! file is encoded with `aws_smithy_types::base64`, the codec the wire serializer uses.
//!
//! RULES under test:
//!
//! 1. A web app's customization is an OBJECT that exists only while some update has
//!    assigned it: `DeleteWebAppCustomization` removes the whole object and answers an
//!    empty 200, after which every reader of it answers that the web app HAS no
//!    customization, while the web app itself is untouched. A later update brings the
//!    object back carrying only the members THAT update named — nothing survives the
//!    delete.
//! 2. The delete is `@idempotent`: it states the end state, so a repeat of it — and a
//!    delete of a web app no update ever customized — is the same empty 200.
//! 3. A customization delete for a `WebAppId` the store does not hold answers this
//!    service's web-app not-found refusal and changes nothing.
//! 4. A delete removes the customization of the web app the request named and of no
//!    other.
//!
//! Each rule is asserted over a TABLE whose rows differ along what it generalises over:
//! which members were assigned before the delete, each member's size (the modelled
//! minimum, a mid value and the modelled maximum), whether a file's bytes are text or
//! arbitrary binary, how many updates and deletes have run and in which order, how many
//! web apps the store holds, and which `WebAppId` the request carries.

use serde_json::{json, Value};

use crate::harness::{assert_members, is_modelled_web_app_id, member_of, Answer, Emulator, ROLE};

/// An IAM Identity Center instance ARN satisfying `IdentityCenterInstanceArn`'s
/// modelled pattern `^arn:[\w-]+:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$`.
const INSTANCE_ARN: &str = "arn:aws:sso:::instance/ssoins-1234567890abcdef";

/// `WebAppTitle` is modelled `len 0..100`, `WebAppLogoFile` `len 1..51200` and
/// `WebAppFaviconFile` `len 1..20960`
/// (`smithy_query(query="operation",operation="UpdateWebAppCustomization")`).
const TITLE_MAX: usize = 100;
const LOGO_MAX: usize = 51200;
const FAVICON_MIN: usize = 1;

/// Syntactically valid `WebAppId`s (`len 24..24`, `^webapp-[0-9a-f]{17}$`) that no
/// `CreateWebApp` ever minted: the low end of the id space, a mid value and the high
/// end.
const NEVER_MINTED: [&str; 3] = [
    "webapp-00000000000000000",
    "webapp-0000000000000beef",
    "webapp-fffffffffffffffff",
];

/// This service answers `ResourceNotFoundException` with HTTP 400 rather than the
/// shape's `@httpError(404)` — `RESOURCE_NOT_FOUND_HTTP_STATUS` in `src/state.rs`, which
/// records the recorded wire status every other suite in this crate asserts too.
const NOT_FOUND_STATUS: u16 = 400;

/// A blob member as it travels on awsJson: base64 of the bytes.
fn blob(bytes: &[u8]) -> Value {
    Value::String(aws_smithy_types::base64::encode(bytes))
}

/// `len` bytes spanning every byte value, so a file's content is arbitrary binary
/// rather than text.
fn binary(len: usize) -> Vec<u8> {
    (0..len).map(|index| (index % 256) as u8).collect()
}

/// Create one web app and hand back its minted `WebAppId`.
async fn create_web_app(emulator: &Emulator, case: &str) -> String {
    let answer = emulator
        .call(
            "CreateWebApp",
            json!({"IdentityProviderDetails": {
                "IdentityCenterConfig": {"InstanceArn": INSTANCE_ARN, "Role": ROLE},
            }}),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected CreateWebApp 200, got {} with body {}",
        answer.status, answer.body
    );
    let id = member_of(&answer.body, "WebAppId")
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: CreateWebApp returned no WebAppId: {}",
                answer.body
            )
        })
        .to_string();
    assert!(
        is_modelled_web_app_id(&id),
        "case {case:?}: expected a WebAppId matching ^webapp-[0-9a-f]{{17}}$, got {id:?}"
    );
    id
}

/// Assign `members` to `web_app_id`'s customization and assert the update succeeded.
async fn update_customization(emulator: &Emulator, case: &str, web_app_id: &str, members: &Value) {
    let mut request = members.clone();
    request["WebAppId"] = json!(web_app_id);
    let answer = emulator.call("UpdateWebAppCustomization", request).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected UpdateWebAppCustomization 200, got {} with body {}",
        answer.status, answer.body
    );
    assert_members(case, &answer.body, &json!({"WebAppId": web_app_id}));
}

/// Delete `web_app_id`'s customization and assert the modelled success: HTTP 200 with an
/// EMPTY body, since the operation's output shape is `Unit`.
async fn delete_customization(emulator: &Emulator, case: &str, web_app_id: &str) {
    let answer = emulator
        .call("DeleteWebAppCustomization", json!({"WebAppId": web_app_id}))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected DeleteWebAppCustomization 200, got {} with body {}",
        answer.status, answer.body
    );
    let members: Vec<&String> = answer
        .body
        .as_object()
        .map(|object| object.keys().collect())
        .unwrap_or_default();
    assert!(
        members.is_empty(),
        "case {case:?}: DeleteWebAppCustomization models an empty output, got the members \
         {members:?}"
    );
}

/// The `WebAppCustomization` structure `DescribeWebAppCustomization` reports for a web
/// app that HAS one.
async fn describe_customization(emulator: &Emulator, case: &str, web_app_id: &str) -> Value {
    let answer = emulator
        .call("DescribeWebAppCustomization", json!({"WebAppId": web_app_id}))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected DescribeWebAppCustomization 200, got {} with body {}",
        answer.status, answer.body
    );
    member_of(&answer.body, "WebAppCustomization").clone()
}

/// Assert that `web_app_id` HAS no customization: the read is refused with
/// `ResourceNotFoundException` whose `Message` names the web app the request addressed.
/// Returns the message, so a caller can compare it across web apps.
async fn assert_has_no_customization(emulator: &Emulator, case: &str, web_app_id: &str) -> String {
    let answer = emulator
        .call("DescribeWebAppCustomization", json!({"WebAppId": web_app_id}))
        .await;
    assert_eq!(
        answer.status, NOT_FOUND_STATUS,
        "case {case:?}: expected HTTP {NOT_FOUND_STATUS} for a web app with no customization, \
         got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got body {}",
        answer.body
    );
    assert_members(
        case,
        &answer.body,
        &json!({
            "Resource": web_app_id,
            "ResourceType": "WebApp",
            "WebAppCustomization": null,
        }),
    );
    let message = member_of(&answer.body, "Message")
        .as_str()
        .unwrap_or_default()
        .to_string();
    // The recorded wording for this condition, whose one variable part is the WebAppId
    // the REQUEST carried — so two web apps with no customization are told apart by it.
    assert_eq!(
        message,
        format!("The WebApp {web_app_id} doesn't have customization"),
        "case {case:?}: unexpected Message for a web app with no customization, got {message:?}"
    );
    message
}

/// Assert the web app ITSELF is still stored and unchanged in identity: a customization
/// delete removes the customization, never the web app.
async fn assert_web_app_alive(emulator: &Emulator, case: &str, web_app_id: &str) {
    let answer = emulator
        .call("DescribeWebApp", json!({"WebAppId": web_app_id}))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected the web app to survive its customization's delete, got \
         DescribeWebApp {} with body {}",
        answer.status, answer.body
    );
    assert_members(
        case,
        member_of(&answer.body, "WebApp"),
        &json!({"WebAppId": web_app_id}),
    );
}

/// Assert `answer` is this service's generic web-app not-found refusal for `web_app_id`.
/// Returns the message so a caller can compare it across rows.
fn assert_unknown_web_app(case: &str, answer: &Answer, web_app_id: &str) -> String {
    assert_eq!(
        answer.status, NOT_FOUND_STATUS,
        "case {case:?}: expected HTTP {NOT_FOUND_STATUS}, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got body {}",
        answer.body
    );
    assert_members(
        case,
        &answer.body,
        &json!({"Resource": web_app_id, "ResourceType": "WebApp"}),
    );
    let message = member_of(&answer.body, "Message")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !message.is_empty() && !message.contains(web_app_id),
        "case {case:?}: expected a non-empty fixed Message carrying no identifier, got \
         {message:?}"
    );
    message
}

// ---------------------------------------------------------------------------
// Rule 1: the delete removes the whole object; a later update rebuilds it.
// ---------------------------------------------------------------------------

/// RULE: whatever an update assigned, the delete removes the WHOLE customization object
/// — the web app then has none, and an update that follows the delete re-creates the
/// object carrying only the members IT named.
///
/// The rows differ along which of the three members were assigned before the delete,
/// their sizes (the modelled minimum, a mid value and the modelled maximum), whether a
/// file's bytes are text or arbitrary binary, and how many updates preceded the delete.
#[tokio::test]
async fn a_delete_removes_the_whole_customization_and_a_later_update_rebuilds_it() {
    let emulator = Emulator::new();

    // (case, the updates to run before the delete, the one member the update AFTER the
    // delete assigns)
    let cases: Vec<(&str, Vec<Value>, Value)> = vec![
        (
            "a title alone",
            vec![json!({"Title": "Partner files"})],
            json!({"Title": "Rebuilt"}),
        ),
        (
            "the empty title the modelled len 0 minimum admits",
            vec![json!({"Title": ""})],
            json!({"Title": "Rebuilt"}),
        ),
        (
            "a title at the modelled len 100 maximum",
            vec![json!({"Title": "t".repeat(TITLE_MAX)})],
            json!({"LogoFile": blob(b"<svg role=\"img\"/>")}),
        ),
        (
            "a logo of text bytes",
            vec![json!({"LogoFile": blob(b"<svg role=\"img\"/>")})],
            json!({"Title": "Rebuilt"}),
        ),
        (
            "a logo at the modelled 51200-byte maximum, arbitrary binary",
            vec![json!({"LogoFile": blob(&binary(LOGO_MAX))})],
            json!({"FaviconFile": blob(&binary(16))}),
        ),
        (
            "an icon of the modelled 1-byte minimum",
            vec![json!({"FaviconFile": blob(&binary(FAVICON_MIN))})],
            json!({"Title": ""}),
        ),
        (
            "all three members at once",
            vec![json!({
                "Title": "Everything",
                "LogoFile": blob(&binary(4096)),
                "FaviconFile": blob(&binary(1024)),
            })],
            json!({"Title": "Rebuilt"}),
        ),
        (
            "three partial updates accumulated before the delete",
            vec![
                json!({"Title": "One"}),
                json!({"LogoFile": blob(&binary(64))}),
                json!({"FaviconFile": blob(&binary(32))}),
            ],
            json!({"LogoFile": blob(&binary(128))}),
        ),
    ];

    for (case, updates, rebuild) in cases {
        let web_app_id = create_web_app(&emulator, case).await;
        for members in &updates {
            update_customization(&emulator, case, &web_app_id, members).await;
        }
        // The object is there to be deleted: the read answers it before the delete.
        let before = describe_customization(&emulator, case, &web_app_id).await;
        assert!(
            ["Title", "LogoFile", "FaviconFile"]
                .iter()
                .any(|member| !member_of(&before, member).is_null()),
            "case {case:?}: expected the updates to have assigned something, got {before:?}"
        );

        delete_customization(&emulator, case, &web_app_id).await;
        assert_has_no_customization(&emulator, case, &web_app_id).await;
        // The customization went; the web app did not.
        assert_web_app_alive(&emulator, case, &web_app_id).await;

        // An update after the delete rebuilds the object from NOTHING: the members it
        // does not name are unset, however many of them earlier updates had assigned.
        update_customization(&emulator, case, &web_app_id, &rebuild).await;
        let rebuilt = describe_customization(&emulator, case, &web_app_id).await;
        for member in ["Title", "LogoFile", "FaviconFile"] {
            let sent = member_of(&rebuild, member);
            let got = member_of(&rebuilt, member);
            if sent.is_null() {
                assert!(
                    got.is_null(),
                    "case {case:?}: the delete removed {member}, so a rebuild that did not \
                     assign it must report none, got {got:?}"
                );
            } else {
                assert_eq!(
                    got, sent,
                    "case {case:?}: expected the rebuilt {member} to be the one just assigned, \
                     got {got:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: @idempotent — the delete states the end state.
// ---------------------------------------------------------------------------

/// RULE: `DeleteWebAppCustomization` is `@idempotent`, so it answers the same empty 200
/// whatever customization the web app held when it arrived — including none at all —
/// and the end state is always "this web app has no customization".
///
/// The rows differ along what the web app held when the delete arrived (nothing ever
/// assigned, one member, all three), how many deletes run in a row (one, two, three),
/// and whether an update ran between two of them.
#[tokio::test]
async fn the_delete_is_idempotent_and_needs_no_customization_to_succeed() {
    let emulator = Emulator::new();

    // (case, the members assigned before the first delete or None for a web app no
    // update ever named, how many deletes to run, whether an update runs between them)
    let cases: Vec<(&str, Option<Value>, usize, bool)> = vec![
        ("a web app no update ever customized", None, 1, false),
        ("a web app no update ever customized, deleted twice", None, 2, false),
        ("a customized web app, deleted once", Some(json!({"Title": "One"})), 1, false),
        (
            "a customized web app, deleted twice in a row",
            Some(json!({"Title": "Two", "LogoFile": blob(&binary(48))})),
            2,
            false,
        ),
        (
            "a customized web app, deleted three times in a row",
            Some(json!({
                "Title": "Three",
                "LogoFile": blob(&binary(16)),
                "FaviconFile": blob(&binary(8)),
            })),
            3,
            false,
        ),
        (
            "a customized web app, re-customized between two deletes",
            Some(json!({"FaviconFile": blob(&binary(24))})),
            2,
            true,
        ),
    ];

    for (case, assigned, deletes, update_between) in cases {
        let web_app_id = create_web_app(&emulator, case).await;
        if let Some(members) = &assigned {
            update_customization(&emulator, case, &web_app_id, members).await;
        } else {
            // Nothing was ever assigned, so the web app has no customization already.
            assert_has_no_customization(&emulator, case, &web_app_id).await;
        }

        for round in 0..deletes {
            if round > 0 && update_between {
                update_customization(
                    &emulator,
                    case,
                    &web_app_id,
                    &json!({"Title": "between the deletes"}),
                )
                .await;
            }
            delete_customization(&emulator, case, &web_app_id).await;
            // The end state the delete states, after every one of its repeats.
            assert_has_no_customization(&emulator, case, &web_app_id).await;
        }

        assert_web_app_alive(&emulator, case, &web_app_id).await;
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a WebAppId the store does not hold.
// ---------------------------------------------------------------------------

/// RULE: a customization delete addressed at a `WebAppId` the store does not hold — one
/// never minted, or one `DeleteWebApp` removed — answers this service's web-app
/// not-found refusal (the capitalized `ResourceType`, the identifier in `Resource`
/// alone, one fixed `Message` carrying none) and leaves every stored customization
/// alone.
///
/// The rows differ along the id: the low end of the id space, a mid value, the high end,
/// and a web app that really existed and was deleted.
#[tokio::test]
async fn a_customization_delete_for_a_web_app_the_store_does_not_hold_is_refused() {
    let emulator = Emulator::new();

    // A store that DOES hold a customized web app, so each refusal is about the id
    // rather than about emptiness.
    let live = create_web_app(&emulator, "a live web app").await;
    update_customization(
        &emulator,
        "a live web app",
        &live,
        &json!({"Title": "Live", "LogoFile": blob(&binary(32))}),
    )
    .await;

    // A web app that existed and no longer does: its id is as unresolvable as one that
    // was never minted.
    let deleted = create_web_app(&emulator, "a deleted web app").await;
    update_customization(
        &emulator,
        "a deleted web app",
        &deleted,
        &json!({"Title": "Doomed"}),
    )
    .await;
    let gone = emulator
        .call("DeleteWebApp", json!({"WebAppId": &deleted}))
        .await;
    assert_eq!(
        gone.status, 200,
        "expected DeleteWebApp 200, got {} with body {}",
        gone.status, gone.body
    );

    let mut absent_ids: Vec<String> = NEVER_MINTED.iter().map(|id| (*id).to_string()).collect();
    absent_ids.push(deleted);

    let mut messages: Vec<String> = Vec::new();
    for absent in &absent_ids {
        assert!(
            is_modelled_web_app_id(absent),
            "the probe id {absent:?} must satisfy WebAppId's modelled pattern"
        );
        let case = format!("DeleteWebAppCustomization for {absent}");
        let answer = emulator
            .call("DeleteWebAppCustomization", json!({"WebAppId": absent}))
            .await;
        messages.push(assert_unknown_web_app(&case, &answer, absent));
    }

    for message in &messages {
        assert_eq!(
            message, &messages[0],
            "expected one fixed WebApp not-found message, got {message:?} beside {:?}",
            messages[0]
        );
    }

    // The refused deletes touched nothing: the live web app keeps its customization.
    let customization = describe_customization(&emulator, "a live web app", &live).await;
    assert_members(
        "a live web app",
        &customization,
        &json!({"Title": "Live", "LogoFile": blob(&binary(32)), "FaviconFile": null}),
    );
}

// ---------------------------------------------------------------------------
// Rule 4: a delete removes one web app's customization and no other's.
// ---------------------------------------------------------------------------

/// RULE: the delete removes the customization of the web app the request named and of no
/// other — so the refusal a reader answers names the web app that has none, and every
/// other web app still reports exactly what its own updates assigned.
///
/// The rows differ along how many web apps the store holds, which of them is deleted and
/// in which order, and which members each one had assigned.
#[tokio::test]
async fn a_delete_removes_the_customization_of_the_named_web_app_alone() {
    let emulator = Emulator::new();

    let assignments: Vec<(&str, Value)> = vec![
        (
            "the first web app",
            json!({"Title": "First", "LogoFile": blob(b"first-logo")}),
        ),
        (
            "the second web app",
            json!({"Title": "Second", "FaviconFile": blob(b"second-icon")}),
        ),
        (
            "the third web app",
            json!({
                "Title": "Third",
                "LogoFile": blob(&binary(256)),
                "FaviconFile": blob(&binary(128)),
            }),
        ),
    ];

    let mut web_apps: Vec<(&str, String, Value)> = Vec::new();
    for (case, members) in assignments {
        let web_app_id = create_web_app(&emulator, case).await;
        update_customization(&emulator, case, &web_app_id, &members).await;
        web_apps.push((case, web_app_id, members));
    }

    // Delete in an order that is not the order the web apps were created in, so a
    // handler that removed "the last one written" fails the first row.
    let mut messages: Vec<(String, String)> = Vec::new();
    for deleted_index in [1usize, 0, 2] {
        let (case, web_app_id, _) = &web_apps[deleted_index];
        delete_customization(&emulator, case, web_app_id).await;
        let message = assert_has_no_customization(&emulator, case, web_app_id).await;
        messages.push((web_app_id.clone(), message));

        // Every web app whose customization has NOT been deleted yet still reports what
        // its own updates assigned.
        for (other_case, other_id, members) in &web_apps {
            if other_id == web_app_id || messages.iter().any(|(id, _)| id == other_id) {
                continue;
            }
            let customization = describe_customization(&emulator, other_case, other_id).await;
            let mut expected = members.clone();
            for member in ["Title", "LogoFile", "FaviconFile"] {
                if member_of(members, member).is_null() {
                    expected[member] = Value::Null;
                }
            }
            assert_members(other_case, &customization, &expected);
        }
    }

    // The refusal's one variable part is the WebAppId the request carried, so no two of
    // these messages read alike.
    for (index, (web_app_id, message)) in messages.iter().enumerate() {
        for (other_id, other_message) in messages.iter().skip(index + 1) {
            assert_ne!(
                message, other_message,
                "expected the message for {web_app_id} and the one for {other_id} to name their \
                 own web app, got {message:?} twice"
            );
        }
    }
}
