// SPDX-License-Identifier: CC0-1.0

//! The Region names a landing zone manifest may govern, and the ones this service
//! reports, end to end over HTTP.
//!
//! Driven through the same [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter)
//! the production emulator dispatches with, so what these tests exercise is the wire
//! behaviour a client sees — status codes, error codes and JSON bodies.
//!
//! The rule under test, asserted over a TABLE of inputs rather than one case: a value
//! this service reports as a Region name is one — hyphen-separated lowercase groups
//! closed by the Region's number, within the length window the member that carries it
//! declares (`smithy_query query="shape" shape="Region,RegionName"`: `Region$name`
//! targets `RegionName`, length 1..50) — so a `CreateLandingZone`/`UpdateLandingZone`
//! whose manifest governs a value no Region name could be is refused with
//! `ValidationException` (400) and persists nothing, while every name it accepts is
//! reported back through `GetEnabledControl`'s `targetRegions` as it stands. The region
//! the service falls back to when a request names no governed region is held to the same
//! rule, because it is reported through the same member.
//!
//! The rows vary along what the rule generalises over: the length of the value (the
//! empty string, the published bound, one past it), its form (case, separators, the
//! closing number, non-ASCII), its position in the list, and the count of names
//! governed.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_controltower_awscontroltowerapis::state::{
    is_region_name, DEFAULT_REGION, REGION_NAME_LENGTH,
};

use crate::enabled_control_lifecycle::{control, details, enable, entries, ou};
use crate::landing_zone_lifecycle::{
    call, call_in_region, emulator, manifest_governing, past_the_settle_span,
    set_up_then_decommission, Answer,
    SIGNED_REGION,
};

/// A landing zone version the catalog offers, for the rows whose subject is the manifest
/// rather than the version.
const VERSION: &str = "3.3";

/// A Region name exactly as long as `RegionName`'s published maximum (50): the form a
/// Region name has, stretched to the bound.
fn longest_admissible_region_name() -> String {
    let position = "a".repeat(*REGION_NAME_LENGTH.end() - "us-".len() - "-1".len());
    format!("us-{position}-1")
}

/// That same name one character PAST the published maximum.
fn one_past_the_longest_region_name() -> String {
    let position = "a".repeat(*REGION_NAME_LENGTH.end() + 1 - "us-".len() - "-1".len());
    format!("us-{position}-1")
}

/// The values no Region name could be: `(case, the value)`.
fn values_that_are_no_region_name() -> Vec<(&'static str, String)> {
    let cases: Vec<(&'static str, String)> = vec![
        (
            "the empty string, below the published minimum",
            String::new(),
        ),
        (
            "one character past the published maximum, otherwise well formed",
            one_past_the_longest_region_name(),
        ),
        ("sixty characters of no Region form at all", "a".repeat(60)),
        ("upper case", "US-WEST-2".to_string()),
        ("underscores for hyphens", "us_west_2".to_string()),
        ("no closing number", "us-west".to_string()),
        ("the number spelled out", "us-west-two".to_string()),
        ("an empty group", "us--west-2".to_string()),
        ("a trailing hyphen", "us-west-2-".to_string()),
        ("a leading hyphen", "-us-west-2".to_string()),
        ("surrounding whitespace", " us-west-2 ".to_string()),
        ("only a geography and a number", "us-2".to_string()),
        ("a digit inside a word group", "us-we5t-2".to_string()),
        ("non-ASCII letters", "éu-west-1".to_string()),
    ];
    // Every row must really be outside the service's own predicate, so the table cannot
    // drift away from the rule it is meant to span.
    for (case, value) in &cases {
        assert!(
            !is_region_name(value),
            "case {case:?}: expected {value:?} to be no Region name"
        );
    }
    cases
}

/// The governed lists the service must accept: `(case, the names governed)`.
///
/// The names are published AWS Region names — the geographies and partitions a Region
/// name is drawn from (commercial, GovCloud, China, isolated) — plus the length bound
/// above, and the rows differ in how many Regions are governed and in which position the
/// interesting name sits.
fn governed_lists_that_are_accepted() -> Vec<(String, Vec<String>)> {
    let lists: Vec<(&str, Vec<String>)> = vec![
        ("one commercial Region", vec!["us-west-2".to_string()]),
        (
            "a two-letter geography with a compound position",
            vec!["ap-southeast-4".to_string()],
        ),
        (
            "a GovCloud Region, three word groups",
            vec!["us-gov-east-1".to_string()],
        ),
        (
            "an isolated Region, four word groups",
            vec!["eu-isoe-west-1".to_string()],
        ),
        ("a China Region", vec!["cn-north-1".to_string()]),
        (
            "several Regions across geographies",
            vec![
                "us-east-1".to_string(),
                "eu-central-1".to_string(),
                "ap-northeast-1".to_string(),
                "sa-east-1".to_string(),
            ],
        ),
        (
            "the longest name the published window admits, after a short one",
            vec!["us-west-2".to_string(), longest_admissible_region_name()],
        ),
        ("no Region governed at all", Vec::new()),
    ];
    for (case, names) in &lists {
        for name in names {
            assert!(
                is_region_name(name),
                "case {case:?}: expected {name:?} to be a Region name"
            );
        }
    }
    lists
        .into_iter()
        .map(|(case, names)| (case.to_string(), names))
        .collect()
}

/// Assert `answer` is the modelled 400 `ValidationException` whose message names
/// `offender`, and that `router` still reports `deployed` as the manifest of the landing
/// zone it holds (`None`: it holds none, so the refusal persisted nothing).
async fn refused(
    router: &MultiServiceRouter,
    case: &str,
    answer: &Answer,
    offender: &str,
    deployed: Option<&serde_json::Value>,
) {
    assert_eq!(
        (answer.status, answer.error_code.as_deref()),
        (400, Some("ValidationException")),
        "case {case:?}: expected 400 ValidationException, got {:?} body {:?}",
        (answer.status, answer.error_code.as_deref()),
        answer.body
    );
    let message = answer
        .get("message")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        message.contains("governedRegions") && message.contains(offender),
        "case {case:?}: expected the refusal to name governedRegions and {offender:?}, \
         got {message:?}"
    );

    let listed = call(router, "/list-landingzones", serde_json::json!({})).await;
    let arns = entries(listed.get("landingZones"));
    match deployed {
        None => assert!(
            arns.is_empty(),
            "case {case:?}: a refused create must persist nothing, got {arns:?}"
        ),
        Some(manifest) => {
            let arn = arns
                .first()
                .and_then(|entry| entry.get("arn"))
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string();
            let detail = call(
                router,
                "/get-landingzone",
                serde_json::json!({ "landingZoneIdentifier": arn }),
            )
            .await;
            assert_eq!(
                detail.get("landingZone").get("manifest"),
                Some(manifest),
                "case {case:?}: a refused update must leave the deployed manifest alone, \
                 got {:?}",
                detail.get("landingZone")
            );
        }
    }
}

/// The `targetRegions` names `GetEnabledControl` reports for the control at `arn`.
async fn reported_regions(router: &MultiServiceRouter, arn: &str) -> Vec<String> {
    let detail = details(router, arn).await;
    entries(
        detail
            .get("targetRegions")
            .unwrap_or(&serde_json::Value::Null),
    )
    .into_iter()
    .map(|region| {
        region
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string()
    })
    .collect()
}

/// Assert every name in `names` is one the modelled member can carry.
fn assert_within_the_published_window(case: &str, names: &[String]) {
    for name in names {
        assert!(
            REGION_NAME_LENGTH.contains(&name.len()),
            "case {case:?}: expected RegionName's length window \
             {REGION_NAME_LENGTH:?}, got {name:?} of length {}",
            name.len()
        );
        assert!(
            is_region_name(name),
            "case {case:?}: expected a Region name, got {name:?}"
        );
    }
}

/// RULE: a manifest whose `governedRegions` carries a value that is no Region name is
/// refused with `ValidationException` (400), naming the offending entry, and persists
/// nothing — on `CreateLandingZone` and on `UpdateLandingZone` alike, because the rule is
/// a property of a landing zone configuration rather than of one call.
#[tokio::test]
async fn a_governed_region_that_is_no_region_name_is_refused() {
    for (case, value) in values_that_are_no_region_name() {
        // The offending value alone, and the same value AFTER a well-formed one: the
        // position of the entry in the list does not change the answer.
        for (position, governed) in [
            ("alone", vec![value.clone()]),
            (
                "after a well-formed name",
                vec!["us-west-2".to_string(), value.clone()],
            ),
        ] {
            let case = format!("{case} ({position})");
            let names: Vec<&str> = governed.iter().map(String::as_str).collect();

            // CreateLandingZone: nothing is persisted at all.
            let router = emulator();
            let created = call(
                &router,
                "/create-landingzone",
                serde_json::json!({ "version": VERSION, "manifest": manifest_governing(&names) }),
            )
            .await;
            refused(&router, &case, &created, &value, None).await;
        }

        // UpdateLandingZone over a deployed landing zone: refused the same way, and the
        // deployed manifest is left exactly as it was.
        let router = emulator();
        let deployed = manifest_governing(&["us-west-2"]);
        let landing_zone = call(
            &router,
            "/create-landingzone",
            serde_json::json!({ "version": VERSION, "manifest": deployed }),
        )
        .await;
        assert_eq!(
            landing_zone.status, 200,
            "case {case:?}: the create the update row starts from expected 200, got {} \
             body {:?}",
            landing_zone.status, landing_zone.body
        );
        tokio::time::sleep(past_the_settle_span()).await;
        let updated = call(
            &router,
            "/update-landingzone",
            serde_json::json!({
                "landingZoneIdentifier": landing_zone.string("arn"),
                "version": VERSION,
                "manifest": manifest_governing(&[value.as_str()]),
            }),
        )
        .await;
        refused(&router, case, &updated, &value, Some(&deployed)).await;
    }
}

/// RULE: every Region name the service accepts into a manifest is reported back through
/// `GetEnabledControl`'s `targetRegions` as it stands, and within the window the modelled
/// member declares.
#[tokio::test]
async fn an_accepted_governed_region_is_reported_as_it_stands() {
    for (case, governed) in governed_lists_that_are_accepted() {
        let router = emulator();
        let names: Vec<&str> = governed.iter().map(String::as_str).collect();
        let created = call(
            &router,
            "/create-landingzone",
            serde_json::json!({ "version": VERSION, "manifest": manifest_governing(&names) }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected a manifest governing {governed:?} to be accepted, \
             got {} body {:?}",
            created.status, created.body
        );
        tokio::time::sleep(past_the_settle_span()).await;

        let (arn, _) = enable(
            &router,
            serde_json::json!({
                "controlIdentifier": control("AWS-GR_ENCRYPTED_VOLUMES"),
                "targetIdentifier": ou("3030"),
            }),
        )
        .await;

        let reported = reported_regions(&router, &arn).await;
        // With no Region governed, the only Region the enablement names is the one its
        // own request was signed for.
        let expected: Vec<String> = if governed.is_empty() {
            vec![SIGNED_REGION.to_string()]
        } else {
            governed.clone()
        };
        assert_eq!(
            reported, expected,
            "case {case:?}: expected target regions {expected:?}, got {reported:?}"
        );
        assert_within_the_published_window(&case, &reported);
    }
}

/// RULE: the Region an enablement falls back to is held to the same rule, because it is
/// reported through the same member — a credential scope whose region is no Region name
/// names no region, so the service reports the region it builds an unscoped request's
/// ARNs in.
#[tokio::test]
async fn the_fallback_region_is_a_region_name_too() {
    // (case, the region the enabling request is signed for, the name reported)
    let mut cases: Vec<(String, String, String)> = vec![
        (
            "a Region name: reported as signed for".to_string(),
            "eu-west-1".to_string(),
            "eu-west-1".to_string(),
        ),
        (
            "a GovCloud Region name".to_string(),
            "us-gov-west-1".to_string(),
            "us-gov-west-1".to_string(),
        ),
    ];
    for (case, value) in values_that_are_no_region_name() {
        // A credential scope is carried in an HTTP header, so the rows that can be signed
        // for are the values a header and a URI authority admit.
        if value.is_empty()
            || value
                .bytes()
                .any(|byte| !(byte.is_ascii_alphanumeric() || byte == b'-'))
        {
            continue;
        }
        cases.push((
            format!("signed for {case}"),
            value,
            DEFAULT_REGION.to_string(),
        ));
    }

    for (case, signed_for, expected) in cases {
        let router = emulator();
        // Control Tower enables a control only for an account it has been set up in, and
        // this case needs NOTHING governed to read a region from, so the landing zone the
        // setup deployed is decommissioned again.
        set_up_then_decommission(&router, &case).await;
        let enabled = call_in_region(
            &router,
            &signed_for,
            "/enable-control",
            serde_json::json!({
                "controlIdentifier": control("AWS-GR_ENCRYPTED_VOLUMES"),
                "targetIdentifier": ou("4040"),
            }),
        )
        .await;
        assert_eq!(
            enabled.status, 200,
            "case {case:?}: enable-control signed for {signed_for:?} expected 200, got {} \
             body {:?}",
            enabled.status, enabled.body
        );

        let arn = enabled.string("arn");
        let reported = reported_regions(&router, &arn).await;
        assert_eq!(
            reported,
            vec![expected.clone()],
            "case {case:?}: expected target regions [{expected:?}], got {reported:?}"
        );
        assert_within_the_published_window(&case, &reported);
    }
}
