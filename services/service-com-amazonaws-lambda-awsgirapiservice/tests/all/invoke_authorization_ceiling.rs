// SPDX-License-Identifier: CC0-1.0

//! The CEILING a permissions boundary puts on the grant a caller's own policies give an
//! invocation — the second complement of the invoke-authorization suite
//! (`tests/all/invoke_identity_authorization.rs`), beside the ordering one
//! (`tests/all/invoke_authorization.rs`, whose fixtures this suite drives its principals
//! through).
//!
//! RULE: "a permissions boundary sets the maximum permissions that an identity-based policy
//! can grant" (IAM, permissions boundaries), so an invocation the caller's own policy allows
//! is served only while the boundary in force allows it too; otherwise it is refused with HTTP
//! 403 `AccessDeniedException`, and the refusal names the boundary as the document that
//! withheld the grant.
//!
//! Every row drives real HTTP requests through the production `MultiServiceRouter` with IAM
//! mounted beside Lambda on ONE engine; each principal is created through the real
//! `CreateUser`, `PutUserPolicy`, `CreatePolicy`, `PutUserPermissionsBoundary` and
//! `CreateAccessKey`, so what the gate reads is the document IAM persisted.
//!
//! The rows differ along what the rule generalises over: what the BOUNDARY allows — every
//! action of this service, exactly this invocation, something else entirely, or this
//! invocation denied explicitly beside a wildcard grant.

use serde_json::json;

use crate::event_source_mapping_role_access::router_with_iam;
use crate::event_source_mappings::create_running_function;
use crate::invoke_authorization::{
    assert_denied, assert_invoked, document, function_arn, invoke_as, principal, refusal,
    statement, user_arn, HANDLER_SOURCE, INVOKE_ACTION,
};

/// The reason a refusal states when the caller's own policies allowed the invocation and its
/// permissions boundary did not.
fn no_boundary_grant() -> String {
    format!("because no permissions boundary allows the {INVOKE_ACTION} action")
}

/// RULE: a permissions boundary sets the maximum permissions an identity-based policy can
/// grant, so an invocation the caller's own policy allows is served only while the boundary
/// allows it too — and the refusal names the boundary as the document that withheld it.
#[tokio::test]
async fn a_permissions_boundary_caps_the_grant_the_callers_policy_gives() {
    let router = router_with_iam();
    let function = "bounded-caller-target";
    create_running_function(
        &router,
        "the function the bounded callers invoke",
        function,
        HANDLER_SOURCE,
        5,
        &json!({}),
    )
    .await;
    let arn = function_arn(function);
    let grant = document(json!([statement("Allow", INVOKE_ACTION, &arn)]));

    // (case, the boundary document, the reason the refusal states — `None` when served)
    let rows: [(&str, String, Option<String>); 4] = [
        (
            "a boundary that allows every action of this service",
            document(json!([statement("Allow", "lambda:*", "*")])),
            None,
        ),
        (
            "a boundary that allows exactly this invocation",
            document(json!([statement("Allow", INVOKE_ACTION, &arn)])),
            None,
        ),
        (
            "a boundary that allows something else",
            document(json!([statement("Allow", "logs:PutLogEvents", "*")])),
            Some(no_boundary_grant()),
        ),
        (
            "a boundary that denies the invocation explicitly",
            document(json!([
                statement("Allow", "lambda:*", "*"),
                statement("Deny", INVOKE_ACTION, &arn),
            ])),
            Some("with an explicit deny in a permissions boundary".to_string()),
        ),
    ];

    for (index, (case, boundary, refused_because)) in rows.into_iter().enumerate() {
        let user = format!("bounded-caller-{index}");
        let key = principal(&router, case, &user, Some(grant.clone()), Some(boundary)).await;
        let answer = invoke_as(&router, Some(&key), function, &[]).await;
        match &refused_because {
            None => assert_invoked(case, &answer),
            Some(reason) => {
                assert_denied(case, &answer, &refusal(&user_arn(&user), &arn, reason))
            }
        }
    }
}
