// SPDX-License-Identifier: CC0-1.0

//! Duration-based session stickiness policies, end to end over the wire.
//!
//! Every request goes through the awsQuery protocol on the multi-service router (see
//! `super::wire`), so what is asserted is the emulator's HTTP behaviour.
//!
//! The rules under test, each over a TABLE of inputs rather than one recorded request:
//!
//! * `CreateLBCookieStickinessPolicy` persists one policy on the load balancer the
//!   request names, under the `PolicyName` it names, carrying the
//!   `CookieExpirationPeriod` it carries — or the documented default of 0 ("the duration
//!   of the browser session") when it carries none — and a later
//!   `DescribeLoadBalancers` reports exactly that policy under
//!   `Policies.LBCookieStickinessPolicies`.
//! * a policy name is unique within ONE load balancer: a repeat is refused
//!   `DuplicatePolicyName` and writes nothing, while the same name on another load
//!   balancer is accepted.
//! * the load balancer is resolved in the region the request is signed for, so a name no
//!   record holds there is refused `LoadBalancerNotFound`.
//! * a request the model cannot constrain — a policy name outside "alphanumeric
//!   characters and dashes (-)", a negative cookie expiration period — is refused
//!   `ValidationException` and writes nothing.
//!
//! Every create is asserted on what a LATER describe reports, never on what the create
//! answered alone.

use super::wire::{call, emulator, kid, kid_text, members, param, parse};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for unless the case names another.
const REGION: &str = "us-east-1";

/// A second region, for the rule that a load balancer is resolved in the region the
/// request is signed for.
const OTHER_REGION: &str = "eu-west-1";

/// One row of the persistence table: the case, the policy name, the
/// `CookieExpirationPeriod` the request carries (`None` leaves the member out), and the
/// period a describe must then report.
struct PolicyCase {
    case: &'static str,
    policy_name: &'static str,
    requested_period: Option<i64>,
    reported_period: i64,
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

/// Ask `CreateLBCookieStickinessPolicy` for `policy_name` on `load_balancer_name`.
async fn create_policy(
    router: &MultiServiceRouter,
    region: &str,
    load_balancer_name: &str,
    policy_name: &str,
    cookie_expiration_period: Option<&str>,
) -> super::wire::Wire {
    let mut params = vec![
        param("LoadBalancerName", load_balancer_name),
        param("PolicyName", policy_name),
    ];
    if let Some(period) = cookie_expiration_period {
        params.push(param("CookieExpirationPeriod", period));
    }
    call(
        router,
        region,
        "CreateLBCookieStickinessPolicy",
        &params,
    )
    .await
}

/// The `(PolicyName, CookieExpirationPeriod)` pairs `DescribeLoadBalancers` reports under
/// `Policies.LBCookieStickinessPolicies` for `name`, in the order it reports them.
async fn described_lb_cookie_policies(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
) -> Vec<(String, String)> {
    let described = call(
        router,
        region,
        "DescribeLoadBalancers",
        &[param("LoadBalancerNames.member.1", name)],
    )
    .await;
    let doc = parse(&described.body);
    let result = described.result(&doc);
    let description = *members(result, "LoadBalancerDescriptions")
        .first()
        .unwrap_or_else(|| panic!("no description of {name} in {}", described.body));
    let policies = kid(description, "Policies")
        .unwrap_or_else(|| panic!("no Policies in {}", described.body));
    members(policies, "LBCookieStickinessPolicies")
        .into_iter()
        .map(|member| {
            (
                kid_text(member, "PolicyName").unwrap_or_default(),
                kid_text(member, "CookieExpirationPeriod").unwrap_or_default(),
            )
        })
        .collect()
}

/// The other two members of the reported `Policies`: the application-controlled
/// stickiness policies and the policies that are neither.
async fn described_other_policy_members(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
) -> (usize, usize) {
    let described = call(
        router,
        region,
        "DescribeLoadBalancers",
        &[param("LoadBalancerNames.member.1", name)],
    )
    .await;
    let doc = parse(&described.body);
    let result = described.result(&doc);
    let description = *members(result, "LoadBalancerDescriptions")
        .first()
        .unwrap_or_else(|| panic!("no description of {name} in {}", described.body));
    let policies = kid(description, "Policies")
        .unwrap_or_else(|| panic!("no Policies in {}", described.body));
    (
        members(policies, "AppCookieStickinessPolicies").len(),
        members(policies, "OtherPolicies").len(),
    )
}

/// A create persists the policy the request describes, and a later describe reports it:
/// one row per `CookieExpirationPeriod` form the member can take, plus the policy-name
/// forms the documented grammar allows.
#[tokio::test]
async fn a_create_persists_the_policy_a_later_describe_reports() {
    // The periods span the member's own class: absent (the documented default), the
    // default stated explicitly, the smallest positive value, two everyday durations and
    // a value past 32 bits, which only a `long` can carry.
    let cases = [
        PolicyCase {
            case: "no CookieExpirationPeriod: the documented default of 0",
            policy_name: "browser-session",
            requested_period: None,
            reported_period: 0,
        },
        PolicyCase {
            case: "the default stated explicitly",
            policy_name: "explicit-zero",
            requested_period: Some(0),
            reported_period: 0,
        },
        PolicyCase {
            case: "the smallest positive period",
            policy_name: "OneSecond1",
            requested_period: Some(1),
            reported_period: 1,
        },
        PolicyCase {
            case: "a minute",
            policy_name: "60",
            requested_period: Some(60),
            reported_period: 60,
        },
        PolicyCase {
            case: "an hour",
            policy_name: "an-hour-long-policy",
            requested_period: Some(3600),
            reported_period: 3600,
        },
        PolicyCase {
            case: "a period past 32 bits, which only a long carries",
            policy_name: "wide-period",
            requested_period: Some(4_294_967_296),
            reported_period: 4_294_967_296,
        },
    ];

    let router = emulator();
    create_load_balancer(&router, REGION, "sticky-lb").await;

    for case in &cases {
        let created = create_policy(
            &router,
            REGION,
            "sticky-lb",
            case.policy_name,
            case.requested_period
                .map(|period| period.to_string())
                .as_deref(),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: the create must answer 200, got {} with {}",
            case.case, created.status, created.body
        );
    }

    // One describe reports every policy the creates persisted, in `PolicyName` order.
    let reported = described_lb_cookie_policies(&router, REGION, "sticky-lb").await;
    let mut expected: Vec<(String, String)> = cases
        .iter()
        .map(|case| {
            (
                case.policy_name.to_string(),
                case.reported_period.to_string(),
            )
        })
        .collect();
    expected.sort();
    assert_eq!(
        reported, expected,
        "the describe must report every created policy with the period the create \
         carried, in PolicyName order; got {reported:?}"
    );

    // A describe is a report: reading twice reports the same policies.
    let again = described_lb_cookie_policies(&router, REGION, "sticky-lb").await;
    assert_eq!(
        again, reported,
        "a second describe must report the same policies, got {again:?}"
    );

    // Nothing here creates an application-controlled or a non-stickiness policy, so the
    // other two members of `Policies` stay empty.
    let (app_policies, other_policies) =
        described_other_policy_members(&router, REGION, "sticky-lb").await;
    assert_eq!(
        (app_policies, other_policies),
        (0, 0),
        "only the duration-based stickiness policies were created, so the other two \
         Policies members must be empty; got {app_policies} and {other_policies}"
    );

    // A load balancer nobody created a policy for holds none.
    create_load_balancer(&router, REGION, "no-policy-lb").await;
    let none = described_lb_cookie_policies(&router, REGION, "no-policy-lb").await;
    assert!(
        none.is_empty(),
        "a load balancer with no policy must report an empty list, got {none:?}"
    );
}

/// A policy name is unique within ONE load balancer: a repeat is refused and writes
/// nothing, and the same name on a second load balancer is accepted.
#[tokio::test]
async fn a_policy_name_is_unique_within_one_load_balancer() {
    let router = emulator();
    create_load_balancer(&router, REGION, "first-lb").await;
    create_load_balancer(&router, REGION, "second-lb").await;

    let created = create_policy(&router, REGION, "first-lb", "shared-name", Some("30")).await;
    assert_eq!(
        created.status, 200,
        "the first create must succeed, got {}",
        created.body
    );

    // A repeat of a taken name is refused whether or not it asks for the same period.
    for (case, period) in [
        ("the same period", Some("30")),
        ("a different period", Some("90")),
        ("no period at all", None),
    ] {
        let repeated = create_policy(&router, REGION, "first-lb", "shared-name", period).await;
        assert_eq!(
            (repeated.status, repeated.error_code().as_str()),
            (400, "DuplicatePolicyName"),
            "case {case:?}: a repeat of a taken policy name must be refused 400 \
             DuplicatePolicyName, got {} with {}",
            repeated.status,
            repeated.body
        );
    }

    // The refused repeats wrote nothing: the stored policy still carries the period the
    // accepted create asked for.
    let reported = described_lb_cookie_policies(&router, REGION, "first-lb").await;
    assert_eq!(
        reported,
        vec![("shared-name".to_string(), "30".to_string())],
        "a refused repeat must leave the stored policy as it was, got {reported:?}"
    );

    // The name is taken on THAT load balancer only.
    let other = create_policy(&router, REGION, "second-lb", "shared-name", Some("90")).await;
    assert_eq!(
        other.status, 200,
        "the same policy name on another load balancer must be accepted, got {}",
        other.body
    );
    let reported = described_lb_cookie_policies(&router, REGION, "second-lb").await;
    assert_eq!(
        reported,
        vec![("shared-name".to_string(), "90".to_string())],
        "the second load balancer must hold its own policy, got {reported:?}"
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
        let refused = create_policy(&router, region, name, "orphan-policy", Some("60")).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "LoadBalancerNotFound"),
            "case {case:?}: the create must be refused 400 LoadBalancerNotFound, got {} \
             with {}",
            refused.status,
            refused.body
        );
    }

    // The refusals wrote nothing on the load balancer that does exist.
    let reported = described_lb_cookie_policies(&router, REGION, "present-lb").await;
    assert!(
        reported.is_empty(),
        "a refused create must persist no policy, got {reported:?}"
    );
}

/// The emulator's per-load-balancer policy bound answers the modelled
/// `TooManyPoliciesException`, and the policies already stored stay as they were.
#[tokio::test]
async fn a_create_past_the_policy_bound_is_refused_too_many_policies() {
    use service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::state::MAX_POLICIES_PER_LOAD_BALANCER;

    let router = emulator();
    create_load_balancer(&router, REGION, "bounded-lb").await;

    for slot in 1..=MAX_POLICIES_PER_LOAD_BALANCER {
        let created = create_policy(
            &router,
            REGION,
            "bounded-lb",
            &format!("policy-{slot}"),
            Some("60"),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {slot}: a create up to the bound must answer 200, got {}",
            created.body
        );
    }

    let refused = create_policy(&router, REGION, "bounded-lb", "one-too-many", Some("60")).await;
    assert_eq!(
        (refused.status, refused.error_code().as_str()),
        (400, "TooManyPolicies"),
        "a create past the bound must be refused 400 TooManyPolicies, got {} with {}",
        refused.status,
        refused.body
    );

    let reported = described_lb_cookie_policies(&router, REGION, "bounded-lb").await;
    assert_eq!(
        reported.len(),
        MAX_POLICIES_PER_LOAD_BALANCER,
        "the refused create must leave the stored policies as they were, got {} of them",
        reported.len()
    );
}

/// A request outside a rule the model cannot express is refused 400 `ValidationException`
/// and writes nothing.
#[tokio::test]
async fn a_request_outside_a_documented_rule_is_refused() {
    let router = emulator();
    create_load_balancer(&router, REGION, "refusal-lb").await;

    // `PolicyName`: "Policy names must consist of alphanumeric characters and dashes
    // (-)", so one row per other character class a caller reaches for, plus the empty
    // name. `CookieExpirationPeriod` is "the time period, in seconds, after which the
    // cookie should be considered stale", so a negative value names no period.
    let cases: Vec<(&str, &str, Option<&str>)> = vec![
        ("an underscore in the policy name", "bad_name", Some("60")),
        ("a space in the policy name", "bad name", Some("60")),
        ("a dot in the policy name", "bad.name", Some("60")),
        ("a slash in the policy name", "bad/name", Some("60")),
        ("a non-ASCII letter in the policy name", "poliçy", Some("60")),
        ("an empty policy name", "", Some("60")),
        ("a negative cookie expiration period", "negative", Some("-1")),
        (
            "a large negative cookie expiration period",
            "very-negative",
            Some("-3600"),
        ),
    ];

    for (case, policy_name, period) in cases {
        let refused = create_policy(&router, REGION, "refusal-lb", policy_name, period).await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "ValidationException"),
            "case {case:?}: the create must be refused 400 ValidationException, got {} \
             with {}",
            refused.status,
            refused.body
        );
    }

    let reported = described_lb_cookie_policies(&router, REGION, "refusal-lb").await;
    assert!(
        reported.is_empty(),
        "a refused create must persist no policy, got {reported:?}"
    );

    // The same request with the one rejected part corrected is accepted, so the refusals
    // above are about the rule and not about the rest of the request.
    let accepted = create_policy(&router, REGION, "refusal-lb", "good-name-1", Some("60")).await;
    assert_eq!(
        accepted.status, 200,
        "a request inside every documented rule must be accepted, got {}",
        accepted.body
    );
    let reported = described_lb_cookie_policies(&router, REGION, "refusal-lb").await;
    assert_eq!(
        reported,
        vec![("good-name-1".to_string(), "60".to_string())],
        "the accepted create must be the one policy the record holds, got {reported:?}"
    );
}
