// SPDX-License-Identifier: CC0-1.0

//! One web ACL in front of MANY protected resources, and refusals that bind
//! nothing — the fan-out side of `AssociateWebACL`, end to end over the wire.
//!
//! The rule: `AssociateWebACL` stores ONE binding keyed by the protected resource
//! the request names, so a web ACL is reported in front of every resource bound to
//! it, up to the number of resources this deployment holds bindings for, and a call
//! that is refused leaves every binding — including one the named resource already
//! carries — exactly as it was.
//!
//! The tables below span the class rather than one case: one row per
//! protected-resource form (including one the report did not cite), fanned out to
//! the bound, one resource past it, and one row per way the named web ACL can fail.
//! The harness is the one in [`crate::web_acl_lifecycle`], so every call goes
//! through the router's front door.

use serde_json::json;
use service_com_amazonaws_wafv2_awswaf_20190729::state::MAX_PROTECTED_RESOURCES_PER_WEB_ACL;

use crate::web_acl_associations::{listed_resources, web_acl_for_resource};
use crate::web_acl_lifecycle::{create_web_acl, Waf};

/// The protected-resource forms this suite fans out over: a label, the
/// `ResourceType` the form names, and the ARN layout, whose `{i}` each row of the
/// fan-out replaces with the resource's index.
///
/// The layouts are three of the formats the `ResourceArn` member documents
/// (`smithy_query(query="doc",shape="AssociateWebACLRequest",member="ResourceArn")`)
/// and the type names are `ResourceType` values
/// (`smithy_query(query="shape",shape="ResourceType")`); the AgentCore gateway is a
/// kind the report did not cite.
const FANOUT_FORMS: &[(&str, &str, &str)] = &[
    (
        "application load balancers",
        "APPLICATION_LOAD_BALANCER",
        "arn:aws:elasticloadbalancing:us-west-2:123456789012:loadbalancer/app/fanout-{i}/50dc6c495c0c9{i}",
    ),
    (
        "Cognito user pools",
        "COGNITO_USER_POOL",
        "arn:aws:cognito-idp:us-west-2:123456789012:userpool/us-west-2_fanout{i}",
    ),
    (
        "Bedrock AgentCore gateways",
        "AGENTCORE_GATEWAY",
        "arn:aws:bedrock-agentcore:us-west-2:123456789012:gateway/gw-fanout{i}",
    ),
];

/// The `index`th protected-resource ARN of the form `layout`.
fn fanout_arn(layout: &str, index: usize) -> String {
    layout.replace("{i}", &index.to_string())
}

/// A web ACL is put in front of every resource bound to it, the listing reports
/// them all, and the binding that would take this deployment past the number of
/// resources one web ACL holds bindings for is refused and stores nothing.
#[tokio::test]
async fn one_web_acl_fronts_resources_up_to_the_bound_and_refuses_the_next() {
    let waf = Waf::new();

    for (case, resource_type, layout) in FANOUT_FORMS {
        let name = format!("fanout-{}", resource_type.to_lowercase());
        let (web_acl_id, web_acl_arn, _lock_token) =
            create_web_acl(&waf, "REGIONAL", &name, json!([])).await;

        let mut bound_arns: Vec<String> = Vec::new();
        for index in 0..MAX_PROTECTED_RESOURCES_PER_WEB_ACL {
            let resource_arn = fanout_arn(layout, index);
            waf.ok(
                "AssociateWebACL",
                json!({ "ResourceArn": resource_arn, "WebACLArn": web_acl_arn }),
            )
            .await;
            bound_arns.push(resource_arn);

            // The listing reports as many resources as there are bindings, from the
            // very first one onwards — not only once the fan-out is complete.
            if index < 2 || index == MAX_PROTECTED_RESOURCES_PER_WEB_ACL / 2 {
                let listed = listed_resources(&waf, &web_acl_arn, Some(resource_type)).await;
                assert_eq!(
                    listed.len(),
                    bound_arns.len(),
                    "case {case:?}: {} binding(s) must be listed, got {listed:?}",
                    bound_arns.len()
                );
            }
        }
        bound_arns.sort();

        let listed = listed_resources(&waf, &web_acl_arn, Some(resource_type)).await;
        assert_eq!(
            listed, bound_arns,
            "case {case:?}: every bound resource must be listed, got {} entr(ies)",
            listed.len()
        );

        // Each of those resources reports the web ACL that is in front of it.
        for resource_arn in [
            bound_arns.first().expect("the fan-out bound a resource"),
            bound_arns.last().expect("the fan-out bound a resource"),
        ] {
            let reported = web_acl_for_resource(&waf, resource_arn)
                .await
                .unwrap_or_else(|| {
                    panic!("case {case:?}: {resource_arn} must report its web ACL")
                });
            assert_eq!(
                reported["Id"].as_str(),
                Some(web_acl_id.as_str()),
                "case {case:?}: {resource_arn} must report the web ACL {web_acl_id}, \
                 got {reported}"
            );
        }

        // One resource past the bound: refused, and nothing is stored for it.
        let past_bound = fanout_arn(layout, MAX_PROTECTED_RESOURCES_PER_WEB_ACL);
        waf.refused(
            "AssociateWebACL",
            json!({ "ResourceArn": past_bound, "WebACLArn": web_acl_arn }),
            "WAFLimitsExceededException",
            case,
        )
        .await;
        let unbound = web_acl_for_resource(&waf, &past_bound).await;
        assert!(
            unbound.is_none(),
            "case {case:?}: a refused association must bind nothing, got {unbound:?}"
        );

        // A web ACL at the bound can still be re-bound to a resource it already
        // protects: that stores no further binding.
        let already_bound = bound_arns[0].clone();
        waf.ok(
            "AssociateWebACL",
            json!({ "ResourceArn": already_bound, "WebACLArn": web_acl_arn }),
        )
        .await;
        let after = listed_resources(&waf, &web_acl_arn, Some(resource_type)).await;
        assert_eq!(
            after, bound_arns,
            "case {case:?}: re-binding a resource the web ACL already protects must \
             change no binding, got {} entr(ies)",
            after.len()
        );
    }
}

/// A refused `AssociateWebACL` leaves the binding the named resource already
/// carries, whichever way the named web ACL fails.
#[tokio::test]
async fn a_refused_association_leaves_the_binding_the_resource_already_carries() {
    let waf = Waf::new();
    let (held_id, held_arn, _held_token) =
        create_web_acl(&waf, "REGIONAL", "holds-the-resource", json!([])).await;
    let (_global_id, cloudfront_arn, _global_token) =
        create_web_acl(&waf, "CLOUDFRONT", "distribution-acl", json!([])).await;
    let resource_arn = fanout_arn(FANOUT_FORMS[0].2, 7);

    waf.ok(
        "AssociateWebACL",
        json!({ "ResourceArn": resource_arn, "WebACLArn": held_arn }),
    )
    .await;

    // One row per way `WebACLArn` fails: a well-formed wafv2 web ACL ARN naming
    // nothing stored, a web ACL of the scope this operation does not bind, and an
    // ARN that is not a wafv2 resource at all.
    let cases: &[(&str, &str, &str)] = &[
        (
            "a web ACL ARN naming nothing this account holds",
            "arn:aws:wafv2:us-west-2:123456789012:regional/webacl/ghost/12345678-1234-1234-1234-123456789012",
            "WAFNonexistentItemException",
        ),
        (
            "a CLOUDFRONT-scoped web ACL",
            cloudfront_arn.as_str(),
            "WAFInvalidParameterException",
        ),
        (
            "an ARN that is not a wafv2 resource",
            "arn:aws:elasticloadbalancing:us-west-2:123456789012:loadbalancer/app/not-an-acl/50dc6c495c0c9188",
            "WAFInvalidParameterException",
        ),
    ];

    for (case, web_acl_arn, code) in cases {
        waf.refused(
            "AssociateWebACL",
            json!({ "ResourceArn": resource_arn, "WebACLArn": web_acl_arn }),
            code,
            case,
        )
        .await;
        let reported = web_acl_for_resource(&waf, &resource_arn)
            .await
            .unwrap_or_else(|| {
                panic!("case {case:?}: the resource must still report the web ACL it carries")
            });
        assert_eq!(
            reported["Id"].as_str(),
            Some(held_id.as_str()),
            "case {case:?}: the binding must still be {held_id}, got {reported}"
        );
        let listed = listed_resources(&waf, &held_arn, None).await;
        assert_eq!(
            listed,
            vec![resource_arn.clone()],
            "case {case:?}: the held web ACL must still list {resource_arn}, got {listed:?}"
        );
    }
}
