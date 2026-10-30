// SPDX-License-Identifier: CC0-1.0

//! `DescribeRegion` end to end over the `awsJson1_1` wire.
//!
//! THE RULE: `DescribeRegion` reports the ONE Region record the instance its `InstanceArn`
//! names holds for the `RegionName` asked about — "Retrieves details about a specific
//! Region enabled in an IAM Identity Center instance. Details include the Region name,
//! current status (ACTIVE, ADDING, or REMOVING), the date when the Region was added, and
//! whether it is the primary Region." — out of the same records `ListRegions` enumerates
//! and `DescribeInstance` projects, so the three cannot disagree about one Region. It is a
//! pure read: repeating it answers the same thing and changes nothing a later caller sees.
//! What it cannot report is refused rather than answered empty: an `InstanceArn` this
//! account holds no instance for and a `RegionName` the instance is not enabled in are the
//! modelled `ResourceNotFoundException`, and "The request must be made from one
//! of the enabled Regions of the IAM Identity Center instance" is the modelled
//! `ValidationException`
//! (`smithy_query(query="operation",operation="DescribeRegion")`) — both at the HTTP 400
//! this awsJson1_1 service answers every refusal of a request with
//! (`scaffolded::modeled_error_status`).
//!
//! The rows span what the rule generalises over: which Region of the instance is asked
//! about (the primary one and additional ones), how many Regions the instance holds, the
//! three values of the model's `RegionStatus` enum, the `RegionName` spellings the model's
//! pattern `^([a-z]+-){2,3}\d$` admits (two- and three-prefix names, several geographies,
//! the GovCloud and China partitions), and the Region the CALL is made in — any enabled one
//! of the instance, not just its primary.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints and
//! the `X-Amz-Target` routing are exercised rather than bypassed; the harness is the one
//! `ssoadmin_permission_sets.rs` owns. Additional Regions are seeded through the service's
//! own `state::Instance::add_region` where a row only needs a record in a given status, and
//! the round trip through `AddRegion`/`RemoveRegion` is driven over the wire; where a test
//! has to be on the far side of a due instant it moves the STORED instant rather than
//! sleeping, leaving the transition itself to the service's own `materialize_due`.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    as_organization_instance, call_in_region, error_code, router_over, with_store,
    UNKNOWN_INSTANCE_ARN,
};

/// One Region record a row seeds: its name and the `RegionStatus` it is in.
type Seed = (&'static str, &'static str);

/// One row of the describe table.
struct Case {
    label: &'static str,
    /// The Region the account's instance is enabled in, so its primary Region.
    primary: &'static str,
    /// The ADDITIONAL Regions the instance holds, with the status each is in.
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
/// The additional records go in through `state::Instance::add_region` — the one place this
/// service's own records append a Region — so a row can hold a Region in any of the three
/// `RegionStatus` values without waiting a workflow out; everything asserted afterwards
/// goes over the wire.
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
                "REMOVING" => {
                    region.start_removal(SystemTime::now());
                    // The removal's own span is short by design, so the row holds its due
                    // instant out of reach: what is under test is the record in
                    // `REMOVING`, not when the clock takes it away.
                    region.removed_due = Some(SystemTime::now() + Duration::from_secs(3_600));
                }
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

/// One `DescribeRegion` call, as `(status, body)`.
async fn describe_region(
    router: &MultiServiceRouter,
    called_from: &str,
    instance_arn: &str,
    region_name: &str,
) -> (u16, Value) {
    call_in_region(
        router,
        called_from,
        "DescribeRegion",
        &json!({"InstanceArn": instance_arn, "RegionName": region_name}),
    )
    .await
}

/// One Region as a reader projects it: name, status, primary flag and `AddedDate`.
#[derive(Debug, Clone, PartialEq)]
struct Reported {
    name: Option<String>,
    status: Option<String>,
    is_primary: Option<bool>,
    added_date: Option<f64>,
}

/// The members of one Region out of any body that carries them — a `DescribeRegion`
/// response, or one `RegionMetadata` entry of `ListRegions`/`DescribeInstance`.
fn reported(entry: &Value) -> Reported {
    Reported {
        name: entry["RegionName"].as_str().map(str::to_string),
        status: entry["Status"].as_str().map(str::to_string),
        is_primary: entry["IsPrimaryRegion"].as_bool(),
        added_date: entry["AddedDate"].as_f64(),
    }
}

/// The `Regions` the instance's own enumeration reports, as the same rows.
async fn listed(
    case: &str,
    router: &MultiServiceRouter,
    called_from: &str,
    instance_arn: &str,
) -> Vec<Reported> {
    let (status, body) = call_in_region(
        router,
        called_from,
        "ListRegions",
        &json!({"InstanceArn": instance_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListRegions expected 200, got {status} {body}"
    );
    body["Regions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListRegions must answer a list: {body}"))
        .iter()
        .map(reported)
        .collect()
}

/// A `RegionName` the model's `RegionName` pattern `^([a-z]+-){2,3}\d$` admits but which no
/// row enables, so it names a Region the instance is not enabled in.
const UNENABLED_REGION: &str = "ap-southeast-7";

/// Move the stored instant at which the clock owes `eu-west-2`'s ACTIVATION to `due_at`, so
/// both sides of it can be observed without the test sleeping. The transition itself is
/// still applied by the service's own `materialize_due`, exactly as in production.
fn hold_activation(
    engine: &Arc<engine::InMemoryEngine>,
    instance_arn: &str,
    due_at: SystemTime,
) {
    with_store(engine, |store| {
        store
            .instance_mut(instance_arn)
            .expect("the account's instance")
            .region_mut("eu-west-2")
            .expect("the Region the add appended")
            .active_due = Some(due_at);
    });
}

/// The same for the instant at which the clock owes `eu-west-2`'s REMOVAL.
fn hold_removal(engine: &Arc<engine::InMemoryEngine>, instance_arn: &str, due_at: SystemTime) {
    with_store(engine, |store| {
        store
            .instance_mut(instance_arn)
            .expect("the account's instance")
            .region_mut("eu-west-2")
            .expect("the Region is still held while it is being removed")
            .removed_due = Some(due_at);
    });
}

/// The describe table. The rows differ in how many Regions the instance holds, which
/// `RegionStatus` each record is in, and the `RegionName` spellings they use — two- and
/// three-prefix names, several geographies, and the GovCloud and China partitions.
const CASES: &[Case] = &[
    Case {
        label: "the primary Region alone",
        primary: "us-east-1",
        additional: &[],
    },
    Case {
        label: "one additional ACTIVE Region",
        primary: "eu-central-1",
        additional: &[("us-west-2", "ACTIVE")],
    },
    Case {
        label: "an additional Region whose add workflow is in progress",
        primary: "us-west-2",
        additional: &[("eu-west-3", "ADDING")],
    },
    Case {
        label: "an additional Region whose remove workflow is in progress",
        primary: "ap-northeast-1",
        additional: &[("ca-central-1", "REMOVING")],
    },
    Case {
        label: "three additional Regions, one per RegionStatus value",
        primary: "us-east-2",
        additional: &[
            ("me-central-1", "ACTIVE"),
            ("ap-southeast-4", "ADDING"),
            ("eu-north-1", "REMOVING"),
        ],
    },
    Case {
        label: "a GovCloud primary Region and names from several partitions",
        primary: "us-gov-west-1",
        additional: &[
            ("cn-north-1", "ACTIVE"),
            ("af-south-1", "ACTIVE"),
            ("us-gov-east-1", "ADDING"),
        ],
    },
    Case {
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

/// THE RULE: every Region the instance holds is describable, with its own status,
/// `AddedDate` and primary flag — the same record the instance's own enumeration reports —
/// and the read changes nothing.
#[tokio::test]
async fn describing_a_region_reports_the_record_the_instance_holds() {
    for case in CASES {
        let Case {
            label,
            primary,
            additional,
        } = case;
        let (router, engine) = router_and_engine();
        let instance_arn = instance_holding(&router, &engine, primary, additional).await;

        let before = listed(label, &router, primary, &instance_arn).await;

        // Exactly the records the instance holds: the Region it was enabled in, flagged
        // primary, plus every additional one with the status it is in.
        let mut expected: Vec<(&str, &str, bool)> = vec![(primary, "ACTIVE", true)];
        expected.extend(
            additional
                .iter()
                .map(|(name, status)| (*name, *status, false)),
        );

        for (name, status, is_primary) in &expected {
            let row = format!("{label}: Region {name}");
            let (http_status, body) = describe_region(&router, primary, &instance_arn, name).await;
            assert_eq!(
                http_status, 200,
                "case {row:?}: DescribeRegion expected 200, got {http_status} {body}"
            );
            let described = reported(&body);
            assert_eq!(
                (
                    described.name.as_deref(),
                    described.status.as_deref(),
                    described.is_primary
                ),
                (Some(*name), Some(*status), Some(*is_primary)),
                "case {row:?}: expected ({name}, {status}, primary={is_primary}), got \
                 {described:?}"
            );
            assert!(
                described.added_date.is_some_and(|date| date > 0.0),
                "case {row:?}: the Region must carry the instant it was added, got \
                 {described:?}"
            );

            // The SAME record the instance's own enumeration reports — one view of a
            // Region, not two.
            let from_list = before
                .iter()
                .find(|entry| entry.name.as_deref() == Some(*name))
                .unwrap_or_else(|| {
                    panic!("case {row:?}: ListRegions must report Region {name}: {before:?}")
                });
            assert_eq!(
                &described, from_list,
                "case {row:?}: DescribeRegion and ListRegions must report the same record"
            );

            // A READ: repeating it answers the same thing.
            let (again_status, again) = describe_region(&router, primary, &instance_arn, name).await;
            assert_eq!(
                again_status, 200,
                "case {row:?}: a repeated DescribeRegion expected 200, got {again_status} \
                 {again}"
            );
            assert_eq!(
                reported(&again),
                described,
                "case {row:?}: a repeated DescribeRegion must answer the same record"
            );
        }

        // Exactly one Region of the instance is the primary one, and it is the Region the
        // instance was enabled in.
        let mut primaries: Vec<&str> = Vec::new();
        for (name, _, _) in &expected {
            let (_, body) = describe_region(&router, primary, &instance_arn, name).await;
            if body["IsPrimaryRegion"] == Value::Bool(true) {
                primaries.push(name);
            }
        }
        assert_eq!(
            primaries,
            vec![*primary],
            "case {label:?}: exactly the Region the instance was enabled in is primary, got \
             {primaries:?}"
        );

        // And nothing the describes did is visible to a later caller.
        let after = listed(label, &router, primary, &instance_arn).await;
        assert_eq!(
            after, before,
            "case {label:?}: describing Regions must leave the instance's records alone"
        );
    }
}

/// A `RegionName` the instance is not enabled in names no resource: it is the modelled
/// `ResourceNotFoundException`, never an empty answer.
#[tokio::test]
async fn a_region_the_instance_is_not_enabled_in_is_not_found() {
    for case in CASES {
        let (router, engine) = router_and_engine();
        let instance_arn =
            instance_holding(&router, &engine, case.primary, case.additional).await;
        let row = format!("{}: an unenabled Region", case.label);

        let (status, body) =
            describe_region(&router, case.primary, &instance_arn, UNENABLED_REGION).await;
        assert_eq!(
            status, 400,
            "case {row:?}: expected HTTP 400 for {UNENABLED_REGION}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {row:?}: expected ResourceNotFoundException, got {body}"
        );
        let message = body["Message"].as_str().unwrap_or_default();
        assert!(
            message.contains(UNENABLED_REGION),
            "case {row:?}: the refusal must name the Region that resolved to nothing, got \
             {message:?}"
        );
    }
}

/// An `InstanceArn` this account holds no instance for is the modelled
/// `ResourceNotFoundException` this operation declares, naming the ARN that resolved to
/// nothing.
#[tokio::test]
async fn an_instance_this_account_does_not_hold_is_not_found() {
    let case = "a pattern-valid InstanceArn nothing created";
    let (router, _engine) = router_and_engine();
    // The account's own instance first, so the ambient one is enabled and the ARN below is
    // refused for not existing rather than for the account holding nothing at all.
    let _ = instance_arn_in(&router, "us-east-1").await;

    let (status, body) =
        describe_region(&router, "us-east-1", UNKNOWN_INSTANCE_ARN, "us-east-1").await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {body}"
    );
    assert!(
        body["Message"]
            .as_str()
            .is_some_and(|message| message.contains(UNKNOWN_INSTANCE_ARN)),
        "case {case:?}: the refusal must name the ARN that resolved to nothing, got {body}"
    );
}

/// "The request must be made from one of the enabled Regions of the IAM Identity Center
/// instance": a call from ANY Region the instance holds a record for is served, whatever
/// status that record is in, and a call from a Region it is not enabled in is refused.
#[tokio::test]
async fn the_request_must_be_made_from_an_enabled_region_of_the_instance() {
    let (router, engine) = router_and_engine();
    let additional: &[Seed] = &[
        ("eu-west-1", "ACTIVE"),
        ("ap-south-1", "ADDING"),
        ("sa-east-1", "REMOVING"),
    ];
    let instance_arn = instance_holding(&router, &engine, "us-east-1", additional).await;

    // From each enabled Region: the same record, however far from the primary one the call
    // was made.
    let from_primary = {
        let (status, body) = describe_region(&router, "us-east-1", &instance_arn, "eu-west-1").await;
        assert_eq!(
            status, 200,
            "case \"from the primary Region\": expected 200, got {status} {body}"
        );
        reported(&body)
    };
    for called_from in ["us-east-1", "eu-west-1", "ap-south-1", "sa-east-1"] {
        let case = format!("called from the enabled Region {called_from}");
        let (status, body) = describe_region(&router, called_from, &instance_arn, "eu-west-1").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body}"
        );
        assert_eq!(
            reported(&body),
            from_primary,
            "case {case:?}: the record must not depend on where the call was made"
        );
    }

    // From a Region the instance is not enabled in: refused, whichever Region of it the
    // call asks about.
    for region_name in ["us-east-1", "eu-west-1", UNENABLED_REGION] {
        let case = format!("called from an unenabled Region, asking about {region_name}");
        let (status, body) =
            describe_region(&router, UNENABLED_REGION, &instance_arn, region_name).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        assert!(
            body["Message"]
                .as_str()
                .is_some_and(|message| message.contains(UNENABLED_REGION)),
            "case {case:?}: the refusal must name the Region the call was made in, got {body}"
        );
    }
}

/// The ROUND TRIP: a Region enabled over the wire is describable straight away, as the
/// `ADDING` its add workflow put it in, and as `ACTIVE` once the clock makes that workflow
/// due; a Region removed over the wire describes as `REMOVING` and is not found once its
/// removal falls due.
#[tokio::test]
async fn a_region_added_over_the_wire_describes_through_its_whole_workflow() {
    let case = "an additional Region added over the wire";
    let (router, engine) = router_and_engine();
    let instance_arn = instance_arn_in(&router, "us-east-1").await;
    // The round trip drives `AddRegion` and `RemoveRegion`, which are served on the
    // ORGANIZATION instance (the account instance's refusal is
    // `account_instance_region_writes.rs`), so the account's instance is seeded into that
    // kind through the shared harness; the describes below still go over the wire.
    as_organization_instance(&engine, &instance_arn);

    let (status, added) = call_in_region(
        &router,
        "us-east-1",
        "AddRegion",
        &json!({"InstanceArn": instance_arn, "RegionName": "eu-west-2"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: AddRegion expected 200, got {status} {added}"
    );
    assert_eq!(
        added["Status"].as_str(),
        Some("ADDING"),
        "case {case:?}: AddRegion must answer ADDING, got {added}"
    );

    // The add's workflow is held in flight the way the clock holds one — only the stored
    // due INSTANT is moved, so both sides of it are observable without the test sleeping —
    // and the describe reads the record as the add left it.
    hold_activation(&engine, &instance_arn, SystemTime::now() + Duration::from_secs(3_600));
    let (status, body) = describe_region(&router, "us-east-1", &instance_arn, "eu-west-2").await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeRegion expected 200, got {status} {body}"
    );
    assert_eq!(
        (
            body["RegionName"].as_str(),
            body["Status"].as_str(),
            body["IsPrimaryRegion"].as_bool()
        ),
        (Some("eu-west-2"), Some("ADDING"), Some(false)),
        "case {case:?}: the added Region describes as a non-primary ADDING Region, got {body}"
    );
    let added_date = body["AddedDate"].as_f64();
    assert!(
        added_date.is_some_and(|date| date > 0.0),
        "case {case:?}: the added Region must carry the instant it was added, got {body}"
    );

    // Past the activation instant, the transition itself still being the service's own
    // `materialize_due`.
    hold_activation(&engine, &instance_arn, SystemTime::now() - Duration::from_secs(1));
    let (status, body) = describe_region(&router, "us-east-1", &instance_arn, "eu-west-2").await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeRegion expected 200, got {status} {body}"
    );
    assert_eq!(
        body["Status"].as_str(),
        Some("ACTIVE"),
        "case {case:?}: a Region whose add workflow fell due describes as ACTIVE, got {body}"
    );
    assert_eq!(
        body["AddedDate"].as_f64(),
        added_date,
        "case {case:?}: AddedDate is the instant the Region was ADDED, so it must not move \
         when the workflow completes, got {body}"
    );

    // Removed over the wire: REMOVING while the removal is in flight.
    let (status, removed) = call_in_region(
        &router,
        "us-east-1",
        "RemoveRegion",
        &json!({"InstanceArn": instance_arn, "RegionName": "eu-west-2"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: RemoveRegion expected 200, got {status} {removed}"
    );
    hold_removal(&engine, &instance_arn, SystemTime::now() + Duration::from_secs(3_600));
    let (status, body) = describe_region(&router, "us-east-1", &instance_arn, "eu-west-2").await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeRegion expected 200, got {status} {body}"
    );
    assert_eq!(
        body["Status"].as_str(),
        Some("REMOVING"),
        "case {case:?}: a Region being removed describes as REMOVING, got {body}"
    );

    // And once the removal falls due the record is gone, so it names nothing.
    hold_removal(&engine, &instance_arn, SystemTime::now() - Duration::from_secs(1));
    let (status, body) = describe_region(&router, "us-east-1", &instance_arn, "eu-west-2").await;
    assert_eq!(
        status, 400,
        "case {case:?}: a Region whose removal fell due is not found, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {body}"
    );
}
