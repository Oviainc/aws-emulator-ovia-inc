// SPDX-License-Identifier: CC0-1.0

//! The two ELBv2 catalogue reads over the wire: `DescribeAccountLimits` and
//! `DescribeSSLPolicies`.
//!
//! Every request goes through the router exactly as a client's would — awsQuery form
//! body in, `<{Operation}Response>` XML out — so these tests exercise the wire protocol
//! as well as the handlers.
//!
//! Neither operation reads a resource a caller created: both report a catalogue the
//! SERVICE publishes (`DescribeAccountLimits` the account's quotas,
//! `DescribeSSLPolicies` the predefined TLS security policies, neither of which any
//! operation in this model creates, changes or deletes). So the rule under test is not
//! one recorded answer but the shape of a catalogue read:
//!
//! * it answers `200` with its whole catalogue, every entry carrying every member its
//!   modelled shape declares;
//! * it is narrowed by the filters the operation models (`Names`, `LoadBalancerType`)
//!   and paged by `PageSize`/`Marker` in one stable order, so the pages of any page size
//!   concatenate to the unpaged order;
//! * an entry the catalogue does not carry is the operation's own modelled refusal
//!   (`SSLPolicyNotFound`, HTTP 400), never a success with an empty list;
//! * and it is a REPORT: the same request answers the same catalogue however often it is
//!   made.
//!
//! Each suite therefore spans a TABLE of inputs — filters, page sizes, both published
//! `PageSize` bounds and one past each — and asserts invariants over EVERY entry the
//! catalogue reports rather than over the one entry the recorded request named.

use crate::provisioning_loop::{action, call, emulator, first_text, members, p, Reply};
use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::{
    LISTENERS_PER_LOAD_BALANCER, LOAD_BALANCER_QUOTA, TARGETS_PER_TARGET_GROUP,
    TARGET_GROUP_QUOTA,
};
use smithy_server_runtime::MultiServiceRouter;

/// The modelled bounds of `PageSize` on both operations (`range 1..400`:
/// `smithy_query(query="operation",operation="DescribeAccountLimits,DescribeSSLPolicies")`).
const PAGE_SIZE_MIN: i64 = 1;
const PAGE_SIZE_MAX: i64 = 400;

/// One awsQuery call to `action_name` with the extra form fields `fields`.
async fn describe(
    router: &MultiServiceRouter,
    action_name: &str,
    fields: &[(String, String)],
) -> Reply {
    let mut form = action(action_name);
    form.extend_from_slice(fields);
    call(router, &form).await
}

// ---------------------------------------------------------------------------
// DescribeAccountLimits
// ---------------------------------------------------------------------------

/// The `(Name, Max)` pairs the reply reports, in the order it reported them.
fn reported_limits(reply: &Reply) -> Vec<(String, String)> {
    members(&reply.body, "Limits")
        .iter()
        .map(|limit| {
            (
                first_text(limit, "Name").unwrap_or_default(),
                first_text(limit, "Max").unwrap_or_default(),
            )
        })
        .collect()
}

#[tokio::test]
async fn every_account_limit_is_reported_with_its_maximum() {
    let (router, _engine) = emulator();
    let reply = describe(&router, "DescribeAccountLimits", &[]).await;

    assert_eq!(
        reply.status, 200,
        "case whole catalogue: expected 200, got {} body {}",
        reply.status, reply.body
    );
    assert!(
        reply.body.contains("<DescribeAccountLimitsResponse"),
        "case whole catalogue: expected a DescribeAccountLimitsResponse envelope, got {}",
        reply.body
    );

    let limits = reported_limits(&reply);
    assert!(
        !limits.is_empty(),
        "case whole catalogue: expected the account's limits, got {}",
        reply.body
    );

    // Every entry carries both members its modelled shape declares, and a maximum is a
    // positive count — whatever the limit is named.
    for (name, max) in &limits {
        assert!(
            !name.is_empty(),
            "case {name:?}: every limit carries a Name, got {}",
            reply.body
        );
        let parsed = max.parse::<u64>();
        assert!(
            parsed.as_ref().is_ok_and(|value| *value >= 1),
            "case {name:?}: expected a positive decimal Max, got {max:?}"
        );
    }

    // No limit is reported twice: a caller keyed by name would silently lose one.
    let mut names: Vec<&str> = limits.iter().map(|(name, _)| name.as_str()).collect();
    names.sort_unstable();
    let unique = {
        let mut unique = names.clone();
        unique.dedup();
        unique
    };
    assert_eq!(
        names, unique,
        "case whole catalogue: expected each limit once, got {names:?}"
    );

    // The quotas this emulator ENFORCES must report the bound a caller actually hits:
    // the constants the create paths refuse past (`crate::state`'s
    // `LOAD_BALANCER_QUOTA`, `TARGET_GROUP_QUOTA`, `LISTENERS_PER_LOAD_BALANCER`).
    let enforced: &[(&str, usize)] = &[
        ("application-load-balancers", LOAD_BALANCER_QUOTA),
        ("network-load-balancers", LOAD_BALANCER_QUOTA),
        ("gateway-load-balancers", LOAD_BALANCER_QUOTA),
        ("target-groups", TARGET_GROUP_QUOTA),
        (
            "listeners-per-application-load-balancer",
            LISTENERS_PER_LOAD_BALANCER,
        ),
        (
            "listeners-per-network-load-balancer",
            LISTENERS_PER_LOAD_BALANCER,
        ),
        ("targets-per-target-group", TARGETS_PER_TARGET_GROUP),
    ];
    for (name, expected) in enforced {
        let reported = limits
            .iter()
            .find(|(limit, _)| limit == name)
            .map(|(_, max)| max.clone());
        assert_eq!(
            reported.as_deref(),
            Some(expected.to_string().as_str()),
            "case {name:?}: expected the enforced bound {expected}, got {reported:?}"
        );
    }

    // Published quotas the emulator does not enforce itself, including names the issue
    // this suite was written for never cited: each is reported, with its published
    // per-Region default.
    let published: &[(&str, &str)] = &[
        ("application-load-balancer-capacity-units", "3000"),
        ("ca-certificates-per-trust-store", "25"),
        ("ca-certificates-subject-size-per-trust-store", "256"),
        ("certificates-per-application-load-balancer", "25"),
        ("certificates-per-network-load-balancer", "25"),
        ("condition-values-per-alb-rule", "5"),
        ("condition-wildcards-per-alb-rule", "5"),
        ("gateway-load-balancer-capacity-units", "3000"),
        ("gateway-load-balancers-per-vpc", "100"),
        ("geneve-target-groups", "100"),
        ("network-load-balancer-capacity-units", "3000"),
        ("network-load-balancer-enis-per-vpc", "1200"),
        ("revocations-per-trust-store", "5"),
        ("revoked-entries-per-trust-store", "100000"),
        ("rules-per-application-load-balancer", "100"),
        ("rules-per-network-load-balancer", "100"),
        ("target-groups-per-action-on-application-load-balancer", "5"),
        ("target-groups-per-action-on-network-load-balancer", "1"),
        ("target-groups-per-application-load-balancer", "100"),
        ("target-groups-per-network-load-balancer", "100"),
        (
            "target-id-registrations-per-application-load-balancer",
            "1000",
        ),
        ("targets-per-application-load-balancer", "1000"),
        (
            "targets-per-availability-zone-per-gateway-load-balancer",
            "300",
        ),
        (
            "targets-per-availability-zone-per-network-load-balancer",
            "500",
        ),
        ("targets-per-network-load-balancer", "3000"),
        ("trust-stores", "1000"),
    ];
    for (name, expected) in published {
        let reported = limits
            .iter()
            .find(|(limit, _)| limit == name)
            .map(|(_, max)| max.clone());
        assert_eq!(
            reported.as_deref(),
            Some(*expected),
            "case {name:?}: expected the published default {expected}, got {reported:?}"
        );
    }

    // The catalogue is exactly the enforced quotas plus the published ones: no limit is
    // reported that neither table names, so a name added to the handler without a source
    // fails here.
    let mut expected_names: Vec<&str> = enforced
        .iter()
        .map(|(name, _)| *name)
        .chain(published.iter().map(|(name, _)| *name))
        .collect();
    expected_names.sort_unstable();
    assert_eq!(
        names, expected_names,
        "case whole catalogue: expected exactly the named quotas, got {names:?}"
    );

    // A read changes nothing: the same request answers the same catalogue.
    let again = describe(&router, "DescribeAccountLimits", &[]).await;
    assert_eq!(
        reported_limits(&again),
        limits,
        "case repeated read: a report must not change what a later caller sees, got {}",
        again.body
    );
}

#[tokio::test]
async fn the_account_limits_catalogue_pages_in_one_stable_order() {
    let (router, _engine) = emulator();
    let whole = reported_limits(&describe(&router, "DescribeAccountLimits", &[]).await);
    assert!(
        whole.len() > 1,
        "case unpaged: the paging table needs more than one limit, got {whole:?}"
    );

    // Pages of any size concatenate to the unpaged order, including a page size larger
    // than the catalogue and the largest the model allows.
    for page_size in [1usize, 2, 7, whole.len(), whole.len() + 1, 400] {
        let case = format!("page size {page_size}");
        let mut walked: Vec<(String, String)> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut fields = vec![p("PageSize", &page_size.to_string())];
            if let Some(marker) = marker.as_deref() {
                fields.push(p("Marker", marker));
            }
            let page = describe(&router, "DescribeAccountLimits", &fields).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: expected 200, got {} body {}",
                page.status, page.body
            );
            let reported = reported_limits(&page);
            assert!(
                reported.len() <= page_size,
                "case {case:?}: a page holds at most {page_size} limits, got {}",
                reported.len()
            );
            walked.extend(reported);
            pages += 1;
            assert!(
                pages <= whole.len() + 1,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
            match page.text("NextMarker") {
                Some(next) => marker = Some(next),
                None => break,
            }
        }
        assert_eq!(
            walked, whole,
            "case {case:?}: expected the unpaged order, got {walked:?}"
        );
    }

    // A marker that is not a position in this catalogue is refused rather than silently
    // treated as the first page.
    for (case, marker) in [
        ("marker that is not a number", "not-a-marker"),
        ("marker past the end", "9999"),
    ] {
        let refused = describe(
            &router,
            "DescribeAccountLimits",
            &[p("Marker", marker)],
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException, got {} body {}",
            refused.status,
            refused.body
        );
    }
}

// ---------------------------------------------------------------------------
// DescribeSSLPolicies
// ---------------------------------------------------------------------------

/// The `<member>` XML of each policy the reply reports.
fn reported_policies(reply: &Reply) -> Vec<String> {
    members(&reply.body, "SslPolicies")
}

/// The policy names the reply reports, in the order it reported them.
fn reported_policy_names(reply: &Reply) -> Vec<String> {
    reported_policies(reply)
        .iter()
        .map(|policy| policy_name(policy))
        .collect()
}

/// The policy name of one `<member>` of `SslPolicies`.
///
/// `SslPolicy` serializes in its schema's member order — `SslProtocols`, `Ciphers`,
/// `Name`, `SupportedLoadBalancerTypes` — and every cipher carries a `<Name>` of its
/// own, so the policy's own name is read from the text AFTER the cipher list.
fn policy_name(member: &str) -> String {
    let tail = member
        .split_once("</Ciphers>")
        .map(|(_, tail)| tail)
        .unwrap_or(member);
    first_text(tail, "Name").unwrap_or_default()
}

/// The `(Name, Priority)` of each cipher of one policy member, in the reported order.
fn reported_ciphers(member: &str) -> Vec<(String, String)> {
    members(member, "Ciphers")
        .iter()
        .map(|cipher| {
            (
                first_text(cipher, "Name").unwrap_or_default(),
                first_text(cipher, "Priority").unwrap_or_default(),
            )
        })
        .collect()
}

#[tokio::test]
async fn every_predefined_security_policy_is_reported_in_full() {
    let (router, _engine) = emulator();
    let reply = describe(&router, "DescribeSSLPolicies", &[]).await;

    assert_eq!(
        reply.status, 200,
        "case whole catalogue: expected 200, got {} body {}",
        reply.status, reply.body
    );
    assert!(
        reply.body.contains("<DescribeSSLPoliciesResponse"),
        "case whole catalogue: expected a DescribeSSLPoliciesResponse envelope, got {}",
        reply.body
    );

    let policies = reported_policies(&reply);
    assert!(
        policies.len() > 1,
        "case whole catalogue: expected the predefined policies, got {}",
        reply.body
    );

    // The invariants every entry of the catalogue holds, asserted over ALL of them
    // rather than over one named policy.
    let mut names: Vec<String> = Vec::new();
    for policy in &policies {
        let name = policy_name(policy);
        assert!(
            name.starts_with("ELBSecurityPolicy-"),
            "case {name:?}: expected a predefined policy name, got {policy}"
        );
        names.push(name.clone());

        let protocols = members(policy, "SslProtocols");
        assert!(
            !protocols.is_empty(),
            "case {name:?}: a policy negotiates at least one protocol, got {policy}"
        );
        for protocol in &protocols {
            assert!(
                protocol.starts_with("TLSv1"),
                "case {name:?}: expected a TLS protocol version, got {protocol:?}"
            );
        }

        // Priority is the 1-based position in AWS's own cipher order, so the reported
        // priorities of a policy are exactly 1..=n with no gap and no repeat.
        let ciphers = reported_ciphers(policy);
        assert!(
            !ciphers.is_empty(),
            "case {name:?}: a policy offers at least one cipher, got {policy}"
        );
        let expected: Vec<String> = (1..=ciphers.len()).map(|at| at.to_string()).collect();
        let priorities: Vec<String> = ciphers
            .iter()
            .map(|(_, priority)| priority.clone())
            .collect();
        assert_eq!(
            priorities, expected,
            "case {name:?}: expected cipher priorities 1..={}, got {priorities:?}",
            ciphers.len()
        );
        for (cipher, _) in &ciphers {
            assert!(
                !cipher.is_empty(),
                "case {name:?}: every cipher carries a Name, got {policy}"
            );
        }

        // A TLS policy can be set on an HTTPS listener of an Application Load Balancer
        // or a TLS listener of a Network Load Balancer; `LoadBalancerTypeEnum`'s third
        // value, `gateway`, has no TLS listener to set one on.
        let types = members(policy, "SupportedLoadBalancerTypes");
        assert!(
            !types.is_empty(),
            "case {name:?}: a policy supports at least one load balancer type, got {policy}"
        );
        for lb_type in &types {
            assert!(
                ["application", "network"].contains(&lb_type.as_str()),
                "case {name:?}: expected a modelled load balancer type, got {lb_type:?}"
            );
        }
    }

    let mut sorted = names.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        names.len(),
        "case whole catalogue: expected each policy once, got {names:?}"
    );

    // The default policy of a new HTTPS or TLS listener, spelled out: 18 ciphers over
    // TLS 1.0, 1.1 and 1.2.
    let default = policies
        .iter()
        .find(|policy| policy_name(policy) == "ELBSecurityPolicy-2016-08")
        .unwrap_or_else(|| panic!("case default policy: expected it in {names:?}"));
    assert_eq!(
        members(default, "SslProtocols"),
        vec!["TLSv1", "TLSv1.1", "TLSv1.2"],
        "case default policy: expected TLS 1.0 through 1.2, got {default}"
    );
    assert_eq!(
        reported_ciphers(default).len(),
        18,
        "case default policy: expected 18 ciphers, got {:?}",
        reported_ciphers(default)
    );
    assert_eq!(
        reported_ciphers(default).first().map(|(name, _)| name.clone()),
        Some("ECDHE-ECDSA-AES128-GCM-SHA256".to_string()),
        "case default policy: expected its highest-priority cipher first, got {default}"
    );

    // A read changes nothing.
    let again = describe(&router, "DescribeSSLPolicies", &[]).await;
    assert_eq!(
        reported_policies(&again),
        policies,
        "case repeated read: a report must not change what a later caller sees, got {}",
        again.body
    );

    // Every policy, asked for BY NAME, is reported exactly as the whole catalogue
    // reported it — including the ones no issue cited.
    for policy in &policies {
        let name = policy_name(policy);
        let one = describe(
            &router,
            "DescribeSSLPolicies",
            &[p("Names.member.1", &name)],
        )
        .await;
        assert_eq!(
            one.status, 200,
            "case {name:?}: expected 200 by name, got {} body {}",
            one.status, one.body
        );
        assert_eq!(
            reported_policies(&one),
            vec![policy.clone()],
            "case {name:?}: expected the same policy document by name, got {}",
            one.body
        );
    }
}

#[tokio::test]
async fn the_security_policy_catalogue_is_narrowed_by_the_modelled_filters() {
    let (router, _engine) = emulator();
    let whole = reported_policy_names(&describe(&router, "DescribeSSLPolicies", &[]).await);

    /// One filter row: the case, the extra form fields, and what must be reported.
    struct Row {
        case: &'static str,
        fields: Vec<(String, String)>,
        /// The exact policy names expected, or `None` when the row expects the whole
        /// catalogue.
        expected: Option<Vec<String>>,
    }

    let rows: Vec<Row> = vec![
        Row {
            // One name, an entry the issue this suite was written for never cited.
            case: "one name",
            fields: vec![p("Names.member.1", "ELBSecurityPolicy-TLS13-1-2-2021-06")],
            expected: Some(vec!["ELBSecurityPolicy-TLS13-1-2-2021-06".to_string()]),
        },
        Row {
            // Two names, reported in the order the request listed them — not in
            // catalogue order, which is the reverse of this.
            case: "two names, listed newest first",
            fields: vec![
                p("Names.member.1", "ELBSecurityPolicy-TLS13-1-3-2021-06"),
                p("Names.member.2", "ELBSecurityPolicy-2016-08"),
            ],
            expected: Some(vec![
                "ELBSecurityPolicy-TLS13-1-3-2021-06".to_string(),
                "ELBSecurityPolicy-2016-08".to_string(),
            ]),
        },
        Row {
            // The same name twice names one policy, so it is reported once.
            case: "one name listed twice",
            fields: vec![
                p("Names.member.1", "ELBSecurityPolicy-FS-1-2-Res-2020-10"),
                p("Names.member.2", "ELBSecurityPolicy-FS-1-2-Res-2020-10"),
            ],
            expected: Some(vec!["ELBSecurityPolicy-FS-1-2-Res-2020-10".to_string()]),
        },
        Row {
            // "The default lists the SSL policies for all load balancers": an absent
            // `LoadBalancerType` filters nothing.
            case: "no filter at all",
            fields: vec![],
            expected: None,
        },
        Row {
            case: "LoadBalancerType application",
            fields: vec![p("LoadBalancerType", "application")],
            expected: None,
        },
        Row {
            case: "LoadBalancerType network",
            fields: vec![p("LoadBalancerType", "network")],
            expected: None,
        },
        Row {
            // `LoadBalancerTypeEnum`'s third value: a Gateway Load Balancer has no TLS
            // listener, so no predefined policy supports it.
            case: "LoadBalancerType gateway",
            fields: vec![p("LoadBalancerType", "gateway")],
            expected: Some(Vec::new()),
        },
        Row {
            // The two filters compose: a named policy that does not support the stated
            // type is filtered out rather than reported.
            case: "a name filtered by an unsupported type",
            fields: vec![
                p("Names.member.1", "ELBSecurityPolicy-2016-08"),
                p("LoadBalancerType", "gateway"),
            ],
            expected: Some(Vec::new()),
        },
        Row {
            case: "a name kept by a supported type",
            fields: vec![
                p("Names.member.1", "ELBSecurityPolicy-2016-08"),
                p("LoadBalancerType", "network"),
            ],
            expected: Some(vec!["ELBSecurityPolicy-2016-08".to_string()]),
        },
    ];

    for row in &rows {
        let reply = describe(&router, "DescribeSSLPolicies", &row.fields).await;
        let case = row.case;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} body {}",
            reply.status, reply.body
        );
        let reported = reported_policy_names(&reply);
        let expected = row.expected.clone().unwrap_or_else(|| whole.clone());
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn an_unknown_security_policy_name_is_the_modelled_refusal() {
    let (router, _engine) = emulator();

    /// One refusal row: the case, the form fields, the expected status and awsQuery
    /// error code.
    type Refusal = (&'static str, Vec<(String, String)>, u16, &'static str);

    let refusals: Vec<Refusal> = vec![
        (
            "a name no predefined policy carries",
            vec![p("Names.member.1", "no-such-policy")],
            400,
            "SSLPolicyNotFound",
        ),
        (
            // A plausible-looking name in the service's own naming scheme is refused
            // the same way: the catalogue, not the spelling, decides.
            "a policy-shaped name the catalogue does not carry",
            vec![p("Names.member.1", "ELBSecurityPolicy-TLS-9-9-2099-01")],
            400,
            "SSLPolicyNotFound",
        ),
        (
            // "One or more": an unknown name beside a known one refuses the whole call
            // rather than reporting the known one.
            "an unknown name beside a known one",
            vec![
                p("Names.member.1", "ELBSecurityPolicy-2016-08"),
                p("Names.member.2", "no-such-policy"),
            ],
            400,
            "SSLPolicyNotFound",
        ),
        (
            "an empty name",
            vec![p("Names.member.1", "")],
            400,
            "SSLPolicyNotFound",
        ),
    ];

    for (case, fields, status, code) in &refusals {
        let reply = describe(&router, "DescribeSSLPolicies", fields).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (*status, *code),
            "case {case:?}: expected {status} {code}, got {} body {}",
            reply.status,
            reply.body
        );
        assert!(
            !reply.body.contains("<SslPolicies"),
            "case {case:?}: a refusal reports no policy, got {}",
            reply.body
        );
    }
}

#[tokio::test]
async fn both_catalogue_reads_honour_the_modelled_page_size_bounds() {
    let (router, _engine) = emulator();

    // Each published bound of `PageSize` is accepted, and one past each is refused.
    for action_name in ["DescribeAccountLimits", "DescribeSSLPolicies"] {
        for size in [PAGE_SIZE_MIN, PAGE_SIZE_MAX] {
            let case = format!("{action_name} at page size {size}");
            let reply =
                describe(&router, action_name, &[p("PageSize", &size.to_string())]).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: a page size inside the modelled range is served, got {} body {}",
                reply.status, reply.body
            );
        }
        for size in [PAGE_SIZE_MIN - 1, PAGE_SIZE_MAX + 1] {
            let case = format!("{action_name} at page size {size}");
            let reply =
                describe(&router, action_name, &[p("PageSize", &size.to_string())]).await;
            assert_eq!(
                reply.status, 400,
                "case {case:?}: a page size outside the modelled range is refused, got {} body {}",
                reply.status, reply.body
            );
        }
    }

    // The smallest page of the policy catalogue reports one policy and a marker that
    // resumes after it; walking the markers reports every policy exactly once.
    let whole = reported_policy_names(&describe(&router, "DescribeSSLPolicies", &[]).await);
    let mut walked: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut fields = vec![p("PageSize", "1")];
        if let Some(marker) = marker.as_deref() {
            fields.push(p("Marker", marker));
        }
        let page = describe(&router, "DescribeSSLPolicies", &fields).await;
        assert_eq!(
            page.status, 200,
            "case policy paging: expected 200, got {} body {}",
            page.status, page.body
        );
        let reported = reported_policy_names(&page);
        assert_eq!(
            reported.len(),
            1,
            "case policy paging: a page of size 1 reports one policy, got {reported:?}"
        );
        walked.extend(reported);
        pages += 1;
        assert!(
            pages <= whole.len() + 1,
            "case policy paging: paging did not terminate after {pages} pages"
        );
        match page.text("NextMarker") {
            Some(next) => marker = Some(next),
            None => break,
        }
    }
    assert_eq!(
        walked, whole,
        "case policy paging: expected the unpaged order, got {walked:?}"
    );
}
