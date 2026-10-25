// SPDX-License-Identifier: CC0-1.0

//! `ListRegions` end to end over the `awsJson1_1` wire.
//!
//! THE RULE: `ListRegions` reports every Region record the instance its `InstanceArn`
//! names currently holds — "all enabled Regions of an IAM Identity Center instance,
//! including those that are being added or removed … Regions with ACTIVE, ADDING, or
//! REMOVING status" — each as the `RegionMetadata` the model declares (`RegionName`,
//! `Status`, `AddedDate`, `IsPrimaryRegion`), and nothing else: the enumeration is a pure
//! read, so two calls at the same state answer the same thing, and exactly the one Region
//! the instance was enabled in carries `IsPrimaryRegion`. It is
//! `@paginated{inputToken: NextToken, outputToken: NextToken, pageSize: MaxResults,
//! items: Regions}` (`smithy_query(query="operation",operation="ListRegions")`), so the
//! records are walked in a stable order under `MaxResults` with a `NextToken` that resumes
//! where the previous page stopped and is absent once "there are no more entries"; a token
//! this service did not issue for this enumeration is refused `ValidationException` rather
//! than answered with an empty page. An `InstanceArn` this account holds no instance for is
//! refused through the only not-found channel this operation models,
//! `AccessDeniedException` — it declares no `ResourceNotFoundException`.
//!
//! The rows span what the rule generalises over: how many Regions the instance holds (the
//! primary one alone, one additional, several, and more than one page of them), the
//! `RegionName` spellings the model's pattern `^([a-z]+-){2,3}\d$` admits (two- and
//! three-prefix names, several geographies, the GovCloud and China partitions), the three
//! values of the model's `RegionStatus` enum, and the `MaxResults` values its
//! `range(min: 1, max: 100)` admits including both published bounds.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints and
//! the `X-Amz-Target` routing are exercised rather than bypassed; the harness is the one
//! `ssoadmin_permission_sets.rs` owns. Additional Regions are seeded through the service's
//! own `state::Instance::add_region`, because `AddRegion` — the operation that enables one
//! over the wire — is still a scaffold stub with an issue of its own; and where a test has
//! to be on the far side of a due instant it moves the STORED instant rather than sleeping,
//! leaving the transition itself to the service's own `materialize_due`.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    call_in_region, error_code, router_over, with_store, UNKNOWN_INSTANCE_ARN,
};

/// One Region record the rows seed: its name and the `RegionStatus` it is in.
type Seed = (&'static str, &'static str);

/// One row of the enumeration table.
struct ListCase {
    label: &'static str,
    /// The Region the account's instance is enabled in, so the primary Region of the row.
    primary: &'static str,
    /// The ADDITIONAL Regions the instance holds, with the status each is in. Deliberately
    /// NOT in name order, so a row can only pass if the handler orders them itself.
    additional: &'static [Seed],
}

/// A router over a fresh engine, and that engine.
fn router_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (router_over(Arc::clone(&engine)), engine)
}

/// The ARN of the account's instance, read the way a client reads it — from `region`, so
/// the instance this account holds is enabled in `region` as its primary one.
async fn instance_arn_in(router: &MultiServiceRouter, region: &str) -> String {
    let (status, body) = call_in_region(router, region, "ListInstances", &json!({})).await;
    assert_eq!(
        status, 200,
        "region {region}: ListInstances expected 200, got {status} {body}"
    );
    body["Instances"][0]["InstanceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("region {region}: no instance in {body}"))
        .to_string()
}

/// An account whose instance is enabled in `primary` and additionally in each seeded
/// Region, and that instance's ARN.
///
/// The additional Regions go in through `state::Instance::add_region` — the one place the
/// service's own records append a Region — because `AddRegion` is still a scaffold stub
/// tracked by its own issue. A seed whose status is not `ACTIVE` has that status written on
/// the record, which is exactly the state an add or remove workflow in progress leaves;
/// everything asserted afterwards goes over the wire.
async fn instance_holding(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    primary: &str,
    additional: &[Seed],
) -> String {
    let instance_arn = instance_arn_in(router, primary).await;
    with_store(engine, |store| {
        let instance = store
            .instance_mut(&instance_arn)
            .expect("the account's instance was just read over the wire");
        for (index, (name, status)) in additional.iter().enumerate() {
            // A distinct AddedDate per record, so a row would fail if the handler reported
            // one Region's timestamp for another.
            let added = SystemTime::now() - Duration::from_secs(60 * (index as u64 + 1));
            let mut region = state::Region::active(name, DateTime::from(added));
            match *status {
                "ADDING" => region.start_addition(),
                "REMOVING" => region.start_removal(SystemTime::now()),
                other => assert_eq!(
                    other, "ACTIVE",
                    "seed {name}: RegionStatus is ACTIVE | ADDING | REMOVING"
                ),
            }
            instance
                .add_region(region)
                .expect("the seeded Regions stay under the emulator's per-instance ceiling");
        }
    });
    instance_arn
}

/// One `ListRegions` call, as `(status, body)`.
async fn list_regions(
    router: &MultiServiceRouter,
    region: &str,
    request: &Value,
) -> (u16, Value) {
    call_in_region(router, region, "ListRegions", request).await
}

/// One `RegionMetadata` as the wire reports it: name, status, primary flag and `AddedDate`.
#[derive(Debug, Clone, PartialEq)]
struct Reported {
    name: String,
    status: String,
    is_primary: bool,
    added_date: Option<String>,
}

/// The `Regions` of one `ListRegions` body, in the order the page carries them.
fn reported(case: &str, body: &Value) -> Vec<Reported> {
    body["Regions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListRegions must answer a Regions list: {body}"))
        .iter()
        .map(|entry| Reported {
            name: entry["RegionName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a Region carries a name: {entry}"))
                .to_string(),
            status: entry["Status"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a Region carries a status: {entry}"))
                .to_string(),
            is_primary: entry["IsPrimaryRegion"].as_bool().unwrap_or_else(|| {
                panic!("case {case:?}: IsPrimaryRegion is a modelled boolean: {entry}")
            }),
            added_date: Some(entry["AddedDate"].to_string()),
        })
        .collect()
}

/// Every Region of the instance, walked through the enumeration's own pagination under
/// `max_results`, with the number of pages it took.
async fn walk(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    instance_arn: &str,
    max_results: Option<i64>,
) -> (Vec<Reported>, usize) {
    let mut collected: Vec<Reported> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut request = json!({"InstanceArn": instance_arn});
        if let Some(max_results) = max_results {
            request["MaxResults"] = json!(max_results);
        }
        if let Some(token) = &token {
            request["NextToken"] = json!(token);
        }
        let (status, body) = list_regions(router, region, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListRegions expected 200, got {status} {body}"
        );
        pages += 1;
        let page = reported(case, &body);
        if let Some(limit) = max_results {
            assert!(
                page.len() as i64 <= limit,
                "case {case:?}: a page must hold at most MaxResults={limit}, got {page:?}"
            );
        }
        collected.extend(page);
        match body["NextToken"].as_str() {
            Some(next) => {
                // The token stays inside `Token`'s `^[-a-zA-Z0-9+=/_]*$` pattern and its
                // 0..2048 length, so a client can send it back verbatim.
                assert!(
                    !next.is_empty()
                        && next.len() <= 2048
                        && next.chars().all(|c| {
                            c.is_ascii_alphanumeric() || matches!(c, '-' | '+' | '=' | '/' | '_')
                        }),
                    "case {case:?}: the token must satisfy Token's pattern and length, got \
                     {next:?}"
                );
                token = Some(next.to_string());
            }
            None => break,
        }
        assert!(
            pages <= state::MAX_REGIONS_PER_INSTANCE + 1,
            "case {case:?}: the enumeration must terminate, got {pages} pages"
        );
    }
    (collected, pages)
}

/// The `Regions` of the instance as `DescribeInstance` reports them, as the same rows.
async fn described(case: &str, router: &MultiServiceRouter, region: &str, arn: &str) -> Vec<Reported> {
    let (status, body) =
        call_in_region(router, region, "DescribeInstance", &json!({"InstanceArn": arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInstance expected 200, got {status} {body}"
    );
    reported(case, &json!({"Regions": body["Regions"].clone()}))
}

/// A `RegionName` the model's `RegionName` pattern `^([a-z]+-){2,3}\d$` admits, distinct
/// for each `index`: the middle segment spells the index in lowercase letters.
fn generated_region(index: usize) -> String {
    const SPELLED: [&str; 10] = [
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
    ];
    format!("ap-{}-{}", SPELLED[index % SPELLED.len()], index % 10)
}

/// The enumeration table. The rows differ in how many Regions the instance holds, in the
/// `RegionName` spellings they use (two- and three-prefix, four geographies, the GovCloud
/// and China partitions) and in which of the three `RegionStatus` values each record is in.
const CASES: &[ListCase] = &[
    ListCase {
        label: "the primary Region alone",
        primary: "us-east-1",
        additional: &[],
    },
    ListCase {
        label: "one additional ACTIVE Region",
        primary: "eu-central-1",
        additional: &[("us-west-2", "ACTIVE")],
    },
    ListCase {
        label: "an additional Region whose add workflow is in progress",
        primary: "us-west-2",
        additional: &[("eu-west-3", "ADDING")],
    },
    ListCase {
        label: "an additional Region whose remove workflow is in progress",
        primary: "ap-northeast-1",
        additional: &[("ca-central-1", "REMOVING")],
    },
    ListCase {
        label: "three additional Regions, one per RegionStatus value",
        primary: "us-east-2",
        additional: &[
            ("me-central-1", "ACTIVE"),
            ("ap-southeast-4", "ADDING"),
            ("eu-north-1", "REMOVING"),
        ],
    },
    ListCase {
        label: "a GovCloud primary Region and names from several partitions",
        primary: "us-gov-west-1",
        additional: &[
            ("cn-north-1", "ACTIVE"),
            ("af-south-1", "ACTIVE"),
            ("us-gov-east-1", "ADDING"),
        ],
    },
    ListCase {
        label: "a China primary Region, additional Regions out of name order",
        primary: "cn-northwest-1",
        additional: &[
            ("sa-east-1", "ACTIVE"),
            ("ap-south-2", "ACTIVE"),
            ("eu-south-1", "REMOVING"),
            ("af-south-1", "ACTIVE"),
        ],
    },
];

/// THE RULE: every Region record the instance holds is reported, with its own status,
/// `AddedDate` and primary flag, in a stable `RegionName` order — and nothing else.
#[tokio::test]
async fn listing_regions_reports_every_region_record_the_instance_holds() {
    for case in CASES {
        let ListCase {
            label,
            primary,
            additional,
        } = case;
        let (router, engine) = router_and_engine();
        let instance_arn = instance_holding(&router, &engine, primary, additional).await;

        let (listed, pages) = walk(label, &router, primary, &instance_arn, None).await;
        assert_eq!(
            pages, 1,
            "case {label:?}: {} Regions fit one default page, got {pages}",
            additional.len() + 1
        );

        // Exactly the records the instance holds: the Region it was enabled in, flagged
        // primary, plus every additional one with the status it is in.
        let mut expected: Vec<(&str, &str, bool)> = vec![(primary, "ACTIVE", true)];
        expected.extend(
            additional
                .iter()
                .map(|(name, status)| (*name, *status, false)),
        );
        expected.sort_by(|left, right| left.0.cmp(right.0));

        assert_eq!(
            listed.len(),
            expected.len(),
            "case {label:?}: expected {} Regions, got {listed:?}",
            expected.len()
        );
        for (position, (name, status, is_primary)) in expected.iter().enumerate() {
            let actual = &listed[position];
            assert_eq!(
                (actual.name.as_str(), actual.status.as_str(), actual.is_primary),
                (*name, *status, *is_primary),
                "case {label:?}: position {position} expected ({name}, {status}, \
                 primary={is_primary}), got {actual:?}"
            );
            assert!(
                actual
                    .added_date
                    .as_deref()
                    .is_some_and(|date| !date.is_empty() && date != "null"),
                "case {label:?}: Region {name} must carry its AddedDate, got {actual:?}"
            );
        }

        // One primary Region, and it is the Region the instance was enabled in.
        let primaries: Vec<&str> = listed
            .iter()
            .filter(|row| row.is_primary)
            .map(|row| row.name.as_str())
            .collect();
        assert_eq!(
            primaries,
            vec![*primary],
            "case {label:?}: exactly the enabled Region is primary, got {listed:?}"
        );

        // The same records `DescribeInstance` reports — one view of the instance's
        // Regions, not two.
        let mut from_describe = described(label, &router, primary, &instance_arn).await;
        from_describe.sort_by(|left, right| left.name.cmp(&right.name));
        assert_eq!(
            from_describe, listed,
            "case {label:?}: ListRegions and DescribeInstance must report the same Regions"
        );

        // A READ: it changes nothing a later caller sees.
        let (again, _) = walk(label, &router, primary, &instance_arn, None).await;
        assert_eq!(
            again, listed,
            "case {label:?}: a repeated ListRegions must answer the same Regions"
        );
    }
}

/// The `MaxResults` values the rows drive: absent, both published bounds of
/// `range(min: 1, max: 100)` and values inside them
/// (`smithy_query(query="operation",operation="ListRegions")`).
const PAGE_SIZES: &[Option<i64>] = &[None, Some(1), Some(2), Some(5), Some(99), Some(100)];

/// THE RULE under pagination: whatever `MaxResults` a caller sends, following `NextToken`
/// walks every Region record exactly once, in the same order, and the page that ends the
/// enumeration carries no token.
#[tokio::test]
async fn paging_walks_every_region_exactly_once() {
    // More Regions than the smallest pages hold, so every row really pages.
    let holdings: &[usize] = &[1, 3, 8];
    for holding in holdings {
        let additional: Vec<String> = (0..*holding).map(generated_region).collect();
        let (router, engine) = router_and_engine();
        let instance_arn = instance_arn_in(&router, "us-east-1").await;
        with_store(&engine, |store| {
            let instance = store.instance_mut(&instance_arn).expect("the instance");
            for name in &additional {
                instance
                    .add_region(state::Region::active(
                        name,
                        DateTime::from(SystemTime::now()),
                    ))
                    .expect("the seeded Regions stay under the per-instance ceiling");
            }
        });

        let mut expected: Vec<String> = additional.clone();
        expected.push("us-east-1".to_string());
        expected.sort();

        for max_results in PAGE_SIZES {
            let case = format!("{holding} additional Regions, MaxResults {max_results:?}");
            let (listed, pages) =
                walk(&case, &router, "us-east-1", &instance_arn, *max_results).await;
            let names: Vec<String> = listed.iter().map(|row| row.name.clone()).collect();
            assert_eq!(
                names, expected,
                "case {case:?}: paging must walk every Region once, in order, got {names:?}"
            );
            let limit = max_results.unwrap_or(state::MAX_PAGE_SIZE as i64);
            let least_pages = expected.len().div_ceil(limit as usize);
            assert!(
                pages >= least_pages && pages <= expected.len() + 1,
                "case {case:?}: {} Regions at {limit} per page take about {least_pages} \
                 pages, got {pages}",
                expected.len()
            );
        }
    }
}

/// A `NextToken` this service did not issue FOR THIS ENUMERATION is a bad request, not an
/// empty page: an empty page would end a caller's enumeration early.
#[tokio::test]
async fn a_token_this_enumeration_did_not_issue_is_refused() {
    let (router, _engine) = router_and_engine();
    let instance_arn = instance_arn_in(&router, "us-east-1").await;

    let rows = [
        (
            "a value this service never issued",
            "notatokenatall".to_string(),
        ),
        (
            "the empty token Token's 0..2048 length admits",
            String::new(),
        ),
        (
            "a token of another enumeration of this service",
            state::encode_page_token(
                &state::cursor_permission_sets(&instance_arn),
                &instance_arn,
            ),
        ),
        (
            "a token of THIS enumeration on another instance",
            state::encode_page_token(&state::cursor_regions(UNKNOWN_INSTANCE_ARN), "us-east-1"),
        ),
    ];
    for (case, token) in rows {
        let (status, body) = list_regions(
            &router,
            "us-east-1",
            &json!({"InstanceArn": instance_arn, "NextToken": token}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {token:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException for {token:?}, got {body}"
        );
    }
}

/// An `InstanceArn` this account holds no instance for is refused through the only
/// not-found channel this operation models — it declares no `ResourceNotFoundException`.
#[tokio::test]
async fn regions_of_an_instance_this_account_does_not_hold_are_refused() {
    let (router, _engine) = router_and_engine();
    let case = "a pattern-valid InstanceArn nothing created";

    let (status, body) = list_regions(
        &router,
        "us-east-1",
        &json!({"InstanceArn": UNKNOWN_INSTANCE_ARN}),
    )
    .await;
    assert_eq!(
        status, 403,
        "case {case:?}: expected HTTP 403, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "AccessDeniedException",
        "case {case:?}: expected AccessDeniedException, got {body}"
    );
    let message = body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_default();
    assert!(
        message.contains(UNKNOWN_INSTANCE_ARN),
        "case {case:?}: the refusal must name the ARN that resolved to nothing, got \
         {message:?}"
    );
}

/// THE CLOCK, not the read: a Region whose removal workflow has run out is no longer one of
/// the instance's Regions, and until it falls due the Region is still listed, as `REMOVING`.
#[tokio::test]
async fn a_region_is_listed_while_it_is_being_removed_and_gone_once_the_removal_falls_due() {
    let case = "an additional Region being removed";
    let (router, engine) = router_and_engine();
    let instance_arn = instance_holding(
        &router,
        &engine,
        "us-east-1",
        &[("eu-west-1", "ACTIVE"), ("ap-south-1", "ACTIVE")],
    )
    .await;

    // The removal is started over the WIRE; only the stored due INSTANT is moved below, so
    // both sides of it can be observed without the test sleeping.
    let (status, removed) = call_in_region(
        &router,
        "us-east-1",
        "RemoveRegion",
        &json!({"InstanceArn": instance_arn, "RegionName": "ap-south-1"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: RemoveRegion expected 200, got {status} {removed}"
    );

    let hold = |due_at: SystemTime| {
        with_store(&engine, |store| {
            let region = store
                .instance_mut(&instance_arn)
                .expect("the instance")
                .region_mut("ap-south-1")
                .expect("the Region is still held while it is being removed");
            region.removed_due = Some(due_at);
        });
    };

    hold(SystemTime::now() + Duration::from_secs(3_600));
    let (pending, _) = walk(case, &router, "us-east-1", &instance_arn, None).await;
    let statuses: Vec<(&str, &str)> = pending
        .iter()
        .map(|row| (row.name.as_str(), row.status.as_str()))
        .collect();
    assert_eq!(
        statuses,
        vec![
            ("ap-south-1", "REMOVING"),
            ("eu-west-1", "ACTIVE"),
            ("us-east-1", "ACTIVE")
        ],
        "case {case:?}: a Region being removed is listed as REMOVING, got {pending:?}"
    );

    hold(SystemTime::now() - Duration::from_secs(1));
    let (after, _) = walk(case, &router, "us-east-1", &instance_arn, None).await;
    let names: Vec<&str> = after.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["eu-west-1", "us-east-1"],
        "case {case:?}: a Region whose removal fell due is no longer listed, got {after:?}"
    );
}
