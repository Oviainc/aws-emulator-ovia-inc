// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests pinning what an `UpdateFilter` is allowed to change.
//!
//! `filter_lifecycle` asserts the merge itself (named members written, unnamed ones
//! left, `Version` incremented, `UpdatedAt` moved). This suite pins the other half of
//! the same rule — its SCOPE:
//!
//! 1. An update addresses exactly ONE saved filter, the one its Region, `DetectorId`
//!    and `FilterName` name: no other saved filter changes in any member, including a
//!    sibling under the same detector and a filter of the SAME name under another
//!    Region's detector, and the detector's filter list keeps the same names.
//! 2. An update the service refuses writes nothing: the filter it named still reports
//!    every value, timestamp and version it reported before the refused call, so a
//!    refusal neither applies a partial merge nor advances the version.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, the same
//! path the production router uses.
//!
//! The bounds the refusal rows sit outside of are the ones the model publishes for
//! this operation — `Rank` range 1..100, `Description` length 0..512, `Action` within
//! the `FilterAction` enum — from
//! `smithy_query(operation="UpdateFilter")` and `smithy_query(shape="FilterAction")`,
//! not from any recorded call.

use serde_json::{json, Value};

use crate::detector_lifecycle::{call_in, error_code, error_shape, new_router, Router};

/// The only client error GuardDuty's model binds to `UpdateFilter`
/// (`smithy_query(operation="UpdateFilter")`), and the shape restJson1 resolves an
/// error response to through `x-amzn-errortype`.
const MODELED_CLIENT_ERROR: &str = "BadRequestException";

/// The code the framework names a refusal by when a service states none of its own.
/// No GuardDuty operation declares a shape by this name, so it must never reach a
/// client of this service.
const UNMODELED_CODE: &str = "ValidationException";

/// The Region the fixture's primary detector lives in.
const HOME_REGION: &str = "us-east-1";

/// A second Region, so a detector and a filter of the same name exist in two Regions
/// at once.
const OTHER_REGION: &str = "eu-west-1";

/// The filter every update in this suite is aimed at.
const TARGET: &str = "target.filter_1";

/// Filters that must come out of every update untouched: two siblings under the same
/// detector, and the same name as [`TARGET`] under the other Region's detector.
const SIBLINGS: [&str; 2] = ["sibling.a_1", "sibling.b_2"];

/// A `CreateFilter` body: the required `name` and `findingCriteria`, plus the members
/// a row names.
fn create_body(name: &str, extra: &[(&str, Value)]) -> Value {
    let mut body = json!({
        "name": name,
        "findingCriteria": {"criterion": {"severity": {"greaterThanOrEqual": 4}}},
        "description": format!("{name} as created"),
        "action": "NOOP",
        "rank": 10,
        "tags": {"env": "prod"},
    });
    for (key, value) in extra {
        body[*key] = value.clone();
    }
    body
}

async fn create_detector_in(router: &Router, region: &str) -> String {
    let (status, body) = call_in(router, region, "POST", "/detector", Some(json!({"enable": true}))).await;
    assert_eq!(
        status, 200,
        "create-detector in {region}: expected HTTP 200, got {status} {body:?}"
    );
    body["detectorId"]
        .as_str()
        .unwrap_or_else(|| panic!("create-detector in {region}: expected a detectorId, got {body:?}"))
        .to_string()
}

async fn create_filter_in(router: &Router, region: &str, detector_id: &str, body: &Value) {
    let (status, created) = call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/filter"),
        Some(body.clone()),
    )
    .await;
    assert_eq!(
        status, 200,
        "create-filter {body} in {region}: expected HTTP 200, got {status} {created:?}"
    );
}

async fn update_filter_in(
    router: &Router,
    region: &str,
    detector_id: &str,
    name: &str,
    body: &Value,
) -> (u16, Value) {
    call_in(
        router,
        region,
        "POST",
        &format!("/detector/{detector_id}/filter/{name}"),
        Some(body.clone()),
    )
    .await
}

async fn get_filter_in(router: &Router, region: &str, detector_id: &str, name: &str) -> Value {
    let (status, read) = call_in(
        router,
        region,
        "GET",
        &format!("/detector/{detector_id}/filter/{name}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "get-filter {name} in {region}: expected HTTP 200, got {status} {read:?}"
    );
    read
}

async fn filter_names_in(router: &Router, region: &str, detector_id: &str) -> Vec<String> {
    let (status, body) = call_in(
        router,
        region,
        "GET",
        &format!("/detector/{detector_id}/filter"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "list-filters in {region}: expected HTTP 200, got {status} {body:?}"
    );
    let mut names: Vec<String> = body["filterNames"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|name| name.as_str().map(str::to_string))
        .collect();
    names.sort();
    names
}

/// The store every row starts from: one detector per Region, the target filter plus
/// two siblings in the home Region, and a filter of the target's name in the other.
struct Fixture {
    router: Router,
    home_detector: String,
    other_detector: String,
}

impl Fixture {
    async fn build() -> Self {
        let router = new_router();
        let home_detector = create_detector_in(&router, HOME_REGION).await;
        let other_detector = create_detector_in(&router, OTHER_REGION).await;

        create_filter_in(&router, HOME_REGION, &home_detector, &create_body(TARGET, &[])).await;
        for (index, sibling) in SIBLINGS.iter().enumerate() {
            let rank = i64::try_from(index).expect("small index") + 20;
            create_filter_in(
                &router,
                HOME_REGION,
                &home_detector,
                &create_body(sibling, &[("rank", json!(rank))]),
            )
            .await;
        }
        // Same NAME, other Region: the pair that proves an update is Region-scoped.
        create_filter_in(
            &router,
            OTHER_REGION,
            &other_detector,
            &create_body(TARGET, &[("rank", json!(55)), ("action", json!("ARCHIVE"))]),
        )
        .await;

        Self {
            router,
            home_detector,
            other_detector,
        }
    }

    /// Every filter that must be untouched, as `(case label, region, detector, name,
    /// body)`.
    async fn witnesses(&self) -> Vec<(String, &'static str, String, &'static str, Value)> {
        let mut snapshots = Vec::new();
        for sibling in SIBLINGS {
            let body =
                get_filter_in(&self.router, HOME_REGION, &self.home_detector, sibling).await;
            snapshots.push((
                format!("sibling {sibling} under the same detector"),
                HOME_REGION,
                self.home_detector.clone(),
                sibling,
                body,
            ));
        }
        let body = get_filter_in(&self.router, OTHER_REGION, &self.other_detector, TARGET).await;
        snapshots.push((
            format!("{TARGET} under the {OTHER_REGION} detector"),
            OTHER_REGION,
            self.other_detector.clone(),
            TARGET,
            body,
        ));
        snapshots
    }

    /// Re-read each witness and fail if any member of any of them moved.
    async fn assert_witnesses_unchanged(
        &self,
        case: &str,
        before: &[(String, &'static str, String, &'static str, Value)],
    ) {
        for (label, region, detector, name, snapshot) in before {
            let after = get_filter_in(&self.router, region, detector, name).await;
            assert_eq!(
                &after, snapshot,
                "case {case}: {label} must be unchanged, expected {snapshot:?}, got {after:?}"
            );
        }
    }
}

/// The epoch-second timestamp a read reports for `member`, as a number.
fn timestamp(case: &str, read: &Value, member: &str) -> f64 {
    read[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case}: expected a numeric {member}, got {read:?}"))
}

/// Accepted update rows, spanning the member subsets a request can name and both ends
/// of the published bounds. Each is `(why, body)`.
fn accepted_rows() -> Vec<(&'static str, Value)> {
    vec![
        ("only rank, at the published minimum of 1", json!({"rank": 1})),
        (
            "only rank, at the published maximum of 100",
            json!({"rank": 100}),
        ),
        ("only action", json!({"action": "ARCHIVE"})),
        ("only description", json!({"description": "changed"})),
        (
            "only description, at the published maximum length of 512",
            json!({"description": "d".repeat(512)}),
        ),
        (
            "only findingCriteria",
            json!({"findingCriteria": {"criterion": {"accountId": {"equals": ["111122223333"]}}}}),
        ),
        (
            "every updatable member at once",
            json!({
                "description": "all four",
                "action": "ARCHIVE",
                "rank": 99,
                "findingCriteria": {"criterion": {"severity": {"lessThan": 3}}},
            }),
        ),
        ("no member at all", json!({})),
    ]
}

#[tokio::test]
async fn an_update_changes_only_the_filter_it_names() {
    for (why, update) in accepted_rows() {
        let case = format!("UpdateFilter with {why}");
        let fixture = Fixture::build().await;
        let witnesses = fixture.witnesses().await;
        let names_before =
            filter_names_in(&fixture.router, HOME_REGION, &fixture.home_detector).await;
        let created = create_body(TARGET, &[]);
        let before = get_filter_in(
            &fixture.router,
            HOME_REGION,
            &fixture.home_detector,
            TARGET,
        )
        .await;

        let (status, body) = update_filter_in(
            &fixture.router,
            HOME_REGION,
            &fixture.home_detector,
            TARGET,
            &update,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} {body:?}"
        );
        assert_eq!(
            body["name"].as_str(),
            Some(TARGET),
            "case {case}: expected the updated filter's name back, got {body:?}"
        );

        // The named filter carries the merge: the members the request named, the
        // create's values for the ones it did not, one more version and a later
        // `updatedAt`.
        let after = get_filter_in(
            &fixture.router,
            HOME_REGION,
            &fixture.home_detector,
            TARGET,
        )
        .await;
        for member in ["description", "action", "rank", "findingCriteria"] {
            let mut expected = update
                .get(member)
                .or_else(|| created.get(member))
                .cloned()
                .unwrap_or(Value::Null);
            if member == "findingCriteria" {
                // A read reports every condition under both spellings of its pair
                // (`filter_condition_spellings`).
                expected = crate::filter_condition_spellings::reported_criteria(&expected);
            }
            assert_eq!(
                after[member], expected,
                "case {case}: expected {member} {expected}, got {after:?}"
            );
        }
        assert_eq!(
            after["version"],
            json!(2),
            "case {case}: one update must increment the version by 1, got {after:?}"
        );
        assert_eq!(
            timestamp(&case, &after, "createdAt"),
            timestamp(&case, &before, "createdAt"),
            "case {case}: an update must not move createdAt, got {after:?}"
        );
        assert!(
            timestamp(&case, &after, "updatedAt") > timestamp(&case, &before, "updatedAt"),
            "case {case}: an update must move updatedAt, got {after:?}"
        );

        // Nothing else moved.
        fixture.assert_witnesses_unchanged(&case, &witnesses).await;
        let names_after =
            filter_names_in(&fixture.router, HOME_REGION, &fixture.home_detector).await;
        assert_eq!(
            names_after, names_before,
            "case {case}: an update must not add or drop a filter, expected {names_before:?}, got {names_after:?}"
        );
    }
}

/// Update rows the service refuses, each outside one published bound of a member this
/// operation accepts, plus a criterion key that names no field of a finding. Each is
/// `(why, body)`.
fn refused_rows() -> Vec<(String, Value)> {
    let mut rows: Vec<(String, Value)> = vec![
        (
            "rank 0: one below the published minimum of 1".to_string(),
            json!({"rank": 0}),
        ),
        (
            "rank -4: below the published minimum and negative".to_string(),
            json!({"rank": -4}),
        ),
        (
            "rank 101: one above the published maximum of 100".to_string(),
            json!({"rank": 101}),
        ),
        (
            "rank i32::MAX: the top of the member's own type".to_string(),
            json!({"rank": i32::MAX}),
        ),
        (
            "a description of 513 characters: one above the published maximum of 512"
                .to_string(),
            json!({"description": "d".repeat(513)}),
        ),
        (
            "a criterion key that names no field of a finding".to_string(),
            json!({"findingCriteria": {"criterion": {"resource.notAField": {"equals": ["1"]}}}}),
        ),
    ];
    // `FilterAction` is the enum ARCHIVE | NOOP, so a member in the wrong case and a
    // value that is no member at all are both outside it.
    for action in ["Archive", "noop", "DELETE"] {
        rows.push((
            format!("action {action:?}, outside the FilterAction enum"),
            json!({"action": action}),
        ));
    }
    rows
}

#[tokio::test]
async fn a_refused_update_writes_nothing() {
    let fixture = Fixture::build().await;
    let witnesses = fixture.witnesses().await;
    let before = get_filter_in(
        &fixture.router,
        HOME_REGION,
        &fixture.home_detector,
        TARGET,
    )
    .await;

    for (why, update) in refused_rows() {
        let case = format!("UpdateFilter with {why}");
        let (status, refused) = update_filter_in(
            &fixture.router,
            HOME_REGION,
            &fixture.home_detector,
            TARGET,
            &update,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {refused:?}"
        );
        assert_eq!(
            error_shape(&refused),
            MODELED_CLIENT_ERROR,
            "case {case}: expected the modelled error shape in x-amzn-errortype, got {refused:?}"
        );
        assert_ne!(
            error_code(&refused),
            UNMODELED_CODE,
            "case {case}: refused under a code this service's model declares for nothing, got {refused:?}"
        );

        // Not a single member of the filter it named moved — no partial merge, no
        // version bump, no new `updatedAt`.
        let after = get_filter_in(
            &fixture.router,
            HOME_REGION,
            &fixture.home_detector,
            TARGET,
        )
        .await;
        assert_eq!(
            after, before,
            "case {case}: a refused update must leave the filter as it was, expected {before:?}, got {after:?}"
        );
        fixture.assert_witnesses_unchanged(&case, &witnesses).await;
    }

    // The filter is still updatable afterwards: the refusals left no latch behind.
    let (status, body) = update_filter_in(
        &fixture.router,
        HOME_REGION,
        &fixture.home_detector,
        TARGET,
        &json!({"rank": 7}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case UpdateFilter after the refusals: expected HTTP 200, got {status} {body:?}"
    );
    let after = get_filter_in(
        &fixture.router,
        HOME_REGION,
        &fixture.home_detector,
        TARGET,
    )
    .await;
    assert_eq!(
        after["rank"],
        json!(7),
        "case UpdateFilter after the refusals: expected the new rank, got {after:?}"
    );
    assert_eq!(
        after["version"],
        json!(2),
        "case UpdateFilter after the refusals: expected exactly one accepted update to count, got {after:?}"
    );
}
