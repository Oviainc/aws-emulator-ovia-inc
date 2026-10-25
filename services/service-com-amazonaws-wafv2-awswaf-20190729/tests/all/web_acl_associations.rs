// SPDX-License-Identifier: CC0-1.0

//! Putting a web ACL in front of a protected resource, end to end over the wire.
//!
//! The rule: `AssociateWebACL` binds the web ACL the request names to the
//! protected resource the request names, so the association reads
//! (`GetWebACLForResource`, `ListResourcesForWebACL`) report that binding until
//! `DisassociateWebACL` takes it away, and a resource carries at most ONE web ACL.
//! A request naming an ARN of a resource kind a web ACL cannot be put in front of
//! is refused with `WAFInvalidParameterException`, and one naming a web ACL this
//! account does not hold with `WAFNonexistentItemException` — both HTTP 400.
//!
//! The tables below span the class rather than one recorded case: one row per
//! resource FORM the API documents (eight kinds, each addressed by its own ARN
//! layout), and one row per way an ARN can fail the parameter. The harness is the
//! one in [`crate::web_acl_lifecycle`], so every call goes through the router's
//! front door and what these tests observe is what an SDK observes.

use serde_json::{json, Value};

use crate::web_acl_lifecycle::{create_ip_set, create_web_acl, rule, sqli_statement, Waf};

/// The account the protected resources below belong to. A protected resource is
/// owned by another service, so its ARN is never one this service minted.
const ACCOUNT: &str = "123456789012";

/// One protected resource per form the API documents: `case`, the resource's ARN,
/// and the `ResourceType` that form is.
///
/// The ARN layouts are the eight the `ResourceArn` member lists
/// (`smithy_query(query="doc",shape="AssociateWebACLRequest",member="ResourceArn")`
/// → "the ARN must be in one of the following formats") and the type names are the
/// `ResourceType` enum's values
/// (`smithy_query(query="shape",shape="ResourceType")`) — so every kind is
/// exercised, not only the load balancer the report named.
const PROTECTED_RESOURCES: &[(&str, &str, &str)] = &[
    (
        "an Application Load Balancer",
        "arn:aws:elasticloadbalancing:us-west-2:123456789012:loadbalancer/app/prod-alb/50dc6c495c0c9188",
        "APPLICATION_LOAD_BALANCER",
    ),
    (
        "an API Gateway REST API stage",
        "arn:aws:apigateway:us-west-2::/restapis/a1b2c3d4e5/stages/prod",
        "API_GATEWAY",
    ),
    (
        "an AppSync GraphQL API",
        "arn:aws:appsync:us-west-2:123456789012:apis/abcdefghijklmnopqrstuvwxyz",
        "APPSYNC",
    ),
    (
        "a Cognito user pool",
        "arn:aws:cognito-idp:us-west-2:123456789012:userpool/us-west-2_aBcDeFgHi",
        "COGNITO_USER_POOL",
    ),
    (
        "an App Runner service",
        "arn:aws:apprunner:us-west-2:123456789012:service/web-front/8fe1e10304f44ed3a03a4b0b1aa7d4a0",
        "APP_RUNNER_SERVICE",
    ),
    (
        "a Verified Access instance",
        "arn:aws:ec2:us-west-2:123456789012:verified-access-instance/vai-0ce000c0b7643abcd",
        "VERIFIED_ACCESS_INSTANCE",
    ),
    (
        "an Amplify application",
        "arn:aws:amplify:us-west-2:123456789012:apps/d1a2b3c4d5e6f7",
        "AMPLIFY",
    ),
    (
        "a Bedrock AgentCore gateway",
        "arn:aws:bedrock-agentcore:us-west-2:123456789012:gateway/gw-abc123def4",
        "AGENTCORE_GATEWAY",
    ),
];

/// The resource ARNs the web ACL `web_acl_arn` is reported as being in front of,
/// of `resource_type` — or of the type the call defaults to when it is `None`.
async fn listed_resources(
    waf: &Waf,
    web_acl_arn: &str,
    resource_type: Option<&str>,
) -> Vec<String> {
    let mut body = json!({ "WebACLArn": web_acl_arn });
    if let Some(resource_type) = resource_type {
        body["ResourceType"] = json!(resource_type);
    }
    let listed = waf.ok("ListResourcesForWebACL", body).await;
    listed["ResourceArns"]
        .as_array()
        .unwrap_or_else(|| panic!("ListResourcesForWebACL answers ResourceArns, got {listed}"))
        .iter()
        .map(|arn| arn.as_str().unwrap_or_default().to_string())
        .collect()
}

/// The web ACL `GetWebACLForResource` reports for `resource_arn`, or `None` when it
/// reports a null web ACL.
async fn web_acl_for_resource(waf: &Waf, resource_arn: &str) -> Option<Value> {
    let reported = waf
        .ok(
            "GetWebACLForResource",
            json!({ "ResourceArn": resource_arn }),
        )
        .await;
    reported
        .get("WebACL")
        .filter(|web_acl| !web_acl.is_null())
        .cloned()
}

/// Every documented resource form can be put in front of a web ACL, both
/// association reads report the binding, and `DisassociateWebACL` takes it away.
#[tokio::test]
async fn associate_binds_each_documented_resource_form_and_the_reads_report_it() {
    let waf = Waf::new();
    let (web_acl_id, web_acl_arn, _lock_token) = create_web_acl(
        &waf,
        "REGIONAL",
        "association-acl",
        json!([rule("block-sqli", sqli_statement())]),
    )
    .await;

    for (case, resource_arn, resource_type) in PROTECTED_RESOURCES {
        let unbound = web_acl_for_resource(&waf, resource_arn).await;
        assert!(
            unbound.is_none(),
            "case {case:?}: a resource nothing was put in front of must report a null \
             web ACL, got {unbound:?}"
        );

        let associated = waf
            .ok(
                "AssociateWebACL",
                json!({ "ResourceArn": resource_arn, "WebACLArn": web_acl_arn }),
            )
            .await;
        assert!(
            associated.is_null()
                || associated
                    .as_object()
                    .is_some_and(|members| members.is_empty()),
            "case {case:?}: AssociateWebACL answers a response with no members, \
             got {associated}"
        );

        // The round trip: the resource now reports the STORED web ACL, rule tree
        // and all, exactly as `GetWebACL` would hand it back.
        let bound = web_acl_for_resource(&waf, resource_arn)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the resource must report its web ACL"));
        assert_eq!(
            bound["ARN"].as_str(),
            Some(web_acl_arn.as_str()),
            "case {case:?}: expected the web ACL {web_acl_arn}, got {bound}"
        );
        assert_eq!(
            bound["Id"].as_str(),
            Some(web_acl_id.as_str()),
            "case {case:?}: expected the web ACL id {web_acl_id}, got {bound}"
        );
        assert_eq!(
            bound["Rules"][0]["Name"].as_str(),
            Some("block-sqli"),
            "case {case:?}: expected the stored rule tree, got {bound}"
        );

        // The listing selects by resource TYPE, so this resource shows up under its
        // own kind and under no other.
        let own_kind = listed_resources(&waf, &web_acl_arn, Some(resource_type)).await;
        assert!(
            own_kind.contains(&(*resource_arn).to_string()),
            "case {case:?}: {resource_type} must list {resource_arn}, got {own_kind:?}"
        );
        let other_type = PROTECTED_RESOURCES
            .iter()
            .map(|(_, _, candidate)| *candidate)
            .find(|candidate| candidate != resource_type)
            .expect("the table holds more than one resource type");
        let other_kind = listed_resources(&waf, &web_acl_arn, Some(other_type)).await;
        assert!(
            !other_kind.contains(&(*resource_arn).to_string()),
            "case {case:?}: {other_type} must not list {resource_arn}, got {other_kind:?}"
        );
    }

    // Every form is bound at once, and each kind reports exactly its own resource.
    for (case, resource_arn, resource_type) in PROTECTED_RESOURCES {
        let listed = listed_resources(&waf, &web_acl_arn, Some(resource_type)).await;
        assert_eq!(
            listed,
            vec![(*resource_arn).to_string()],
            "case {case:?}: {resource_type} must list only {resource_arn}, got {listed:?}"
        );
    }

    for (case, resource_arn, resource_type) in PROTECTED_RESOURCES {
        let disassociated = waf
            .ok(
                "DisassociateWebACL",
                json!({ "ResourceArn": resource_arn }),
            )
            .await;
        assert!(
            disassociated.is_null()
                || disassociated
                    .as_object()
                    .is_some_and(|members| members.is_empty()),
            "case {case:?}: DisassociateWebACL answers a response with no members, \
             got {disassociated}"
        );
        let after = web_acl_for_resource(&waf, resource_arn).await;
        assert!(
            after.is_none(),
            "case {case:?}: a disassociated resource must report a null web ACL, \
             got {after:?}"
        );
        let listed = listed_resources(&waf, &web_acl_arn, Some(resource_type)).await;
        assert!(
            listed.is_empty(),
            "case {case:?}: {resource_type} must list nothing once {resource_arn} is \
             disassociated, got {listed:?}"
        );

        // "Disassociates the specified resource from its web ACL association, if it
        // has one": a second call is a success that changes nothing.
        let repeated = waf
            .ok(
                "DisassociateWebACL",
                json!({ "ResourceArn": resource_arn }),
            )
            .await;
        assert!(
            repeated.is_null()
                || repeated
                    .as_object()
                    .is_some_and(|members| members.is_empty()),
            "case {case:?}: disassociating twice must succeed, got {repeated}"
        );
    }
}

/// An ARN naming a resource kind a web ACL cannot be put in front of is refused,
/// and binds nothing — on every operation that takes a protected-resource ARN.
#[tokio::test]
async fn an_arn_no_web_acl_can_protect_is_refused_on_every_association_operation() {
    // One row per way this parameter fails: not an ARN at all, a resource whose
    // service cannot be protected, a resource of a protectable service whose kind
    // is not the protectable one, and a protectable layout with a segment missing
    // or empty. Each is at least the 20 characters `ResourceArn` requires, so the
    // refusal under test is the handler's and not the length constraint's.
    let cases: &[(&str, &str)] = &[
        ("a string that is not an ARN", "not-an-arn-but-long-enough"),
        (
            "a CloudFront distribution, which carries its web ACL in its own configuration",
            "arn:aws:cloudfront::123456789012:distribution/E1XMPLDISTRIBUTION",
        ),
        (
            "a Network Load Balancer",
            "arn:aws:elasticloadbalancing:us-west-2:123456789012:loadbalancer/net/prod-nlb/50dc6c495c0c9188",
        ),
        (
            "a load balancer target group",
            "arn:aws:elasticloadbalancing:us-west-2:123456789012:targetgroup/prod-tg/50dc6c495c0c9188",
        ),
        (
            "an S3 bucket",
            "arn:aws:s3:::a-bucket-no-web-acl-protects",
        ),
        (
            "a Lambda function",
            "arn:aws:lambda:us-west-2:123456789012:function:handler",
        ),
        (
            "a REST API with no stage segment",
            "arn:aws:apigateway:us-west-2::/restapis/a1b2c3d4e5",
        ),
        (
            "an Application Load Balancer ARN whose id segment is empty",
            "arn:aws:elasticloadbalancing:us-west-2:123456789012:loadbalancer/app/prod-alb/",
        ),
        (
            "an ARN with no partition",
            "arn::elasticloadbalancing:us-west-2:123456789012:loadbalancer/app/a/b",
        ),
    ];

    let waf = Waf::new();
    let (_id, web_acl_arn, _lock_token) =
        create_web_acl(&waf, "REGIONAL", "refusal-acl", json!([])).await;

    for (case, resource_arn) in cases {
        let refused = waf
            .refused(
                "AssociateWebACL",
                json!({ "ResourceArn": resource_arn, "WebACLArn": web_acl_arn }),
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
        waf.refused(
            "GetWebACLForResource",
            json!({ "ResourceArn": resource_arn }),
            "WAFInvalidParameterException",
            case,
        )
        .await;
        waf.refused(
            "DisassociateWebACL",
            json!({ "ResourceArn": resource_arn }),
            "WAFInvalidParameterException",
            case,
        )
        .await;
    }

    // Nothing was bound by any of those refusals.
    for (_, _, resource_type) in PROTECTED_RESOURCES {
        let listed = listed_resources(&waf, &web_acl_arn, Some(resource_type)).await;
        assert!(
            listed.is_empty(),
            "a refused association must store nothing, yet {resource_type} lists {listed:?}"
        );
    }
}

/// A resource carries at most one web ACL: re-binding it moves the association,
/// and re-binding it to the web ACL it already carries changes nothing.
#[tokio::test]
async fn a_resource_carries_one_web_acl_and_rebinding_moves_it() {
    let waf = Waf::new();
    let (_first_id, first_arn, _first_token) =
        create_web_acl(&waf, "REGIONAL", "first-acl", json!([])).await;
    let (_second_id, second_arn, _second_token) =
        create_web_acl(&waf, "REGIONAL", "second-acl", json!([])).await;
    let resource_arn = PROTECTED_RESOURCES[0].1;

    for (case, expected_arn) in [
        ("the first binding", &first_arn),
        ("the same binding again", &first_arn),
        ("re-bound to another web ACL", &second_arn),
    ] {
        waf.ok(
            "AssociateWebACL",
            json!({ "ResourceArn": resource_arn, "WebACLArn": expected_arn }),
        )
        .await;
        let bound = web_acl_for_resource(&waf, resource_arn)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the resource must report a web ACL"));
        assert_eq!(
            bound["ARN"].as_str(),
            Some(expected_arn.as_str()),
            "case {case:?}: expected {expected_arn}, got {bound}"
        );
        let listed = listed_resources(&waf, expected_arn, None).await;
        assert_eq!(
            listed,
            vec![resource_arn.to_string()],
            "case {case:?}: the bound web ACL must list the resource once, got {listed:?}"
        );
    }

    // The web ACL the resource left no longer lists it.
    let left_behind = listed_resources(&waf, &first_arn, None).await;
    assert!(
        left_behind.is_empty(),
        "the web ACL a resource was moved off must list nothing, got {left_behind:?}"
    );
}

/// A `WebACLArn` that names no web ACL of this account, or a CLOUDFRONT-scoped one,
/// binds nothing.
#[tokio::test]
async fn a_web_acl_arn_that_cannot_be_bound_is_refused() {
    let waf = Waf::new();
    let (_id, regional_arn, _token) =
        create_web_acl(&waf, "REGIONAL", "bindable-acl", json!([])).await;
    let (_global_id, cloudfront_arn, _global_token) =
        create_web_acl(&waf, "CLOUDFRONT", "distribution-acl", json!([])).await;
    let (_ip_set_id, ip_set_arn, _ip_set_token) =
        create_ip_set(&waf, "REGIONAL", "not-a-web-acl").await;
    let resource_arn = PROTECTED_RESOURCES[0].1;

    // `Field`/`Parameter` are asserted per row below; the code is what a client
    // branches on.
    let cases: &[(&str, &str, &str)] = &[
        (
            "a well-formed wafv2 web ACL ARN naming nothing stored",
            "arn:aws:wafv2:us-west-2:123456789012:regional/webacl/ghost/12345678-1234-1234-1234-123456789012",
            "WAFNonexistentItemException",
        ),
        (
            "an ARN of another wafv2 resource kind",
            ip_set_arn.as_str(),
            "WAFNonexistentItemException",
        ),
        (
            "a wafv2 ARN with too few resource segments",
            "arn:aws:wafv2:us-west-2:123456789012:regional/webacl/only-three",
            "WAFInvalidParameterException",
        ),
        (
            "a CLOUDFRONT-scoped web ACL",
            cloudfront_arn.as_str(),
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
        let bound = web_acl_for_resource(&waf, resource_arn).await;
        assert!(
            bound.is_none(),
            "case {case:?}: a refused association must bind nothing, got {bound:?}"
        );
    }

    // The same split on the listing side.
    waf.refused(
        "ListResourcesForWebACL",
        json!({ "WebACLArn": "arn:aws:wafv2:us-west-2:123456789012:regional/webacl/ghost/12345678-1234-1234-1234-123456789012" }),
        "WAFNonexistentItemException",
        "listing a web ACL naming nothing stored",
    )
    .await;
    waf.refused(
        "ListResourcesForWebACL",
        json!({ "WebACLArn": regional_arn, "ResourceType": "DATABASE" }),
        "ValidationException",
        "listing by a resource type the model does not declare",
    )
    .await;
    // A CLOUDFRONT-scoped web ACL is bound through the distribution's own
    // configuration, so this service holds no binding naming one.
    let global = listed_resources(&waf, &cloudfront_arn, None).await;
    assert!(
        global.is_empty(),
        "a CLOUDFRONT-scoped web ACL must list no protected resource, got {global:?}"
    );
}

/// The listing defaults to the load balancer kind: "if you don't provide a resource
/// type, the call uses the resource type APPLICATION_LOAD_BALANCER".
#[tokio::test]
async fn a_listing_with_no_resource_type_selects_load_balancers() {
    let waf = Waf::new();
    let (_id, web_acl_arn, _token) =
        create_web_acl(&waf, "REGIONAL", "default-type-acl", json!([])).await;
    let load_balancer = PROTECTED_RESOURCES[0].1;
    let user_pool = format!("arn:aws:cognito-idp:us-west-2:{ACCOUNT}:userpool/us-west-2_aBcDeFgHi");

    for resource_arn in [load_balancer, user_pool.as_str()] {
        waf.ok(
            "AssociateWebACL",
            json!({ "ResourceArn": resource_arn, "WebACLArn": web_acl_arn }),
        )
        .await;
    }

    let defaulted = listed_resources(&waf, &web_acl_arn, None).await;
    assert_eq!(
        defaulted,
        vec![load_balancer.to_string()],
        "a listing naming no resource type must select load balancers, got {defaulted:?}"
    );
    let explicit = listed_resources(&waf, &web_acl_arn, Some("COGNITO_USER_POOL")).await;
    assert_eq!(
        explicit,
        vec![user_pool.clone()],
        "a listing of user pools must select the user pool, got {explicit:?}"
    );
}

/// A web ACL that is still in front of a resource is in use, and cannot be deleted
/// until the association is taken away.
#[tokio::test]
async fn a_web_acl_in_front_of_a_resource_cannot_be_deleted() {
    let waf = Waf::new();
    let (web_acl_id, web_acl_arn, lock_token) =
        create_web_acl(&waf, "REGIONAL", "in-use-acl", json!([])).await;
    let resource_arn = PROTECTED_RESOURCES[0].1;

    waf.ok(
        "AssociateWebACL",
        json!({ "ResourceArn": resource_arn, "WebACLArn": web_acl_arn }),
    )
    .await;
    waf.refused(
        "DeleteWebACL",
        json!({
            "Name": "in-use-acl",
            "Scope": "REGIONAL",
            "Id": web_acl_id,
            "LockToken": lock_token,
        }),
        "WAFAssociatedItemException",
        "deleting a web ACL that is in front of a resource",
    )
    .await;

    waf.ok(
        "DisassociateWebACL",
        json!({ "ResourceArn": resource_arn }),
    )
    .await;
    waf.ok(
        "DeleteWebACL",
        json!({
            "Name": "in-use-acl",
            "Scope": "REGIONAL",
            "Id": web_acl_id,
            "LockToken": lock_token,
        }),
    )
    .await;
    let gone = web_acl_for_resource(&waf, resource_arn).await;
    assert!(
        gone.is_none(),
        "the deleted web ACL must be in front of nothing, got {gone:?}"
    );
}
