// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the input bounds `CreateWorkspace` enforces before it
//! writes, driven over HTTP through the same `MultiServiceRouter` the emulator
//! boots with, so every assertion is about the wire behaviour a real client sees.
//!
//! `workspace_lifecycle.rs` pins what an ACCEPTED create persists and reads back;
//! this suite pins the gate in front of that write:
//!
//! * a create whose member carries a value its own modelled `@length` or
//!   `@pattern` refuses is refused with the client-input error the operation
//!   models — `InvalidRequestException`, HTTP 400 — and stores no workspace, so
//!   the store holds exactly the workspaces the accepted requests made;
//! * a value at each published bound, and in each form the member's pattern
//!   admits, is accepted, persisted and reads back through `DescribeWorkspace`
//!   exactly as it was sent.
//!
//! The bounds are the model's, not the handler's: `workspaceName` (`WorkspaceName`,
//! len 1..64, pattern `^[a-zA-Z0-9_-]+$`), `workspaceDescription` (`Description`,
//! len 1..2048, pattern `^[^\u0000-\u001F\u007F]+$`) and `clientToken`
//! (`ClientToken`, len 36..64, pattern `^\S{36,64}$`), from
//! `smithy_query(query="operation", operation="CreateWorkspace")`. Each length is
//! walked at both of its published ends and one character past each; each pattern
//! is walked on a value spanning the character classes it admits and on one
//! carrying a character it excludes.
//!
//! The `tags` key bound, which the framework's constraint walk does not reach,
//! has its own cover in `tag_key_refusals.rs` and `resource_tags.rs`; this suite
//! pins no rule about it.
//!
//! The HTTP harness (`router_with_engine`, `error_code`) is the asset suite's
//! and the workspace routes (`create`, `describe`, `expected_arn`) are
//! `workspace_lifecycle.rs`'s, imported rather than copied so the suites cannot
//! drift on the wire form.

use serde_json::{json, Value};

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::service_state;

use crate::asset_lifecycle::{error_code, router_with_engine};
use crate::workspace_lifecycle::{create, default_encryption, describe, expected_arn};

/// The published ends of `WorkspaceName`'s `@length`.
const NAME_MIN: usize = 1;
/// See [`NAME_MIN`].
const NAME_MAX: usize = 64;

/// The published ends of `Description`'s `@length`.
const DESCRIPTION_MIN: usize = 1;
/// See [`DESCRIPTION_MIN`].
const DESCRIPTION_MAX: usize = 2048;

/// The published ends of `ClientToken`'s `@length`.
const TOKEN_MIN: usize = 36;
/// See [`TOKEN_MIN`].
const TOKEN_MAX: usize = 64;

/// A `clientToken` of `length` characters, every one of them a non-space the
/// `^\S{36,64}$` pattern admits, so only the LENGTH tells the rows apart.
fn token(length: usize) -> String {
    "t".repeat(length)
}

/// One row: a case label, the create body it sends, and whether the modelled
/// constraints admit it. An admitted row also states the `workspaceName` it
/// creates under and the `workspaceDescription` a read must report for it.
struct BoundCase {
    label: &'static str,
    body: Value,
    accepted: bool,
    /// The name of the workspace an accepted row creates, read back afterwards.
    name: Option<String>,
    /// The `workspaceDescription` a read of that workspace must carry — `null`
    /// for a row that sent none.
    expected_description: Value,
}

/// An accepted row.
fn accepted(
    label: &'static str,
    name: String,
    description: Option<String>,
    body: Value,
) -> BoundCase {
    BoundCase {
        label,
        body,
        accepted: true,
        name: Some(name),
        expected_description: description.map_or(Value::Null, |text| json!(text)),
    }
}

/// A refused row.
fn refused(label: &'static str, body: Value) -> BoundCase {
    BoundCase {
        label,
        body,
        accepted: false,
        name: None,
        expected_description: Value::Null,
    }
}

/// The create body for `name`, with the members `extra` adds folded in.
fn body(name: &str, extra: Value) -> Value {
    let mut request = json!({
        "workspaceName": name,
        "encryptionConfiguration": default_encryption(),
    });
    let object = request.as_object_mut().expect("an object body");
    for (member, value) in extra.as_object().expect("an object of extra members") {
        object.insert(member.clone(), value.clone());
    }
    request
}

/// The rows: one per end of each modelled bound, one past each end, and one per
/// pattern — inside it and outside it.
fn bound_cases() -> Vec<BoundCase> {
    let shortest_name = "a".repeat(NAME_MIN);
    let longest_name = "n".repeat(NAME_MAX);
    let shortest_description = "d".repeat(DESCRIPTION_MIN);
    let longest_description = "d".repeat(DESCRIPTION_MAX);
    // Every character class `^[a-zA-Z0-9_-]+$` admits, in one name.
    let patterned_name = "Plant_07-Line_2".to_string();
    // A description inside `^[^\u0000-\u001F\u007F]+$`: that pattern excludes the
    // C0 controls and DEL and nothing else, so spaces, punctuation and non-ASCII
    // are all admitted.
    let patterned_description = "Ligne nº7 — étage 2, «ouest»: 50 % de charge.".to_string();

    vec![
        accepted(
            "a name at the bottom of its length",
            shortest_name.clone(),
            None,
            body(&shortest_name, json!({})),
        ),
        accepted(
            "a name at the top of its length",
            longest_name.clone(),
            None,
            body(&longest_name, json!({})),
        ),
        accepted(
            "a name spanning every character class its pattern admits",
            patterned_name.clone(),
            None,
            body(&patterned_name, json!({})),
        ),
        refused(
            "a name one character past the top of its length",
            body(&"n".repeat(NAME_MAX + 1), json!({})),
        ),
        refused(
            "a name one character short of the bottom of its length",
            body(&"n".repeat(NAME_MIN - 1), json!({})),
        ),
        refused(
            "a name carrying a space, which its pattern excludes",
            body("plant floor", json!({})),
        ),
        refused(
            "a name carrying a dot, which its pattern excludes",
            body("plant.floor", json!({})),
        ),
        accepted(
            "a description at the bottom of its length",
            "described-at-the-floor".to_string(),
            Some(shortest_description.clone()),
            body(
                "described-at-the-floor",
                json!({ "workspaceDescription": shortest_description }),
            ),
        ),
        accepted(
            "a description at the top of its length",
            "described-at-the-ceiling".to_string(),
            Some(longest_description.clone()),
            body(
                "described-at-the-ceiling",
                json!({ "workspaceDescription": longest_description }),
            ),
        ),
        accepted(
            "a description of printable characters its pattern admits",
            "described-in-full".to_string(),
            Some(patterned_description.clone()),
            body(
                "described-in-full",
                json!({ "workspaceDescription": patterned_description }),
            ),
        ),
        refused(
            "a description one character past the top of its length",
            body(
                "described-past-the-ceiling",
                json!({ "workspaceDescription": "d".repeat(DESCRIPTION_MAX + 1) }),
            ),
        ),
        refused(
            "a description one character short of the bottom of its length",
            body(
                "described-below-the-floor",
                json!({ "workspaceDescription": "d".repeat(DESCRIPTION_MIN - 1) }),
            ),
        ),
        refused(
            "a description carrying a control character, which its pattern excludes",
            body(
                "described-with-a-tab",
                json!({ "workspaceDescription": "north\tline" }),
            ),
        ),
        refused(
            "a description carrying DEL, which its pattern excludes",
            body(
                "described-with-del",
                json!({ "workspaceDescription": "north\u{7f}line" }),
            ),
        ),
        accepted(
            "a client token at the bottom of its length",
            "token-at-the-floor".to_string(),
            None,
            body(
                "token-at-the-floor",
                json!({ "clientToken": token(TOKEN_MIN) }),
            ),
        ),
        accepted(
            "a client token at the top of its length",
            "token-at-the-ceiling".to_string(),
            None,
            body(
                "token-at-the-ceiling",
                json!({ "clientToken": token(TOKEN_MAX) }),
            ),
        ),
        refused(
            "a client token one character short of the bottom of its length",
            body(
                "token-below-the-floor",
                json!({ "clientToken": token(TOKEN_MIN - 1) }),
            ),
        ),
        refused(
            "a client token one character past the top of its length",
            body(
                "token-past-the-ceiling",
                json!({ "clientToken": token(TOKEN_MAX + 1) }),
            ),
        ),
        refused(
            "a client token carrying a space, which its pattern excludes",
            body(
                "token-with-a-space",
                json!({ "clientToken": format!("{} {}", token(17), token(18)) }),
            ),
        ),
    ]
}

#[tokio::test]
async fn a_create_outside_a_modelled_input_bound_is_refused_and_persists_nothing() {
    let (router, engine) = router_with_engine();

    let mut stored = 0usize;
    for case in bound_cases() {
        let (status, answer) = create(&router, case.body.clone()).await;

        if case.accepted {
            stored += 1;
            assert_eq!(
                status, 200,
                "case {:?}: a value inside the bound must be accepted, got {status} {answer}",
                case.label
            );
        } else {
            assert_eq!(
                (status, error_code(&answer)),
                (400, "InvalidRequestException"),
                "case {:?}: a value outside the bound must be refused with the modelled \
                 client-input error, got {status} {answer}",
                case.label
            );
        }

        // A refused create reaches no map: the store holds exactly the workspaces
        // the accepted rows made, counted on the one shared engine because the API
        // exposes no reader for the whole set yet.
        let state = service_state(&*engine).expect("the iotsitewise store is mounted");
        let held = state.lock().workspaces.len();
        assert_eq!(
            held, stored,
            "case {:?}: expected {stored} stored workspaces, got {held}",
            case.label
        );
    }
}

#[tokio::test]
async fn a_create_inside_every_modelled_input_bound_reads_back_as_it_was_sent() {
    let (router, _engine) = router_with_engine();

    for case in bound_cases().into_iter().filter(|case| case.accepted) {
        let name = case
            .name
            .clone()
            .expect("an accepted row names its workspace");
        let (status, created) = create(&router, case.body.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: create got {status} {created}",
            case.label
        );
        assert_eq!(
            created["workspaceName"],
            json!(name),
            "case {:?}: the create must report the name it was given, got {created}",
            case.label
        );
        assert_eq!(
            created["workspaceArn"],
            json!(expected_arn(&name)),
            "case {:?}: expected the workspace ARN {}, got {created}",
            case.label,
            expected_arn(&name)
        );

        let (status, read) = describe(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {:?}: describe got {status} {read}",
            case.label
        );
        assert_eq!(
            read["workspaceName"],
            json!(name),
            "case {:?}: expected the stored name, got {read}",
            case.label
        );
        match case.expected_description.as_str() {
            // A row that sent a description reads it back verbatim.
            Some(description) => assert_eq!(
                read["workspaceDescription"],
                json!(description),
                "case {:?}: expected the description the create carried ({description:?}), \
                 got {read}",
                case.label
            ),
            // A row that sent none has no description to report, so the member is
            // absent from the read rather than present and empty.
            None => assert!(
                read.get("workspaceDescription").is_none(),
                "case {:?}: a workspace created with no description must report none, got {read}",
                case.label
            ),
        }
    }
}
