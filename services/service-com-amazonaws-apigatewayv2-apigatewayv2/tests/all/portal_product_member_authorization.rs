// SPDX-License-Identifier: CC0-1.0

//! IAM authorization of the operations that address ONE portal product — the read, the
//! update and the DESTRUCTIVE delete — over the restJson1 wire.
//!
//! The rule under test: an operation whose model declares `AccessDeniedException` is served
//! only to a caller whose identity-based IAM policies permit THAT operation's own action
//! (`apigateway:<OperationName>`) on the ARN the request addresses; every other caller is
//! refused with `AccessDeniedException` (HTTP 403) and IAM's own sentence BEFORE the
//! operation reads or writes anything, so a refused mutation — a delete included — leaves
//! the account's state exactly as the next caller will find it.
//!
//! The table spans what the rule generalises over: the operation asked for (a read, a
//! member update and a delete, each with its own action, method and modelled success
//! status) and what the caller's policies say (nothing at all, a wildcard allow, an allow
//! for one action only, an allow confined to ANOTHER product's ARN, and a wildcard allow
//! beside an explicit deny of the delete). The recorded case that filed the issue — a
//! `DeletePortalProduct` under an explicit deny on `apigateway:DeletePortalProduct` — is one
//! row of it, and its follow-on legs (the permitted delete, and the `NotFoundException` a
//! later read of the removed id answers) are pinned here too.
//!
//! The decision is the real one: the principals come from IAM's and STS's OWN operations
//! over the same engine, through the harness `portal_identity_authorization` builds.

use serde_json::{json, Value};

use service_com_amazonaws_apigatewayv2_apigatewayv2::state::portal_product_arn;

use crate::portal_identity_authorization::{
    allow, allow_all_but, Emulator, Reply, ACCESS_DENIED, ACCOUNT_KEY, REGION,
};

/// The portal-product collection's URI (`CreatePortalProduct` binds `POST
/// /v2/portalproducts`); one product hangs off it as `{PRODUCTS}/{PortalProductId}`, which
/// is what the member operations bind.
const PRODUCTS: &str = "/v2/portalproducts";

/// The error code a read of an id the account holds no product under carries, from the
/// model's own 404 error.
const NOT_FOUND: &str = "NotFoundException";

/// The description every product this suite creates carries, so a later read shows whether
/// a call left the record alone.
const SETUP_DESCRIPTION: &str = "a product this suite judges a caller's policies against";

/// The description the permitted `UpdatePortalProduct` writes, read back to show that an
/// authorized mutation really reached the store.
const EDITED_DESCRIPTION: &str = "edited by the authorized caller";

/// One operation that addresses a SINGLE portal product, as the model binds it
/// (`smithy_query(query="operation",
/// operation="GetPortalProduct,UpdatePortalProduct,DeletePortalProduct")`): the method, a
/// body for the one operation that takes one, and the status its success carries.
struct MemberOperation {
    operation: &'static str,
    method: &'static str,
    body: Option<Value>,
    success_status: u16,
}

impl MemberOperation {
    /// The IAM action this operation is judged under: this service's action prefix followed
    /// by the operation's own name.
    fn action(&self) -> String {
        format!("apigateway:{}", self.operation)
    }

    /// The URI this operation addresses one product by.
    fn path(&self, product_id: &str) -> String {
        format!("{PRODUCTS}/{product_id}")
    }
}

/// Every operation of this model that addresses one portal product: `GET` (200), `PATCH`
/// (200) and `DELETE` (204) on `/v2/portalproducts/{PortalProductId}`.
fn member_operations() -> Vec<MemberOperation> {
    vec![
        MemberOperation {
            operation: "GetPortalProduct",
            method: "GET",
            body: None,
            success_status: 200,
        },
        MemberOperation {
            operation: "UpdatePortalProduct",
            method: "PATCH",
            body: Some(json!({ "description": EDITED_DESCRIPTION })),
            success_status: 200,
        },
        MemberOperation {
            operation: "DeletePortalProduct",
            method: "DELETE",
            body: None,
            success_status: 204,
        },
    ]
}

/// Create one portal product with the account's own credential, and answer the id the
/// service minted for it.
async fn create_product(emulator: &Emulator, display_name: &str) -> String {
    let created = emulator
        .call(
            ACCOUNT_KEY,
            "POST",
            PRODUCTS,
            Some(json!({
                "displayName": display_name,
                "description": SETUP_DESCRIPTION,
                "tags": { "suite": "member-authorization" }
            })),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "case {display_name:?}: the setup create must succeed, got {} {}",
        created.status, created.body
    );
    created
        .body
        .get("portalProductId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "case {display_name:?}: CreatePortalProduct answers a PortalProductId, got {}",
                created.body
            )
        })
        .to_string()
}

/// Read one product back with the account's own credential — the state a later caller finds.
async fn read_product(emulator: &Emulator, product_id: &str) -> Reply {
    emulator
        .call(ACCOUNT_KEY, "GET", &format!("{PRODUCTS}/{product_id}"), None)
        .await
}

/// What a PERMITTED call must have left behind, read back through `GetPortalProduct`: the
/// update's edit visible, the delete's product gone, and a read having changed nothing.
async fn assert_success_reached_the_store(
    emulator: &Emulator,
    operation: &MemberOperation,
    product_id: &str,
    case: &str,
) {
    let read_back = read_product(emulator, product_id).await;
    if operation.operation == "DeletePortalProduct" {
        assert_eq!(
            read_back.status,
            404,
            "case ({case:?}, {:?}): expected the permitted delete to have removed the \
             product, got {} {}",
            operation.operation,
            read_back.status,
            read_back.body
        );
        return;
    }
    assert_eq!(
        read_back.status,
        200,
        "case ({case:?}, {:?}): expected the product to still be readable, got {} {}",
        operation.operation,
        read_back.status,
        read_back.body
    );
    // A read leaves the record as it was; the update's own edit is what the next reader sees.
    let expected = if operation.operation == "UpdatePortalProduct" {
        EDITED_DESCRIPTION
    } else {
        SETUP_DESCRIPTION
    };
    assert_eq!(
        read_back.body.get("description").and_then(Value::as_str),
        Some(expected),
        "case ({case:?}, {:?}): expected the description {expected:?} after the permitted \
         call, got {}",
        operation.operation,
        read_back.body
    );
}

/// Whether a policy row leaves one member operation authorized.
type Authorizes = Box<dyn Fn(&MemberOperation) -> bool>;

/// RULE: each member operation is served only to a principal whose identity-based policies
/// permit that operation's own action on the ARN of the product the request addresses, and a
/// refused call — the destructive one included — changes nothing.
#[tokio::test]
async fn each_member_operation_is_judged_on_its_own_action_and_the_addressed_products_arn() {
    // The rows span what the rule turns on: no document at all, a wildcard allow, an allow
    // for one action, an allow confined to another product's ARN, and a wildcard allow
    // beside an explicit deny of the delete.
    let rows: Vec<(&str, Option<String>, Authorizes)> = vec![
        (
            "no identity-based policy at all",
            None,
            Box::new(|_: &MemberOperation| false),
        ),
        (
            "a wildcard allow of every action of this service",
            Some(allow("apigateway:*", "*")),
            Box::new(|_: &MemberOperation| true),
        ),
        (
            "an allow of the read action only",
            Some(allow("apigateway:GetPortalProduct", "*")),
            Box::new(|operation: &MemberOperation| operation.operation == "GetPortalProduct"),
        ),
        (
            // A member ARN names the product the request addressed, so a document written
            // about a DIFFERENT product's ARN grants nothing about this one.
            "an allow confined to another product's ARN",
            Some(allow(
                "apigateway:*",
                &portal_product_arn(REGION, "zzzzzzzzzz"),
            )),
            Box::new(|_: &MemberOperation| false),
        ),
        (
            "a wildcard allow beside an explicit deny of the delete",
            Some(allow_all_but("apigateway:DeletePortalProduct")),
            Box::new(|operation: &MemberOperation| {
                operation.operation != "DeletePortalProduct"
            }),
        ),
    ];

    for (index, (case, policy, expected)) in rows.into_iter().enumerate() {
        let emulator = Emulator::new();
        let principal = emulator
            .assumed_role(&format!("product-member-{index}"), policy.as_deref())
            .await;
        for operation in member_operations() {
            // Each pair is judged about a product of its own, so what one call does cannot
            // change what the next one is asked about.
            let product_id =
                create_product(&emulator, &format!("product-{index}-{}", operation.operation))
                    .await;
            let reply = emulator
                .call(
                    &principal.access_key_id,
                    operation.method,
                    &operation.path(&product_id),
                    operation.body.clone(),
                )
                .await;
            let authorized = expected(&operation);
            assert_eq!(
                reply.authorized(),
                authorized,
                "case ({case:?}, {:?}): expected authorized={authorized}, got {} {:?} {}",
                operation.operation,
                reply.status,
                reply.error_code,
                reply.body
            );
            if authorized {
                assert_eq!(
                    reply.status,
                    operation.success_status,
                    "case ({case:?}, {:?}): expected the modelled success status {}, got {} {}",
                    operation.operation,
                    operation.success_status,
                    reply.status,
                    reply.body
                );
                // The member operations that answer a body report the product the request
                // addressed, so a permitted call cannot pass on an empty 200.
                if operation.success_status == 200 {
                    assert_eq!(
                        reply.body.get("portalProductId").and_then(Value::as_str),
                        Some(product_id.as_str()),
                        "case ({case:?}, {:?}): expected the answer to report the addressed \
                         product, got {}",
                        operation.operation,
                        reply.body
                    );
                }
                // A permitted call is backed by state: the update's edit and the delete's
                // removal are both visible to the next reader.
                assert_success_reached_the_store(&emulator, &operation, &product_id, case).await;
                continue;
            }
            assert_eq!(
                reply.status, 403,
                "case ({case:?}, {:?}): expected HTTP 403, got {} {}",
                operation.operation, reply.status, reply.body
            );
            assert_eq!(
                reply.error_code, ACCESS_DENIED,
                "case ({case:?}, {:?}): expected {ACCESS_DENIED}, got {:?} {}",
                operation.operation, reply.error_code, reply.body
            );
            // The refusal happens before the store is reached, so the product the request
            // named is still there for the next caller — whatever the refused operation
            // would have done to it.
            let surviving = read_product(&emulator, &product_id).await;
            assert_eq!(
                surviving.status,
                200,
                "case ({case:?}, {:?}): expected the product to survive the refusal, got {} {}",
                operation.operation,
                surviving.status,
                surviving.body
            );
            assert_eq!(
                surviving.body.get("portalProductId").and_then(Value::as_str),
                Some(product_id.as_str()),
                "case ({case:?}, {:?}): expected the same product to be readable, got {}",
                operation.operation,
                surviving.body
            );
        }
    }
}

/// RULE: a refusal from an explicit deny states IAM's own sentence — the principal the
/// credential speaks for, the action the operation names and the ARN the request addressed —
/// and the record it named is left byte for byte as it was.
#[tokio::test]
async fn an_explicit_deny_refuses_in_iams_words_and_leaves_the_product_as_it_was() {
    for operation in member_operations() {
        let emulator = Emulator::new();
        let action = operation.action();
        let denied = emulator
            .assumed_role("product-denied", Some(&allow_all_but(&action)))
            .await;
        let product_id = create_product(&emulator, "denied-product").await;
        let before = read_product(&emulator, &product_id).await;

        let reply = emulator
            .call(
                &denied.access_key_id,
                operation.method,
                &operation.path(&product_id),
                operation.body.clone(),
            )
            .await;
        assert_eq!(
            reply.status, 403,
            "case {:?}: expected HTTP 403, got {} {}",
            operation.operation, reply.status, reply.body
        );
        assert_eq!(
            reply.error_code, ACCESS_DENIED,
            "case {:?}: expected {ACCESS_DENIED}, got {:?} {}",
            operation.operation, reply.error_code, reply.body
        );
        let message = reply.message();
        assert_eq!(
            message,
            format!(
                "User: {} is not authorized to perform: {action} on resource: {} with an \
                 explicit deny in an identity-based policy",
                denied.arn,
                portal_product_arn(REGION, &product_id)
            ),
            "case {:?}: expected IAM's explicit-deny sentence, got {message:?}",
            operation.operation
        );

        let after = read_product(&emulator, &product_id).await;
        assert_eq!(
            after.status, 200,
            "case {:?}: expected the product to still be readable, got {} {}",
            operation.operation, after.status, after.body
        );
        assert_eq!(
            after.body, before.body,
            "case {:?}: expected the refused call to have changed nothing, got {} (was {})",
            operation.operation, after.body, before.body
        );
    }
}

/// RULE: the same delete a denied principal is refused succeeds for a principal the account
/// permits, and the id it removed then reads back as `NotFoundException`.
#[tokio::test]
async fn a_permitted_delete_removes_the_product_and_a_later_read_reports_it_missing() {
    let emulator = Emulator::new();
    let product_id = create_product(&emulator, "deleted-product").await;
    let permitted = emulator
        .assumed_role("product-deleter", Some(&allow("apigateway:*", "*")))
        .await;

    let deleted = emulator
        .call(
            &permitted.access_key_id,
            "DELETE",
            &format!("{PRODUCTS}/{product_id}"),
            None,
        )
        .await;
    assert_eq!(
        deleted.status, 204,
        "case \"a permitted delete\": expected the modelled 204, got {} {}",
        deleted.status, deleted.body
    );
    assert_eq!(
        deleted.body,
        Value::Null,
        "case \"a permitted delete\": expected an empty body, got {}",
        deleted.body
    );

    let missing = read_product(&emulator, &product_id).await;
    assert_eq!(
        missing.status, 404,
        "case \"a read of the removed id\": expected HTTP 404, got {} {}",
        missing.status, missing.body
    );
    assert_eq!(
        missing.error_code, NOT_FOUND,
        "case \"a read of the removed id\": expected {NOT_FOUND}, got {:?} {}",
        missing.error_code, missing.body
    );
    // API Gateway reports an identifier that resolves to nothing account-qualified, the way
    // it reports an unknown api id: the account the lookup was made in, and the id the
    // request supplied — spelled out here rather than taken from the handler's own helper, so
    // the wording itself is what this assertion pins.
    let message = missing.message();
    assert_eq!(
        message,
        format!(
            "Product not found in account {} with id {product_id}",
            ::engine::emulator_account_id()
        ),
        "case \"a read of the removed id\": expected the account-qualified not-found \
         sentence, got {message:?}"
    );
}
