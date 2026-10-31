// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the ONE rule every write that submits Cedar content
//! obeys: what the policy store it names says that content is validated against.
//!
//! `ValidationSettings` documents it for both kinds of content at once — "Cedar
//! performs validation on all submitted new or updated static policies and policy
//! templates", and "If Mode=STRICT and the policy store doesn't contain a schema,
//! Verified Permissions rejects all static policies and policy templates because
//! there is no schema to validate against" — so the rule has exactly three
//! answers, and each of the four write operations that validate Cedar
//! (`CreatePolicy`, `UpdatePolicy`, `CreatePolicyTemplate`,
//! `UpdatePolicyTemplate`) must give the same one:
//!
//! 1. the store validates nothing, and the content is stored as submitted
//!    ([`a_store_that_validates_nothing_stores_what_no_schema_sanctions`]);
//! 2. the store is `STRICT` and holds a schema, which the content is validated
//!    against ([`a_strict_store_validates_every_cedar_write_against_its_schema`]);
//! 3. the store is `STRICT` and holds none, so the request is refused naming
//!    `policyStoreId`
//!    ([`a_strict_store_with_no_schema_refuses_every_cedar_write`]).
//!
//! Every assertion goes over the wire through the same router the emulator binary
//! serves, reusing the harness in [`crate::harness`], so a value is only visible
//! to a later request if it really reached state.

use serde_json::{json, Value};
use service_com_amazonaws_verifiedpermissions_verifiedpermissions::ops::INVALID_INPUT_MESSAGE;

use crate::harness::{call, error_code, ok, router, store_with_mode};
use crate::schema::photo_app_schema;

/// A static policy every part of which [`photo_app_schema`] declares, at the
/// types it declares them at.
pub(crate) const CONFORMANT_POLICY: &str = "permit(principal == PhotoApp::User::\"alice\", \
                                 action == PhotoApp::Action::\"viewPhoto\", \
                                 resource == PhotoApp::Photo::\"vacation.jpg\");";

/// A replacement for [`CONFORMANT_POLICY`] that changes only its ACTION — the one
/// scope member `UpdatePolicy` allows a replacement to change — to the other
/// action [`photo_app_schema`] declares.
pub(crate) const CONFORMANT_REPLACEMENT: &str = "permit(principal == PhotoApp::User::\"alice\", \
                                      action == PhotoApp::Action::\"sharePhoto\", \
                                      resource == PhotoApp::Photo::\"vacation.jpg\");";

/// [`CONFORMANT_POLICY`] with an action [`photo_app_schema`] never declares, and
/// nothing else changed: valid Cedar, so only a store that validates its content
/// against a schema has anything to refuse it for.
const UNSANCTIONED_POLICY: &str = "permit(principal == PhotoApp::User::\"alice\", \
                                   action == PhotoApp::Action::\"deletePhoto\", \
                                   resource == PhotoApp::Photo::\"vacation.jpg\");";

/// A policy TEMPLATE whose scope [`photo_app_schema`] sanctions, with the
/// principal left as the `?principal` placeholder a template may carry.
pub(crate) const CONFORMANT_TEMPLATE: &str = "permit(principal == ?principal, \
                                   action == PhotoApp::Action::\"viewPhoto\", \
                                   resource is PhotoApp::Photo);";

/// A replacement for [`CONFORMANT_TEMPLATE`] changing only its action, which is
/// what `UpdatePolicyTemplate` allows a replacement to change.
pub(crate) const CONFORMANT_TEMPLATE_REPLACEMENT: &str = "permit(principal == ?principal, \
                                               action == PhotoApp::Action::\"sharePhoto\", \
                                               resource is PhotoApp::Photo);";

/// [`CONFORMANT_TEMPLATE`] with an action [`photo_app_schema`] never declares:
/// the template counterpart of [`UNSANCTIONED_POLICY`].
const UNSANCTIONED_TEMPLATE: &str = "permit(principal == ?principal, \
                                     action == PhotoApp::Action::\"deletePhoto\", \
                                     resource is PhotoApp::Photo);";

/// The reason a store that validates against a schema it does not have gives for
/// refusing `content`. The refusal is the same sentence for every operation and
/// every kind of content, down to the noun naming what was submitted.
pub(crate) fn no_schema_reason(content: &str) -> String {
    format!(
        "its validation mode is STRICT and it contains no schema, so there is nothing to \
         validate {content} against"
    )
}

/// Create a static policy carrying `statement` in `store_id`, requiring success,
/// and return its `policyId`.
pub(crate) async fn policy_in(
    router: &smithy_server_runtime::MultiServiceRouter,
    store_id: &str,
    statement: &str,
) -> String {
    let created = ok(
        router,
        "CreatePolicy",
        &json!({
            "policyStoreId": store_id,
            "definition": { "static": { "statement": statement } },
        }),
    )
    .await;
    created["policyId"]
        .as_str()
        .expect("policyId is required on the CreatePolicy response")
        .to_string()
}

/// Create a policy template carrying `statement` in `store_id`, requiring
/// success, and return its `policyTemplateId`.
pub(crate) async fn template_in(
    router: &smithy_server_runtime::MultiServiceRouter,
    store_id: &str,
    statement: &str,
) -> String {
    let created = ok(
        router,
        "CreatePolicyTemplate",
        &json!({ "policyStoreId": store_id, "statement": statement }),
    )
    .await;
    created["policyTemplateId"]
        .as_str()
        .expect("policyTemplateId is required on the CreatePolicyTemplate response")
        .to_string()
}

/// The Cedar text state holds for a static policy, read back through
/// `GetPolicy`.
pub(crate) async fn stored_policy_statement(
    router: &smithy_server_runtime::MultiServiceRouter,
    store_id: &str,
    policy_id: &str,
) -> String {
    let fetched = ok(
        router,
        "GetPolicy",
        &json!({ "policyStoreId": store_id, "policyId": policy_id }),
    )
    .await;
    fetched["definition"]["static"]["statement"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The Cedar text state holds for a policy template, read back through
/// `GetPolicyTemplate`.
pub(crate) async fn stored_template_statement(
    router: &smithy_server_runtime::MultiServiceRouter,
    store_id: &str,
    template_id: &str,
) -> String {
    let fetched = ok(
        router,
        "GetPolicyTemplate",
        &json!({ "policyStoreId": store_id, "policyTemplateId": template_id }),
    )
    .await;
    fetched["statement"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The `fieldList` entry at `index` as `(path, message)`.
fn field(body: &Value, index: usize) -> (String, String) {
    let entry = &body["fieldList"][index];
    (
        entry["path"].as_str().unwrap_or_default().to_string(),
        entry["message"].as_str().unwrap_or_default().to_string(),
    )
}

/// Answer 3: a `STRICT` store holding no schema refuses EVERY write that submits
/// Cedar content, with the modelled `ValidationException` naming
/// `/policyStoreId` — the member the caller can act on — and the same reason
/// sentence for every one of them.
///
/// The rows span the four write operations the rule covers, both kinds of content
/// (a policy and a policy template), and both ways a store reaches the state
/// WITHOUT a schema ever being deleted (one created `STRICT`, and one switched to
/// `STRICT` after it was given content — the only way such a store can hold a
/// policy to update at all). The third way, a store whose schema was DELETED by
/// putting the empty document, is pinned in [`crate::schema_deletion`].
#[tokio::test]
async fn a_strict_store_with_no_schema_refuses_every_cedar_write() {
    let router = router();

    // The switched store: content goes in while it validates nothing, so the two
    // update rows below have something to replace.
    let switched = store_with_mode(&router, "OFF").await;
    let policy_id = policy_in(&router, &switched, CONFORMANT_POLICY).await;
    let template_id = template_in(&router, &switched, CONFORMANT_TEMPLATE).await;
    ok(
        &router,
        "UpdatePolicyStore",
        &json!({
            "policyStoreId": &switched,
            "validationSettings": { "mode": "STRICT" },
        }),
    )
    .await;

    // The store that was STRICT from the start, and so can only be asked to
    // create.
    let fresh = store_with_mode(&router, "STRICT").await;

    // (case, operation, request, the noun the reason names)
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "create a policy in a store that was always STRICT",
            "CreatePolicy",
            json!({
                "policyStoreId": &fresh,
                "definition": { "static": { "statement": CONFORMANT_POLICY } },
            }),
            "a policy",
        ),
        (
            "create a policy template in a store that was always STRICT",
            "CreatePolicyTemplate",
            json!({ "policyStoreId": &fresh, "statement": CONFORMANT_TEMPLATE }),
            "a policy template",
        ),
        (
            "create a policy in a store switched to STRICT",
            "CreatePolicy",
            json!({
                "policyStoreId": &switched,
                "definition": { "static": { "statement": CONFORMANT_POLICY } },
            }),
            "a policy",
        ),
        (
            "replace a policy's statement",
            "UpdatePolicy",
            json!({
                "policyStoreId": &switched,
                "policyId": &policy_id,
                "definition": { "static": { "statement": CONFORMANT_REPLACEMENT } },
            }),
            "a policy",
        ),
        (
            "create a policy template in a store switched to STRICT",
            "CreatePolicyTemplate",
            json!({ "policyStoreId": &switched, "statement": CONFORMANT_TEMPLATE }),
            "a policy template",
        ),
        (
            "replace a policy template's statement",
            "UpdatePolicyTemplate",
            json!({
                "policyStoreId": &switched,
                "policyTemplateId": &template_id,
                "statement": CONFORMANT_TEMPLATE_REPLACEMENT,
            }),
            "a policy template",
        ),
    ];

    for (case, operation, request, content) in &cases {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(INVALID_INPUT_MESSAGE),
            "case {case:?}: expected the fixed message, got {body}"
        );
        let (path, message) = field(&body, 0);
        assert_eq!(
            path, "/policyStoreId",
            "case {case:?}: the member at fault is the store, got {body}"
        );
        assert_eq!(
            message,
            no_schema_reason(content),
            "case {case:?}: unexpected reason, got {body}"
        );
        assert!(
            body["fieldList"][1].is_null(),
            "case {case:?}: one member is at fault, got {body}"
        );
    }

    // A refused write stores nothing: both resources still hold the text they
    // were created with.
    assert_eq!(
        stored_policy_statement(&router, &switched, &policy_id).await,
        CONFORMANT_POLICY,
        "a refused update must leave the stored policy untouched"
    );
    assert_eq!(
        stored_template_statement(&router, &switched, &template_id).await,
        CONFORMANT_TEMPLATE,
        "a refused update must leave the stored template untouched"
    );
}

/// Answer 1: a store whose validation mode is `OFF` validates nothing, so every
/// write that submits Cedar content stores it as submitted — including content no
/// schema sanctions — and a reader reports exactly that text back.
#[tokio::test]
async fn a_store_that_validates_nothing_stores_what_no_schema_sanctions() {
    let router = router();
    let store = store_with_mode(&router, "OFF").await;

    // The rows differ along the operation and along whether a schema would have
    // sanctioned the content: a store that validates nothing treats both alike.
    let policy_cases: &[(&str, &str)] = &[
        ("a policy a schema would sanction", CONFORMANT_POLICY),
        ("a policy a schema would refuse", UNSANCTIONED_POLICY),
    ];
    for (case, statement) in policy_cases {
        let policy_id = policy_in(&router, &store, statement).await;
        assert_eq!(
            stored_policy_statement(&router, &store, &policy_id).await,
            *statement,
            "case {case:?}: the submitted statement must be what is stored"
        );
        // And the same text is accepted as a REPLACEMENT, which goes through the
        // same rule: only the action changes, so nothing immutable moves.
        ok(
            &router,
            "UpdatePolicy",
            &json!({
                "policyStoreId": &store,
                "policyId": &policy_id,
                "definition": { "static": { "statement": UNSANCTIONED_POLICY } },
            }),
        )
        .await;
        assert_eq!(
            stored_policy_statement(&router, &store, &policy_id).await,
            UNSANCTIONED_POLICY,
            "case {case:?}: the replacement must be what is stored"
        );
    }

    let template_cases: &[(&str, &str)] = &[
        ("a template a schema would sanction", CONFORMANT_TEMPLATE),
        ("a template a schema would refuse", UNSANCTIONED_TEMPLATE),
    ];
    for (case, statement) in template_cases {
        let template_id = template_in(&router, &store, statement).await;
        assert_eq!(
            stored_template_statement(&router, &store, &template_id).await,
            *statement,
            "case {case:?}: the submitted statement must be what is stored"
        );
        ok(
            &router,
            "UpdatePolicyTemplate",
            &json!({
                "policyStoreId": &store,
                "policyTemplateId": &template_id,
                "statement": UNSANCTIONED_TEMPLATE,
            }),
        )
        .await;
        assert_eq!(
            stored_template_statement(&router, &store, &template_id).await,
            UNSANCTIONED_TEMPLATE,
            "case {case:?}: the replacement must be what is stored"
        );
    }
}

/// Answer 2: a `STRICT` store that holds a schema validates every write against
/// it — content the schema sanctions is stored and read back, and content it does
/// not is refused at the member whose CONTENT was validated, not at the store.
#[tokio::test]
async fn a_strict_store_validates_every_cedar_write_against_its_schema() {
    let router = router();
    let store = store_with_mode(&router, "STRICT").await;
    ok(
        &router,
        "PutSchema",
        &json!({
            "policyStoreId": &store,
            "definition": { "cedarJson": photo_app_schema() },
        }),
    )
    .await;

    // The sanctioned content, through both creates and both updates.
    let policy_id = policy_in(&router, &store, CONFORMANT_POLICY).await;
    let template_id = template_in(&router, &store, CONFORMANT_TEMPLATE).await;
    ok(
        &router,
        "UpdatePolicy",
        &json!({
            "policyStoreId": &store,
            "policyId": &policy_id,
            "definition": { "static": { "statement": CONFORMANT_REPLACEMENT } },
        }),
    )
    .await;
    ok(
        &router,
        "UpdatePolicyTemplate",
        &json!({
            "policyStoreId": &store,
            "policyTemplateId": &template_id,
            "statement": CONFORMANT_TEMPLATE_REPLACEMENT,
        }),
    )
    .await;
    assert_eq!(
        stored_policy_statement(&router, &store, &policy_id).await,
        CONFORMANT_REPLACEMENT,
        "what the schema sanctioned must be what is stored"
    );
    assert_eq!(
        stored_template_statement(&router, &store, &template_id).await,
        CONFORMANT_TEMPLATE_REPLACEMENT,
        "what the schema sanctioned must be what is stored"
    );

    // (case, operation, request, the member whose content was validated)
    let refusals: Vec<(&str, &str, Value, &str)> = vec![
        (
            "create a policy the schema does not sanction",
            "CreatePolicy",
            json!({
                "policyStoreId": &store,
                "definition": { "static": { "statement": UNSANCTIONED_POLICY } },
            }),
            "/definition",
        ),
        (
            "replace a policy with one the schema does not sanction",
            "UpdatePolicy",
            json!({
                "policyStoreId": &store,
                "policyId": &policy_id,
                "definition": { "static": { "statement": UNSANCTIONED_POLICY } },
            }),
            "/definition",
        ),
        (
            "create a policy template the schema does not sanction",
            "CreatePolicyTemplate",
            json!({ "policyStoreId": &store, "statement": UNSANCTIONED_TEMPLATE }),
            "/template/statement",
        ),
        (
            "replace a policy template with one the schema does not sanction",
            "UpdatePolicyTemplate",
            json!({
                "policyStoreId": &store,
                "policyTemplateId": &template_id,
                "statement": UNSANCTIONED_TEMPLATE,
            }),
            "/template/statement",
        ),
    ];

    for (case, operation, request, member) in &refusals {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
        let (path, message) = field(&body, 0);
        assert_eq!(
            path, *member,
            "case {case:?}: the schema refused the content, so the validated member is at \
             fault, got {body}"
        );
        assert!(
            !message.is_empty(),
            "case {case:?}: a refusal must say why, got {body}"
        );
    }

    // Every refused write left the sanctioned text in place.
    assert_eq!(
        stored_policy_statement(&router, &store, &policy_id).await,
        CONFORMANT_REPLACEMENT,
        "a refused update must leave the stored policy untouched"
    );
    assert_eq!(
        stored_template_statement(&router, &store, &template_id).await,
        CONFORMANT_TEMPLATE_REPLACEMENT,
        "a refused update must leave the stored template untouched"
    );
}
