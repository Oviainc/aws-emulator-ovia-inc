// SPDX-License-Identifier: CC0-1.0

//! Deleting a policy store's schema, and what a store with no schema then does.
//!
//! RULE: a `PutSchema` whose definition is the EMPTY Cedar schema document
//! DELETES the store's schema — `SchemaDefinition$cedarJson` documents it ("To
//! delete the schema, run PutSchema with `{}` for this parameter") — so the store
//! is afterwards in exactly the state a store that was never given a schema is
//! in; every other document creates or updates the schema. The consequence
//! `ValidationSettings$mode` states then applies unchanged: "If Mode=STRICT and
//! the policy store doesn't contain a schema, Verified Permissions rejects all
//! static policies and policy templates because there is no schema to validate
//! against."
//!
//! What a `STRICT` store with no schema answers, and that every one of the four
//! Cedar writes answers it alike, is the rule [`crate::store_validation_settings`]
//! pins; this suite is about the way a store REACHES that state after holding a
//! schema, which that suite's rows do not cover. The refusal's wording, the
//! content constants and the readers are reused from it rather than restated.
//!
//! Every assertion goes over the wire through the same router the emulator binary
//! serves, reusing the harness in [`crate::harness`], so a value is only visible
//! to a later request if it really reached state.

use serde_json::{json, Value};
use service_com_amazonaws_verifiedpermissions_verifiedpermissions::ops::INVALID_INPUT_MESSAGE;

use crate::harness::{call, error_code, field_list, ok, router, store_with_mode};
use crate::schema::photo_app_schema;
use crate::store_validation_settings::{
    no_schema_reason, policy_in, stored_policy_statement, stored_template_statement, template_in,
    CONFORMANT_POLICY, CONFORMANT_REPLACEMENT, CONFORMANT_TEMPLATE,
    CONFORMANT_TEMPLATE_REPLACEMENT,
};

/// The document a store holds when it holds no schema: a schema that declares
/// nothing. `SchemaDefinition$cedarJson` names it as the definition that deletes
/// a schema, and `GetSchema` reports it for a store that has none.
const EMPTY_DOCUMENT: &str = "{}";

/// One deletion scenario.
struct Row {
    case: &'static str,
    /// The definitions put, in order, each of them a spelling of the EMPTY
    /// document: the service reports a document with its insignificant
    /// whitespace removed, so every spelling here is the same document and each
    /// put is a delete.
    deletes: &'static [&'static str],
    /// Whether the store is given a real schema — and content validated against
    /// it — before the deletion. A store that never had one has nothing to
    /// update, so only the two creates are exercised for it.
    seeded: bool,
}

/// `GetSchema` for `store_id`.
async fn get_schema(
    router: &smithy_server_runtime::MultiServiceRouter,
    store_id: &str,
) -> Value {
    ok(router, "GetSchema", &json!({ "policyStoreId": store_id })).await
}

/// The `createdDate` the policy store itself reports, which is the instant the
/// empty document a schema-less store holds is dated at.
async fn store_created_date(
    router: &smithy_server_runtime::MultiServiceRouter,
    store_id: &str,
) -> Value {
    ok(
        router,
        "GetPolicyStore",
        &json!({ "policyStoreId": store_id }),
    )
    .await["createdDate"]
        .clone()
}

/// Assert that `store_id` reports holding NO schema: the empty document, no
/// namespaces, and both stamps at the store's own creation — the same answer a
/// store that was never given a schema gives.
async fn assert_holds_no_schema(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    store_id: &str,
) {
    let created = store_created_date(router, store_id).await;
    let fetched = get_schema(router, store_id).await;
    assert_eq!(
        fetched["schema"].as_str(),
        Some(EMPTY_DOCUMENT),
        "case {case:?}: a store holding no schema reports the empty document, got {fetched}"
    );
    assert_eq!(
        fetched["definition"]["cedarJson"].as_str(),
        Some(EMPTY_DOCUMENT),
        "case {case:?}: the definition carries the same document, got {fetched}"
    );
    assert_eq!(
        fetched["namespaces"].as_array().map(Vec::len),
        Some(0),
        "case {case:?}: the empty document declares no namespace, got {fetched}"
    );
    assert_eq!(
        fetched["createdDate"], created,
        "case {case:?}: the document a schema-less store holds dates from the store's own \
         creation, got {fetched}"
    );
    assert_eq!(
        fetched["lastUpdatedDate"], created,
        "case {case:?}: the document a schema-less store holds dates from the store's own \
         creation, got {fetched}"
    );
}

/// Assert `operation` is refused for a `STRICT` store that holds no schema: HTTP
/// 400 `ValidationException`, one `fieldList` entry naming `/policyStoreId`, and
/// the reason sentence for `content`.
async fn assert_refused_for_lack_of_schema(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    operation: &str,
    request: &Value,
    content: &str,
) {
    let (status, body) = call(router, operation, request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 from {operation}, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case {case:?}: expected a ValidationException from {operation}, got {body}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(INVALID_INPUT_MESSAGE),
        "case {case:?}: expected the fixed message from {operation}, got {body}"
    );
    assert_eq!(
        field_list(&body),
        vec![("/policyStoreId".to_string(), no_schema_reason(content))],
        "case {case:?}: {operation} must name the store as the member at fault, got {body}"
    );
}

/// Putting the empty document DELETES a store's schema, and a `STRICT` store whose
/// schema was deleted refuses every Cedar write exactly as one that never had a
/// schema does.
///
/// The rows differ along the SPELLING of the empty document (so the rule is about
/// the document, not about one text), along how many times it is put (the delete
/// is idempotent), and along whether the store held a schema — and content
/// validated against it — to begin with.
#[tokio::test]
async fn putting_the_empty_document_deletes_the_schema_and_strict_writes_are_refused() {
    let rows: &[Row] = &[
        Row {
            case: "a stored schema deleted with the empty object",
            deletes: &["{}"],
            seeded: true,
        },
        Row {
            case: "a stored schema deleted with the empty object spread over lines",
            deletes: &["{\n  \n}"],
            seeded: true,
        },
        Row {
            case: "a stored schema deleted twice",
            deletes: &["{}", "{ }"],
            seeded: true,
        },
        Row {
            case: "a store that was never given a schema",
            deletes: &["{}"],
            seeded: false,
        },
    ];

    let router = router();
    for Row {
        case,
        deletes,
        seeded,
    } in rows
    {
        let store_id = store_with_mode(&router, "STRICT").await;
        let created = store_created_date(&router, &store_id).await;

        // A seeded store holds a schema and two pieces of content that validated
        // against it, so the two UPDATE writes have something to replace.
        let content = if *seeded {
            ok(
                &router,
                "PutSchema",
                &json!({
                    "policyStoreId": &store_id,
                    "definition": { "cedarJson": photo_app_schema() },
                }),
            )
            .await;
            let policy_id = policy_in(&router, &store_id, CONFORMANT_POLICY).await;
            let template_id = template_in(&router, &store_id, CONFORMANT_TEMPLATE).await;
            Some((policy_id, template_id))
        } else {
            None
        };

        for text in *deletes {
            let put = ok(
                &router,
                "PutSchema",
                &json!({ "policyStoreId": &store_id, "definition": { "cedarJson": text } }),
            )
            .await;
            assert_eq!(
                put["policyStoreId"].as_str(),
                Some(store_id.as_str()),
                "case {case:?}: PutSchema reported the wrong policyStoreId, got {put}"
            );
            assert_eq!(
                put["namespaces"].as_array().map(Vec::len),
                Some(0),
                "case {case:?}: the empty document declares no namespace, got {put}"
            );
            // The put answers with the document the store now holds, which is the
            // one a schema-less store has held since it was created.
            assert_eq!(
                put["createdDate"], created,
                "case {case:?}: a deleted schema leaves the document the store was created \
                 with, got {put}"
            );
            assert_eq!(
                put["lastUpdatedDate"], created,
                "case {case:?}: a deleted schema leaves the document the store was created \
                 with, got {put}"
            );
        }

        assert_holds_no_schema(&router, case, &store_id).await;

        // Every Cedar write the rule covers is refused, with the same reason: the
        // two creates for every row, and the two updates wherever there is
        // content to replace.
        assert_refused_for_lack_of_schema(
            &router,
            case,
            "CreatePolicy",
            &json!({
                "policyStoreId": &store_id,
                "definition": { "static": { "statement": CONFORMANT_POLICY } },
            }),
            "a policy",
        )
        .await;
        assert_refused_for_lack_of_schema(
            &router,
            case,
            "CreatePolicyTemplate",
            &json!({ "policyStoreId": &store_id, "statement": CONFORMANT_TEMPLATE }),
            "a policy template",
        )
        .await;
        if let Some((policy_id, template_id)) = &content {
            assert_refused_for_lack_of_schema(
                &router,
                case,
                "UpdatePolicy",
                &json!({
                    "policyStoreId": &store_id,
                    "policyId": policy_id,
                    "definition": { "static": { "statement": CONFORMANT_REPLACEMENT } },
                }),
                "a policy",
            )
            .await;
            assert_refused_for_lack_of_schema(
                &router,
                case,
                "UpdatePolicyTemplate",
                &json!({
                    "policyStoreId": &store_id,
                    "policyTemplateId": template_id,
                    "statement": CONFORMANT_TEMPLATE_REPLACEMENT,
                }),
                "a policy template",
            )
            .await;
        }

        // A refused write stored nothing, and the content the store already held
        // was not re-validated away by the deletion.
        let listed = ok(
            &router,
            "ListPolicies",
            &json!({ "policyStoreId": &store_id }),
        )
        .await;
        let expected = usize::from(*seeded);
        assert_eq!(
            listed["policies"].as_array().map(Vec::len),
            Some(expected),
            "case {case:?}: expected {expected} stored policy, got {listed}"
        );
        if let Some((policy_id, template_id)) = &content {
            assert_eq!(
                stored_policy_statement(&router, &store_id, policy_id).await,
                CONFORMANT_POLICY,
                "case {case:?}: a refused update must leave the stored policy untouched"
            );
            assert_eq!(
                stored_template_statement(&router, &store_id, template_id).await,
                CONFORMANT_TEMPLATE,
                "case {case:?}: a refused update must leave the stored template untouched"
            );
        }
    }
}

/// A schema put after a deletion is a NEW schema, not an update of the deleted
/// one: its two stamps are the one instant that put happened, and the store
/// validates against it again.
///
/// The rows differ along how many times the schema is put and deleted, so the
/// answer cannot depend on the store having been through the cycle once.
#[tokio::test]
async fn a_schema_put_after_a_deletion_is_created_again() {
    let router = router();
    let store_id = store_with_mode(&router, "STRICT").await;

    for round in ["first", "second", "third"] {
        let put = ok(
            &router,
            "PutSchema",
            &json!({
                "policyStoreId": &store_id,
                "definition": { "cedarJson": photo_app_schema() },
            }),
        )
        .await;
        assert_eq!(
            put["createdDate"], put["lastUpdatedDate"],
            "case {round:?}: a schema put into a store that holds none is CREATED, so its \
             two stamps are the one instant, got {put}"
        );
        assert_eq!(
            put["namespaces"].as_array().map(Vec::len),
            Some(1),
            "case {round:?}: the schema declares one namespace, got {put}"
        );

        // The store validates against it again: content it sanctions is stored.
        let policy_id = policy_in(&router, &store_id, CONFORMANT_POLICY).await;
        assert_eq!(
            stored_policy_statement(&router, &store_id, &policy_id).await,
            CONFORMANT_POLICY,
            "case {round:?}: the schema sanctioned the policy, so it must be stored"
        );
        let fetched = get_schema(&router, &store_id).await;
        assert_eq!(
            fetched["schema"].as_str(),
            Some(photo_app_schema().as_str()),
            "case {round:?}: GetSchema must report the schema just put, got {fetched}"
        );
        assert_eq!(
            fetched["createdDate"], put["createdDate"],
            "case {round:?}: the reader reports the stamps the put minted, got {fetched}"
        );

        // And deleting it returns the store to holding no schema, whatever it
        // holds in policies.
        ok(
            &router,
            "PutSchema",
            &json!({
                "policyStoreId": &store_id,
                "definition": { "cedarJson": EMPTY_DOCUMENT },
            }),
        )
        .await;
        assert_holds_no_schema(&router, round, &store_id).await;
        assert_refused_for_lack_of_schema(
            &router,
            round,
            "CreatePolicy",
            &json!({
                "policyStoreId": &store_id,
                "definition": { "static": { "statement": CONFORMANT_POLICY } },
            }),
            "a policy",
        )
        .await;
    }
}

/// A store whose validation mode is `OFF` validates nothing, so deleting its
/// schema changes nothing about what it accepts: the deletion only decides what
/// there is to validate against, not whether anything is validated.
#[tokio::test]
async fn deleting_the_schema_of_a_store_that_validates_nothing_refuses_nothing() {
    let router = router();
    let store_id = store_with_mode(&router, "OFF").await;
    ok(
        &router,
        "PutSchema",
        &json!({
            "policyStoreId": &store_id,
            "definition": { "cedarJson": photo_app_schema() },
        }),
    )
    .await;
    ok(
        &router,
        "PutSchema",
        &json!({
            "policyStoreId": &store_id,
            "definition": { "cedarJson": EMPTY_DOCUMENT },
        }),
    )
    .await;
    assert_holds_no_schema(&router, "validation OFF", &store_id).await;

    let policy_id = policy_in(&router, &store_id, CONFORMANT_POLICY).await;
    assert_eq!(
        stored_policy_statement(&router, &store_id, &policy_id).await,
        CONFORMANT_POLICY,
        "case \"validation OFF\": a store that validates nothing stores what was submitted"
    );
    let template_id = template_in(&router, &store_id, CONFORMANT_TEMPLATE).await;
    assert_eq!(
        stored_template_statement(&router, &store_id, &template_id).await,
        CONFORMANT_TEMPLATE,
        "case \"validation OFF\": a store that validates nothing stores what was submitted"
    );
}
