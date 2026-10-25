// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the three response-fidelity rules this service answers a
//! read and a refusal with:
//!
//! * the schema document a read reports — the submitted JSON minified, beside the
//!   `definition` member that carries the same document;
//! * where a `ValidationException` anchors `path` for a policy TEMPLATE's
//!   statement, and what the field message says when Cedar refuses the text;
//! * what an AUTHORIZATION call's `ResourceNotFoundException` message names,
//!   against what the control plane's own not-found answers.
//!
//! Every assertion goes over the wire through the shared [`super::harness`], so a
//! value is only visible here if it really reached this service's state.

use serde_json::{json, Value};

use super::harness::{
    call, error_code, fault_message, ok, router, store_with_mode, store_with_validation_off,
};

// ---------------------------------------------------------------------------
// The schema document a read reports
// ---------------------------------------------------------------------------

/// The Cedar schema document every row below submits, already minified: one
/// namespace, one entity type and one action over it.
///
/// Its members are written `entityTypes` BEFORE `actions` — an order no
/// re-serialization through a sorted map could produce — so a row that reports
/// this text back proves the member order survived as well as the whitespace.
const SCHEMA_MINIFIED: &str = concat!(
    r#"{"PhotoApp":{"entityTypes":{"User":{"shape":{"type":"Record","attributes":{}}}},"#,
    r#""actions":{"view":{"appliesTo":{"principalTypes":["User"],"resourceTypes":["User"]}}}}}"#
);

/// The same document with a space after every `:` and `,` — the spelling a
/// renderer whose separators carry one produces.
const SCHEMA_SPACED: &str = concat!(
    r#"{"PhotoApp": {"entityTypes": {"User": {"shape": {"type": "Record", "attributes": {}}}}, "#,
    r#""actions": {"view": {"appliesTo": {"principalTypes": ["User"], "resourceTypes": ["User"]}}}}}"#
);

/// The same document indented over several lines.
const SCHEMA_INDENTED: &str = r#"{
  "PhotoApp": {
    "entityTypes": {
      "User": { "shape": { "type": "Record", "attributes": {} } }
    },
    "actions": {
      "view": {
        "appliesTo": { "principalTypes": ["User"], "resourceTypes": ["User"] }
      }
    }
  }
}"#;

/// The same document with its two namespace members in the other order, so the
/// rows span member ORDER and not only where the whitespace sits.
const SCHEMA_ACTIONS_FIRST_MINIFIED: &str = concat!(
    r#"{"PhotoApp":{"actions":{"view":{"appliesTo":{"principalTypes":["User"],"#,
    r#""resourceTypes":["User"]}}},"entityTypes":{"User":{"shape":{"type":"Record","#,
    r#""attributes":{}}}}}}"#
);

/// … as submitted, spaced.
const SCHEMA_ACTIONS_FIRST_SPACED: &str = concat!(
    r#"{"PhotoApp": {"actions": {"view": {"appliesTo": {"principalTypes": ["User"], "#,
    r#""resourceTypes": ["User"]}}}, "entityTypes": {"User": {"shape": {"type": "Record", "#,
    r#""attributes": {}}}}}}"#
);

/// A document whose action id carries a SPACE inside the string literal — a
/// legal Cedar action id (`Action::"view photo"`), so the space is part of the
/// value and not between two tokens.
const SCHEMA_SPACE_IN_STRING_MINIFIED: &str = concat!(
    r#"{"PhotoApp":{"entityTypes":{"User":{"shape":{"type":"Record","attributes":{}}}},"#,
    r#""actions":{"view photo":{"appliesTo":{"principalTypes":["User"],"resourceTypes":["User"]}}}}}"#
);

/// … as submitted, spaced.
const SCHEMA_SPACE_IN_STRING_SPACED: &str = concat!(
    r#"{"PhotoApp": {"entityTypes": {"User": {"shape": {"type": "Record", "attributes": {}}}}, "#,
    r#""actions": {"view photo": {"appliesTo": {"principalTypes": ["User"], "#,
    r#""resourceTypes": ["User"]}}}}}"#
);

/// The namespaces a response reports, sorted so the assertion does not depend on
/// the order the parse walked them in. An absent list reads as empty.
fn namespaces(value: &Value) -> Vec<String> {
    let mut out: Vec<String> = value
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item.as_str().unwrap_or_default().to_string())
        .collect();
    out.sort();
    out
}

/// `GetSchema` reports the stored schema DOCUMENT — the submitted JSON with the
/// insignificant whitespace between its tokens removed and nothing else about it
/// changed — as both `schema` and `definition.cedarJson`.
///
/// The rows differ along what the rule generalises over: where the whitespace sits
/// (none at all, after every separator, as indentation, around the whole
/// document), whether it is inside a string literal (where it belongs to the
/// value), the ORDER of the document's members, and whether the document declares
/// anything at all.
#[tokio::test]
async fn get_schema_reports_the_document_minified_and_carries_it_in_definition() {
    /// One row: what a failing assertion calls it, the text submitted, the text
    /// reported back, and the namespaces the document declares.
    struct Row {
        case: &'static str,
        submitted: &'static str,
        reported: &'static str,
        namespaces: &'static [&'static str],
    }

    let rows = &[
        Row {
            case: "a document already carrying no whitespace",
            submitted: SCHEMA_MINIFIED,
            reported: SCHEMA_MINIFIED,
            namespaces: &["PhotoApp"],
        },
        Row {
            case: "a space after every separator",
            submitted: SCHEMA_SPACED,
            reported: SCHEMA_MINIFIED,
            namespaces: &["PhotoApp"],
        },
        Row {
            case: "indented over several lines",
            submitted: SCHEMA_INDENTED,
            reported: SCHEMA_MINIFIED,
            namespaces: &["PhotoApp"],
        },
        Row {
            case: "wrapped in whitespace a JSON parser ignores",
            submitted: concat!(
                "\n\t ",
                r#"{"PhotoApp":{"entityTypes":{"User":{"shape":{"type":"Record","attributes":{}}}},"#,
                r#""actions":{"view":{"appliesTo":{"principalTypes":["User"],"resourceTypes":["User"]}}}}}"#,
                " \n"
            ),
            reported: SCHEMA_MINIFIED,
            namespaces: &["PhotoApp"],
        },
        Row {
            case: "the namespace's members in the other order",
            submitted: SCHEMA_ACTIONS_FIRST_SPACED,
            reported: SCHEMA_ACTIONS_FIRST_MINIFIED,
            namespaces: &["PhotoApp"],
        },
        Row {
            case: "a space inside a string literal, which is part of the value",
            submitted: SCHEMA_SPACE_IN_STRING_SPACED,
            reported: SCHEMA_SPACE_IN_STRING_MINIFIED,
            namespaces: &["PhotoApp"],
        },
        Row {
            case: "the empty document, spread over two lines",
            submitted: "{\n}",
            reported: "{}",
            namespaces: &[],
        },
    ];

    let router = router();
    for Row {
        case,
        submitted,
        reported,
        namespaces: declared,
    } in rows
    {
        let store_id = store_with_mode(&router, "OFF").await;
        let put = ok(
            &router,
            "PutSchema",
            &json!({ "policyStoreId": store_id, "definition": { "cedarJson": submitted } }),
        )
        .await;
        let declared: Vec<String> = declared.iter().map(ToString::to_string).collect();
        assert_eq!(
            namespaces(&put["namespaces"]),
            declared,
            "case {case:?}: PutSchema reported the wrong namespaces, got {put}"
        );

        let fetched = ok(&router, "GetSchema", &json!({ "policyStoreId": store_id })).await;
        let schema = fetched["schema"].as_str().unwrap_or_default();
        assert_eq!(
            schema, *reported,
            "case {case:?}: expected GetSchema to report {reported:?}, got {schema:?}"
        );
        let definition = fetched["definition"]["cedarJson"].as_str().unwrap_or_default();
        assert_eq!(
            definition, *reported,
            "case {case:?}: expected definition.cedarJson to carry {reported:?}, got \
             {definition:?} in {fetched}"
        );
        assert_eq!(
            namespaces(&fetched["namespaces"]),
            declared,
            "case {case:?}: GetSchema reported different namespaces, got {fetched}"
        );

        // The reported form is a FIXED POINT: putting back what was read reports
        // the same bytes again, so a caller that round-trips the document never
        // sees a difference it did not make.
        ok(
            &router,
            "PutSchema",
            &json!({ "policyStoreId": store_id, "definition": { "cedarJson": reported } }),
        )
        .await;
        let again = ok(&router, "GetSchema", &json!({ "policyStoreId": store_id })).await;
        let round_tripped = again["schema"].as_str().unwrap_or_default();
        assert_eq!(
            round_tripped, *reported,
            "case {case:?}: re-putting the reported document must report it unchanged, got \
             {round_tripped:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Where a refusal of a policy TEMPLATE's statement points
// ---------------------------------------------------------------------------

/// The element path a `ValidationException` names for a policy template's Cedar
/// text.
const TEMPLATE_STATEMENT_PATH: &str = "/template/statement";

/// A Cedar template the create accepts, so the update rows have one to replace.
const VALID_TEMPLATE: &str =
    "permit(principal == ?principal, action == Action::\"view\", resource == Photo::\"p\");";

/// Cedar's own diagnostic for a static policy handed to the template parser. It
/// is the message the field carries: Verified Permissions IS Cedar, so what it
/// reports for text Cedar will not parse is Cedar's sentence, unwrapped.
const STATIC_POLICY_DIAGNOSTIC: &str = "expected a template, got a static policy";

/// Every refusal of a policy template's statement names
/// `/template/statement` in `fieldList`, and a refusal by the Cedar parser carries
/// the parser's own diagnostic rather than prose this emulator wrote around it.
///
/// The rows differ along the operation that takes the statement
/// (`CreatePolicyTemplate`, `UpdatePolicyTemplate`), what is wrong with the text
/// (a static policy where a template is required, text no Cedar parser accepts,
/// a template the store's schema refuses in `STRICT` mode) and, for the update, a
/// replacement that changes an element the operation does not let change.
#[tokio::test]
async fn a_refused_template_statement_names_the_template_statement_element() {
    /// One row: what a failing assertion calls it, the operation, the store's
    /// validation mode, the statement submitted, and the exact field message when
    /// the rule fixes it.
    struct Row {
        case: &'static str,
        operation: &'static str,
        mode: &'static str,
        statement: &'static str,
        message: Option<&'static str>,
    }

    let rows = &[
        Row {
            case: "CreatePolicyTemplate given a static policy",
            operation: "CreatePolicyTemplate",
            mode: "OFF",
            statement: "permit(principal == User::\"alice\", action == Action::\"view\", \
                        resource == Photo::\"vacation.jpg\");",
            message: Some(STATIC_POLICY_DIAGNOSTIC),
        },
        Row {
            case: "CreatePolicyTemplate given text no parser accepts",
            operation: "CreatePolicyTemplate",
            mode: "OFF",
            statement: "permit(principal == ?principal, action, resource",
            message: None,
        },
        Row {
            case: "CreatePolicyTemplate in STRICT mode naming an undeclared action",
            operation: "CreatePolicyTemplate",
            mode: "STRICT",
            statement: "permit(principal == ?principal, \
                        action == PhotoApp::Action::\"deletePhoto\", \
                        resource == PhotoApp::Photo::\"p\");",
            message: None,
        },
        Row {
            case: "UpdatePolicyTemplate given a static policy",
            operation: "UpdatePolicyTemplate",
            mode: "OFF",
            statement: "permit(principal == User::\"alice\", action == Action::\"view\", \
                        resource == Photo::\"vacation.jpg\");",
            message: Some(STATIC_POLICY_DIAGNOSTIC),
        },
        Row {
            case: "UpdatePolicyTemplate given text no parser accepts",
            operation: "UpdatePolicyTemplate",
            mode: "OFF",
            statement: "permit(principal == ?principal, action, resource",
            message: None,
        },
        Row {
            case: "UpdatePolicyTemplate changing the template's effect",
            operation: "UpdatePolicyTemplate",
            mode: "OFF",
            statement: "forbid(principal == ?principal, action == Action::\"view\", \
                        resource == Photo::\"p\");",
            message: None,
        },
    ];

    let router = router();
    for Row {
        case,
        operation,
        mode,
        statement,
        message: expected_message,
    } in rows
    {
        let store_id = store_with_mode(&router, "OFF").await;
        // The store holds a template before the update rows run, created while
        // validation is OFF so only the row's own statement is under test.
        let created = ok(
            &router,
            "CreatePolicyTemplate",
            &json!({ "policyStoreId": store_id, "statement": VALID_TEMPLATE }),
        )
        .await;
        let template_id = created["policyTemplateId"]
            .as_str()
            .expect("policyTemplateId is required on the CreatePolicyTemplate response")
            .to_string();
        if *mode == "STRICT" {
            ok(
                &router,
                "PutSchema",
                &json!({
                    "policyStoreId": store_id,
                    "definition": { "cedarJson": SCHEMA_MINIFIED },
                }),
            )
            .await;
            ok(
                &router,
                "UpdatePolicyStore",
                &json!({
                    "policyStoreId": store_id,
                    "validationSettings": { "mode": "STRICT" },
                }),
            )
            .await;
        }

        let mut request = json!({ "policyStoreId": store_id, "statement": statement });
        if *operation == "UpdatePolicyTemplate" {
            request["policyTemplateId"] = json!(template_id);
        }
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: unexpected error code, got {body}"
        );

        let fields = body["fieldList"].as_array().cloned().unwrap_or_default();
        assert!(
            !fields.is_empty(),
            "case {case:?}: a refusal of the statement must name it in fieldList, got {body}"
        );
        for field in &fields {
            let path = field["path"].as_str().unwrap_or_default();
            assert_eq!(
                path, TEMPLATE_STATEMENT_PATH,
                "case {case:?}: expected path {TEMPLATE_STATEMENT_PATH:?}, got {path:?} in {body}"
            );
            let field_message = field["message"].as_str().unwrap_or_default();
            assert!(
                !field_message.is_empty(),
                "case {case:?}: every fieldList entry carries a message, got {body}"
            );
            // Nothing the emulator wrote around the engine's own words: the
            // field message is the diagnostic, not a sentence about it.
            assert!(
                !field_message.contains("is not valid Cedar"),
                "case {case:?}: the field message must be the engine's own diagnostic, got \
                 {field_message:?}"
            );
        }
        if let Some(expected) = expected_message {
            let field_message = fields[0]["message"].as_str().unwrap_or_default();
            assert_eq!(
                field_message, *expected,
                "case {case:?}: expected the field message {expected:?}, got {field_message:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// What an authorization call's not-found message names
// ---------------------------------------------------------------------------

/// The message the CONTROL PLANE answers a not-found with, whatever was missing.
const CONTROL_PLANE_NOT_FOUND: &str = "Resource not found";

/// An AUTHORIZATION call's `ResourceNotFoundException` message names the KIND of
/// resource that was not found and the id the request referenced it by, while the
/// control plane's own not-found keeps its one fixed summary.
///
/// The rows differ along the authorization operation (`IsAuthorized`,
/// `BatchIsAuthorized`), the FORM of the reference that resolves to nothing (a
/// store id, an id carrying every punctuation the pattern admits, an id at the
/// modelled length bound, an alias reference) and therefore the KIND of resource
/// the reference named (`POLICY_STORE`, `POLICY_STORE_ALIAS`).
#[tokio::test]
async fn an_authorization_not_found_names_the_resource_kind_and_the_requested_id() {
    /// One row: what a failing assertion calls it, the operation, the
    /// `policyStoreId` reference sent, and the kind and id the refusal reports.
    struct Row {
        case: &'static str,
        operation: &'static str,
        reference: String,
        resource_type: &'static str,
        /// The words the message opens with for that `ResourceType`, written out
        /// here rather than derived, so the test states the sentence independently
        /// of the code that renders it.
        kind_words: &'static str,
        resource_id: String,
    }

    let longest_store = format!("PS{}", "a".repeat(198));
    let rows = vec![
        Row {
            case: "IsAuthorized against a store id that was never minted",
            operation: "IsAuthorized",
            reference: "PSNONEXISTENT00000000".to_string(),
            resource_type: "POLICY_STORE",
            kind_words: "Policy store",
            resource_id: "PSNONEXISTENT00000000".to_string(),
        },
        Row {
            case: "IsAuthorized against an id carrying every admitted punctuation",
            operation: "IsAuthorized",
            reference: "PS-store_1/2".to_string(),
            resource_type: "POLICY_STORE",
            kind_words: "Policy store",
            resource_id: "PS-store_1/2".to_string(),
        },
        Row {
            case: "IsAuthorized against an id at the modelled length bound",
            operation: "IsAuthorized",
            reference: longest_store.clone(),
            resource_type: "POLICY_STORE",
            kind_words: "Policy store",
            resource_id: longest_store.clone(),
        },
        Row {
            // An alias reference names an ALIAS, by its bare name: that is the
            // resource the request referenced, so that is what the message names.
            case: "IsAuthorized through an alias that resolves to nothing",
            operation: "IsAuthorized",
            reference: "policy-store-alias/no-such-alias".to_string(),
            resource_type: "POLICY_STORE_ALIAS",
            kind_words: "Policy store alias",
            resource_id: "no-such-alias".to_string(),
        },
        Row {
            case: "BatchIsAuthorized against a store id that was never minted",
            operation: "BatchIsAuthorized",
            reference: "PSNONEXISTENT00000000".to_string(),
            resource_type: "POLICY_STORE",
            kind_words: "Policy store",
            resource_id: "PSNONEXISTENT00000000".to_string(),
        },
        Row {
            case: "BatchIsAuthorized through an alias that resolves to nothing",
            operation: "BatchIsAuthorized",
            reference: "policy-store-alias/no-such-alias".to_string(),
            resource_type: "POLICY_STORE_ALIAS",
            kind_words: "Policy store alias",
            resource_id: "no-such-alias".to_string(),
        },
    ];

    let router = router();
    for Row {
        case,
        operation,
        reference,
        resource_type,
        kind_words,
        resource_id,
    } in &rows
    {
        let one = json!({
            "principal": { "entityType": "PhotoApp::User", "entityId": "alice" },
            "action": { "actionType": "PhotoApp::Action", "actionId": "view" },
            "resource": { "entityType": "PhotoApp::Photo", "entityId": "vacation" },
        });
        let request = if *operation == "BatchIsAuthorized" {
            json!({ "policyStoreId": reference, "requests": [one] })
        } else {
            let mut request = one;
            request["policyStoreId"] = json!(reference);
            request
        };

        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code, got {body}"
        );

        let expected = format!("{kind_words} with id '{resource_id}' not found");
        let message = fault_message(&body);
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
        // The modelled members still report the same identity the message names.
        assert_eq!(
            body["resourceId"].as_str().unwrap_or_default(),
            resource_id.as_str(),
            "case {case:?}: expected resourceId {resource_id:?}, got {body}"
        );
        assert_eq!(
            body["resourceType"].as_str().unwrap_or_default(),
            *resource_type,
            "case {case:?}: expected resourceType {resource_type:?}, got {body}"
        );

        // The same reference on a CONTROL-PLANE call keeps the fixed summary: the
        // two halves of the API word their not-found differently, which is the
        // other side of this rule.
        let (control_status, control_body) = call(
            &router,
            "GetPolicyStore",
            &json!({ "policyStoreId": reference }),
        )
        .await;
        assert_eq!(
            control_status, 404,
            "case {case:?}: expected the control-plane read to 404, got {control_status} \
             {control_body}"
        );
        let control_message = fault_message(&control_body);
        assert_eq!(
            control_message, CONTROL_PLANE_NOT_FOUND,
            "case {case:?}: expected the control plane's {CONTROL_PLANE_NOT_FOUND:?}, got \
             {control_message:?}"
        );
    }
}

/// A repeated authorization not-found answers byte-identically: the message is
/// built from the REQUEST, so nothing in it varies between two calls that carry
/// the same reference, and a different reference is the only thing that changes
/// it.
#[tokio::test]
async fn the_authorization_not_found_message_varies_only_with_the_reference() {
    let router = router();
    // A live store, so the suite is not asserting about an engine that never
    // served anything.
    let live = store_with_validation_off(&router).await;
    let request = |store: &str| {
        json!({
            "policyStoreId": store,
            "principal": { "entityType": "PhotoApp::User", "entityId": "alice" },
            "action": { "actionType": "PhotoApp::Action", "actionId": "view" },
            "resource": { "entityType": "PhotoApp::Photo", "entityId": "vacation" },
        })
    };
    assert_eq!(
        call(&router, "IsAuthorized", &request(&live)).await.0,
        200,
        "case \"a live store\": IsAuthorized must still answer a store that exists"
    );

    let (_, first) = call(&router, "IsAuthorized", &request("PSMISSINGMISSINGMISS1")).await;
    let (_, second) = call(&router, "IsAuthorized", &request("PSMISSINGMISSINGMISS1")).await;
    assert_eq!(
        first, second,
        "case \"the same reference twice\": the whole fault must be identical, got {first} then \
         {second}"
    );

    let (_, other) = call(&router, "IsAuthorized", &request("PSMISSINGMISSINGMISS2")).await;
    assert_ne!(
        fault_message(&other),
        fault_message(&first),
        "case \"a different reference\": the message names the id, so it must differ, got {other}"
    );
}
