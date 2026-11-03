// SPDX-License-Identifier: CC0-1.0

//! `DescribeLoadBalancerPolicies`: the policies one describe reports, end to end over the
//! awsQuery wire.
//!
//! Every request goes through the awsQuery protocol on the multi-service router (see
//! `super::wire`), so what is asserted is the emulator's HTTP behaviour.
//!
//! The rules under test, each over a TABLE of inputs rather than one recorded request:
//!
//! * a request naming a load balancer is answered from the policies that load balancer
//!   HOLDS: every policy a create persisted, each reported under the base policy type it
//!   is of and carrying the attributes it was created with — whichever of the three
//!   policy-creating operations minted it.
//! * `PolicyNames` selects from that set: the named policies alone, every policy when the
//!   request names none, and `PolicyNotFound` when it names one the set does not hold.
//! * a load balancer is resolved in the region the request is signed for, and a name no
//!   record holds there is refused `LoadBalancerNotFound`.
//! * a request naming NO load balancer is answered from the sample policies the service
//!   predefines, whose names carry the documented `ELBSample-` prefix, selected by
//!   `PolicyNames` the same way.
//! * the reported set follows the creates and the deletes, and a describe is a report: two
//!   identical describes with any number of describes between them answer the same.

use super::wire::{call, emulator, error_message, kid, kid_text, members, param, parse, Wire};
use service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::state::{
    APP_COOKIE_STICKINESS_POLICY_TYPE, LB_COOKIE_STICKINESS_POLICY_TYPE, SAMPLE_POLICIES,
    SAMPLE_POLICY_NAME_PREFIX, SSL_NEGOTIATION_POLICY_TYPE,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for unless the case names another.
const REGION: &str = "us-east-1";

/// A second region, for the rule that a load balancer is resolved in the region the
/// request is signed for.
const OTHER_REGION: &str = "eu-west-1";

/// One attribute, as a create request carries it and a describe reports it: the
/// `AttributeName`, and the `AttributeValue` — `None` meaning the member is carried by
/// neither.
type Attribute = (&'static str, Option<&'static str>);

/// Which operation mints the policy of a row.
enum Create {
    /// `CreateLBCookieStickinessPolicy`, carrying the `CookieExpirationPeriod` named here
    /// — `None` leaving that member out of the request.
    LbCookieStickiness(Option<&'static str>),
    /// `CreateAppCookieStickinessPolicy`, following the application cookie named here.
    AppCookieStickiness(&'static str),
    /// `CreateLoadBalancerPolicy`, of the row's base policy type and carrying the row's
    /// attributes.
    LoadBalancerPolicy,
}

/// One row of the table: the policy a create persists, and the description a later
/// describe must report for it.
struct PolicyCase {
    case: &'static str,
    create: Create,
    policy_name: &'static str,
    /// The base policy type the description must report the policy under.
    policy_type_name: &'static str,
    /// The attributes the description must report, in the order it must report them.
    attributes: &'static [Attribute],
}

/// One policy as a describe reported it.
#[derive(Debug, PartialEq, Eq)]
struct Reported {
    policy_name: String,
    policy_type_name: String,
    attributes: Vec<(String, Option<String>)>,
}

/// The table: one row per policy kind a load balancer can hold, differing in the
/// operation that created it, the base policy type, and the number and the form of the
/// attributes it carries (several, one, one with no value, none).
fn policy_cases() -> Vec<PolicyCase> {
    vec![
        PolicyCase {
            case: "duration-based stickiness, no period named",
            create: Create::LbCookieStickiness(None),
            policy_name: "sticky-default",
            policy_type_name: LB_COOKIE_STICKINESS_POLICY_TYPE,
            // The documented default: "the default value is 0, which indicates that the
            // sticky session should last for the duration of the browser session".
            attributes: &[("CookieExpirationPeriod", Some("0"))],
        },
        PolicyCase {
            case: "duration-based stickiness, 300 second period",
            create: Create::LbCookieStickiness(Some("300")),
            policy_name: "sticky-300",
            policy_type_name: LB_COOKIE_STICKINESS_POLICY_TYPE,
            attributes: &[("CookieExpirationPeriod", Some("300"))],
        },
        PolicyCase {
            case: "application-controlled stickiness",
            create: Create::AppCookieStickiness("SESSION-ID"),
            policy_name: "app-cookie",
            policy_type_name: APP_COOKIE_STICKINESS_POLICY_TYPE,
            attributes: &[("CookieName", Some("SESSION-ID"))],
        },
        PolicyCase {
            case: "proxy protocol, one attribute",
            create: Create::LoadBalancerPolicy,
            policy_name: "proxy-protocol",
            policy_type_name: "ProxyProtocolPolicyType",
            attributes: &[("ProxyProtocol", Some("true"))],
        },
        PolicyCase {
            case: "ssl negotiation, two attributes",
            create: Create::LoadBalancerPolicy,
            policy_name: "ssl-negotiation",
            policy_type_name: SSL_NEGOTIATION_POLICY_TYPE,
            attributes: &[
                ("Protocol-TLSv1.2", Some("true")),
                ("Protocol-SSLv3", Some("false")),
            ],
        },
        PolicyCase {
            case: "public key, an attribute carrying no value",
            create: Create::LoadBalancerPolicy,
            policy_name: "public-key",
            policy_type_name: "PublicKeyPolicyType",
            attributes: &[("PublicKey", None)],
        },
        PolicyCase {
            case: "back-end authentication, no attributes at all",
            create: Create::LoadBalancerPolicy,
            policy_name: "backend-auth",
            policy_type_name: "BackendServerAuthenticationPolicyType",
            attributes: &[],
        },
    ]
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

/// Mint one row's policy on `load_balancer_name`, through the operation the row names,
/// failing the test if the create does not answer 200.
async fn create_policy(
    router: &MultiServiceRouter,
    region: &str,
    load_balancer_name: &str,
    case: &PolicyCase,
) {
    let mut params = vec![
        param("LoadBalancerName", load_balancer_name),
        param("PolicyName", case.policy_name),
    ];
    let action = match &case.create {
        Create::LbCookieStickiness(period) => {
            if let Some(period) = period {
                params.push(param("CookieExpirationPeriod", period));
            }
            "CreateLBCookieStickinessPolicy"
        }
        Create::AppCookieStickiness(cookie_name) => {
            params.push(param("CookieName", cookie_name));
            "CreateAppCookieStickinessPolicy"
        }
        Create::LoadBalancerPolicy => {
            params.push(param("PolicyTypeName", case.policy_type_name));
            for (index, (name, value)) in case.attributes.iter().enumerate() {
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
            "CreateLoadBalancerPolicy"
        }
    };
    let created = call(router, region, action, &params).await;
    assert_eq!(
        created.status,
        200,
        "case {:?}: {action} must succeed, got {}",
        case.case,
        created.body
    );
}

/// Ask `DescribeLoadBalancerPolicies` about `load_balancer_name` (`None` leaving the
/// member out of the request) and `policy_names` (empty leaving the list out).
async fn describe(
    router: &MultiServiceRouter,
    region: &str,
    load_balancer_name: Option<&str>,
    policy_names: &[&str],
) -> Wire {
    let mut params = Vec::new();
    if let Some(load_balancer_name) = load_balancer_name {
        params.push(param("LoadBalancerName", load_balancer_name));
    }
    for (index, policy_name) in policy_names.iter().enumerate() {
        params.push(param(
            &format!("PolicyNames.member.{}", index + 1),
            policy_name,
        ));
    }
    call(router, region, "DescribeLoadBalancerPolicies", &params).await
}

/// The `PolicyDescriptions` of a success envelope, in the order it reported them.
fn reported(answer: &Wire) -> Vec<Reported> {
    let doc = parse(&answer.body);
    let result = answer.result(&doc);
    members(result, "PolicyDescriptions")
        .into_iter()
        .map(|description| Reported {
            policy_name: kid_text(description, "PolicyName").unwrap_or_default(),
            policy_type_name: kid_text(description, "PolicyTypeName").unwrap_or_default(),
            attributes: members(description, "PolicyAttributeDescriptions")
                .into_iter()
                .map(|attribute| {
                    (
                        kid_text(attribute, "AttributeName").unwrap_or_default(),
                        kid_text(attribute, "AttributeValue"),
                    )
                })
                .collect(),
        })
        .collect()
}

/// The description a row's policy must be reported as.
fn expected(case: &PolicyCase) -> Reported {
    Reported {
        policy_name: case.policy_name.to_string(),
        policy_type_name: case.policy_type_name.to_string(),
        attributes: case
            .attributes
            .iter()
            .map(|(name, value)| (name.to_string(), value.map(str::to_string)))
            .collect(),
    }
}

#[tokio::test]
async fn every_policy_a_load_balancer_holds_is_reported_as_its_create_configured_it() {
    let router = emulator();
    let cases = policy_cases();
    create_load_balancer(&router, REGION, "described-policies").await;
    for case in &cases {
        create_policy(&router, REGION, "described-policies", case).await;
    }
    // A second load balancer with a policy of its own, so what the describe reports is
    // the policy set of the load balancer it NAMES rather than every policy stored.
    create_load_balancer(&router, REGION, "other-policies").await;
    create_policy(
        &router,
        REGION,
        "other-policies",
        &PolicyCase {
            case: "the other load balancer's own policy",
            create: Create::LoadBalancerPolicy,
            policy_name: "elsewhere",
            policy_type_name: "ProxyProtocolPolicyType",
            attributes: &[("ProxyProtocol", Some("true"))],
        },
    )
    .await;

    // "If you specify a load balancer name, the action returns the descriptions of all
    // policies created for the load balancer" — in the store's own (name) order.
    let mut all_expected: Vec<Reported> = cases.iter().map(expected).collect();
    all_expected.sort_by(|left, right| left.policy_name.cmp(&right.policy_name));
    let all = describe(&router, REGION, Some("described-policies"), &[]).await;
    assert_eq!(all.status, 200, "the describe must succeed, got {}", all.body);
    let all_reported = reported(&all);
    assert_eq!(
        all_reported, all_expected,
        "a describe naming the load balancer alone must report every policy it holds, \
         got {}",
        all.body
    );

    // "If you specify a policy name associated with your load balancer, the action
    // returns the description of that policy."
    for case in &cases {
        let one = describe(
            &router,
            REGION,
            Some("described-policies"),
            &[case.policy_name],
        )
        .await;
        assert_eq!(
            one.status, 200,
            "case {:?}: the describe must succeed, got {}",
            case.case, one.body
        );
        assert_eq!(
            reported(&one),
            vec![expected(case)],
            "case {:?}: naming one policy must report that policy alone, got {}",
            case.case,
            one.body
        );
    }

    // Two names select two policies, in the same order the whole set is reported in.
    let pair = describe(
        &router,
        REGION,
        Some("described-policies"),
        &["sticky-300", "app-cookie"],
    )
    .await;
    let pair_expected: Vec<Reported> = cases
        .iter()
        .filter(|case| matches!(case.policy_name, "sticky-300" | "app-cookie"))
        .map(expected)
        .collect();
    let mut pair_expected_sorted = pair_expected;
    pair_expected_sorted.sort_by(|left, right| left.policy_name.cmp(&right.policy_name));
    assert_eq!(
        reported(&pair),
        pair_expected_sorted,
        "naming two policies must report those two, got {}",
        pair.body
    );

    // A repeated name names the same policy once: repeating it is not an error and does
    // not report it twice.
    let repeated = describe(
        &router,
        REGION,
        Some("described-policies"),
        &["app-cookie", "app-cookie"],
    )
    .await;
    assert_eq!(
        reported(&repeated),
        vec![expected(
            cases
                .iter()
                .find(|case| case.policy_name == "app-cookie")
                .expect("the application stickiness row")
        )],
        "a repeated policy name must report that policy once, got {}",
        repeated.body
    );

    // The other load balancer's set is its own.
    let other = describe(&router, REGION, Some("other-policies"), &[]).await;
    assert_eq!(
        reported(&other)
            .into_iter()
            .map(|description| description.policy_name)
            .collect::<Vec<String>>(),
        vec!["elsewhere".to_string()],
        "the describe must report the named load balancer's policies alone, got {}",
        other.body
    );

    // A describe is a report: everything above left the store as it found it.
    let again = describe(&router, REGION, Some("described-policies"), &[]).await;
    assert_eq!(
        reported(&again),
        all_reported,
        "describing twice must answer the same, got {}",
        again.body
    );
}

#[tokio::test]
async fn a_name_the_asked_about_set_does_not_hold_is_refused_policy_not_found() {
    let router = emulator();
    create_load_balancer(&router, REGION, "refusing-policies").await;
    create_policy(
        &router,
        REGION,
        "refusing-policies",
        &PolicyCase {
            case: "the one policy this load balancer holds",
            create: Create::LbCookieStickiness(Some("60")),
            policy_name: "held",
            policy_type_name: LB_COOKIE_STICKINESS_POLICY_TYPE,
            attributes: &[("CookieExpirationPeriod", Some("60"))],
        },
    )
    .await;
    create_load_balancer(&router, REGION, "neighbour-policies").await;
    create_policy(
        &router,
        REGION,
        "neighbour-policies",
        &PolicyCase {
            case: "a policy of the neighbouring load balancer",
            create: Create::AppCookieStickiness("SESSION"),
            policy_name: "next-door",
            policy_type_name: APP_COOKIE_STICKINESS_POLICY_TYPE,
            attributes: &[("CookieName", Some("SESSION"))],
        },
    )
    .await;

    // Each row is a name the asked-about load balancer does not hold: one nothing ever
    // created, one a SAMPLE policy carries (a sample policy is no load balancer's), one
    // another load balancer holds, and one asked for beside a name this load balancer
    // does hold — "one or more of the specified policies do not exist".
    let unknown_cases: &[(&str, &[&str])] = &[
        ("a name nothing created", &["never-created"]),
        ("a sample policy's name", &["ELBSample-ELBDefaultCipherPolicy"]),
        ("a name another load balancer holds", &["next-door"]),
        ("one unknown name beside a held one", &["held", "never-created"]),
    ];
    for (case, policy_names) in unknown_cases {
        let refused = describe(&router, REGION, Some("refusing-policies"), policy_names).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "PolicyNotFound"),
            "case {case:?}: the describe must be refused 400 PolicyNotFound, got {} {}",
            refused.status,
            refused.body
        );
        let unknown = policy_names
            .iter()
            .find(|name| **name != "held")
            .expect("each row names at least one unknown policy");
        assert_eq!(
            error_message(&refused),
            format!(
                "There is no policy with name {unknown} for load balancer refusing-policies."
            ),
            "case {case:?}: the refusal must name the policy and the load balancer, got {}",
            refused.body
        );
    }

    // A name no sample policy carries is the same refusal, stated over the policy alone
    // because the request asked about no load balancer.
    for case in ["ELBSample-NotAPolicy", "held"] {
        let refused = describe(&router, REGION, None, &[case]).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "PolicyNotFound"),
            "case {case:?}: a sample describe naming it must be refused 400 PolicyNotFound, \
             got {} {}",
            refused.status,
            refused.body
        );
        assert_eq!(
            error_message(&refused),
            format!("There is no policy with name {case}."),
            "case {case:?}: the refusal must name the policy, got {}",
            refused.body
        );
    }

    // A load balancer is resolved in the region the request is signed for: a name no
    // record holds there is not found, whichever policies the request named.
    let missing_cases: &[(&str, &str, &str, &[&str])] = &[
        ("a name nothing created", REGION, "no-such-balancer", &[]),
        ("the same name in another region", OTHER_REGION, "refusing-policies", &[]),
        (
            "an existing policy name on a missing load balancer",
            REGION,
            "no-such-balancer",
            &["held"],
        ),
    ];
    for (case, region, load_balancer_name, policy_names) in missing_cases {
        let refused = describe(&router, region, Some(load_balancer_name), policy_names).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "LoadBalancerNotFound"),
            "case {case:?}: the describe must be refused 400 LoadBalancerNotFound, got {} {}",
            refused.status,
            refused.body
        );
        assert_eq!(
            error_message(&refused),
            format!("There is no ACTIVE Load Balancer named '{load_balancer_name}'"),
            "case {case:?}: the refusal must name the load balancer, got {}",
            refused.body
        );
    }

    // A refused describe reported nothing and stored nothing: the policy set is as the
    // creates left it.
    let held = describe(&router, REGION, Some("refusing-policies"), &[]).await;
    assert_eq!(
        reported(&held)
            .into_iter()
            .map(|description| description.policy_name)
            .collect::<Vec<String>>(),
        vec!["held".to_string()],
        "the refusals must have left the policy set alone, got {}",
        held.body
    );
}

#[tokio::test]
async fn a_describe_naming_no_load_balancer_reports_the_sample_policies() {
    let router = emulator();

    // "If you don't specify a load balancer name, the action returns descriptions of the
    // specified sample policies, or descriptions of all sample policies. The names of the
    // sample policies have the ELBSample- prefix."
    let all = describe(&router, REGION, None, &[]).await;
    assert_eq!(
        all.status, 200,
        "the sample describe must succeed, got {}",
        all.body
    );
    let samples = reported(&all);
    assert!(
        !samples.is_empty(),
        "a describe naming no load balancer must report the sample policies, got {}",
        all.body
    );
    for sample in &samples {
        assert!(
            sample.policy_name.starts_with(SAMPLE_POLICY_NAME_PREFIX),
            "case {:?}: a sample policy's name must carry the documented prefix \
             {SAMPLE_POLICY_NAME_PREFIX:?}, got {:?}",
            sample.policy_name,
            sample.policy_name
        );
        assert!(
            !sample.attributes.is_empty(),
            "case {:?}: a sample policy must report the attributes its configuration \
             sets, got {:?}",
            sample.policy_name,
            sample.attributes
        );
        assert!(
            sample
                .attributes
                .iter()
                .all(|(_, value)| value.as_deref().is_some_and(|value| !value.is_empty())),
            "case {:?}: every reported attribute must carry its value, got {:?}",
            sample.policy_name,
            sample.attributes
        );
    }
    let mut names: Vec<&str> = samples
        .iter()
        .map(|sample| sample.policy_name.as_str())
        .collect();
    names.sort_unstable();
    let distinct = names.len();
    names.dedup();
    assert_eq!(
        names.len(),
        distinct,
        "every sample policy must be reported once, got {names:?}"
    );

    // Each sample policy is selectable by name, and selecting it reports exactly what the
    // unfiltered describe reported for it.
    for sample in SAMPLE_POLICIES {
        let one = describe(&router, REGION, None, &[sample.name]).await;
        assert_eq!(
            one.status, 200,
            "case {:?}: the sample describe must succeed, got {}",
            sample.name, one.body
        );
        let selected = reported(&one);
        let whole: Vec<&Reported> = samples
            .iter()
            .filter(|reported| reported.policy_name == sample.name)
            .collect();
        assert_eq!(
            selected.iter().collect::<Vec<&Reported>>(),
            whole,
            "case {:?}: naming one sample policy must report what the whole set reports \
             for it, got {}",
            sample.name,
            one.body
        );
    }

    // A policy of a LOAD BALANCER is not a sample policy, and a sample describe is a
    // report: it answers the same after the load balancer's policies exist.
    create_load_balancer(&router, REGION, "sample-neighbour").await;
    create_policy(
        &router,
        REGION,
        "sample-neighbour",
        &PolicyCase {
            case: "a policy of a load balancer",
            create: Create::LoadBalancerPolicy,
            policy_name: "mine",
            policy_type_name: SSL_NEGOTIATION_POLICY_TYPE,
            attributes: &[("Protocol-TLSv1.2", Some("true"))],
        },
    )
    .await;
    let again = describe(&router, REGION, None, &[]).await;
    assert_eq!(
        reported(&again),
        samples,
        "the sample policies must not follow a load balancer's policy set, got {}",
        again.body
    );
}

#[tokio::test]
async fn the_reported_set_follows_every_create_and_delete() {
    let router = emulator();
    create_load_balancer(&router, REGION, "policy-lifecycle").await;
    let cases = policy_cases();
    let mut held: Vec<&PolicyCase> = Vec::new();
    for case in &cases {
        create_policy(&router, REGION, "policy-lifecycle", case).await;
        held.push(case);
        assert_eq!(
            reported_names(&router, "policy-lifecycle").await,
            sorted_names(&held),
            "case {:?}: the create must be reported by the next describe",
            case.case
        );
    }

    for case in &cases {
        let deleted = call(
            &router,
            REGION,
            "DeleteLoadBalancerPolicy",
            &[
                param("LoadBalancerName", "policy-lifecycle"),
                param("PolicyName", case.policy_name),
            ],
        )
        .await;
        assert_eq!(
            deleted.status, 200,
            "case {:?}: the delete must succeed, got {}",
            case.case, deleted.body
        );
        held.retain(|candidate| candidate.policy_name != case.policy_name);
        assert_eq!(
            reported_names(&router, "policy-lifecycle").await,
            sorted_names(&held),
            "case {:?}: the delete must be reported by the next describe",
            case.case
        );
        // The deleted policy is gone from the set, so naming it is now not found.
        let refused = describe(
            &router,
            REGION,
            Some("policy-lifecycle"),
            &[case.policy_name],
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "PolicyNotFound"),
            "case {:?}: naming the deleted policy must be refused 400 PolicyNotFound, \
             got {} {}",
            case.case,
            refused.status,
            refused.body
        );
    }

    // Every policy deleted: the describe reports the empty set rather than refusing.
    let empty = describe(&router, REGION, Some("policy-lifecycle"), &[]).await;
    assert_eq!(
        empty.status, 200,
        "a load balancer holding no policy must still be described, got {}",
        empty.body
    );
    assert!(
        reported(&empty).is_empty(),
        "a load balancer holding no policy must report no description, got {}",
        empty.body
    );
    // The envelope carries the (empty) list member rather than leaving it out.
    let doc = parse(&empty.body);
    assert!(
        kid(empty.result(&doc), "PolicyDescriptions").is_some(),
        "the answer must carry the PolicyDescriptions member, got {}",
        empty.body
    );
}

#[tokio::test]
async fn a_long_policy_names_list_is_answered_by_the_same_rule() {
    let router = emulator();
    create_load_balancer(&router, REGION, "bounded-policies").await;
    create_policy(
        &router,
        REGION,
        "bounded-policies",
        &PolicyCase {
            case: "the one policy this load balancer holds",
            create: Create::LbCookieStickiness(Some("120")),
            policy_name: "repeated",
            policy_type_name: LB_COOKIE_STICKINESS_POLICY_TYPE,
            attributes: &[("CookieExpirationPeriod", Some("120"))],
        },
    )
    .await;

    // However many names a request carries, the rule is the same one: a name the load
    // balancer does not hold is not found, and the refusal names the FIRST such name the
    // request carried rather than whichever one sorts first.
    let names: Vec<String> = (0..256).map(|index| format!("policy-{index}")).collect();
    let mut asked: Vec<&str> = vec!["repeated"];
    asked.extend(names.iter().map(String::as_str));
    let refused = describe(&router, REGION, Some("bounded-policies"), &asked).await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (400, "PolicyNotFound"),
        "a long list naming policies the load balancer does not hold must be refused 400 \
         PolicyNotFound, got {} {}",
        refused.status,
        refused.body
    );
    assert_eq!(
        error_message(&refused),
        "There is no policy with name policy-0 for load balancer bounded-policies.",
        "the refusal must name the first policy the request carried that does not exist, \
         got {}",
        refused.body
    );

    // Repeating ONE name is not naming many: a long list that names a single policy is
    // accepted, and the policy is reported once.
    let repeated: Vec<&str> = vec!["repeated"; 256];
    let answered = describe(&router, REGION, Some("bounded-policies"), &repeated).await;
    assert_eq!(
        answered.status, 200,
        "a list repeating one policy name must be accepted, got {}",
        answered.body
    );
    assert_eq!(
        reported(&answered)
            .into_iter()
            .map(|description| description.policy_name)
            .collect::<Vec<String>>(),
        vec!["repeated".to_string()],
        "a repeated policy name must report that policy once, got {}",
        answered.body
    );
}

/// The `PolicyName`s a describe of `load_balancer_name` reports, in the order it reports
/// them.
async fn reported_names(router: &MultiServiceRouter, load_balancer_name: &str) -> Vec<String> {
    let described = describe(router, REGION, Some(load_balancer_name), &[]).await;
    assert_eq!(
        described.status, 200,
        "the describe must succeed, got {}",
        described.body
    );
    reported(&described)
        .into_iter()
        .map(|description| description.policy_name)
        .collect()
}

/// The names of `held`, in the order a describe reports them.
fn sorted_names(held: &[&PolicyCase]) -> Vec<String> {
    let mut names: Vec<String> = held
        .iter()
        .map(|case| case.policy_name.to_string())
        .collect();
    names.sort();
    names
}
