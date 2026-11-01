// SPDX-License-Identifier: CC0-1.0

//! The policy set of a Classic load balancer: the three operations that create a policy,
//! the one that deletes a policy, and the policies a later describe reports — end to end
//! over the awsQuery wire.
//!
//! Every request goes through the awsQuery protocol on the multi-service router (see
//! `super::wire`), so what is asserted is the emulator's HTTP behaviour.
//!
//! The rules under test, each over a TABLE of inputs rather than one recorded request:
//!
//! * `CreateLoadBalancerPolicy` persists one policy on the load balancer the request
//!   names, under the `PolicyName` it names, of the `PolicyTypeName` it names and
//!   carrying the `PolicyAttributes` it carries; `CreateAppCookieStickinessPolicy`
//!   persists one following the application cookie its `CookieName` names.
//! * `DescribeLoadBalancers` reports EVERY policy the load balancer holds in the
//!   `Policies` member whose documentation names the operation that created it, so a
//!   policy created by `CreateLoadBalancerPolicy` is reported by name under
//!   `Policies.OtherPolicies` — "the policies other than the stickiness policies" — as
//!   one `<member>` element per policy of the `PolicyNames` list that member targets,
//!   and each stickiness operation's policies under their own member.
//! * the reported set follows the creates and the deletes: `DeleteLoadBalancerPolicy`
//!   removes exactly the named policy, and a name the load balancer does not hold leaves
//!   the set as it is.
//! * a `PolicyTypeName` outside the policy types the service predefines is refused
//!   `PolicyTypeNotFound` and writes nothing, while every predefined type is accepted.
//! * a policy name is unique within the whole policy set of ONE load balancer, whichever
//!   operation created the policy holding it, and the load balancer is resolved in the
//!   region the request is signed for.
//! * a request outside a rule the model cannot express — a policy name outside
//!   "alphanumeric characters and dashes (-)", an attribute carrying no name, a cookie
//!   name naming no cookie — is refused `ValidationError` and writes nothing.
//!
//! Every create and every delete is asserted on what a LATER describe reports, never on
//! what the call answered alone.

use super::wire::{
    call, emulator, emulator_with_engine, kid, kid_text, member_texts, members, param, parse,
};
use service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::state::{
    ComAmazonawsElasticloadbalancingElasticloadbalancingV7State as ElbState, PolicyKind,
    POLICY_TYPE_NAMES,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for unless the case names another.
const REGION: &str = "us-east-1";

/// A second region, for the rule that a load balancer is resolved in the region the
/// request is signed for.
const OTHER_REGION: &str = "eu-west-1";

/// One attribute of a create request, as the tables below carry it: the `AttributeName`,
/// and the `AttributeValue` — `None` leaving that member out of the request.
type Attribute = (&'static str, Option<&'static str>);

/// One row of the persistence table: the case, the policy name and base policy type the
/// create names, and the attributes it carries.
struct PolicyCase {
    case: &'static str,
    policy_name: &'static str,
    policy_type_name: &'static str,
    attributes: &'static [Attribute],
}

/// Create a load balancer named `name` in `region` with one HTTP listener, failing the
/// test if the create does not answer 200.
async fn create_load_balancer(router: &MultiServiceRouter, region: &str, name: &str) {
    let created = call(
        router,
        region,
        "CreateLoadBalancer",
        &[
            param("LoadBalancerName", name),
            param("AvailabilityZones.member.1", &format!("{region}a")),
            param("Listeners.member.1.Protocol", "HTTP"),
            param("Listeners.member.1.LoadBalancerPort", "80"),
            param("Listeners.member.1.InstancePort", "8080"),
        ],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "creating the load balancer {name:?} in {region:?} must succeed, got {}",
        created.body
    );
}

/// Ask `CreateLoadBalancerPolicy` for `policy_name` of `policy_type_name` on
/// `load_balancer_name`, carrying `attributes`.
async fn create_policy(
    router: &MultiServiceRouter,
    region: &str,
    load_balancer_name: &str,
    policy_name: &str,
    policy_type_name: &str,
    attributes: &[(&str, Option<&str>)],
) -> super::wire::Wire {
    let mut params = vec![
        param("LoadBalancerName", load_balancer_name),
        param("PolicyName", policy_name),
        param("PolicyTypeName", policy_type_name),
    ];
    for (index, (name, value)) in attributes.iter().enumerate() {
        params.push(param(
            &format!("PolicyAttributes.member.{}.AttributeName", index + 1),
            name,
        ));
        if let Some(value) = value {
            params.push(param(
                &format!("PolicyAttributes.member.{}.AttributeValue", index + 1),
                value,
            ));
        }
    }
    call(router, region, "CreateLoadBalancerPolicy", &params).await
}

/// The three `Policies` members `DescribeLoadBalancers` reports for one load balancer,
/// each read as the member's own modelled shape.
#[derive(Debug, Default, PartialEq, Eq)]
struct ReportedPolicies {
    /// The `PolicyName` of each member of `OtherPolicies`, in the order reported.
    other: Vec<String>,
    /// The `(PolicyName, CookieName)` of each `AppCookieStickinessPolicies` member.
    app_cookie: Vec<(String, String)>,
    /// The `(PolicyName, CookieExpirationPeriod)` of each `LBCookieStickinessPolicies`
    /// member.
    lb_cookie: Vec<(String, String)>,
}

/// What `DescribeLoadBalancers` reports under `Policies` for `name`.
async fn described_policies(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
) -> ReportedPolicies {
    let described = call(
        router,
        region,
        "DescribeLoadBalancers",
        &[param("LoadBalancerNames.member.1", name)],
    )
    .await;
    assert_eq!(
        described.status, 200,
        "describing {name:?} must answer 200, got {}",
        described.body
    );
    let doc = parse(&described.body);
    let result = described.result(&doc);
    let description = *members(result, "LoadBalancerDescriptions")
        .first()
        .unwrap_or_else(|| panic!("no description of {name} in {}", described.body));
    let policies = kid(description, "Policies")
        .unwrap_or_else(|| panic!("no Policies in {}", described.body));
    let pairs = |member: &str, second: &str| -> Vec<(String, String)> {
        members(policies, member)
            .into_iter()
            .map(|entry| {
                (
                    kid_text(entry, "PolicyName").unwrap_or_default(),
                    kid_text(entry, second).unwrap_or_default(),
                )
            })
            .collect()
    };
    ReportedPolicies {
        other: member_texts(policies, "OtherPolicies"),
        app_cookie: pairs("AppCookieStickinessPolicies", "CookieName"),
        lb_cookie: pairs("LBCookieStickinessPolicies", "CookieExpirationPeriod"),
    }
}

/// What the store holds for one policy created by `CreateLoadBalancerPolicy`: its base
/// policy type and its attributes, in the order they were persisted.
///
/// Read through the shared engine because the attributes a create persists are what
/// `DescribeLoadBalancerPolicies` reports as a policy's `PolicyAttributeDescriptions`,
/// and that operation is still a scaffold (its own issue).
fn stored_policy(
    engine: &engine::InMemoryEngine,
    region: &str,
    load_balancer_name: &str,
    policy_name: &str,
) -> (String, Vec<(String, Option<String>)>) {
    let state = engine.state::<ElbState>();
    let guard = state.lock();
    let record = guard
        .load_balancers_in(region)
        .and_then(|records| records.get(load_balancer_name))
        .unwrap_or_else(|| panic!("no stored load balancer {load_balancer_name}"));
    let policy = record
        .policies
        .get(policy_name)
        .unwrap_or_else(|| panic!("no stored policy {policy_name}"));
    match &policy.kind {
        PolicyKind::Other {
            policy_type_name,
            attributes,
        } => (
            policy_type_name.clone(),
            attributes
                .iter()
                .map(|attribute| (attribute.name.clone(), attribute.value.clone()))
                .collect(),
        ),
        held => panic!(
            "policy {policy_name} is stored as {held:?} rather than as the policy \
             CreateLoadBalancerPolicy persists"
        ),
    }
}

/// A create persists the policy the request describes, and a later describe reports its
/// name under `Policies.OtherPolicies`: one row per form the request's attribute list can
/// take, over policy types the service predefines.
#[tokio::test]
async fn a_create_persists_the_policy_a_later_describe_reports_in_other_policies() {
    // The rows differ in the base policy type and in the shape of the attribute list:
    // none at all, one, several, and a name carrying no value. The two stickiness types
    // are left to the catalog table below, because the two stickiness `Policies` members
    // are documented as the policies the two stickiness OPERATIONS create.
    let cases = [
        PolicyCase {
            case: "the Proxy Protocol type with its one attribute",
            policy_name: "proxy-protocol-1",
            policy_type_name: "ProxyProtocolPolicyType",
            attributes: &[("ProxyProtocol", Some("true"))],
        },
        PolicyCase {
            case: "a policy carrying no attribute at all",
            policy_name: "bare-policy",
            policy_type_name: "ProxyProtocolPolicyType",
            attributes: &[],
        },
        PolicyCase {
            case: "several attributes on one policy",
            policy_name: "SSLNegotiation2",
            policy_type_name: "SSLNegotiationPolicyType",
            attributes: &[
                ("Protocol-TLSv1.2", Some("true")),
                ("Protocol-TLSv1", Some("false")),
                ("ECDHE-ECDSA-AES128-GCM-SHA256", Some("true")),
            ],
        },
        PolicyCase {
            case: "an attribute value carrying characters a form must encode",
            policy_name: "public-key-policy",
            policy_type_name: "PublicKeyPolicyType",
            attributes: &[(
                "PublicKey",
                Some("MIIBIjANBgkqhkiG9w0BAQ+FAAOCAQ8A/wKCAQ==\nsecond line"),
            )],
        },
        PolicyCase {
            case: "an attribute carrying a name but no value",
            policy_name: "backend-auth",
            policy_type_name: "BackendServerAuthenticationPolicyType",
            attributes: &[("PublicKeyPolicyName", None)],
        },
    ];

    let (router, engine) = emulator_with_engine();
    create_load_balancer(&router, REGION, "policy-lb").await;

    for case in &cases {
        let created = create_policy(
            &router,
            REGION,
            "policy-lb",
            case.policy_name,
            case.policy_type_name,
            case.attributes,
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: the create must answer 200, got {} with {}",
            case.case, created.status, created.body
        );

        // The policy is persisted whole: its base type and every attribute the request
        // carried, in the order it carried them.
        let (stored_type, stored_attributes) =
            stored_policy(&engine, REGION, "policy-lb", case.policy_name);
        let expected_attributes: Vec<(String, Option<String>)> = case
            .attributes
            .iter()
            .map(|(name, value)| (name.to_string(), value.map(str::to_string)))
            .collect();
        assert_eq!(
            (stored_type.as_str(), stored_attributes.as_slice()),
            (case.policy_type_name, expected_attributes.as_slice()),
            "case {:?}: the create must persist the policy type and the attributes the \
             request carried, got {stored_type:?} and {stored_attributes:?}",
            case.case
        );
    }

    // ONE describe reports every policy the creates persisted, each as its own member of
    // `OtherPolicies`, in `PolicyName` order.
    let reported = described_policies(&router, REGION, "policy-lb").await;
    let mut expected: Vec<String> = cases
        .iter()
        .map(|case| case.policy_name.to_string())
        .collect();
    expected.sort();
    assert_eq!(
        reported.other, expected,
        "the describe must report every created policy name under OtherPolicies, in \
         PolicyName order; got {:?}",
        reported.other
    );
    assert_eq!(
        (
            reported.app_cookie.as_slice(),
            reported.lb_cookie.as_slice()
        ),
        ([].as_slice(), [].as_slice()),
        "no stickiness policy was created, so both stickiness members must stay empty; \
         got {:?} and {:?}",
        reported.app_cookie,
        reported.lb_cookie
    );

    // A describe is a report: reading twice reports the same policies.
    let again = described_policies(&router, REGION, "policy-lb").await;
    assert_eq!(
        again, reported,
        "a second describe must report the same policies, got {again:?}"
    );

    // A load balancer nobody created a policy for holds none, so the member carries no
    // `<member>` at all.
    create_load_balancer(&router, REGION, "no-policy-lb").await;
    let none = described_policies(&router, REGION, "no-policy-lb").await;
    assert_eq!(
        none,
        ReportedPolicies::default(),
        "a load balancer with no policy must report every Policies member empty, got \
         {none:?}"
    );
}

/// `PolicyTypeName` is "the name of the base policy type": every type the service
/// predefines is accepted, and a name no policy type carries is refused
/// `PolicyTypeNotFound` and writes nothing.
#[tokio::test]
async fn the_base_policy_type_must_name_a_policy_type_the_service_predefines() {
    let router = emulator();
    create_load_balancer(&router, REGION, "typed-lb").await;

    // The whole catalog, so no entry is accepted only because a test names it.
    for (index, policy_type_name) in POLICY_TYPE_NAMES.iter().enumerate() {
        let created = create_policy(
            &router,
            REGION,
            "typed-lb",
            &format!("typed-{index}"),
            policy_type_name,
            &[],
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {policy_type_name:?}: a predefined policy type must be accepted, got \
             {} with {}",
            created.status, created.body
        );
    }

    create_load_balancer(&router, REGION, "untyped-lb").await;
    // Rows differ in HOW the name misses the catalog: a type name nothing carries, a
    // predefined name in the wrong case, one without its `PolicyType` suffix, one with
    // surrounding space, and the empty name.
    let unknown = [
        ("a name no policy type carries", "NotAPolicyType"),
        ("a predefined name lower-cased", "proxyprotocolpolicytype"),
        ("a predefined name without its suffix", "ProxyProtocol"),
        ("a predefined name with trailing space", "PublicKeyPolicyType "),
        ("the empty policy type name", ""),
    ];
    for (case, policy_type_name) in unknown {
        let refused = create_policy(
            &router,
            REGION,
            "untyped-lb",
            "unknown-type-policy",
            policy_type_name,
            &[("ProxyProtocol", Some("true"))],
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "PolicyTypeNotFound"),
            "case {case:?}: the create must be refused 400 PolicyTypeNotFound, got {} \
             with {}",
            refused.status,
            refused.body
        );
    }

    let reported = described_policies(&router, REGION, "untyped-lb").await;
    assert!(
        reported.other.is_empty(),
        "a refused create must persist no policy, got {:?}",
        reported.other
    );
}

/// A policy name is unique across the WHOLE policy set of one load balancer, whichever
/// operation created the policy that holds it, and the load balancer is resolved in the
/// region the request is signed for.
#[tokio::test]
async fn a_policy_name_is_unique_within_one_load_balancer() {
    let router = emulator();
    create_load_balancer(&router, REGION, "first-lb").await;
    create_load_balancer(&router, REGION, "second-lb").await;

    let created = create_policy(
        &router,
        REGION,
        "first-lb",
        "shared-name",
        "ProxyProtocolPolicyType",
        &[("ProxyProtocol", Some("true"))],
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the first create must succeed, got {}",
        created.body
    );

    // A repeat of a taken name is refused whether it asks for the same policy or another
    // one, and whichever operation asks for it.
    for (case, policy_type_name) in [
        ("the same policy type", "ProxyProtocolPolicyType"),
        ("another policy type", "PublicKeyPolicyType"),
    ] {
        let repeated = create_policy(
            &router,
            REGION,
            "first-lb",
            "shared-name",
            policy_type_name,
            &[],
        )
        .await;
        assert_eq!(
            (repeated.status, repeated.error_code().as_str()),
            (400, "DuplicatePolicyName"),
            "case {case:?}: a repeat of a taken policy name must be refused 400 \
             DuplicatePolicyName, got {} with {}",
            repeated.status,
            repeated.body
        );
    }
    let stickiness = call(
        &router,
        REGION,
        "CreateLBCookieStickinessPolicy",
        &[
            param("LoadBalancerName", "first-lb"),
            param("PolicyName", "shared-name"),
            param("CookieExpirationPeriod", "60"),
        ],
    )
    .await;
    assert_eq!(
        (stickiness.status, stickiness.error_code().as_str()),
        (400, "DuplicatePolicyName"),
        "case {:?}: a stickiness create must be refused the name this policy holds, got \
         {} with {}",
        "a stickiness policy under the taken name",
        stickiness.status,
        stickiness.body
    );

    // The refused repeats wrote nothing, and the name is taken on THAT load balancer
    // only.
    let reported = described_policies(&router, REGION, "first-lb").await;
    assert_eq!(
        (reported.other.as_slice(), reported.lb_cookie.as_slice()),
        ([String::from("shared-name")].as_slice(), [].as_slice()),
        "a refused repeat must leave the stored policy set as it was, got {reported:?}"
    );
    let elsewhere = create_policy(
        &router,
        REGION,
        "second-lb",
        "shared-name",
        "PublicKeyPolicyType",
        &[("PublicKey", Some("MIIBIjANBgkq"))],
    )
    .await;
    assert_eq!(
        elsewhere.status, 200,
        "the same policy name on another load balancer must be accepted, got {}",
        elsewhere.body
    );
    let reported = described_policies(&router, REGION, "second-lb").await;
    assert_eq!(
        reported.other,
        vec!["shared-name".to_string()],
        "the second load balancer must hold its own policy, got {:?}",
        reported.other
    );
}

/// The load balancer is resolved in the region the request is signed for, so a name no
/// record holds there is the operation's not-found failure.
#[tokio::test]
async fn a_create_for_a_load_balancer_no_record_holds_is_refused() {
    let router = emulator();
    create_load_balancer(&router, REGION, "present-lb").await;

    for (case, region, name) in [
        ("a name nothing created", REGION, "no-such-lb"),
        (
            "a name held only in another region",
            OTHER_REGION,
            "present-lb",
        ),
    ] {
        let refused = create_policy(
            &router,
            region,
            name,
            "orphan-policy",
            "ProxyProtocolPolicyType",
            &[("ProxyProtocol", Some("true"))],
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "LoadBalancerNotFound"),
            "case {case:?}: the create must be refused 400 LoadBalancerNotFound, got {} \
             with {}",
            refused.status,
            refused.body
        );
    }

    let reported = described_policies(&router, REGION, "present-lb").await;
    assert!(
        reported.other.is_empty(),
        "a refused create must persist no policy, got {:?}",
        reported.other
    );
}

/// A request outside a rule the model cannot express is refused 400 `ValidationError` and
/// writes nothing.
#[tokio::test]
async fn a_request_outside_a_documented_rule_is_refused() {
    let router = emulator();
    create_load_balancer(&router, REGION, "refusal-lb").await;

    // `PolicyName`: "Policy names must consist of alphanumeric characters and dashes
    // (-)", so one row per other character class a caller reaches for, plus the empty
    // name. `PolicyAttribute$AttributeName` is "the name of the attribute", so an
    // attribute carrying none names nothing the policy could be configured by.
    let cases: Vec<(&str, &str, &[Attribute])> = vec![
        ("an underscore in the policy name", "bad_name", &[]),
        ("a space in the policy name", "bad name", &[]),
        ("a dot in the policy name", "bad.name", &[]),
        ("a non-ASCII letter in the policy name", "poliçy", &[]),
        ("an empty policy name", "", &[]),
        (
            "an attribute with an empty name",
            "empty-attribute-name",
            &[("", Some("true"))],
        ),
        (
            "an attribute with an empty name beside a named one",
            "second-attribute-unnamed",
            &[("ProxyProtocol", Some("true")), ("", Some("true"))],
        ),
    ];

    for (case, policy_name, attributes) in cases {
        let refused = create_policy(
            &router,
            REGION,
            "refusal-lb",
            policy_name,
            "ProxyProtocolPolicyType",
            attributes,
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "ValidationError"),
            "case {case:?}: the create must be refused 400 ValidationError, got {} with \
             {}",
            refused.status,
            refused.body
        );
    }

    let reported = described_policies(&router, REGION, "refusal-lb").await;
    assert!(
        reported.other.is_empty(),
        "a refused create must persist no policy, got {:?}",
        reported.other
    );

    // The same request with the one rejected part corrected is accepted, so the refusals
    // above are about the rule and not about the rest of the request.
    let accepted = create_policy(
        &router,
        REGION,
        "refusal-lb",
        "good-name-1",
        "ProxyProtocolPolicyType",
        &[("ProxyProtocol", Some("true"))],
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "a request inside every documented rule must be accepted, got {}",
        accepted.body
    );
    let reported = described_policies(&router, REGION, "refusal-lb").await;
    assert_eq!(
        reported.other,
        vec!["good-name-1".to_string()],
        "the accepted create must be the one policy the record holds, got {:?}",
        reported.other
    );
}

/// What one policy of the lifecycle table is, by the operation that creates it: each
/// `Policies` member is documented as the policies created by one of these.
#[derive(Clone, Copy, Debug)]
enum Created {
    /// `CreateAppCookieStickinessPolicy`, carrying the application cookie the sticky
    /// session follows.
    AppCookie { cookie_name: &'static str },
    /// `CreateLBCookieStickinessPolicy`, carrying the period after which its cookie goes
    /// stale.
    LbCookie { cookie_expiration_period: &'static str },
    /// `CreateLoadBalancerPolicy`, of one of the predefined policy types.
    Generic { policy_type_name: &'static str },
}

/// One row of the lifecycle table: the case, the policy name, and what creates it.
struct LifecycleCase {
    case: &'static str,
    policy_name: &'static str,
    created: Created,
}

/// Create the policy `case` describes on `load_balancer_name`.
async fn create_lifecycle_policy(
    router: &MultiServiceRouter,
    region: &str,
    load_balancer_name: &str,
    case: &LifecycleCase,
) -> super::wire::Wire {
    match case.created {
        Created::AppCookie { cookie_name } => {
            call(
                router,
                region,
                "CreateAppCookieStickinessPolicy",
                &[
                    param("LoadBalancerName", load_balancer_name),
                    param("PolicyName", case.policy_name),
                    param("CookieName", cookie_name),
                ],
            )
            .await
        }
        Created::LbCookie {
            cookie_expiration_period,
        } => {
            call(
                router,
                region,
                "CreateLBCookieStickinessPolicy",
                &[
                    param("LoadBalancerName", load_balancer_name),
                    param("PolicyName", case.policy_name),
                    param("CookieExpirationPeriod", cookie_expiration_period),
                ],
            )
            .await
        }
        Created::Generic { policy_type_name } => {
            create_policy(
                router,
                region,
                load_balancer_name,
                case.policy_name,
                policy_type_name,
                &[],
            )
            .await
        }
    }
}

/// Ask `DeleteLoadBalancerPolicy` for `policy_name` on `load_balancer_name`.
async fn delete_policy(
    router: &MultiServiceRouter,
    region: &str,
    load_balancer_name: &str,
    policy_name: &str,
) -> super::wire::Wire {
    call(
        router,
        region,
        "DeleteLoadBalancerPolicy",
        &[
            param("LoadBalancerName", load_balancer_name),
            param("PolicyName", policy_name),
        ],
    )
    .await
}

/// What a describe must report for exactly the policies of `held`: each row in the member
/// its creating operation's documentation names, in `PolicyName` order.
fn expected_policies(held: &[&LifecycleCase]) -> ReportedPolicies {
    let mut reported = ReportedPolicies::default();
    for case in held {
        match case.created {
            Created::AppCookie { cookie_name } => reported
                .app_cookie
                .push((case.policy_name.to_string(), cookie_name.to_string())),
            Created::LbCookie {
                cookie_expiration_period,
            } => reported.lb_cookie.push((
                case.policy_name.to_string(),
                cookie_expiration_period.to_string(),
            )),
            Created::Generic { .. } => reported.other.push(case.policy_name.to_string()),
        }
    }
    reported.other.sort();
    reported.app_cookie.sort();
    reported.lb_cookie.sort();
    reported
}

/// The reported policy set follows every create and every delete: it is always exactly
/// the policies created so far and not deleted since, each in its own `Policies` member.
#[tokio::test]
async fn the_reported_policy_set_follows_every_create_and_delete() {
    // The rows span the three operations that create a policy, each with the member it
    // is reported under, and more than one policy per member so a delete cannot be
    // mistaken for emptying the member.
    let cases = [
        LifecycleCase {
            case: "an application-controlled stickiness policy",
            policy_name: "app-stickiness-1",
            created: Created::AppCookie {
                cookie_name: "SESSIONID",
            },
        },
        LifecycleCase {
            case: "a second application cookie, named differently",
            policy_name: "app-stickiness-2",
            created: Created::AppCookie {
                cookie_name: "app-cookie-2",
            },
        },
        LifecycleCase {
            case: "a duration-based stickiness policy",
            policy_name: "lb-stickiness-1",
            created: Created::LbCookie {
                cookie_expiration_period: "60",
            },
        },
        LifecycleCase {
            case: "a Proxy Protocol policy",
            policy_name: "proxy-1",
            created: Created::Generic {
                policy_type_name: "ProxyProtocolPolicyType",
            },
        },
        LifecycleCase {
            case: "an SSL negotiation policy",
            policy_name: "ssl-1",
            created: Created::Generic {
                policy_type_name: "SSLNegotiationPolicyType",
            },
        },
    ];

    let router = emulator();
    create_load_balancer(&router, REGION, "lifecycle-lb").await;

    for case in &cases {
        let created = create_lifecycle_policy(&router, REGION, "lifecycle-lb", case).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: the create must answer 200, got {} with {}",
            case.case, created.status, created.body
        );
    }
    let reported = described_policies(&router, REGION, "lifecycle-lb").await;
    let held: Vec<&LifecycleCase> = cases.iter().collect();
    assert_eq!(
        reported,
        expected_policies(&held),
        "the describe must report every created policy in the member its creating \
         operation names, got {reported:?}"
    );

    // Deleting a name no policy holds leaves the set as it is: the model declares no
    // not-found error for a policy, so what the caller asked for is already true.
    let absent = delete_policy(&router, REGION, "lifecycle-lb", "no-such-policy").await;
    assert_eq!(
        absent.status, 200,
        "deleting a policy the load balancer does not hold must answer 200, got {} with \
         {}",
        absent.status, absent.body
    );
    let unchanged = described_policies(&router, REGION, "lifecycle-lb").await;
    assert_eq!(
        unchanged, reported,
        "deleting an absent policy must change nothing, got {unchanged:?}"
    );

    // One delete at a time, in an order that spans the three members: after each one the
    // reported set is exactly the policies not deleted yet.
    for (deleted, case) in cases.iter().enumerate() {
        let removed = delete_policy(&router, REGION, "lifecycle-lb", case.policy_name).await;
        assert_eq!(
            removed.status, 200,
            "case {:?}: the delete must answer 200, got {} with {}",
            case.case, removed.status, removed.body
        );
        let remaining: Vec<&LifecycleCase> = cases.iter().skip(deleted + 1).collect();
        let reported = described_policies(&router, REGION, "lifecycle-lb").await;
        assert_eq!(
            reported,
            expected_policies(&remaining),
            "case {:?}: the describe must report exactly the policies not deleted yet, \
             got {reported:?}",
            case.case
        );
    }

    // Every policy deleted: the three members are empty rather than absent.
    let emptied = described_policies(&router, REGION, "lifecycle-lb").await;
    assert_eq!(
        emptied,
        ReportedPolicies::default(),
        "a load balancer whose policies were all deleted must report every Policies \
         member empty, got {emptied:?}"
    );

    // A delete is resolved in the region the request is signed for, and against a load
    // balancer the account holds there.
    for (case, region, name) in [
        ("a name nothing created", REGION, "no-such-lb"),
        (
            "a name held only in another region",
            OTHER_REGION,
            "lifecycle-lb",
        ),
    ] {
        let refused = delete_policy(&router, region, name, "app-stickiness-1").await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "LoadBalancerNotFound"),
            "case {case:?}: the delete must be refused 400 LoadBalancerNotFound, got {} \
             with {}",
            refused.status,
            refused.body
        );
    }
}

/// `CreateAppCookieStickinessPolicy` carries the application cookie the sticky session
/// follows, so a request naming no cookie is refused and writes nothing.
#[tokio::test]
async fn an_application_stickiness_policy_must_name_the_cookie_it_follows() {
    let router = emulator();
    create_load_balancer(&router, REGION, "cookie-lb").await;

    let refused = call(
        &router,
        REGION,
        "CreateAppCookieStickinessPolicy",
        &[
            param("LoadBalancerName", "cookie-lb"),
            param("PolicyName", "no-cookie-policy"),
            param("CookieName", ""),
        ],
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (400, "ValidationError"),
        "an empty CookieName must be refused 400 ValidationError, got {} with {}",
        refused.status,
        refused.body
    );
    let reported = described_policies(&router, REGION, "cookie-lb").await;
    assert_eq!(
        reported,
        ReportedPolicies::default(),
        "a refused create must persist no policy, got {reported:?}"
    );

    // The same request naming a cookie is accepted, and the describe reports the cookie
    // it named.
    let accepted = call(
        &router,
        REGION,
        "CreateAppCookieStickinessPolicy",
        &[
            param("LoadBalancerName", "cookie-lb"),
            param("PolicyName", "app-cookie-policy"),
            param("CookieName", "MY_APP_COOKIE"),
        ],
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "a request naming a cookie must be accepted, got {}",
        accepted.body
    );
    let reported = described_policies(&router, REGION, "cookie-lb").await;
    assert_eq!(
        reported.app_cookie,
        vec![(
            "app-cookie-policy".to_string(),
            "MY_APP_COOKIE".to_string()
        )],
        "the describe must report the policy under the cookie it follows, got {:?}",
        reported.app_cookie
    );
}
