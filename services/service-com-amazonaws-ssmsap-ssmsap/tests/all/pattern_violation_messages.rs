// SPDX-License-Identifier: CC0-1.0

//! The unanchored `@pattern` rule on the application-lookup and listing operations.
//!
//! The rule is the one [`crate::validation_message_rendering`] states — a `@pattern`
//! violation reports the modelled expression WITHOUT its outer `^`/`$`, that anchoring
//! being how a model spells a whole-string match rather than part of the expression the
//! service advertises. This suite asserts it on the operations and members the
//! application-lookup recordings refuse, which that suite's rows do not reach:
//! `GetApplication`, `DeregisterApplication`, `ListComponents`, `ListDatabases` and
//! `ListOperations` on `ApplicationId`, `GetComponent` on `ComponentId`, and
//! `GetDatabase` on `DatabaseId` — three constrained shapes over seven operations,
//! required and optional members alike. The recorded `GetApplication` / `'bad app id!'`
//! request is ONE row.
//!
//! The expected code, the expected expressions and the message reading are IMPORTED from
//! that suite rather than restated, so the two cannot drift on the rule they both pin;
//! the envelope's other halves (the plural count, the `; ` join, every non-`@pattern`
//! requirement) are pinned there and are not restated here.
//!
//! Every call goes through `MultiServiceRouter::dispatch` (the harness
//! `tests/all/application_lifecycle.rs` owns), so the restJson1 routing, the boundary
//! constraint walk and this service's `override_refusal` all run exactly as they do in
//! the emulator binary.

use serde_json::{json, Value};

use crate::application_lifecycle::Emulator;
use crate::validation_message_rendering::{
    message, APPLICATION_ID_PATTERN, COMPONENT_ID_PATTERN, DATABASE_ID_PATTERN,
    VALIDATION_EXCEPTION,
};

/// A well-formed `ApplicationId`, for a row whose failing member is another one.
const GOOD_APPLICATION_ID: &str = "HDB-TEST";

/// One row: a label, the operation's URI path, the request body, the input position the
/// walk names, the value it echoes and the expression the requirement must report.
struct Case {
    label: &'static str,
    path: &'static str,
    body: Value,
    member: &'static str,
    submitted: &'static str,
    pattern: &'static str,
}

/// The rows: the six operation/member pairs the application-lookup recordings refuse,
/// plus `GetDatabase`'s `DatabaseId` for the end-anchored shape.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "GetApplication / ApplicationId",
            path: "/get-application",
            body: json!({ "ApplicationId": "bad app id!" }),
            member: "applicationId",
            submitted: "bad app id!",
            pattern: APPLICATION_ID_PATTERN,
        },
        Case {
            label: "DeregisterApplication / ApplicationId",
            path: "/deregister-application",
            body: json!({ "ApplicationId": "bad id!!" }),
            member: "applicationId",
            submitted: "bad id!!",
            pattern: APPLICATION_ID_PATTERN,
        },
        Case {
            label: "ListComponents / ApplicationId",
            path: "/list-components",
            body: json!({ "ApplicationId": "bad app id!" }),
            member: "applicationId",
            submitted: "bad app id!",
            pattern: APPLICATION_ID_PATTERN,
        },
        Case {
            label: "ListDatabases / ApplicationId",
            path: "/list-databases",
            body: json!({ "ApplicationId": "bad app id!" }),
            member: "applicationId",
            submitted: "bad app id!",
            pattern: APPLICATION_ID_PATTERN,
        },
        Case {
            label: "ListOperations / ApplicationId",
            path: "/list-operations",
            body: json!({ "ApplicationId": "bad app id!" }),
            member: "applicationId",
            submitted: "bad app id!",
            pattern: APPLICATION_ID_PATTERN,
        },
        Case {
            label: "GetComponent / ComponentId",
            path: "/get-component",
            body: json!({
                "ApplicationId": GOOD_APPLICATION_ID,
                "ComponentId": "bad id!",
            }),
            member: "componentId",
            submitted: "bad id!",
            pattern: COMPONENT_ID_PATTERN,
        },
        Case {
            label: "GetDatabase / DatabaseId (end-anchored shape)",
            path: "/get-database",
            body: json!({
                "ApplicationId": GOOD_APPLICATION_ID,
                "DatabaseId": "bad id!",
            }),
            member: "databaseId",
            submitted: "bad id!",
            pattern: DATABASE_ID_PATTERN,
        },
    ]
}

/// RULE: a `@pattern` violation on any of these members is refused as
/// `ValidationException` (HTTP 400) whose message states the modelled expression with its
/// outer `^`/`$` removed, inside the framework's envelope and echoing the value the
/// caller submitted at the position the walk names.
#[tokio::test]
async fn a_pattern_violation_reports_the_expression_without_the_models_outer_anchors() {
    let emulator = Emulator::new();

    for case in cases() {
        let Case {
            label,
            path,
            body,
            member,
            submitted,
            pattern,
        } = case;
        let reply = emulator.send(path, body).await;

        assert_eq!(
            reply.status, 400,
            "case {label:?}: expected 400, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case {label:?}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
        );

        let actual = message(&reply);
        let expected = format!(
            "1 validation error detected: Value '{submitted}' at '{member}' failed to \
             satisfy constraint: Member must satisfy regular expression pattern: {pattern}"
        );
        assert_eq!(
            actual, expected,
            "case {label:?}: expected {expected:?}, got {actual:?}"
        );
        // No anchor survives anywhere in the message: none of these shapes' patterns
        // carries one of its own, so a `^` or `$` left in it is the model's spelling
        // leaking out — the divergence this rule exists for.
        assert!(
            !actual.contains('^') && !actual.contains('$'),
            "case {label:?}: the message still reports an anchor, got {actual:?}"
        );
    }
}

/// RULE: the refusal is the pattern's, not the member's — a value INSIDE
/// `ApplicationId`'s pattern is never refused for that pattern, whichever of the
/// expression's character classes it uses and wherever it sits in the modelled length
/// range (1..60).
#[tokio::test]
async fn a_value_inside_its_pattern_is_not_refused_for_it() {
    let emulator = Emulator::new();

    // Word characters, a dot, a hyphen, an underscore and digits — the classes
    // `^[\w\d\.-]+$` admits — at the minimum length, in between, and at the maximum.
    let at_maximum = "A".repeat(60);
    for id in [GOOD_APPLICATION_ID, "a", "app.1-2_3", at_maximum.as_str()] {
        let reply = emulator
            .send("/list-components", json!({ "ApplicationId": id }))
            .await;
        // A tolerant read, unlike the shared `message`: whether these ids reach a 200 or
        // the handler's own "no such application" refusal is another suite's rule, and
        // this one only asserts that no CONSTRAINT refusal names the pattern.
        let stated = reply
            .member("message")
            .or_else(|| reply.member("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            !stated.contains("regular expression pattern"),
            "case {id:?}: a value inside its pattern must not be refused for it, got \
             {stated:?}"
        );
    }
}
