// SPDX-License-Identifier: CC0-1.0

//! The statement-size bound on the SCHEMA VALIDATION path.
//!
//! The schema round-trip and what a stored schema governs live in
//! [`crate::schema`]; the statement bound on a store with validation `OFF` (where
//! only the Cedar parse runs) lives in
//! [`crate::policy_store_authorization::evaluator_input_bounds_refuse_instead_of_evaluating`].
//! What neither covers is the path this suite pins: a `STRICT` store with a
//! stored schema, where the statement additionally reaches Cedar's VALIDATOR, so
//! the size refusal has to dominate that too.
//!
//! Every assertion goes over the wire through the same router the emulator binary
//! serves, reusing the shared harness in [`crate::harness`] and the schema
//! document in [`crate::schema`].

use serde_json::json;
use service_com_amazonaws_verifiedpermissions_verifiedpermissions::state::cedar::MAX_STATEMENT_BYTES;

use crate::harness::{call, error_code, ok, router, store_with_mode};
use crate::schema::photo_app_schema;

/// A static policy that VALIDATES against [`photo_app_schema`], padded to exactly
/// `bytes` with a trailing Cedar comment. Padding with a comment (rather than
/// with a longer entity id) keeps the statement valid under the schema at every
/// size, which is what makes the size the only thing a refusal can be about.
fn validating_statement_of(bytes: usize) -> String {
    let head = "permit(principal == PhotoApp::User::\"alice\", \
                action == PhotoApp::Action::\"viewPhoto\", \
                resource == PhotoApp::Photo::\"vacation.jpg\");\n// ";
    format!("{head}{}", "x".repeat(bytes.saturating_sub(head.len())))
}

/// A `STRICT` store runs a submitted statement through Cedar's validator against
/// its stored schema, and the statement's SIZE is refused before that: the bound
/// itself validates and is stored, one byte past it is refused, and the refusal
/// names the bound rather than any validator diagnostic.
#[tokio::test]
async fn a_strict_store_bounds_the_statement_before_it_validates_it() {
    // Rows: at the bound, and one byte past it.
    let cases: &[(&str, usize, u16)] = &[
        ("at the byte bound", MAX_STATEMENT_BYTES, 200),
        ("one byte past the bound", MAX_STATEMENT_BYTES + 1, 400),
    ];

    let router = router();
    for (case, bytes, expected) in cases {
        let store_id = store_with_mode(&router, "STRICT").await;
        ok(
            &router,
            "PutSchema",
            &json!({
                "policyStoreId": store_id,
                "definition": { "cedarJson": photo_app_schema() },
            }),
        )
        .await;

        let statement = validating_statement_of(*bytes);
        assert_eq!(
            statement.len(),
            *bytes,
            "case {case:?}: the padded statement should be {bytes} bytes, got {}",
            statement.len()
        );

        let (status, body) = call(
            &router,
            "CreatePolicy",
            &json!({
                "policyStoreId": store_id,
                "definition": { "static": { "statement": statement } },
            }),
        )
        .await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected}, got {status} {body}"
        );
        if *expected != 200 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {body}"
            );
            assert!(
                body["message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("bytes a policy may carry"),
                "case {case:?}: the size bound must refuse it before any parse or \
                 validation, got {body}"
            );
        }

        // Either way, the store holds only what was accepted.
        let listed = ok(
            &router,
            "ListPolicies",
            &json!({ "policyStoreId": store_id }),
        )
        .await;
        let expected_count = usize::from(*expected == 200);
        assert_eq!(
            listed["policies"].as_array().map(Vec::len),
            Some(expected_count),
            "case {case:?}: expected {expected_count} stored policy, got {listed}"
        );
    }
}
