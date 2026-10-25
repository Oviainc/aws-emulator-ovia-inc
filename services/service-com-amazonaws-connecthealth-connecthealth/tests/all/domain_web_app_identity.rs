// SPDX-License-Identifier: CC0-1.0

//! Which Identity Center instance a Domain's web application lives in.
//!
//! The rule: an Identity Center application is identified WITHIN its instance, so the
//! `idcApplicationId` this service reports for a domain's web app is the application's
//! identifier in the instance the create named —
//! `arn:aws:sso::<account>:application/<instanceId>/apl-<suffix>`, with no region
//! segment because Identity Center is a global namespace. A domain created against one
//! instance is therefore distinguishable from a domain created against another, each
//! domain owns its OWN application in its instance, and every read reports the
//! persisted identifier rather than a freshly minted one.
//!
//! The table's rows differ along what the rule generalises over: the instance value, its
//! shape, its length at the modelled `idcInstanceId` bounds (1..256 —
//! `smithy_query(query="shape", shape="CreateWebAppConfiguration")`), the Identity
//! Center region, and the EHR role. The case the issue cited (a `ssoins-…` id) is one
//! row.
//!
//! The harness (`Emulator`, `string_at`) is the sibling suites': one way of driving this
//! service through signed restJson1 requests, not two.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use service_com_amazonaws_connecthealth_connecthealth::state::MAX_IDC_INSTANCE_SEGMENT;

use super::domain_subscription_lifecycle::{string_at, Emulator};

/// `WebAppConfiguration$idcApplicationId`: String, length 1..256
/// (`smithy_query(query="shape", shape="WebAppConfiguration")`). Every identifier the
/// service reports has to fit it, whatever instance id the create named.
const IDC_APPLICATION_ID_LENGTH: (usize, usize) = (1, 256);

/// The prefix of an Identity Center application ARN, up to its account segment:
/// Identity Center is global, so the region segment is EMPTY.
const IDC_APPLICATION_ARN_HEAD: &str = "arn:aws:sso::";

/// One row of the table: what the create names, and the instance id the reported
/// application identifier must then carry.
struct Case {
    case: &'static str,
    region: &'static str,
    ehr_role: String,
    idc_instance_id: String,
    idc_region: &'static str,
    /// The instance segment the reported identifier must carry: the value the create
    /// named, as far as `MAX_IDC_INSTANCE_SEGMENT` carries it (which is the whole value
    /// for every instance id Identity Center issues, and what keeps the identifier
    /// inside [`IDC_APPLICATION_ID_LENGTH`] for a longer one the model still accepts).
    expected_instance: String,
}

/// An Identity Center application identifier, taken apart: the account it belongs to,
/// the instance it lives in, and the application's own id in that instance.
struct ApplicationId {
    account_id: String,
    instance_id: String,
    application: String,
}

/// Parse `arn:aws:sso::<account>:application/<instanceId>/<application>`, or panic
/// naming the case and the value.
fn parse_application_id(case: &str, value: &str) -> ApplicationId {
    let rest = value.strip_prefix(IDC_APPLICATION_ARN_HEAD).unwrap_or_else(|| {
        panic!(
            "case {case:?}: idcApplicationId must be an Identity Center application ARN \
             starting {IDC_APPLICATION_ARN_HEAD:?}, got {value:?}"
        )
    });
    let (account_id, resource) = rest.split_once(':').unwrap_or_else(|| {
        panic!("case {case:?}: an ARN with an account segment, got {value:?}")
    });
    let path = resource.strip_prefix("application/").unwrap_or_else(|| {
        panic!("case {case:?}: an `application/…` resource, got {value:?}")
    });
    let (instance_id, application) = path.rsplit_once('/').unwrap_or_else(|| {
        panic!(
            "case {case:?}: an application identified within its instance \
             (`application/<instanceId>/<application>`), got {value:?}"
        )
    });
    ApplicationId {
        account_id: account_id.to_string(),
        instance_id: instance_id.to_string(),
        application: application.to_string(),
    }
}

/// Whether `value` is an Identity Center application id: `apl-` and 16 hex characters.
fn is_application_id(value: &str) -> bool {
    value.strip_prefix("apl-").is_some_and(|body| {
        body.len() == 16 && body.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}

fn web_app_setup(case: &Case) -> Value {
    json!({
        "ehrRole": case.ehr_role,
        "idcInstanceId": case.idc_instance_id,
        "idcRegion": case.idc_region,
    })
}

fn cases() -> Vec<Case> {
    let role = |suffix: &str| format!("arn:aws:iam::000000000000:role/{suffix}");
    // The modelled `idcInstanceId` maximum (256) — longer than an identifier bounded at
    // 256 can carry whole, so it is carried as far as `MAX_IDC_INSTANCE_SEGMENT` goes.
    let longest_instance = "z".repeat(256);
    let longest_expected: String = longest_instance
        .chars()
        .take(MAX_IDC_INSTANCE_SEGMENT)
        .collect();
    vec![
        Case {
            case: "the instance the issue cited",
            region: "us-east-1",
            ehr_role: role("ehr-operations"),
            idc_instance_id: "ssoins-72230ed02aa4b4ea".to_string(),
            idc_region: "us-east-1",
            expected_instance: "ssoins-72230ed02aa4b4ea".to_string(),
        },
        Case {
            case: "another instance entirely",
            region: "us-east-1",
            ehr_role: role("ehr-operations"),
            idc_instance_id: "ssoins-1111111111111111".to_string(),
            idc_region: "us-east-1",
            expected_instance: "ssoins-1111111111111111".to_string(),
        },
        Case {
            case: "an instance id of another shape entirely",
            region: "eu-west-2",
            ehr_role: role("ehr/nested/path"),
            idc_instance_id: "d-9067a8b4c2".to_string(),
            idc_region: "eu-west-1",
            expected_instance: "d-9067a8b4c2".to_string(),
        },
        Case {
            case: "an instance id at the modelled minimum length (1)",
            region: "ap-southeast-2",
            ehr_role: role("a"),
            idc_instance_id: "a".to_string(),
            idc_region: "ap-southeast-2",
            expected_instance: "a".to_string(),
        },
        Case {
            case: "an instance id at the modelled maximum length (256)",
            region: "us-west-2",
            ehr_role: role("ehr-max"),
            idc_instance_id: longest_instance,
            idc_region: "us-west-2",
            expected_instance: longest_expected,
        },
        Case {
            case: "an Identity Center region apart from the domain's",
            region: "us-west-2",
            ehr_role: role("ehr-cross-region"),
            idc_instance_id: "ssoins-abcdef0123456789".to_string(),
            idc_region: "eu-central-1",
            expected_instance: "ssoins-abcdef0123456789".to_string(),
        },
    ]
}

#[tokio::test]
async fn the_reported_application_identifier_names_the_identity_center_instance_supplied() {
    // Every application identifier this suite sees, so no two rows share one.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    // The instance each identifier named, by the value the create supplied: two
    // DIFFERENT instances must never produce the same instance segment.
    let mut by_supplied: BTreeMap<String, String> = BTreeMap::new();

    for case in cases() {
        let label = case.case;
        let emulator = Emulator::new(case.region);
        let setup = web_app_setup(&case);
        let created = emulator
            .create_domain(&json!({ "name": "clinic-web", "webAppSetupConfiguration": setup }))
            .await;
        let configured = created
            .get("webAppConfiguration")
            .cloned()
            .unwrap_or_else(|| panic!("case {label:?}: webAppConfiguration in {created:?}"));

        // What the caller supplied is persisted as supplied.
        assert_eq!(
            configured.get("ehrRole").and_then(Value::as_str),
            Some(case.ehr_role.as_str()),
            "case {label:?}: the reported ehrRole must be the one supplied, got {configured:?}"
        );
        assert_eq!(
            configured.get("idcRegion").and_then(Value::as_str),
            Some(case.idc_region),
            "case {label:?}: the reported idcRegion must be the one supplied, got {configured:?}"
        );

        let reported = string_at(&configured, "idcApplicationId");
        let (min, max) = IDC_APPLICATION_ID_LENGTH;
        assert!(
            (min..=max).contains(&reported.chars().count()),
            "case {label:?}: idcApplicationId must be {min}..{max} characters, got \
             {} in {reported:?}",
            reported.chars().count()
        );
        let parsed = parse_application_id(label, &reported);
        assert_eq!(
            parsed.account_id,
            string_at(&created, "accountId"),
            "case {label:?}: the application must belong to the account that owns the \
             domain, got {reported:?}"
        );
        assert!(
            is_application_id(&parsed.application),
            "case {label:?}: the application's own id must be `apl-` and 16 hex \
             characters, got {:?}",
            parsed.application
        );
        assert_eq!(
            parsed.instance_id, case.expected_instance,
            "case {label:?}: the identifier must name the Identity Center instance the \
             create supplied ({:?}), got {reported:?}",
            case.idc_instance_id
        );
        assert!(
            seen.insert(reported.clone()),
            "case {label:?}: the application identifier {reported:?} was reported for \
             two different domains"
        );
        let previous = by_supplied.insert(case.idc_instance_id.clone(), parsed.instance_id.clone());
        if let Some(other) = previous {
            assert_eq!(
                other, parsed.instance_id,
                "case {label:?}: one instance value must always resolve to one instance \
                 segment, got {reported:?}"
            );
        }

        // The round trip: the identifier is PERSISTED, so every read reports the one the
        // create recorded rather than minting another.
        let domain_id = string_at(&created, "domainId");
        let path = format!("/domain/{domain_id}");
        for read in 1..=2 {
            let (status, fetched) = emulator.send("GET", &path, None).await;
            assert_eq!(status, 200, "case {label:?}: GetDomain got {fetched:?}");
            assert_eq!(
                fetched.get("webAppConfiguration"),
                Some(&configured),
                "case {label:?}: read {read} must report the web-app configuration the \
                 create recorded, got {fetched:?}"
            );
        }
    }

    // Two instances that differ are reported as instances that differ: the segments
    // collected above are as distinct as the values they came from.
    let segments: BTreeSet<&String> = by_supplied.values().collect();
    assert_eq!(
        segments.len(),
        by_supplied.len(),
        "domains created against different Identity Center instances must be \
         distinguishable, got {by_supplied:?}"
    );
}

#[tokio::test]
async fn every_domain_owns_its_own_application_inside_the_instance_it_named() {
    let emulator = Emulator::new("us-east-1");
    let instance = "ssoins-72230ed02aa4b4ea";
    let setup = json!({
        "ehrRole": "arn:aws:iam::000000000000:role/ehr-operations",
        "idcInstanceId": instance,
        "idcRegion": "us-east-1",
    });

    let mut applications: BTreeSet<String> = BTreeSet::new();
    for index in 0..3 {
        let case = format!("domain {index} against {instance}");
        let created = emulator
            .create_domain(&json!({
                "name": format!("clinic-{index}"),
                "webAppSetupConfiguration": setup,
            }))
            .await;
        let configured = created
            .get("webAppConfiguration")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: webAppConfiguration in {created:?}"));
        let reported = string_at(&configured, "idcApplicationId");
        let parsed = parse_application_id(&case, &reported);
        assert_eq!(
            parsed.instance_id, instance,
            "case {case:?}: every one of these domains lives in the same instance, got \
             {reported:?}"
        );
        assert!(
            applications.insert(parsed.application.clone()),
            "case {case:?}: each domain owns its OWN application in the instance, but \
             {:?} was handed out twice",
            parsed.application
        );
    }
}

#[tokio::test]
async fn a_domain_with_no_web_app_setup_reports_no_web_app_at_all() {
    let emulator = Emulator::new("us-east-1");
    let created = emulator.create_domain(&json!({ "name": "clinic-plain" })).await;
    assert!(
        created.get("webAppConfiguration").is_none() && created.get("webAppUrl").is_none(),
        "no web app was asked for, so none is reported, got {created:?}"
    );
    let domain_id = string_at(&created, "domainId");
    let (status, fetched) = emulator
        .send("GET", &format!("/domain/{domain_id}"), None)
        .await;
    assert_eq!(status, 200, "GetDomain got {fetched:?}");
    assert!(
        fetched.get("webAppConfiguration").is_none() && fetched.get("webAppUrl").is_none(),
        "a read of a domain with no web app reports none, got {fetched:?}"
    );
}
