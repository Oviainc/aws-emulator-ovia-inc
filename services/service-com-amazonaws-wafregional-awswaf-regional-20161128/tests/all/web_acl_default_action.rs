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
//! so a value the model adds later cannot slip past untested. Accepted values are read
//! back through `GetWebACL`, so what is asserted is the stored record.

use std::sync::Arc;

use serde_json::{json, Value};

use service_com_amazonaws_wafregional_awswaf_regional_20161128::ops::types::WAF_ACTION_TYPE_VALUES;
use service_com_amazonaws_wafregional_awswaf_regional_20161128::state::{
    service_state, CHANGE_TOKEN_PROVISIONED,
};

use crate::harness::{create, create_in_region, created_web_acl, expected_arn};
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
/// action may carry is stored and read back; the excluded one is refused. A value the
/// model adds later becomes a row here without touching this test.
#[tokio::test]
async fn every_other_modelled_action_type_is_accepted_and_reads_back() {
    let router = router();

    for action in WAF_ACTION_TYPE_VALUES {
        if *action == COUNT {
            continue;
        }
        let request = request_with_action(&format!("acl-{action}"), action);
        let (status, body, _) = create(&router, &request).await;
        let created = created_web_acl(status, &body, action);
        let web_acl_id = created["WebACLId"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            created["DefaultAction"]["Type"].as_str(),
            Some(*action),
            "case {action}: the stored default action must be the one created, got {created}"
        );
        // The round trip: the reader reports the stored record, not the request.
        let read = ok_call(&router, "GetWebACL", &json!({"WebACLId": &web_acl_id})).await;
        assert_eq!(
            read["WebACL"], created,
            "case {action}: GetWebACL must read back the created WebACL, got {read}"
        );
        assert_eq!(
            read["WebACL"]["WebACLArn"].as_str(),
            Some(expected_arn(DEFAULT_TEST_REGION, &web_acl_id).as_str()),
            "case {action}: the stored ARN must name the region the create was signed \
             for, got {read}"
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

/// The region a create is signed for is the region the stored ARN names, whatever the
/// default action — the two rules are independent.
#[tokio::test]
async fn the_stored_arn_names_the_signed_region_for_every_default_action() {
    let router = router();

    for (region, action) in [("eu-west-2", "ALLOW"), ("ap-south-1", "BLOCK")] {
        let (status, body, _) =
            create_in_region(&router, region, &request_with_action("regional-acl", action)).await;
        let created = created_web_acl(status, &body, region);
        let web_acl_id = created["WebACLId"].as_str().unwrap_or_default().to_string();
        assert_eq!(
            created["WebACLArn"].as_str(),
            Some(expected_arn(region, &web_acl_id).as_str()),
            "case {region}: expected the ARN for the signed region, got {created}"
        );
        let read = ok_call(&router, "GetWebACL", &json!({"WebACLId": &web_acl_id})).await;
        assert_eq!(
            read["WebACL"], created,
            "case {region}: GetWebACL must read back the created WebACL, got {read}"
        );
    }
}
