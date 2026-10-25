// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the approval portal an identity source reports.
//!
//! The rule under test: the approval portal of an identity source belongs to the IAM
//! Identity Center instance the source is associated with, so once that association is
//! live the service reports a portal URL derived from the stored instance's own id and
//! region — the same URL on every read and on every page, one URL per instance — and
//! reports none while the association is still being established.
//!
//! The table spans the class the rule generalises over: the FORM of the instance ARN
//! (partition, the optional `sso` prefix of the id, the character classes the id allows,
//! letter case) and the instance's region (the caller's, another one, a Region in
//! another partition, none at all). Where it names a published bound a comment names
//! the modelled member it comes from.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{text, Harness};

/// Slack over [`state::IDENTITY_SOURCE_ACTIVATION_DELAY`] before the clock-owed
/// activation is expected to have been materialised.
const CLOCK_SLACK: Duration = Duration::from_millis(250);

/// `ApprovalPortalUrl` is modelled `String` with `length(max: 1000)`.
const MAX_APPROVAL_PORTAL_URL: usize = 1000;

/// One row: the IAM Identity Center instance a create names, and the hostname label its
/// id must appear as.
struct PortalCase {
    case: &'static str,
    /// An ARN satisfying `IdcInstanceArn`'s modelled pattern
    /// `^arn:.+:sso:::instance/(?:sso)?ins-[a-zA-Z0-9-.]{16}$`.
    instance_arn: &'static str,
    /// `IamIdentityCenter$Region`: the INSTANCE's region, which need not be the region
    /// the request is signed for.
    instance_region: &'static str,
    /// The DNS suffix of the partition the instance ARN names.
    dns_suffix: &'static str,
}

/// The table: rows differ along the partition, the `ins-`/`ssoins-` forms of the
/// instance id, the dots, dashes and letter case the id's `[a-zA-Z0-9-.]` class allows,
/// and the instance's region (including none).
fn portal_cases() -> Vec<PortalCase> {
    vec![
        PortalCase {
            case: "ins- form, instance in the caller's region",
            instance_arn: "arn:aws:sso:::instance/ins-1234567890abcdef",
            instance_region: "us-west-2",
            dns_suffix: "amazonaws.com",
        },
        PortalCase {
            case: "ssoins- form, instance in another region",
            instance_arn: "arn:aws:sso:::instance/ssoins-abcdef1234567890",
            instance_region: "us-east-1",
            dns_suffix: "amazonaws.com",
        },
        PortalCase {
            case: "dots and dashes in the id",
            instance_arn: "arn:aws:sso:::instance/ins-a.b-c.d-12345678",
            instance_region: "eu-west-1",
            dns_suffix: "amazonaws.com",
        },
        PortalCase {
            case: "mixed-case id",
            instance_arn: "arn:aws:sso:::instance/ssoins-AbCdEf0123456789",
            instance_region: "ap-southeast-2",
            dns_suffix: "amazonaws.com",
        },
        PortalCase {
            case: "another partition",
            instance_arn: "arn:aws-us-gov:sso:::instance/ins-fedcba0987654321",
            instance_region: "us-gov-west-1",
            dns_suffix: "amazonaws.com",
        },
        PortalCase {
            // Every hostname in the China partition ends in `amazonaws.com.cn`.
            case: "the China partition",
            instance_arn: "arn:aws-cn:sso:::instance/ssoins-0123456789abcdef",
            instance_region: "cn-north-1",
            dns_suffix: "amazonaws.com.cn",
        },
        PortalCase {
            // `IamIdentityCenter$Region` is `length(max: 1000)` with no pattern, so an
            // empty Region is a request the constraint boundary admits.
            case: "an instance that names no region",
            instance_arn: "arn:aws:sso:::instance/ins-99887766554433aa",
            instance_region: "",
            dns_suffix: "amazonaws.com",
        },
    ]
}

/// The `CreateIdentitySource` body naming one row's instance.
fn create_body(case: &PortalCase) -> Value {
    json!({
        "IdentitySourceParameters": {
            "IamIdentityCenter": {
                "InstanceArn": case.instance_arn,
                "Region": case.instance_region,
            }
        }
    })
}

/// `POST /identity-sources`, asserting it succeeded, and returning the minted ARN.
async fn create(harness: &Harness, case: &PortalCase) -> String {
    let (status, created) = harness
        .call("POST", "/identity-sources", Some(create_body(case)))
        .await;
    assert_eq!(
        status, 200,
        "case {:?}: CreateIdentitySource must answer 200, got {status} {created:?}",
        case.case
    );
    text(&created, "IdentitySourceArn")
}

/// `POST /identity-sources/?List`, with `query` appended to the `?List` marker.
async fn list(harness: &Harness, query: &str) -> Value {
    let uri = if query.is_empty() {
        "/identity-sources/?List".to_string()
    } else {
        format!("/identity-sources/?List&{query}")
    };
    let (status, body) = harness.call("POST", &uri, None).await;
    assert_eq!(
        status, 200,
        "ListIdentitySources must answer 200, got {status} {body:?}"
    );
    body
}

/// The entry one page carries for `arn`.
fn entry_for(page: &Value, arn: &str) -> Value {
    page["IdentitySources"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|entry| text(entry, "IdentitySourceArn") == arn)
        .unwrap_or_else(|| panic!("no listed entry for {arn:?}: {page:?}"))
}

/// The `IamIdentityCenter` parameters of a listed entry.
fn parameters(entry: &Value) -> &Value {
    &entry["IdentitySourceParameters"]["IamIdentityCenter"]
}

/// `ApprovalPortalUrl`, or `None` when the entry reports no portal.
fn reported_portal(entry: &Value) -> Option<String> {
    parameters(entry)
        .get("ApprovalPortalUrl")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The id of the instance an ARN names, as ONE hostname label: lowercased, with the dots
/// the id's character class allows turned into hyphens, since a hostname label carries
/// only letters, digits and hyphens.
fn expected_host_label(instance_arn: &str) -> String {
    instance_arn
        .rsplit_once('/')
        .expect("an instance ARN names its instance after `instance/`")
        .1
        .to_ascii_lowercase()
        .replace('.', "-")
}

/// Assert the portal URL a row's listed entry reports is the one the rule fixes for that
/// instance.
fn assert_portal(case: &PortalCase, entry: &Value) -> String {
    let reported = reported_portal(entry).unwrap_or_else(|| {
        panic!(
            "case {:?}: an active identity source must report an ApprovalPortalUrl, got {entry:?}",
            case.case
        )
    });

    let host = reported.strip_prefix("https://").unwrap_or_else(|| {
        panic!(
            "case {:?}: expected an https approval-portal URL, got {reported:?}",
            case.case
        )
    });
    let (label, domain) = host.split_once('.').unwrap_or_else(|| {
        panic!(
            "case {:?}: expected the instance's id as the leading hostname label, got \
             {reported:?}",
            case.case
        )
    });
    assert_eq!(
        label,
        expected_host_label(case.instance_arn),
        "case {:?}: expected the instance's own id as the leading label, got {reported:?}",
        case.case
    );
    let expected_domain = if case.instance_region.is_empty() {
        format!("mpa.{}", case.dns_suffix)
    } else {
        format!("mpa.{}.{}", case.instance_region, case.dns_suffix)
    };
    assert_eq!(
        domain, expected_domain,
        "case {:?}: expected the portal under {expected_domain:?}, got {reported:?}",
        case.case
    );
    assert!(
        reported.len() <= MAX_APPROVAL_PORTAL_URL,
        "case {:?}: ApprovalPortalUrl is length(max: {MAX_APPROVAL_PORTAL_URL}), got {} \
         characters: {reported:?}",
        case.case,
        reported.len()
    );
    assert_eq!(
        reported,
        state::approval_portal_url(case.instance_arn, case.instance_region),
        "case {:?}: the reported portal must be the one minted for the stored instance, \
         got {reported:?}",
        case.case
    );
    reported
}

/// Every identity source whose association with its instance is live reports the
/// approval portal of that instance, derived from the stored instance and the same on
/// every read and every page.
#[tokio::test]
async fn an_active_identity_source_reports_the_portal_of_its_instance() {
    let cases = portal_cases();
    let harness = Harness::new();
    let mut arns: Vec<String> = Vec::new();
    for case in &cases {
        arns.push(create(&harness, case).await);
    }

    tokio::time::sleep(state::IDENTITY_SOURCE_ACTIVATION_DELAY + CLOCK_SLACK).await;
    let page = list(&harness, "").await;

    let mut portals: Vec<String> = Vec::new();
    for (case, arn) in cases.iter().zip(arns.iter()) {
        let entry = entry_for(&page, arn);
        assert_eq!(
            text(&entry, "Status"),
            state::IdentitySourceStatus::Active.as_wire(),
            "case {:?}: the source must be ACTIVE once its activation is due: {entry:?}",
            case.case
        );
        let reported = assert_portal(case, &entry);

        // The portal is recorded on the stored source, so the reported URL is the
        // store's own rather than one computed per request.
        let store = state::service_state(&*harness.engine).expect("the service store");
        let guard = store.lock();
        let stored = guard
            .identity_source(arn)
            .unwrap_or_else(|| panic!("case {:?}: nothing is stored under {arn:?}", case.case));
        assert_eq!(
            stored.approval_portal_url.as_deref(),
            Some(reported.as_str()),
            "case {:?}: expected the stored portal {:?}, got {reported:?}",
            case.case,
            stored.approval_portal_url
        );
        portals.push(reported);
    }

    // A read changes nothing: the same URL comes back on a second read and on a paged
    // read. `MaxResults` is modelled `range(min: 1, max: 20)`, so 1 is the narrowest
    // page a client may ask for.
    let again = list(&harness, "").await;
    assert_eq!(
        again, page,
        "ListIdentitySources is a read: two calls must agree"
    );
    for (case, (arn, portal)) in cases.iter().zip(arns.iter().zip(portals.iter())) {
        let single = list(&harness, &format!("MaxResults=1&NextToken={}", page_token(arn))).await;
        let entry = entry_for(&single, arn);
        assert_eq!(
            reported_portal(&entry).as_deref(),
            Some(portal.as_str()),
            "case {:?}: a paged read must report the same portal, got {entry:?}",
            case.case
        );
    }

    // One portal per instance: rows naming different instances are told apart, and two
    // sources associated with the SAME instance in the same region share its portal.
    let distinct: std::collections::BTreeSet<&str> = portals.iter().map(String::as_str).collect();
    assert_eq!(
        distinct.len(),
        portals.len(),
        "every row names a different instance, so every portal must differ: {portals:?}"
    );
    let shared = create(&harness, &cases[0]).await;
    tokio::time::sleep(state::IDENTITY_SOURCE_ACTIVATION_DELAY + CLOCK_SLACK).await;
    let page = list(&harness, "").await;
    assert_eq!(
        reported_portal(&entry_for(&page, &shared)).as_deref(),
        Some(portals[0].as_str()),
        "case {:?}: a second source for one instance must report that instance's portal: \
         {page:?}",
        cases[0].case
    );
}

/// The `NextToken` that makes the next page start at `arn`: the creation-order sequence
/// number of the source BEFORE it, which this service's token is.
fn page_token(arn: &str) -> String {
    let seq: u64 = arn
        .rsplit_once("/ids-")
        .expect("an identity-source ARN carries its creation-order id")
        .1
        .parse()
        .expect("a numeric identity-source id");
    (seq - 1).to_string()
}

/// A source whose association with its instance is not established yet reports no
/// portal, so a reader can tell a portal-less source from one that is not ready.
#[tokio::test]
async fn a_creating_identity_source_reports_no_portal() {
    for case in portal_cases() {
        let harness = Harness::new();
        let arn = create(&harness, &case).await;

        let page = list(&harness, "").await;
        let entry = entry_for(&page, &arn);
        assert_eq!(
            text(&entry, "Status"),
            state::IdentitySourceStatus::Creating.as_wire(),
            "case {:?}: a caller listing straight back must still see CREATING: {page:?}",
            case.case
        );
        assert_eq!(
            reported_portal(&entry),
            None,
            "case {:?}: a source still being created has no portal yet, got {entry:?}",
            case.case
        );

        // The instance itself is still reported, so the absent portal is the only
        // difference between a source being created and an active one.
        assert_eq!(
            text(parameters(&entry), "InstanceArn"),
            case.instance_arn,
            "case {:?}: the instance must be reported from the store: {entry:?}",
            case.case
        );

        tokio::time::sleep(state::IDENTITY_SOURCE_ACTIVATION_DELAY + CLOCK_SLACK).await;
        let page = list(&harness, "").await;
        assert_portal(&case, &entry_for(&page, &arn));
    }
}
