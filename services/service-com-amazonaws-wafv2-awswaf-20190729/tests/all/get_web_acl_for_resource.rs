// SPDX-License-Identifier: CC0-1.0

//! What `GetWebACLForResource` reports for a protected resource, end to end over
//! the wire.
//!
//! The rule: for a protected-resource ARN of a kind a web ACL can be put in front
//! of, the call REPORTS the web ACL the live binding names — the same shape
//! `GetWebACL` answers for that web ACL, whole rule tree included — reports a null
//! web ACL for a resource that carries no binding, and refuses any other ARN with
//! `WAFInvalidParameterException`; being a report, it changes nothing a later
//! caller sees.
//!
//! The tables below span the class rather than the one recorded case: one row per
//! web ACL SHAPE (no rules, one rule, several rules, a nested statement tree), each
//! on a different documented resource form, and one row per way an ARN can fail the
//! parameter. Every call goes through the router's front door, so what these tests
//! observe is what an SDK observes.

use serde_json::{json, Value};

use crate::web_acl_associations::{listed_resources, web_acl_for_resource, PROTECTED_RESOURCES};
use crate::web_acl_lifecycle::{
    create_ip_set, create_web_acl, rule, sqli_statement, visibility, xss_statement, Waf,
};

/// [`rule`] at an explicit priority, so a web ACL can carry more than one rule
/// (`Rule$Priority` orders them and WAF takes no two rules at the same priority).
fn prioritized_rule(name: &str, priority: i64, statement: Value) -> Value {
    let mut built = rule(name, statement);
    built["Priority"] = json!(priority);
    built
}

/// One row per shape of stored web ACL the report has to hand back whole: an empty
/// rule list, one rule, several rules, and a rule whose statement nests
/// (`NotStatement` over an `OrStatement` of two leaves — the members
/// `smithy_query(query="shape",shape="NotStatement,OrStatement")` declares).
fn web_acl_shapes() -> Vec<(&'static str, Value)> {
    vec![
        ("a web ACL with no rules", json!([])),
        (
            "a web ACL with one rule",
            json!([rule("block-sqli", sqli_statement())]),
        ),
        (
            "a web ACL with two rules at different priorities",
            json!([
                prioritized_rule("block-sqli", 0, sqli_statement()),
                prioritized_rule("block-xss", 1, xss_statement()),
            ]),
        ),
        (
            "a web ACL whose statement tree nests",
            json!([rule(
                "block-neither",
                json!({
                    "NotStatement": {
                        "Statement": {
                            "OrStatement": { "Statements": [sqli_statement(), xss_statement()] }
                        }
                    }
                }),
            )]),
        ),
    ]
}

/// The report hands back the stored web ACL, member for member the same tree
/// `GetWebACL` answers — whatever shape that web ACL has, and whatever documented
/// resource form it is in front of.
#[tokio::test]
async fn reports_the_stored_web_acl_shape_get_web_acl_answers() {
    let waf = Waf::new();

    for (index, (case, rules)) in web_acl_shapes().into_iter().enumerate() {
        // A different documented resource form per row, so the report is exercised
        // over resource kinds as well as over web ACL shapes.
        let (resource_case, resource_arn, _resource_type) =
            PROTECTED_RESOURCES[index % PROTECTED_RESOURCES.len()];
        let case = format!("{case} in front of {resource_case}");
        let name = format!("report-acl-{index}");
        let (web_acl_id, web_acl_arn, _lock_token) =
            create_web_acl(&waf, "REGIONAL", &name, rules.clone()).await;

        waf.ok(
            "AssociateWebACL",
            json!({ "ResourceArn": resource_arn, "WebACLArn": web_acl_arn }),
        )
        .await;

        let reported = web_acl_for_resource(&waf, resource_arn)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the resource must report its web ACL"));
        let directly = waf.ok("GetWebACL", json!({ "ARN": web_acl_arn })).await;
        assert_eq!(
            reported, directly["WebACL"],
            "case {case:?}: expected the same web ACL GetWebACL answers \
             ({}), got {reported}",
            directly["WebACL"]
        );
        assert_eq!(
            reported["Rules"], rules,
            "case {case:?}: expected the stored rule tree {rules}, got {reported}"
        );
        assert_eq!(
            reported["Id"].as_str(),
            Some(web_acl_id.as_str()),
            "case {case:?}: expected the web ACL id {web_acl_id}, got {reported}"
        );
        assert_eq!(
            reported["Name"].as_str(),
            Some(name.as_str()),
            "case {case:?}: expected the web ACL name {name}, got {reported}"
        );

        waf.ok(
            "DisassociateWebACL",
            json!({ "ResourceArn": resource_arn }),
        )
        .await;
        let after = web_acl_for_resource(&waf, resource_arn).await;
        assert!(
            after.is_none(),
            "case {case:?}: a resource whose binding was taken away must report a \
             null web ACL, got {after:?}"
        );
    }
}

/// The report follows the STORED web ACL: an update to the web ACL shows up in what
/// the resource reports, without the binding being written again.
#[tokio::test]
async fn the_report_follows_the_stored_web_acl_through_an_update() {
    let waf = Waf::new();
    let (web_acl_id, web_acl_arn, create_lock) =
        create_web_acl(&waf, "REGIONAL", "updated-acl", json!([])).await;
    let (_case, resource_arn, _resource_type) = PROTECTED_RESOURCES[0];

    waf.ok(
        "AssociateWebACL",
        json!({ "ResourceArn": resource_arn, "WebACLArn": web_acl_arn }),
    )
    .await;

    // One row per generation of the stored web ACL, each with its own rule tree and
    // description, applied with the token the previous generation handed back.
    let generations = web_acl_shapes();
    let mut lock_token = create_lock;
    for (case, rules) in &generations {
        let description = format!("generation of {case}");
        let updated = waf
            .ok(
                "UpdateWebACL",
                json!({
                    "Name": "updated-acl",
                    "Scope": "REGIONAL",
                    "Id": web_acl_id,
                    "DefaultAction": { "Allow": {} },
                    "VisibilityConfig": visibility("updated-acl"),
                    "LockToken": lock_token,
                    "Description": description,
                    "Rules": rules,
                }),
            )
            .await;
        lock_token = updated["NextLockToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: UpdateWebACL answers a NextLockToken"))
            .to_string();

        let reported = web_acl_for_resource(&waf, resource_arn)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the resource must report its web ACL"));
        assert_eq!(
            reported["Rules"], *rules,
            "case {case:?}: expected the updated rule tree {rules}, got {reported}"
        );
        assert_eq!(
            reported["Description"].as_str(),
            Some(description.as_str()),
            "case {case:?}: expected the updated description {description:?}, got {reported}"
        );
        let directly = waf.ok("GetWebACL", json!({ "ARN": web_acl_arn })).await;
        assert_eq!(
            reported, directly["WebACL"],
            "case {case:?}: expected the same web ACL GetWebACL answers, got {reported}"
        );
    }
}

/// A report changes nothing a later caller sees: reading a bound resource over and
/// over answers the same body and leaves the lock token, the binding and the
/// listing where they were, and reading an UNBOUND resource binds nothing.
#[tokio::test]
async fn a_report_changes_nothing_a_later_caller_sees() {
    let waf = Waf::new();
    let (_web_acl_id, web_acl_arn, lock_token) = create_web_acl(
        &waf,
        "REGIONAL",
        "read-only-acl",
        json!([rule("block-sqli", sqli_statement())]),
    )
    .await;
    let (bound_case, bound_arn, bound_type) = PROTECTED_RESOURCES[0];
    let (unbound_case, unbound_arn, unbound_type) = PROTECTED_RESOURCES[1];

    waf.ok(
        "AssociateWebACL",
        json!({ "ResourceArn": bound_arn, "WebACLArn": web_acl_arn }),
    )
    .await;

    let first = web_acl_for_resource(&waf, bound_arn)
        .await
        .unwrap_or_else(|| panic!("case {bound_case:?}: the resource must report its web ACL"));
    for read in 2..=4 {
        let again = web_acl_for_resource(&waf, bound_arn).await;
        assert_eq!(
            again.as_ref(),
            Some(&first),
            "case {bound_case:?}: read {read} must answer the same web ACL as the \
             first, got {again:?}"
        );
        let unbound = web_acl_for_resource(&waf, unbound_arn).await;
        assert!(
            unbound.is_none(),
            "case {unbound_case:?}: read {read} of an unbound resource must answer a \
             null web ACL, got {unbound:?}"
        );
    }

    // The reads consumed no lock token and moved no binding.
    let directly = waf.ok("GetWebACL", json!({ "ARN": web_acl_arn })).await;
    assert_eq!(
        directly["LockToken"].as_str(),
        Some(lock_token.as_str()),
        "the reads must leave the lock token at {lock_token}, got {directly}"
    );
    let listed = listed_resources(&waf, &web_acl_arn, Some(bound_type)).await;
    assert_eq!(
        listed,
        vec![bound_arn.to_string()],
        "case {bound_case:?}: the reads must leave the listing reporting only \
         {bound_arn}, got {listed:?}"
    );
    let unbound_listing = listed_resources(&waf, &web_acl_arn, Some(unbound_type)).await;
    assert!(
        unbound_listing.is_empty(),
        "case {unbound_case:?}: reading an unbound resource must bind nothing, yet \
         {unbound_type} lists {unbound_listing:?}"
    );
}

/// An ARN the parameter does not accept is refused with the structured
/// `WAFInvalidParameterException`, and no such ARN is ever reported as carrying a
/// web ACL.
#[tokio::test]
async fn an_arn_no_web_acl_can_protect_is_refused() {
    let waf = Waf::new();
    let (_id, web_acl_arn, _lock) =
        create_web_acl(&waf, "REGIONAL", "refused-report-acl", json!([])).await;
    let (_ip_set_id, ip_set_arn, _ip_set_lock) =
        create_ip_set(&waf, "REGIONAL", "not-a-protected-resource").await;
    let (_case, bound_arn, _bound_type) = PROTECTED_RESOURCES[0];
    waf.ok(
        "AssociateWebACL",
        json!({ "ResourceArn": bound_arn, "WebACLArn": web_acl_arn }),
    )
    .await;

    // One row per way this parameter fails. The accepted forms are the eight the
    // `ResourceArn` member lists and the `ResourceType` enum names
    // (`smithy_query(query="doc",shape="GetWebACLForResourceRequest",member="ResourceArn")`),
    // so each row below is a resource OUTSIDE that catalogue, or a layout inside it
    // with a segment wrong. Each is at least the 20 characters the member requires,
    // so what refuses it is this parameter's judgement and not the length
    // constraint.
    let cases: &[(&str, &str)] = &[
        ("a string that is not an ARN at all", "nowhere-near-an-arn-at-all"),
        (
            "a CloudFront distribution, bound through its own configuration",
            "arn:aws:cloudfront::123456789012:distribution/E1XMPLDISTRIBUTION",
        ),
        (
            "a Global Accelerator accelerator",
            "arn:aws:globalaccelerator::123456789012:accelerator/1234abcd-abcd-1234-abcd-1234abcdefgh",
        ),
        (
            "an EC2 instance, which is not a protectable kind of ec2 resource",
            "arn:aws:ec2:us-west-2:123456789012:instance/i-0abcd1234ef567890",
        ),
        (
            "an AppSync API with an extra segment",
            "arn:aws:appsync:us-west-2:123456789012:apis/abcdefghij/schema",
        ),
        (
            "an Amplify branch rather than an application",
            "arn:aws:amplify:us-west-2:123456789012:apps/d1a2b3c4d5e6f7/branches/main",
        ),
        (
            "a REST API stage whose stage segment is empty",
            "arn:aws:apigateway:us-west-2::/restapis/a1b2c3d4e5/stages/",
        ),
        (
            "this service's own IP set, which is not a protected resource",
            ip_set_arn.as_str(),
        ),
        (
            "a web ACL ARN, which names no resource a web ACL is in front of",
            web_acl_arn.as_str(),
        ),
    ];

    for (case, resource_arn) in cases {
        let refused = waf
            .refused(
                "GetWebACLForResource",
                json!({ "ResourceArn": resource_arn }),
                "WAFInvalidParameterException",
                case,
            )
            .await;
        assert_eq!(
            refused["Field"].as_str(),
            Some("RESOURCE_ARN"),
            "case {case:?}: the refusal must name the resource ARN field, got {refused}"
        );
        assert_eq!(
            refused["Parameter"].as_str(),
            Some(*resource_arn),
            "case {case:?}: the refusal must carry the rejected ARN, got {refused}"
        );
        assert!(
            refused["Reason"].as_str().is_some_and(|reason| !reason.is_empty()),
            "case {case:?}: the refusal must carry a reason, got {refused}"
        );
    }

    // None of those refusals disturbed the binding that does exist.
    let still_bound = web_acl_for_resource(&waf, bound_arn).await;
    assert_eq!(
        still_bound.as_ref().and_then(|acl| acl["ARN"].as_str()),
        Some(web_acl_arn.as_str()),
        "the refusals must leave the binding of {bound_arn} alone, got {still_bound:?}"
    );
}
