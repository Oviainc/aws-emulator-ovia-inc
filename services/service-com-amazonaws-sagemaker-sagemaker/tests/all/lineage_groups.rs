// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's lineage-group listing (`ListLineageGroups`).
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! SageMaker models no `CreateLineageGroup`: an account HOLDS one lineage group per
//! Region from the start — the default group, which this crate answers from
//! `ComAmazonawsSagemakerSagemakerState::default_lineage_group` — and any other group is
//! materialised when the account's lineage entities put something in it, which in this
//! crate is `ComAmazonawsSagemakerSagemakerState::ensure_lineage_group`, the hook every
//! lineage-entity create calls. So one suite below does the whole round trip over the wire
//! (`CreateContext` / `CreateArtifact` / `CreateAction` -> `ListLineageGroups`), and the
//! ordering, filtering and paging suites materialise groups through that same state API,
//! which is how they reach the names, Regions and stored instants no single create can
//! produce. `lineage_group_reporting.rs` does the same for `DescribeLineageGroup`.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: every
//! Region lists its default group, and a group a lineage entity materialised is listed in
//! its own Region only; a summary replays the
//! stored name, ARN, display name and instants; every `SortBy` x `SortOrder` pair orders
//! the result; the `CreatedAfter` / `CreatedBefore` bounds select inclusively; `MaxResults`
//! pages the ordered result exactly once, at its documented default of 10 when absent; and
//! a request outside the modelled constraints is refused.
//!
//! The wire harness is the crate's shared one, from `inference_deployment_lifecycle`.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use service_com_amazonaws_sagemaker_sagemaker::state::{self, default_lineage_group_name};

use super::inference_deployment_lifecycle::{
    epoch_seconds, error_code, materialise_lineage_group, Emulator, ACCOUNT,
};

/// The instant the seeded fixtures are anchored to. Fixed, so every `CreationTime` in
/// this suite is exact and no test sleeps or races the clock.
const FIXTURE_EPOCH_SECS: u64 = 1_700_000_000;

/// The crate's shared `materialise_lineage_group` plus the assertion every row in this
/// suite wants: the store really did materialise that group, so a case that lists nothing
/// is about the listing rather than about an empty store.
fn materialise(emulator: &Emulator, region: &str, name: &str, at: SystemTime) {
    assert!(
        materialise_lineage_group(emulator, region, name, at).is_some(),
        "case {region}/{name}: the store must materialise this group"
    );
}

/// The instant `offset` seconds after [`FIXTURE_EPOCH_SECS`].
fn fixture_instant(offset: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(FIXTURE_EPOCH_SECS + offset)
}

fn summaries(body: &Value) -> Vec<Value> {
    body.get("LineageGroupSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn listed_names(body: &Value) -> Vec<String> {
    summaries(body)
        .iter()
        .map(|summary| {
            summary
                .get("LineageGroupName")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The fixture every ordering and filtering case shares: names whose alphabetical order is
/// the REVERSE of their creation order, plus two groups materialised in the same instant so
/// the tie-break is observable. Every row is anchored well before "now", so the Region's
/// own default group — which the account has held since its state began — is the LAST of
/// them in creation order. Returns each name with its `CreationTime` in epoch seconds.
fn seed_fixture(emulator: &Emulator) -> Vec<(&'static str, f64)> {
    let rows: Vec<(&'static str, u64)> = vec![
        ("alpha-group", 300),
        ("beta-group", 200),
        ("demo-group", 100),
        ("epsilon-group", 400),
        ("zeta-group", 400),
    ];
    rows.into_iter()
        .map(|(name, offset)| {
            materialise(emulator, "us-east-1", name, fixture_instant(offset));
            (name, (FIXTURE_EPOCH_SECS + offset) as f64)
        })
        .collect()
}

/// RULE: an account HOLDS one lineage group per Region from the start — SageMaker models
/// no `CreateLineageGroup` — so a Region nothing has put an entity in still lists its
/// default group, with that Region's ARN, and the listing is stable across reads.
#[tokio::test]
async fn every_region_holds_its_default_lineage_group_from_the_start() {
    // Rows differ along the Region the request signs for, including the emulator's
    // default one and an unsigned request (which falls back to it).
    let regions = [
        "us-east-1",
        "us-west-2",
        "eu-west-2",
        "ap-southeast-1",
        "ca-central-1",
    ];

    for region in regions {
        // A fresh emulator per row: nothing has created a lineage entity anywhere.
        let emulator = Emulator::new();
        let group_name = default_lineage_group_name(region);
        let listed = {
            let (status, parsed) = emulator.call_in(region, "ListLineageGroups", &json!({})).await;
            assert_eq!(
                status, 200,
                "case {region}: expected HTTP 200, got {status} with {parsed}"
            );
            parsed
        };
        assert_eq!(
            listed_names(&listed),
            vec![group_name.clone()],
            "case {region}: a fresh Region lists its default group, got {listed:?}"
        );
        let summary = summaries(&listed).first().cloned().unwrap_or(Value::Null);
        assert_eq!(
            summary.get("LineageGroupArn").and_then(Value::as_str),
            Some(
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:lineage-group/{group_name}").as_str()
            ),
            "case {region}: LineageGroupArn, got {summary:?}"
        );
        assert_eq!(
            summary.get("DisplayName").and_then(Value::as_str),
            Some(group_name.as_str()),
            "case {region}: nothing supplies a display name for the account's own group, \
             so its name is displayed, got {summary:?}"
        );

        // The group is addressable: the describe and the policy reader both answer it.
        let described = {
            let (status, parsed) = emulator
                .call_in(
                    region,
                    "DescribeLineageGroup",
                    &json!({ "LineageGroupName": &group_name }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {region}: DescribeLineageGroup, got {status} with {parsed}"
            );
            parsed
        };
        assert_eq!(
            described.get("LineageGroupArn").and_then(Value::as_str),
            summary.get("LineageGroupArn").and_then(Value::as_str),
            "case {region}: the describe and the listing must report one ARN, \
             got {described:?}"
        );
        let (policy_status, policy) = emulator
            .call_in(
                region,
                "GetLineageGroupPolicy",
                &json!({ "LineageGroupName": &group_name }),
            )
            .await;
        assert_eq!(
            policy_status, 200,
            "case {region}: GetLineageGroupPolicy, got {policy_status} with {policy}"
        );

        // A read changes nothing a later caller sees: the instants are the same ones.
        let again = {
            let (status, parsed) = emulator.call_in(region, "ListLineageGroups", &json!({})).await;
            assert_eq!(
                status, 200,
                "case {region}: expected HTTP 200, got {status} with {parsed}"
            );
            parsed
        };
        assert_eq!(
            again, listed,
            "case {region}: the listing must be stable across reads, got {again:?}"
        );
    }

    // An unsigned request names no Region, so it lists the default Region's group.
    let emulator = Emulator::new();
    let (status, parsed) = emulator.call_unsigned("ListLineageGroups", &json!({})).await;
    assert_eq!(
        status, 200,
        "case unsigned: expected HTTP 200, got {status} with {parsed}"
    );
    let summary = summaries(&parsed).first().cloned().unwrap_or(Value::Null);
    assert_eq!(
        summary.get("LineageGroupArn").and_then(Value::as_str),
        Some(
            format!(
                "arn:aws:sagemaker:{}:{ACCOUNT}:lineage-group/{}",
                state::DEFAULT_REGION,
                default_lineage_group_name(state::DEFAULT_REGION)
            )
            .as_str()
        ),
        "case unsigned: the default Region's group must be listed, got {parsed:?}"
    );
}

/// RULE: the listing reports the lineage groups the caller's Region holds — its default
/// group, which the account holds from the start, plus every group a lineage entity has
/// materialised there — and listing them changes nothing a later caller sees.
#[tokio::test]
async fn creating_a_lineage_entity_makes_its_region_default_group_listable() {
    // Rows differ along the lineage-entity create that materialises the group (every one
    // implemented today) and the Region the request signed for, because the group is
    // materialised by ANY lineage entity rather than by one particular request.
    let cases: Vec<(&str, &str, &str, Value)> = vec![
        (
            "context",
            "us-east-1",
            "CreateContext",
            json!({
                "ContextName": "context-one",
                "ContextType": "DemoType",
                "Source": { "SourceUri": "s3://bucket/context-one" },
            }),
        ),
        (
            "artifact",
            "eu-west-2",
            "CreateArtifact",
            json!({
                "ArtifactType": "DataSet",
                "Source": { "SourceUri": "s3://bucket/one" },
            }),
        ),
        (
            "action",
            "ap-southeast-1",
            "CreateAction",
            json!({
                "ActionName": "action-one",
                "ActionType": "ModelDeployment",
                "Source": { "SourceUri": "s3://bucket/action-one" },
            }),
        ),
    ];

    for (case, region, target, body) in cases {
        // A fresh emulator per row: the Region already holds its DEFAULT group — the
        // account has one per Region, with no call creating it — and the create below
        // leaves it the Region's only group.
        let emulator = Emulator::new();
        let group_name = default_lineage_group_name(region);
        let (empty_status, empty) = emulator.call_in(region, "ListLineageGroups", &json!({})).await;
        assert_eq!(
            empty_status, 200,
            "case {case}: expected HTTP 200, got {empty_status} with {empty}"
        );
        assert_eq!(
            listed_names(&empty),
            vec![group_name.clone()],
            "case {case}: a Region holds its default lineage group from the start, \
             got {empty:?}"
        );
        assert!(
            empty.get("NextToken").is_none(),
            "case {case}: a complete page carries no NextToken, got {empty:?}"
        );

        let (created_status, created) = emulator.call_in(region, target, &body).await;
        assert_eq!(
            created_status, 200,
            "case {case}: {target}, got {created_status} with {created}"
        );

        let listed = {
            let (status, parsed) = emulator.call_in(region, "ListLineageGroups", &json!({})).await;
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} with {parsed}"
            );
            parsed
        };
        assert_eq!(
            listed_names(&listed),
            vec![group_name.clone()],
            "case {case}: the entity's own group must be the whole listing, got {listed:?}"
        );
        let summary = summaries(&listed).first().cloned().unwrap_or(Value::Null);
        assert_eq!(
            summary.get("LineageGroupArn").and_then(Value::as_str),
            Some(
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:lineage-group/{group_name}").as_str()
            ),
            "case {case}: LineageGroupArn, got {summary:?}"
        );
        assert!(
            summary.get("CreationTime").is_some_and(Value::is_number),
            "case {case}: CreationTime must be the instant the group was materialised, \
             got {summary:?}"
        );

        // A read is a read: listing again answers the same record, and does not
        // materialise a group in any other Region either.
        let again = {
            let (status, parsed) = emulator.call_in(region, "ListLineageGroups", &json!({})).await;
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} with {parsed}"
            );
            parsed
        };
        assert_eq!(
            again, listed,
            "case {case}: the listing must be stable across reads, got {again:?}"
        );
        for elsewhere in ["us-west-2", "eu-central-1"] {
            let its_group = default_lineage_group_name(elsewhere);
            let (status, parsed) = emulator
                .call_in(elsewhere, "ListLineageGroups", &json!({}))
                .await;
            assert_eq!(
                status, 200,
                "case {case}/{elsewhere}: expected HTTP 200, got {status} with {parsed}"
            );
            assert_eq!(
                listed_names(&parsed),
                vec![its_group.clone()],
                "case {case}/{elsewhere}: another Region lists its OWN default group and \
                 nothing the create in {region} materialised, got {parsed:?}"
            );
            let summary = summaries(&parsed).first().cloned().unwrap_or(Value::Null);
            assert_eq!(
                summary.get("LineageGroupArn").and_then(Value::as_str),
                Some(
                    format!(
                        "arn:aws:sagemaker:{elsewhere}:{ACCOUNT}:lineage-group/{its_group}"
                    )
                    .as_str()
                ),
                "case {case}/{elsewhere}: the default group's ARN names that Region, \
                 got {summary:?}"
            );
        }
    }
}

/// RULE: a lineage group is a REGIONAL resource, so a listing reports the groups of the
/// Region the request signed for — the emulator's default Region when the request is
/// unsigned — each with the ARN it was materialised under.
#[tokio::test]
async fn a_listing_reports_the_groups_of_the_region_the_request_signed_for() {
    let emulator = Emulator::new();

    // Rows: the emulator's default Region, two other Regions a client may sign for, each
    // holding its OWN groups, plus one Region nothing has materialised a group in, which
    // still holds its default one.
    let regions: Vec<(&str, Vec<String>)> = vec![
        (
            "us-east-1",
            vec![
                default_lineage_group_name("us-east-1"),
                "east-only-group".to_string(),
            ],
        ),
        ("eu-west-2", vec!["eu-group".to_string()]),
        (
            "ap-southeast-1",
            vec![
                default_lineage_group_name("ap-southeast-1"),
                "ap-group-a".to_string(),
                "ap-group-b".to_string(),
            ],
        ),
        ("us-west-2", vec![]),
    ];
    for (region, names) in &regions {
        for (index, name) in names.iter().enumerate() {
            materialise(&emulator, region, name, fixture_instant(index as u64));
        }
    }

    for (region, names) in &regions {
        let (status, parsed) = emulator
            .call_in(region, "ListLineageGroups", &json!({ "SortBy": "Name" }))
            .await;
        assert_eq!(
            status, 200,
            "case {region}: expected HTTP 200, got {status} with {parsed}"
        );
        let mut expected: Vec<String> = names.clone();
        // Every Region holds its default group whether or not a row materialised one.
        let default_name = default_lineage_group_name(region);
        if !expected.contains(&default_name) {
            expected.push(default_name);
        }
        expected.sort();
        assert_eq!(
            listed_names(&parsed),
            expected,
            "case {region}: a Region lists exactly its own groups, got {parsed:?}"
        );
        for summary in summaries(&parsed) {
            let name = summary
                .get("LineageGroupName")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                summary.get("LineageGroupArn").and_then(Value::as_str),
                Some(
                    format!("arn:aws:sagemaker:{region}:{ACCOUNT}:lineage-group/{name}").as_str()
                ),
                "case {region}/{name}: LineageGroupArn, got {summary:?}"
            );
        }
    }

    // An unsigned request carries no credential scope, so it lists the default Region's
    // groups (`state::DEFAULT_REGION`).
    let (status, parsed) = emulator
        .call_unsigned("ListLineageGroups", &json!({ "SortBy": "Name" }))
        .await;
    assert_eq!(
        status, 200,
        "case unsigned: expected HTTP 200, got {status} with {parsed}"
    );
    let mut expected: Vec<String> = regions
        .iter()
        .find(|(region, _)| *region == state::DEFAULT_REGION)
        .map(|(_, names)| names.clone())
        .unwrap_or_default();
    let default_name = default_lineage_group_name(state::DEFAULT_REGION);
    if !expected.contains(&default_name) {
        expected.push(default_name);
    }
    expected.sort();
    assert_eq!(
        listed_names(&parsed),
        expected,
        "case unsigned: the default Region's groups must be listed, got {parsed:?}"
    );
}

/// RULE: a summary replays what state holds for that group — its name, its ARN, its
/// display name and its stored creation and last-modified instants — whatever the name's
/// shape.
#[tokio::test]
async fn a_summary_replays_the_stored_members() {
    let emulator = Emulator::new();

    // Rows span the name forms `ExperimentEntityName` admits (len 1..120, pattern
    // `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}$` — smithy_query shape=LineageGroupSummary): a
    // one-character name, a hyphenated and mixed-case one, a digit-led one, the name AWS
    // gives the group it materialises itself, and a name at the published 120-character
    // maximum.
    let max_length_name = format!("g{}", "a".repeat(119));
    let rows: Vec<(String, u64)> = vec![
        ("a".to_string(), 10),
        ("mixed-Case-group-7".to_string(), 20),
        ("9lives".to_string(), 30),
        (default_lineage_group_name("us-east-1"), 35),
        (max_length_name.clone(), 40),
    ];
    for (name, offset) in &rows {
        materialise(&emulator, "us-east-1", name, fixture_instant(*offset));
    }

    let listed = emulator
        .ok(
            "ListLineageGroups",
            &json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
        )
        .await;
    let found = summaries(&listed);

    for (name, offset) in &rows {
        let summary = found
            .iter()
            .find(|summary| summary.get("LineageGroupName").and_then(Value::as_str) == Some(name))
            .unwrap_or_else(|| panic!("case {name}: must be listed, got {found:?}"));
        assert_eq!(
            summary.get("LineageGroupArn").and_then(Value::as_str),
            Some(format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:lineage-group/{name}").as_str()),
            "case {name}: LineageGroupArn, got {summary:?}"
        );
        // Nothing in the API supplies a display name for a group the service materialises
        // itself, so the stored display name is the group's own name.
        assert_eq!(
            summary.get("DisplayName").and_then(Value::as_str),
            Some(name.as_str()),
            "case {name}: DisplayName, got {summary:?}"
        );
        let expected_time = epoch_seconds(fixture_instant(*offset));
        for member in ["CreationTime", "LastModifiedTime"] {
            let reported = summary.get(member).and_then(Value::as_f64).unwrap_or(f64::NAN);
            assert!(
                (reported - expected_time).abs() < 1e-6,
                "case {name}: {member} must be the stored instant {expected_time}, \
                 got {summary:?}"
            );
        }
    }
}

/// RULE: `SortBy` picks the sort key and `SortOrder` reverses it — defaulting to
/// `CreationTime` / `Ascending`, this operation's documented defaults — with the group name
/// as the tie-break in both directions.
#[tokio::test]
async fn every_sort_key_and_order_pair_orders_the_result() {
    let emulator = Emulator::new();
    seed_fixture(&emulator);

    // Rows: the full cross-product of the `SortLineageGroupsBy` and `SortOrder` members
    // (smithy_query shape=SortLineageGroupsBy / SortOrder), plus the requests that send
    // neither or only one, which must equal the documented defaults. `epsilon-group` and
    // `zeta-group` share a creation instant, so the name tie-break shows in both orders,
    // and the Region's default group is in every row: the account has held it since its
    // state began, which is AFTER every seeded fixture instant.
    let default_group = default_lineage_group_name("us-east-1");
    let by_name_ascending = vec![
        "alpha-group".to_string(),
        "beta-group".to_string(),
        "demo-group".to_string(),
        "epsilon-group".to_string(),
        default_group.clone(),
        "zeta-group".to_string(),
    ];
    let by_name_descending: Vec<String> = by_name_ascending.iter().rev().cloned().collect();
    let by_time_ascending = vec![
        "demo-group".to_string(),
        "beta-group".to_string(),
        "alpha-group".to_string(),
        "epsilon-group".to_string(),
        "zeta-group".to_string(),
        default_group.clone(),
    ];
    let by_time_descending = vec![
        default_group,
        "epsilon-group".to_string(),
        "zeta-group".to_string(),
        "alpha-group".to_string(),
        "beta-group".to_string(),
        "demo-group".to_string(),
    ];
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "name-ascending",
            json!({ "SortBy": "Name", "SortOrder": "Ascending" }),
            by_name_ascending.clone(),
        ),
        (
            "name-descending",
            json!({ "SortBy": "Name", "SortOrder": "Descending" }),
            by_name_descending,
        ),
        (
            "creation-time-ascending",
            json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
            by_time_ascending.clone(),
        ),
        (
            "creation-time-descending",
            json!({ "SortBy": "CreationTime", "SortOrder": "Descending" }),
            by_time_descending,
        ),
        ("both-defaulted", json!({}), by_time_ascending.clone()),
        (
            "sort-order-defaulted",
            json!({ "SortBy": "Name" }),
            by_name_ascending,
        ),
        (
            "sort-by-defaulted",
            json!({ "SortOrder": "Ascending" }),
            by_time_ascending,
        ),
    ];

    for (case, body, expected) in cases {
        let listed = emulator.ok("ListLineageGroups", &body).await;
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {case}: ListLineageGroups order, got {listed:?}"
        );
    }
}

/// RULE: `CreatedAfter` and `CreatedBefore` select the groups on their side of the bound,
/// inclusively — so a timestamp copied out of a previous response still selects the group
/// it came from.
#[tokio::test]
async fn the_created_after_and_created_before_bounds_select_inclusively() {
    let emulator = Emulator::new();
    let seeded = seed_fixture(&emulator);
    let created_at = |name: &str| -> f64 {
        seeded
            .iter()
            .find(|(seeded_name, _)| *seeded_name == name)
            .map(|(_, at)| *at)
            .expect("a seeded group")
    };
    let demo = created_at("demo-group");
    let alpha = created_at("alpha-group");
    let tied = created_at("zeta-group");

    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "after-is-inclusive-of-the-bound-itself",
            json!({ "CreatedAfter": alpha, "CreatedBefore": alpha }),
            vec!["alpha-group".to_string()],
        ),
        (
            "before-the-earliest",
            json!({ "CreatedBefore": demo }),
            vec!["demo-group".to_string()],
        ),
        (
            "between-two-bounds",
            json!({ "CreatedAfter": demo, "CreatedBefore": alpha }),
            vec![
                "demo-group".to_string(),
                "beta-group".to_string(),
                "alpha-group".to_string(),
            ],
        ),
        (
            "a-bound-that-admits-the-tied-pair",
            json!({ "CreatedAfter": tied, "CreatedBefore": tied }),
            vec!["epsilon-group".to_string(), "zeta-group".to_string()],
        ),
        (
            "bounds-that-exclude-everything-below",
            json!({ "CreatedBefore": demo - 1.0 }),
            vec![],
        ),
        (
            "bounds-that-exclude-everything-above",
            json!({ "CreatedAfter": tied + 1.0 }),
            // Every seeded fixture is below the bound; the Region's default group, held
            // since the account's state began, is above it.
            vec![default_lineage_group_name("us-east-1")],
        ),
    ];

    for (case, body, expected) in cases {
        let listed = emulator.ok("ListLineageGroups", &body).await;
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {case}: ListLineageGroups selection, got {listed:?}"
        );
    }
}

/// RULE: `MaxResults` caps the page and `NextToken` resumes immediately after it, so a
/// paginating client walks the whole ordered result exactly once — and with no `MaxResults`
/// at all the page holds the documented default of 10.
#[tokio::test]
async fn paging_walks_the_whole_result_exactly_once() {
    // Past the documented default page size, so a defaulted page and an uncapped one are
    // distinguishable.
    const SEEDED: u64 = 23;
    /// `MaxResults` defaults to 10 ("This value defaults to 10." — smithy_query
    /// shape=ListLineageGroupsRequest).
    const DEFAULT_PAGE: usize = 10;

    let emulator = Emulator::new();
    for index in 0..SEEDED {
        materialise(
            &emulator,
            "us-east-1",
            &format!("group-{index:03}"),
            fixture_instant(index),
        );
    }
    // In `Name` order the Region's own default group — held since the account's state
    // began — follows every `group-NNN` of the fixture.
    let mut ordered: Vec<String> = (0..SEEDED).map(|index| format!("group-{index:03}")).collect();
    ordered.push(default_lineage_group_name("us-east-1"));

    // Rows: the published minimum of `MaxResults` (range 1..100), sizes around the result
    // length, the published maximum, and the request that sends none.
    let cases: Vec<(&str, Option<i64>, usize)> = vec![
        ("max-results-at-the-minimum", Some(1), 1),
        ("max-results-three", Some(3), 3),
        (
            "max-results-one-short",
            Some(ordered.len() as i64 - 1),
            ordered.len() - 1,
        ),
        (
            "max-results-exactly-the-result",
            Some(ordered.len() as i64),
            ordered.len(),
        ),
        ("max-results-at-the-maximum", Some(100), ordered.len()),
        ("max-results-absent", None, DEFAULT_PAGE),
    ];

    for (case, max_results, expected_first_page) in cases {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut body = json!({ "SortBy": "Name", "SortOrder": "Ascending" });
            if let Some(max) = max_results {
                body["MaxResults"] = json!(max);
            }
            if let Some(ref value) = token {
                body["NextToken"] = Value::String(value.clone());
            }
            let listed = emulator.ok("ListLineageGroups", &body).await;
            let names = listed_names(&listed);
            if pages == 0 {
                assert_eq!(
                    names.len(),
                    expected_first_page,
                    "case {case}: first page size, got {listed:?}"
                );
            }
            assert!(
                names.len() <= expected_first_page,
                "case {case}: a page must never exceed the cap, got {names:?}"
            );
            seen.extend(names);
            pages += 1;
            assert!(
                pages <= ordered.len() + 1,
                "case {case}: paging did not terminate, saw {seen:?}"
            );
            match listed.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }

        assert_eq!(
            seen, ordered,
            "case {case}: paging must walk the whole result exactly once"
        );
    }
}

/// RULE: the page is capped server-side, so no single call serialises the whole store
/// however large `MaxResults` is or whether the caller sends it at all — and the resume
/// token still walks the full set.
#[tokio::test]
async fn the_page_is_capped_server_side_even_when_the_caller_asks_for_more() {
    // `state::MAX_PAGE_SIZE` is this service's OWN ceiling — the emulator's choice, below
    // the modelled maximum of 100, which SageMaker itself does not document; the store is
    // seeded past it so a capped page and an uncapped one are distinguishable. A caller
    // asking for 100 therefore gets this many records plus a `NextToken`, which is legal
    // pagination either way.
    let ceiling = usize::try_from(state::MAX_PAGE_SIZE).expect("a positive page ceiling");
    let seeded = ceiling + 5;
    let emulator = Emulator::new();
    for index in 0..seeded {
        materialise(
            &emulator,
            "us-east-1",
            &format!("group-{index:03}"),
            fixture_instant(index as u64),
        );
    }
    let mut ordered: Vec<String> = (0..seeded).map(|index| format!("group-{index:03}")).collect();
    ordered.push(default_lineage_group_name("us-east-1"));

    // Rows: no `MaxResults` at all (the documented default of 10), the modelled maximum
    // (above the ceiling), the ceiling itself, and a value below it, which still wins
    // because it is the smaller cap.
    let cases: Vec<(&str, Value, usize)> = vec![
        ("max-results-absent", json!({}), 10),
        (
            "max-results-at-the-modelled-maximum",
            json!({ "MaxResults": 100 }),
            ceiling,
        ),
        (
            "max-results-at-the-ceiling",
            json!({ "MaxResults": state::MAX_PAGE_SIZE }),
            ceiling,
        ),
        (
            "max-results-below-the-ceiling",
            json!({ "MaxResults": state::MAX_PAGE_SIZE / 2 }),
            usize::try_from(state::MAX_PAGE_SIZE / 2).expect("a positive page size"),
        ),
    ];

    for (case, extra, expected_page) in cases {
        let mut body = json!({ "SortBy": "Name", "SortOrder": "Ascending" });
        for (key, value) in extra.as_object().expect("a JSON object of extra members") {
            body[key] = value.clone();
        }
        let listed = emulator.ok("ListLineageGroups", &body).await;
        assert_eq!(
            listed_names(&listed).len(),
            expected_page,
            "case {case}: page size, got {listed:?}"
        );
        assert!(
            listed.get("NextToken").is_some(),
            "case {case}: more groups remain, so a NextToken is required, got {listed:?}"
        );
    }

    // And the capped pages still walk the whole store exactly once.
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({ "SortBy": "Name", "SortOrder": "Ascending", "MaxResults": 100 });
        if let Some(ref value) = token {
            body["NextToken"] = Value::String(value.clone());
        }
        let listed = emulator.ok("ListLineageGroups", &body).await;
        let names = listed_names(&listed);
        assert!(
            names.len() <= ceiling,
            "case capped-walk: a page must never exceed the ceiling, got {} entries",
            names.len()
        );
        seen.extend(names);
        assert!(
            seen.len() <= ordered.len(),
            "case capped-walk: paging did not terminate, saw {} entries",
            seen.len()
        );
        match listed.get("NextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    assert_eq!(
        seen, ordered,
        "case capped-walk: the capped pages must walk the whole store exactly once"
    );
}

/// RULE: a request the model or the enum members do not admit is refused with HTTP 400
/// `ValidationException` — never a 200 with a silently restarted or re-sorted listing, and
/// never the scaffold's 501.
#[tokio::test]
async fn requests_outside_the_modelled_constraints_are_refused() {
    let emulator = Emulator::new();
    seed_fixture(&emulator);

    // Rows: one past each published bound (`MaxResults` range 1..100 — smithy_query
    // operation=ListLineageGroups), a value outside each enum's members, and a resume
    // token that names no boundary of the result set.
    let cases: Vec<(&str, Value)> = vec![
        ("max-results-below-the-minimum", json!({ "MaxResults": 0 })),
        (
            "max-results-above-the-maximum",
            json!({ "MaxResults": 101 }),
        ),
        ("sort-by-outside-the-enum", json!({ "SortBy": "Nmae" })),
        (
            "sort-order-outside-the-enum",
            json!({ "SortOrder": "ascending" }),
        ),
        (
            "next-token-that-names-no-boundary",
            json!({ "NextToken": "not-a-token" }),
        ),
        (
            "next-token-naming-a-group-outside-the-filtered-result",
            json!({
                "NextToken": hex::encode("alpha-group"),
                "CreatedBefore": (FIXTURE_EPOCH_SECS + 100) as f64,
            }),
        ),
    ];

    for (case, body) in cases {
        let (status, parsed) = emulator.call("ListLineageGroups", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: error code, got {parsed:?}"
        );
    }

    // And the published bounds themselves are accepted.
    for max_results in [1, 100] {
        let (status, parsed) = emulator
            .call("ListLineageGroups", &json!({ "MaxResults": max_results }))
            .await;
        assert_eq!(
            status, 200,
            "case max-results-{max_results}: a published bound must be accepted, \
             got {status} with {parsed}"
        );
    }
}
