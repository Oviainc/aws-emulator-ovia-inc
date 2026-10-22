// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a Transfer Family web app's customization — the page title,
//! logo file and icon file `UpdateWebAppCustomization` assigns and
//! `DescribeWebAppCustomization` reports.
//!
//! Every test drives the real wire path through the shared [`crate::harness`] — `POST /`
//! with `X-Amz-Target: TransferService.<Op>` — so what is asserted is what an SDK sees,
//! not what a handler returns. Blob members travel base64-encoded on awsJson, so each
//! file is encoded with `aws_smithy_types::base64`, the codec the wire serializer uses.
//!
//! RULES under test:
//!
//! 1. `UpdateWebAppCustomization` assigns the customization members the request CARRIES
//!    to the addressed web app and leaves the ones it omits exactly as they stand, and
//!    `DescribeWebAppCustomization` reports them back under the web app's own ARN and
//!    id, reporting only the members some update assigned.
//! 2. A customization belongs to the web app it was assigned to and to no other.
//! 3. A `WebAppId` the store does not hold is answered with this service's not-found
//!    refusal, by the update as well as by the read.
//! 4. A value outside a member's MODELLED bound is refused before anything is
//!    persisted, so a refused update assigns nothing.
//!
//! Each rule is asserted over a TABLE whose rows differ along what it generalises over:
//! which members the request carries, each member's size (the modelled minimum, a mid
//! value, the modelled maximum, and one byte/character past it), whether a file's bytes
//! are text or arbitrary binary, and the order several updates arrive in.

use serde_json::{json, Value};

use crate::harness::{assert_members, is_modelled_web_app_id, member_of, Emulator, ACCOUNT, REGION, ROLE};

/// An IAM Identity Center instance ARN satisfying `IdentityCenterInstanceArn`'s
/// modelled pattern `^arn:[\w-]+:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$`.
const INSTANCE_ARN: &str = "arn:aws:sso:::instance/ssoins-1234567890abcdef";

/// `WebAppTitle` is modelled `len 0..100`
/// (`smithy_query(query="shape",shape="WebAppTitle")`).
const TITLE_MAX: usize = 100;

/// `WebAppLogoFile` is modelled `len 1..51200`
/// (`smithy_query(query="shape",shape="WebAppLogoFile")`).
const LOGO_MIN: usize = 1;
const LOGO_MAX: usize = 51200;

/// `WebAppFaviconFile` is modelled `len 1..20960`
/// (`smithy_query(query="operation",operation="UpdateWebAppCustomization")`).
const FAVICON_MIN: usize = 1;
const FAVICON_MAX: usize = 20960;

/// Syntactically valid `WebAppId`s (`len 24..24`, `^webapp-[0-9a-f]{17}$`) that no
/// `CreateWebApp` ever minted: the low end of the id space, a mid value and the high
/// end.
const NEVER_MINTED: [&str; 3] = [
    "webapp-00000000000000000",
    "webapp-00000000000000bad",
    "webapp-fffffffffffffffff",
];

/// A blob member as it travels on awsJson: base64 of the bytes.
fn blob(bytes: &[u8]) -> Value {
    Value::String(aws_smithy_types::base64::encode(bytes))
}

/// `len` bytes spanning every byte value, so a file's content is arbitrary binary
/// rather than text: a handler or codec that assumed UTF-8 cannot round-trip it.
fn binary(len: usize) -> Vec<u8> {
    (0..len).map(|index| (index % 256) as u8).collect()
}

/// Create one web app and hand back its `WebAppId`.
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
        .unwrap_or_else(|| panic!("case {case:?}: CreateWebApp returned no WebAppId: {}", answer.body))
        .to_string();
    assert!(
        is_modelled_web_app_id(&id),
        "case {case:?}: expected a WebAppId matching ^webapp-[0-9a-f]{{17}}$, got {id:?}"
    );
    id
}

/// Send `members` as an `UpdateWebAppCustomization` for `web_app_id`, assert the update
/// succeeded and answered that same id, and assert it echoed no customization back
/// (`UpdateWebAppCustomizationResponse` models `WebAppId` alone).
async fn update_customization(emulator: &Emulator, case: &str, web_app_id: &str, members: &Value) {
    let mut request = members.clone();
    request["WebAppId"] = json!(web_app_id);
    let answer = emulator.call("UpdateWebAppCustomization", request).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected UpdateWebAppCustomization 200, got {} with body {}",
        answer.status, answer.body
    );
    assert_members(
        case,
        &answer.body,
        &json!({"WebAppId": web_app_id, "Title": null, "LogoFile": null, "FaviconFile": null}),
    );
}

/// Describe `web_app_id`'s customization and hand back the `WebAppCustomization`
/// structure, having checked the two identifiers every customization carries: the web
/// app's own ARN (`Arn` is documented as "the ARN for the web app") and its id.
async fn describe_customization(emulator: &Emulator, case: &str, web_app_id: &str) -> Value {
    let answer = emulator
        .call("DescribeWebAppCustomization", json!({"WebAppId": web_app_id}))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected DescribeWebAppCustomization 200, got {} with body {}",
        answer.status, answer.body
    );
    let customization = member_of(&answer.body, "WebAppCustomization").clone();
    assert_members(
        case,
        &customization,
        &json!({
            "WebAppId": web_app_id,
            "Arn": format!("arn:aws:transfer:{REGION}:{ACCOUNT}:webapp/{web_app_id}"),
        }),
    );
    customization
}

// ---------------------------------------------------------------------------
// Rule 1: the update assigns, the read reports — over the whole input class.
// ---------------------------------------------------------------------------

/// RULE: every customization member an update carries is persisted as given and read
/// back byte for byte, and a member no update assigned is reported by no reader.
///
/// The rows differ along which of the three members the request carries, each one's
/// size — the modelled minimum, a mid value and the modelled MAXIMUM of every bounded
/// member — and whether a file's bytes are text or arbitrary binary. A fresh web app
/// per row, so each row also asserts what an UNcustomized web app reports for the
/// members it did not assign.
#[tokio::test]
async fn an_update_persists_every_customization_member_and_the_read_rebuilds_it() {
    let emulator = Emulator::new();

    let cases: Vec<(&str, Value)> = vec![
        ("a title alone", json!({"Title": "Partner files"})),
        (
            "the empty title the modelled len 0 minimum admits",
            json!({"Title": ""}),
        ),
        (
            "a title at the modelled len 100 maximum",
            json!({"Title": "t".repeat(TITLE_MAX)}),
        ),
        (
            "a title of non-ASCII characters, whose length counts characters",
            json!({"Title": "Übersicht – Dateien ✓".repeat(2)}),
        ),
        (
            "a logo of the modelled 1-byte minimum",
            json!({"LogoFile": blob(&binary(LOGO_MIN))}),
        ),
        (
            "a logo of text bytes",
            json!({"LogoFile": blob(b"<svg role=\"img\"/>")}),
        ),
        (
            "a logo at the modelled 51200-byte maximum, arbitrary binary",
            json!({"LogoFile": blob(&binary(LOGO_MAX))}),
        ),
        (
            "an icon of the modelled 1-byte minimum",
            json!({"FaviconFile": blob(&binary(FAVICON_MIN))}),
        ),
        (
            "an icon at the modelled 20960-byte maximum, arbitrary binary",
            json!({"FaviconFile": blob(&binary(FAVICON_MAX))}),
        ),
        (
            "all three members at once",
            json!({
                "Title": "Everything",
                "LogoFile": blob(&binary(4096)),
                "FaviconFile": blob(&binary(1024)),
            }),
        ),
        (
            "two files whose bytes are identical, so neither can be read from the other",
            json!({
                "LogoFile": blob(&binary(300)),
                "FaviconFile": blob(&binary(300)),
            }),
        ),
    ];

    for (case, members) in cases {
        let web_app_id = create_web_app(&emulator, case).await;

        // Before any update: a web app nothing customized reports its identifiers and
        // none of the three customization members.
        let fresh = describe_customization(&emulator, case, &web_app_id).await;
        assert_members(
            case,
            &fresh,
            &json!({"Title": null, "LogoFile": null, "FaviconFile": null}),
        );

        update_customization(&emulator, case, &web_app_id, &members).await;
        let customization = describe_customization(&emulator, case, &web_app_id).await;

        for member in ["Title", "LogoFile", "FaviconFile"] {
            let sent = member_of(&members, member);
            let got = member_of(&customization, member);
            if sent.is_null() {
                assert!(
                    got.is_null(),
                    "case {case:?}: {member} was never assigned, so no reader may report it, \
                     got {got:?}"
                );
            } else {
                assert_eq!(
                    got, sent,
                    "case {case:?}: expected {member} to round trip as sent, got {got:?}"
                );
            }
        }

        // A description is a READ: a second one answers exactly the same thing.
        let again = describe_customization(&emulator, case, &web_app_id).await;
        assert_eq!(
            again, customization,
            "case {case:?}: expected a repeated DescribeWebAppCustomization to answer the same \
             customization, got {again:?} after {customization:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 1, the partial half: an omitted member is "not modified".
// ---------------------------------------------------------------------------

/// RULE: an update assigns the members it carries and leaves every member it omits
/// exactly as it stands — so a sequence of updates accumulates, each member holding
/// what the LAST update that carried it assigned.
///
/// The rows are one sequence over a single web app, differing along which member each
/// step carries (one, two, all three, none at all) and whether it is the first
/// assignment of that member or a replacement of an earlier one. The expectation is
/// the running state of all three members, so a handler that cleared the omitted ones
/// fails at the first step that carries fewer than three.
#[tokio::test]
async fn an_update_assigns_the_members_it_carries_and_leaves_the_others_as_they_stand() {
    let emulator = Emulator::new();
    let case = "a sequence of partial updates";
    let web_app_id = create_web_app(&emulator, case).await;

    let first_logo = blob(&binary(64));
    let second_logo = blob(&binary(128));
    let icon = blob(&binary(32));

    // (step, the members this step sends, the state all three must hold afterwards)
    let steps: Vec<(&str, Value, Value)> = vec![
        (
            "the title first, nothing else assigned yet",
            json!({"Title": "One"}),
            json!({"Title": "One", "LogoFile": null, "FaviconFile": null}),
        ),
        (
            "a logo, with the title left alone",
            json!({"LogoFile": first_logo.clone()}),
            json!({"Title": "One", "LogoFile": first_logo.clone(), "FaviconFile": null}),
        ),
        (
            "an icon, with the title and logo left alone",
            json!({"FaviconFile": icon.clone()}),
            json!({"Title": "One", "LogoFile": first_logo.clone(), "FaviconFile": icon.clone()}),
        ),
        (
            "a replacement title, with both files left alone",
            json!({"Title": "Two"}),
            json!({"Title": "Two", "LogoFile": first_logo.clone(), "FaviconFile": icon.clone()}),
        ),
        (
            "a replacement logo, with the icon left alone",
            json!({"LogoFile": second_logo.clone()}),
            json!({"Title": "Two", "LogoFile": second_logo.clone(), "FaviconFile": icon.clone()}),
        ),
        (
            "the title cleared to the empty one the modelled len 0 minimum admits",
            json!({"Title": ""}),
            json!({"Title": "", "LogoFile": second_logo.clone(), "FaviconFile": icon.clone()}),
        ),
        (
            "an update carrying no customization member at all assigns nothing",
            json!({}),
            json!({"Title": "", "LogoFile": second_logo.clone(), "FaviconFile": icon.clone()}),
        ),
        (
            "all three at once",
            json!({"Title": "Three", "LogoFile": first_logo.clone(), "FaviconFile": second_logo.clone()}),
            json!({"Title": "Three", "LogoFile": first_logo.clone(), "FaviconFile": second_logo.clone()}),
        ),
    ];

    for (step, members, expected) in steps {
        update_customization(&emulator, step, &web_app_id, &members).await;
        let customization = describe_customization(&emulator, step, &web_app_id).await;
        assert_members(step, &customization, &expected);
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a customization belongs to one web app.
// ---------------------------------------------------------------------------

/// RULE: a customization is assigned to the web app the request named and to no other
/// — so one web app's title, logo and icon are invisible to every other web app,
/// however many updates have run.
///
/// The rows differ along how many web apps are in the store and which of them is
/// customized: two customized web apps plus one left untouched, each with its own
/// values.
#[tokio::test]
async fn a_customization_belongs_to_the_web_app_it_was_assigned_to() {
    let emulator = Emulator::new();

    let customized: Vec<(&str, Value)> = vec![
        (
            "the first web app",
            json!({"Title": "First", "LogoFile": blob(b"first-logo")}),
        ),
        (
            "the second web app",
            json!({"Title": "Second", "FaviconFile": blob(b"second-icon")}),
        ),
    ];

    let mut assigned: Vec<(&str, String, Value)> = Vec::new();
    for (case, members) in customized {
        let web_app_id = create_web_app(&emulator, case).await;
        update_customization(&emulator, case, &web_app_id, &members).await;
        assigned.push((case, web_app_id, members));
    }

    // A web app no update ever named, created after the others were customized.
    let untouched_case = "a web app nothing customized";
    let untouched = create_web_app(&emulator, untouched_case).await;

    for (case, web_app_id, members) in &assigned {
        let customization = describe_customization(&emulator, case, web_app_id).await;
        let mut expected = members.clone();
        for member in ["Title", "LogoFile", "FaviconFile"] {
            if member_of(members, member).is_null() {
                expected[member] = Value::Null;
            }
        }
        assert_members(case, &customization, &expected);
    }

    let customization = describe_customization(&emulator, untouched_case, &untouched).await;
    assert_members(
        untouched_case,
        &customization,
        &json!({"Title": null, "LogoFile": null, "FaviconFile": null}),
    );
}

// ---------------------------------------------------------------------------
// Rule 3: a web app the store does not hold.
// ---------------------------------------------------------------------------

/// RULE: a customization call for a `WebAppId` the store does not hold answers this
/// service's not-found refusal — HTTP 400 `ResourceNotFoundException` with the
/// capitalized `ResourceType`, the requested identifier in `Resource` alone, and a
/// fixed generic `Message` carrying no identifier — and the update persists nothing.
///
/// The rows differ along the operation (the write and the read) and along the id: the
/// low end of the id space, a mid value and the high end.
#[tokio::test]
async fn a_customization_call_for_a_web_app_that_was_never_created_answers_the_generic_404() {
    let emulator = Emulator::new();

    // A store that DOES hold a customized web app, so each refusal is about the id
    // rather than about emptiness.
    let live = create_web_app(&emulator, "a live web app").await;
    update_customization(&emulator, "a live web app", &live, &json!({"Title": "Live"})).await;

    let mut messages: Vec<String> = Vec::new();

    for absent in NEVER_MINTED {
        assert!(
            is_modelled_web_app_id(absent),
            "the probe id {absent:?} must satisfy WebAppId's modelled pattern"
        );
        let calls: Vec<(&str, Value)> = vec![
            (
                "UpdateWebAppCustomization",
                json!({"WebAppId": absent, "Title": "nowhere", "LogoFile": blob(b"logo")}),
            ),
            ("DescribeWebAppCustomization", json!({"WebAppId": absent})),
        ];
        for (op, request) in calls {
            let case = format!("{op} for {absent}");
            let answer = emulator.call(op, request).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {}",
                answer.body
            );
            assert_members(
                &case,
                &answer.body,
                &json!({"Resource": absent, "ResourceType": "WebApp", "WebAppId": null}),
            );
            let message = member_of(&answer.body, "Message")
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                !message.is_empty() && !message.contains(absent),
                "case {case:?}: expected a non-empty Message carrying no identifier, \
                 got {message:?}"
            );
            messages.push(message);
        }
    }

    for message in &messages {
        assert_eq!(
            message, &messages[0],
            "expected one fixed WebApp not-found message, got {message:?} beside {:?}",
            messages[0]
        );
    }

    // The refused calls left the live web app's own customization alone.
    let customization = describe_customization(&emulator, "a live web app", &live).await;
    assert_members(
        "a live web app",
        &customization,
        &json!({"Title": "Live", "LogoFile": null, "FaviconFile": null}),
    );
}

// ---------------------------------------------------------------------------
// Rule 4: a value outside a modelled bound assigns nothing.
// ---------------------------------------------------------------------------

/// RULE: a customization member outside its MODELLED bound is refused with HTTP 400
/// `ValidationException` before anything is persisted, so the web app keeps the
/// customization it already had.
///
/// The rows are each bounded member one step past each of its published bounds —
/// `WebAppTitle` `len 0..100`, `WebAppLogoFile` `len 1..51200` and `WebAppFaviconFile`
/// `len 1..20960`, from `smithy_query(query="operation",
/// operation="UpdateWebAppCustomization")` — with a legal member beside the illegal one
/// in some rows, so a refusal cannot be mistaken for "nothing was sent". The bounds
/// themselves are accepted and round-tripped by
/// [`an_update_persists_every_customization_member_and_the_read_rebuilds_it`].
#[tokio::test]
async fn a_customization_member_outside_its_modelled_bound_assigns_nothing() {
    let emulator = Emulator::new();
    let case = "a customized web app";
    let web_app_id = create_web_app(&emulator, case).await;

    let logo = blob(&binary(16));
    let assigned = json!({"Title": "Kept", "LogoFile": logo.clone(), "FaviconFile": null});
    update_customization(
        &emulator,
        case,
        &web_app_id,
        &json!({"Title": "Kept", "LogoFile": logo.clone()}),
    )
    .await;

    let refused: Vec<(&str, Value)> = vec![
        (
            "a title one character past the modelled 100 maximum",
            json!({"Title": "t".repeat(TITLE_MAX + 1)}),
        ),
        (
            "a logo one byte past the modelled 51200 maximum",
            json!({"LogoFile": blob(&binary(LOGO_MAX + 1))}),
        ),
        (
            "a logo one byte below the modelled 1-byte minimum",
            json!({"LogoFile": blob(&[])}),
        ),
        (
            "an icon one byte past the modelled 20960 maximum",
            json!({"FaviconFile": blob(&binary(FAVICON_MAX + 1))}),
        ),
        (
            "an icon one byte below the modelled 1-byte minimum",
            json!({"FaviconFile": blob(&[])}),
        ),
        (
            "a legal title beside an illegal icon",
            json!({"Title": "Replaced", "FaviconFile": blob(&[])}),
        ),
    ];

    for (refusal_case, members) in refused {
        let mut request = members.clone();
        request["WebAppId"] = json!(&web_app_id);
        let answer = emulator.call("UpdateWebAppCustomization", request).await;
        assert_eq!(
            answer.status, 400,
            "case {refusal_case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {refusal_case:?}: expected ValidationException, got {}",
            answer.body
        );

        // Nothing the refusal carried reached the store.
        let customization = describe_customization(&emulator, refusal_case, &web_app_id).await;
        assert_members(refusal_case, &customization, &assigned);
    }
}
