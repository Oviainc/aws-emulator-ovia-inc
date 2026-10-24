// SPDX-License-Identifier: CC0-1.0

//! AWS WAF Classic's end of support, end to end over the wire: `CreateWebACL` admits no
//! new web ACL.
//!
//! AWS WAF Classic (AWS WAF v1, of which these regional endpoints are the regional
//! half) reached end of support on 2025-09-30 (`state::END_OF_SUPPORT_DATE`), and its
//! control plane refuses every `CreateWebACL` with the modelled
//! `WAFBadRequestException` (HTTP 400) — a SERVICE-lifecycle refusal rather than a
//! per-parameter one, so it answers however well formed the request is and however often
//! it is retried. Source: recorded behaviour of the service's own regional endpoint.
//!
//! What this suite pins:
//!
//! * the refusal's status (400) and code (`WAFBadRequestException`) over a table of
//!   well-formed requests spanning every `DefaultAction` the catalog allows for a
//!   default action, the `Name`/`MetricName` length and form bounds, requests with and
//!   without `Tags`, and more than one signed region;
//! * that it changes NOTHING: no web ACL is stored, no id is spent, and the change token
//!   the request carried is still the outstanding one — still usable by the mutating call
//!   it was minted for;
//! * the ORDER of the two refusals: this request's own per-parameter refusals (a
//!   disallowed `MetricName`, the one `WafActionType` a default action may not carry) come
//!   first, and only a request whose parameters are all acceptable reaches the
//!   end-of-support refusal;
//! * that the web ACLs which already existed go on reading back unchanged across a
//!   refused create.
//!
//! Every call goes through the real router (see `tests/all/wire.rs`), so what these
//! tests assert is what an SDK client sees.

use std::sync::Arc;

use serde_json::{json, Value};

use service_com_amazonaws_wafregional_awswaf_regional_20161128::ops::create_web_acl::DEFAULT_ACTION_EXCLUDED;
use service_com_amazonaws_wafregional_awswaf_regional_20161128::ops::types::WAF_ACTION_TYPE_VALUES;
use service_com_amazonaws_wafregional_awswaf_regional_20161128::state::{
    service_state, MAX_TAGS_PER_RESOURCE, MESSAGE_DISALLOWED_NAME, MESSAGE_END_OF_SUPPORT,
    RESERVED_METRIC_NAMES,
};

use crate::harness::{expected_arn, store_web_acl};
use crate::wire::{
    call_in_region, change_token, error_code, error_message, expect_error, ok_call, router_on,
    token_status, DEFAULT_TEST_REGION,
};

/// The `ChangeTokenStatus` of a token no mutating call has used.
const PROVISIONED: &str = "PROVISIONED";

/// One row of the refusal table: a label, the `Name`, the `MetricName`, the
/// `DefaultAction.Type`, the `Tags` the request carries (if any) and the region it is
/// signed for.
struct Case {
    label: &'static str,
    name: String,
    metric_name: String,
    default_action: &'static str,
    tags: Option<Value>,
    region: &'static str,
}

/// How many web ACLs the store holds.
fn stored_web_acls(engine: &engine::InMemoryEngine) -> usize {
    service_state(engine)
        .expect("the service state is reachable from the engine")
        .lock()
        .web_acls
        .len()
}

/// A `CreateWebACL` body for `case`, carrying `token`.
fn request_for(case: &Case, token: &str) -> Value {
    let mut body = json!({
        "Name": case.name,
        "MetricName": case.metric_name,
        "DefaultAction": {"Type": case.default_action},
        "ChangeToken": token,
    });
    if let Some(tags) = &case.tags {
        body["Tags"] = tags.clone();
    }
    body
}

/// Every well-formed `CreateWebACL` is refused with HTTP 400 and
/// `WAFBadRequestException`, stores nothing and spends no change token — and a retry of
/// the identical request is refused identically.
#[tokio::test]
async fn every_well_formed_create_web_acl_is_refused_as_a_bad_request() {
    // The engine is held here as well as by the router, so the store a create would
    // have written to can be read directly.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(Arc::clone(&engine));

    let cases = vec![
        Case {
            label: "allow by default",
            name: "bootstrap-webacl".to_string(),
            metric_name: "bootstrapWebACL".to_string(),
            default_action: "ALLOW",
            tags: None,
            region: DEFAULT_TEST_REGION,
        },
        Case {
            label: "block by default",
            name: "edge-block".to_string(),
            metric_name: "edgeBlock".to_string(),
            default_action: "BLOCK",
            tags: None,
            region: DEFAULT_TEST_REGION,
        },
        Case {
            label: "one character of each (the minimum modelled length)",
            name: "x".to_string(),
            metric_name: "m".to_string(),
            default_action: "ALLOW",
            tags: None,
            region: DEFAULT_TEST_REGION,
        },
        Case {
            label: "128 characters of each (the maximum modelled length)",
            name: "n".repeat(128),
            metric_name: "M".repeat(128),
            default_action: "BLOCK",
            tags: None,
            region: DEFAULT_TEST_REGION,
        },
        Case {
            label: "a name with mixed case, digits and separators",
            name: "Prod_WebACL-2".to_string(),
            metric_name: "ProdWebACL2".to_string(),
            default_action: "ALLOW",
            tags: None,
            region: DEFAULT_TEST_REGION,
        },
        Case {
            label: "an interior space in the name",
            name: "corp office acl".to_string(),
            metric_name: "corpOffice".to_string(),
            default_action: "BLOCK",
            tags: None,
            region: DEFAULT_TEST_REGION,
        },
        Case {
            label: "one tag",
            name: "tagged-acl".to_string(),
            metric_name: "taggedAcl".to_string(),
            default_action: "ALLOW",
            tags: Some(json!([{"Key": "team", "Value": "edge"}])),
            region: DEFAULT_TEST_REGION,
        },
        Case {
            label: "the most tags a resource may carry",
            name: "heavily-tagged-acl".to_string(),
            metric_name: "heavilyTaggedAcl".to_string(),
            default_action: "BLOCK",
            tags: Some(Value::Array(
                (0..MAX_TAGS_PER_RESOURCE)
                    .map(|i| json!({"Key": format!("key-{i}"), "Value": format!("value-{i}")}))
                    .collect(),
            )),
            region: DEFAULT_TEST_REGION,
        },
        Case {
            label: "signed for another commercial region",
            name: "regional-acl".to_string(),
            metric_name: "regionalAcl".to_string(),
            default_action: "ALLOW",
            tags: None,
            region: "eu-west-3",
        },
        Case {
            label: "signed for a longer region name",
            name: "regional-acl".to_string(),
            metric_name: "regionalAcl".to_string(),
            default_action: "BLOCK",
            tags: None,
            region: "ap-southeast-4",
        },
    ];

    // The table spans the whole GENERATED catalog of actions a default action may carry:
    // every `WafActionType` except the one value `WafAction$Type` excludes from a
    // default action (which `the_two_refusals_answer_in_order` covers) is a row here, so
    // a value the model adds later cannot slip past untested.
    for action in WAF_ACTION_TYPE_VALUES
        .iter()
        .filter(|action| **action != DEFAULT_ACTION_EXCLUDED)
    {
        assert!(
            cases.iter().any(|case| case.default_action == *action),
            "the table must cover the default-action WafActionType {action:?}"
        );
    }

    let mut messages: Vec<String> = Vec::new();
    for case in &cases {
        let label = case.label;
        let token = change_token(&router).await;
        let request = request_for(case, &token);

        // Twice over: the refusal is a property of the service, so a retry of the same
        // request is answered the same way rather than drifting into a stale-token or
        // already-exists refusal.
        let mut answers: Vec<(u16, Value)> = Vec::new();
        for attempt in 1..=2 {
            let (status, body) = call_in_region(&router, case.region, "CreateWebACL", &request).await;
            assert_eq!(
                status, 400,
                "case {label} (attempt {attempt}): CreateWebACL must answer 400, \
                 got {status}: {body}"
            );
            assert!(
                error_code(&body).contains("WAFBadRequestException"),
                "case {label} (attempt {attempt}): expected WAFBadRequestException, got {} \
                 ({body})",
                error_code(&body)
            );
            answers.push((status, body));
        }
        assert_eq!(
            answers[0], answers[1],
            "case {label}: the second attempt must be refused exactly like the first, \
             got {:?} then {:?}",
            answers[0], answers[1]
        );

        // The message is one fixed sentence for the class, carrying nothing of the
        // request.
        let text = error_message(&answers[0].1);
        assert_eq!(
            text, MESSAGE_END_OF_SUPPORT,
            "case {label}: the refusal must carry the end-of-support sentence, got {text:?}"
        );
        for echoed in [case.name.as_str(), case.metric_name.as_str(), token.as_str()] {
            assert!(
                !text.contains(echoed),
                "case {label}: the refusal must not echo {echoed:?} from the request, \
                 got {text:?}"
            );
        }
        messages.push(text);

        // Nothing changed: no record, and the token the request carried is still the
        // outstanding one.
        let stored = stored_web_acls(&engine);
        assert_eq!(
            stored, 0,
            "case {label}: a refused create must store nothing, got {stored} web ACL(s)"
        );
        assert_eq!(
            token_status(&router, &token).await,
            PROVISIONED,
            "case {label}: a refused create must not consume the change token"
        );
        assert_eq!(
            change_token(&router).await,
            token,
            "case {label}: the token the refused create carried must still be the \
             outstanding one"
        );
    }

    assert!(
        messages.windows(2).all(|pair| pair[0] == pair[1]),
        "every WAFBadRequestException must carry the same sentence, got {messages:?}"
    );
}

/// The two refusals answer in order: a request this operation's own parameter rules
/// reject is refused by THEM, and only a request whose parameters are all acceptable
/// reaches the end-of-support refusal.
///
/// `MetricName` "can contain only alphanumeric characters (A-Z, a-z, 0-9) ... It can't
/// contain whitespace or metric names reserved for AWS WAF" (the model's `MetricName`
/// documentation) and `WafAction$Type` states "You can't specify `COUNT` for the default
/// action for a `WebACL`".
#[tokio::test]
async fn the_two_refusals_answer_in_order() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(Arc::clone(&engine));

    // (label, MetricName, DefaultAction.Type, the error code expected)
    let mut cases: Vec<(String, String, &str, &str)> = vec![
        (
            "an acceptable metric name and an acceptable default action".to_string(),
            "orderProbe".to_string(),
            "ALLOW",
            "WAFBadRequestException",
        ),
        (
            "the excluded default action".to_string(),
            "orderProbe".to_string(),
            DEFAULT_ACTION_EXCLUDED,
            "WAFInvalidParameterException",
        ),
        (
            "a metric name with an interior space".to_string(),
            "order probe".to_string(),
            "ALLOW",
            "WAFDisallowedNameException",
        ),
        (
            "a metric name with a separator".to_string(),
            "order-probe".to_string(),
            "BLOCK",
            "WAFDisallowedNameException",
        ),
        (
            "a metric name outside ASCII".to_string(),
            "orderProbé".to_string(),
            "ALLOW",
            "WAFDisallowedNameException",
        ),
        (
            "a disallowed metric name AND the excluded default action".to_string(),
            "order probe".to_string(),
            DEFAULT_ACTION_EXCLUDED,
            "WAFDisallowedNameException",
        ),
    ];
    // Every name the service reserves for AWS WAF is refused as a disallowed name,
    // whatever the default action: the catalog is the generated `RESERVED_METRIC_NAMES`,
    // so a name added to it becomes a row here on its own.
    for reserved in RESERVED_METRIC_NAMES {
        cases.push((
            format!("the reserved metric name {reserved}"),
            (*reserved).to_string(),
            "ALLOW",
            "WAFDisallowedNameException",
        ));
    }

    for (label, metric_name, default_action, expected) in &cases {
        let token = change_token(&router).await;
        let body = expect_error(
            &router,
            "CreateWebACL",
            &json!({
                "Name": "order-probe",
                "MetricName": metric_name,
                "DefaultAction": {"Type": default_action},
                "ChangeToken": token,
            }),
            expected,
            label,
        )
        .await;

        if *expected == "WAFInvalidParameterException" {
            assert_eq!(
                body["field"].as_str(),
                Some("WAF_ACTION"),
                "case {label}: the refusal must name the member it rejected, got {body}"
            );
            assert_eq!(
                body["parameter"].as_str(),
                Some(*default_action),
                "case {label}: the refusal must name the value it rejected, got {body}"
            );
            assert_eq!(
                body["reason"].as_str(),
                Some("INVALID_OPTION"),
                "case {label}: the refusal must carry a modelled ParameterExceptionReason, \
                 got {body}"
            );
        }
        if *expected == "WAFDisallowedNameException" {
            assert_eq!(
                error_message(&body),
                MESSAGE_DISALLOWED_NAME,
                "case {label}: the refusal must carry the disallowed-name sentence, got {:?}",
                error_message(&body)
            );
        }

        // Whichever refusal answered, it changed nothing.
        let stored = stored_web_acls(&engine);
        assert_eq!(
            stored, 0,
            "case {label}: a refused create must store nothing, got {stored} web ACL(s)"
        );
        assert_eq!(
            token_status(&router, &token).await,
            PROVISIONED,
            "case {label}: a refused create must not consume the change token"
        );
    }
}

/// The change token a refused `CreateWebACL` carried is still usable: the refusal comes
/// before the change-token protocol, so the token is spent by the next mutating call
/// that uses it — and a token some OTHER call has already spent still gets the
/// end-of-support refusal rather than a stale-token one.
#[tokio::test]
async fn a_refused_create_leaves_its_change_token_usable() {
    let router = router_on(Arc::new(engine::InMemoryEngine::new()));

    let token = change_token(&router).await;
    for attempt in ["the first attempt", "the second attempt"] {
        expect_error(
            &router,
            "CreateWebACL",
            &json!({
                "Name": "token-probe",
                "MetricName": "tokenProbe",
                "DefaultAction": {"Type": "ALLOW"},
                "ChangeToken": token,
            }),
            "WAFBadRequestException",
            attempt,
        )
        .await;
    }

    // The token was never consumed, so another mutating call takes it.
    let created = ok_call(
        &router,
        "CreateIPSet",
        &json!({"Name": "token-probe-set", "ChangeToken": token}),
    )
    .await;
    assert_eq!(
        created["ChangeToken"].as_str(),
        Some(token.as_str()),
        "the token a refused CreateWebACL carried must still be usable, got {created}"
    );

    // Now consumed — and the service-lifecycle refusal still answers, because it is
    // decided before the token is looked at.
    expect_error(
        &router,
        "CreateWebACL",
        &json!({
            "Name": "token-probe",
            "MetricName": "tokenProbe",
            "DefaultAction": {"Type": "ALLOW"},
            "ChangeToken": token,
        }),
        "WAFBadRequestException",
        "a token another call has spent",
    )
    .await;
}

/// The web ACLs that already exist are untouched by a refused create: each reads back
/// field for field, before and after, and no record is added.
#[tokio::test]
async fn a_refused_create_leaves_the_existing_web_acls_as_they_were() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(Arc::clone(&engine));

    // (label, Name, MetricName, DefaultAction.Type, the region the record's ARN names)
    let existing = [
        ("an allowing web ACL", "existing-allow", "existingAllow", "ALLOW", DEFAULT_TEST_REGION),
        ("a blocking web ACL", "existing-block", "existingBlock", "BLOCK", "eu-west-2"),
    ];
    let mut ids: Vec<(&str, String)> = Vec::new();
    for (label, name, metric_name, action, region) in existing {
        let id = store_web_acl(&engine, region, name, metric_name, action, &[]);
        let read = ok_call(&router, "GetWebACL", &json!({"WebACLId": &id})).await;
        assert_eq!(
            read["WebACL"]["Name"].as_str(),
            Some(name),
            "case {label}: GetWebACL must report the stored name, got {read}"
        );
        assert_eq!(
            read["WebACL"]["DefaultAction"]["Type"].as_str(),
            Some(action),
            "case {label}: GetWebACL must report the stored default action, got {read}"
        );
        assert_eq!(
            read["WebACL"]["WebACLArn"].as_str(),
            Some(expected_arn(region, &id).as_str()),
            "case {label}: GetWebACL must report the stored ARN, got {read}"
        );
        ids.push((label, id));
    }

    let before: Vec<Value> = {
        let mut snapshot = Vec::new();
        for (_, id) in &ids {
            snapshot.push(ok_call(&router, "GetWebACL", &json!({"WebACLId": id})).await);
        }
        snapshot
    };

    let token = change_token(&router).await;
    expect_error(
        &router,
        "CreateWebACL",
        &json!({
            "Name": "one-more-web-acl",
            "MetricName": "oneMoreWebAcl",
            "DefaultAction": {"Type": "ALLOW"},
            "ChangeToken": token,
        }),
        "WAFBadRequestException",
        "a create beside existing web ACLs",
    )
    .await;

    let stored = stored_web_acls(&engine);
    assert_eq!(
        stored, ids.len(),
        "a refused create must add no record, got {stored} web ACL(s) for {} existing",
        ids.len()
    );
    for (index, (label, id)) in ids.iter().enumerate() {
        let after = ok_call(&router, "GetWebACL", &json!({"WebACLId": id})).await;
        assert_eq!(
            after, before[index],
            "case {label}: the refused create must leave the record as it was, got {after} \
             after {}",
            before[index]
        );
    }
}
