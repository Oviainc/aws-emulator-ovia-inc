// SPDX-License-Identifier: CC0-1.0

//! What a `WebACL`'s `DefaultAction` may be, end to end over the wire.
//!
//! The modelled `WafActionType` enum has three values (the generated
//! `WAF_ACTION_TYPE_VALUES`: `ALLOW`, `BLOCK`, `COUNT`), but a web ACL's DEFAULT action
//! takes a narrower set: `WafAction$Type`'s own documentation says of `COUNT` that "AWS
//! WAF increments a counter of the requests that match all of the conditions in the
//! rule. … You can't specify `COUNT` for the default action for a `WebACL`." So `COUNT`
//! is an action a RULE may take and a default action may not, and "You specified an
//! invalid value" is one of the cases `WAFInvalidParameterException` documents.
//!
//! The suite pins that rule twice over: once as a literal (`COUNT` by name must be
//! refused, and must store nothing), and once as a catalog walk over the GENERATED enum
//! so a value the model adds later cannot slip past untested.
//!
//! A value the rule ADMITS is not stored either: AWS WAF Classic is past its end of
//! support, so `CreateWebACL` answers every request whose parameters it accepts with the
//! modelled `WAFBadRequestException` (`web_acl_end_of_support.rs`). That is what a walk
//! over the admitted values asserts here — they get PAST this parameter check — while
//! the default action of a web ACL that already exists is read back through `GetWebACL`,
//! so what is asserted of a stored action is the stored record.

use std::sync::Arc;

use serde_json::{json, Value};

use service_com_amazonaws_wafregional_awswaf_regional_20161128::ops::types::WAF_ACTION_TYPE_VALUES;
use service_com_amazonaws_wafregional_awswaf_regional_20161128::state::{
    service_state, CHANGE_TOKEN_PROVISIONED,
};

use crate::harness::{expected_arn, store_web_acl};
use crate::wire::{
    change_token, expect_error, ok_call, router, router_on, token_status, DEFAULT_TEST_REGION,
};

/// The `WafActionType` a web ACL's default action may not carry, written out here as a
/// literal so this suite pins the rule independently of the constant the handler reads.
const COUNT: &str = "COUNT";

/// A create body carrying `default_action`, with no change token (the fixtures mint
/// one).
fn request_with_action(name: &str, default_action: &str) -> Value {
    json!({
        "Name": name,
        "MetricName": "defaultAction",
        "DefaultAction": {"Type": default_action},
    })
}

/// A `DefaultAction` of `COUNT` is refused with the modelled
/// `WAFInvalidParameterException`, naming the member, the value and a modelled reason —
/// and the refused create stores nothing and consumes no change token.
#[tokio::test]
async fn count_is_refused_as_a_web_acl_default_action() {
    // The engine is held here as well as by the router, so the store a refused create
    // would have written to can be read directly.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(Arc::clone(&engine));

    let token = change_token(&router).await;
    let mut refused = request_with_action("count-by-default", COUNT);
    refused["ChangeToken"] = json!(token);
    let body = expect_error(
        &router,
        "CreateWebACL",
        &refused,
        "WAFInvalidParameterException",
        "COUNT as a default action",
    )
    .await;
    assert_eq!(
        body["field"].as_str(),
        Some("WAF_ACTION"),
        "case COUNT as a default action: the refusal must name the member it rejected, \
         got {body}"
    );
    assert_eq!(
        body["parameter"].as_str(),
        Some(COUNT),
        "case COUNT as a default action: the refusal must name the value it rejected, \
         got {body}"
    );
    assert_eq!(
        body["reason"].as_str(),
        Some("INVALID_OPTION"),
        "case COUNT as a default action: the refusal must carry a modelled \
         ParameterExceptionReason, got {body}"
    );

    // Nothing was stored: the token is still the one `GetChangeToken` hands out, and
    // the store holds no web ACL.
    assert_eq!(
        token_status(&router, &token).await,
        CHANGE_TOKEN_PROVISIONED,
        "case COUNT as a default action: a refused create consumes no change token"
    );
    assert_eq!(
        change_token(&router).await,
        token,
        "case COUNT as a default action: the outstanding token is still outstanding"
    );
    let stored = service_state(&*engine)
        .expect("the service state is reachable from the shared engine")
        .lock()
        .web_acls
        .len();
    assert_eq!(
        stored, 0,
        "case COUNT as a default action: a refused create must store nothing, \
         got {stored} stored web ACL(s)"
    );
}

/// The whole modelled catalog, walked from the GENERATED enum: every value a default
/// action may carry gets PAST this parameter check — it is answered by the service's
/// end-of-support refusal (`WAFBadRequestException`) rather than by
/// `WAFInvalidParameterException` — and the excluded one is refused by the check itself.
/// A value the model adds later becomes a row here without touching this test.
#[tokio::test]
async fn every_other_modelled_action_type_gets_past_the_parameter_check() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(Arc::clone(&engine));

    for action in WAF_ACTION_TYPE_VALUES {
        let expected = if *action == COUNT {
            "WAFInvalidParameterException"
        } else {
            "WAFBadRequestException"
        };
        let token = change_token(&router).await;
        let mut request = request_with_action(&format!("acl-{action}"), action);
        request["ChangeToken"] = json!(token);
        expect_error(&router, "CreateWebACL", &request, expected, action).await;

        let stored = service_state(&*engine)
            .expect("the service state is reachable from the shared engine")
            .lock()
            .web_acls
            .len();
        assert_eq!(
            stored, 0,
            "case {action}: a refused create must store nothing, got {stored} web ACL(s)"
        );
    }
}

/// A `DefaultAction.Type` the enum does not declare at all is refused by the modelled
/// constraint before any handler runs — the refusal of a value outside a catalog
/// belongs to the model, not to the handler.
#[tokio::test]
async fn a_default_action_outside_the_modelled_enum_is_refused_by_the_constraint() {
    let router = router();

    // Values differing in case, spelling and shape: none is a `WafActionType`.
    for (label, action) in [
        ("an action no AWS WAF version declares", "DENY"),
        ("the right word in the wrong case", "allow"),
        ("a value with separators in it", "ALLOW,BLOCK"),
    ] {
        let token = change_token(&router).await;
        let mut refused = request_with_action("outside-the-enum", action);
        refused["ChangeToken"] = json!(token);
        expect_error(
            &router,
            "CreateWebACL",
            &refused,
            "ValidationException",
            label,
        )
        .await;
        assert_eq!(
            token_status(&router, &token).await,
            CHANGE_TOKEN_PROVISIONED,
            "case {label}: a request refused at the boundary consumes no change token"
        );
    }
}

/// The default action of a web ACL that EXISTS is reported as it is stored, beside the
/// ARN naming the region that web ACL belongs to — the two are properties of the record,
/// and independent of each other.
#[tokio::test]
async fn a_stored_default_action_reads_back_beside_the_stored_arn() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(Arc::clone(&engine));

    for (region, action) in [
        ("eu-west-2", "ALLOW"),
        ("ap-south-1", "BLOCK"),
        (DEFAULT_TEST_REGION, "ALLOW"),
    ] {
        let web_acl_id = store_web_acl(
            &engine,
            region,
            "regional-acl",
            "defaultAction",
            action,
            &[],
        );
        let read = ok_call(&router, "GetWebACL", &json!({"WebACLId": &web_acl_id})).await;
        assert_eq!(
            read["WebACL"]["DefaultAction"]["Type"].as_str(),
            Some(action),
            "case {region}/{action}: the reported default action must be the stored one, \
             got {read}"
        );
        assert_eq!(
            read["WebACL"]["WebACLArn"].as_str(),
            Some(expected_arn(region, &web_acl_id).as_str()),
            "case {region}/{action}: expected the ARN of the record's own region, got {read}"
        );
    }
}
