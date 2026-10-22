// SPDX-License-Identifier: CC0-1.0

//! The `WebACL` test harness: the fixtures a web-ACL suite needs on top of the wire
//! harness in `wire.rs` (which owns the router, the awsJson1_1 call and the
//! change-token helpers every suite in this crate shares).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call_in_region, change_token, DEFAULT_TEST_REGION};

/// Submit `CreateWebACL` for `request` (a body WITHOUT `ChangeToken`) with a change
/// token freshly minted for this call, signed for `region`. Answers
/// `(http status, body, the token the request carried)`.
pub async fn create_in_region(
    router: &MultiServiceRouter,
    region: &str,
    request: &Value,
) -> (u16, Value, String) {
    let token = change_token(router).await;
    let mut body = request.clone();
    body["ChangeToken"] = json!(token);
    let (status, parsed) = call_in_region(router, region, "CreateWebACL", &body).await;
    (status, parsed, token)
}

/// [`create_in_region`] signed for the wire harness's default region.
pub async fn create(router: &MultiServiceRouter, request: &Value) -> (u16, Value, String) {
    create_in_region(router, DEFAULT_TEST_REGION, request).await
}

/// The `WebACL` a successful create answered, or an explanation.
pub fn created_web_acl(status: u16, body: &Value, label: &str) -> Value {
    assert_eq!(
        status, 200,
        "case {label}: CreateWebACL must answer 200, got {status}: {body}"
    );
    body["WebACL"].clone()
}

/// The ARN AWS WAF Classic's regional endpoints give a web ACL:
/// `arn:aws:waf-regional:<region>:<account>:webacl/<WebACLId>`. The account segment is
/// the emulator's canonical one, taken from the engine rather than written out.
pub fn expected_arn(region: &str, web_acl_id: &str) -> String {
    format!(
        "arn:aws:waf-regional:{region}:{}:webacl/{web_acl_id}",
        engine::emulator_account_id()
    )
}
