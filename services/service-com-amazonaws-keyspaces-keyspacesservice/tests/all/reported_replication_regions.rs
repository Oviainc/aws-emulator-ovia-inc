// SPDX-License-Identifier: CC0-1.0

//! Every reader of a keyspace reports the SAME `replicationRegions`, driven end to end
//! over the service's own `awsJson1_0` wire protocol through the harness in
//! [`crate::keyspace_table_lifecycle`].
//!
//! The rule: a report of a keyspace carries a replication Region list exactly when the
//! keyspace's `replicationStrategy` is `MULTI_REGION`, and that list is the Regions the
//! stored keyspace is replicated in, unchanged ("if the `replicationStrategy` of the
//! keyspace is `MULTI_REGION`, a list of replication Regions is returned" —
//! smithy_query(query="doc",shape="KeyspaceSummary",member="replicationRegions")). It is
//! one rule over the stored keyspace, so `GetKeyspace` and the `ListKeyspaces` summary of
//! the same keyspace cannot answer it differently — for a keyspace created under any
//! strategy, for a keyspace the service manages itself, and after `UpdateKeyspace` has
//! widened the stored Region list.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use crate::keyspace_table_lifecycle::{assert_ok, settle_wait, Emulator, REGION};

/// Replication Regions of a `MULTI_REGION` keyspace other than the request's own, so a
/// case is neither only ever a pair nor only ever the Region the call is signed for.
const OTHER_REGION: &str = "eu-west-1";
const THIRD_REGION: &str = "ap-south-1";

/// What one reader reported about a keyspace: the two members the rule spans, plus the
/// identity members, so a disagreement cannot be a mix-up of two keyspaces.
#[derive(Debug, PartialEq, Eq)]
struct Report {
    name: Value,
    resource_arn: Value,
    replication_strategy: Value,
    replication_regions: Value,
}

impl Report {
    /// The report `GetKeyspace` or one `ListKeyspaces` summary carries.
    fn of(body: &Value) -> Self {
        Self {
            name: body["keyspaceName"].clone(),
            resource_arn: body["resourceArn"].clone(),
            replication_strategy: body["replicationStrategy"].clone(),
            replication_regions: body["replicationRegions"].clone(),
        }
    }
}

/// `GetKeyspace`'s report of `name`, requiring 200.
async fn get_report(emulator: &Emulator, case: &str, name: &str) -> Report {
    let body = assert_ok(
        case,
        emulator
            .call("GetKeyspace", json!({"keyspaceName": name}))
            .await,
    );
    Report::of(&body)
}

/// The `ListKeyspaces` summary of `name`, requiring 200 and that the listing holds it.
///
/// A listing that names no `maxResults` reports up to `MAX_PAGE_SIZE` keyspaces, which is
/// far more than this suite creates, so one call sees every keyspace.
async fn list_report(emulator: &Emulator, case: &str, name: &str) -> Report {
    let body = assert_ok(case, emulator.call("ListKeyspaces", json!({})).await);
    let summaries = body["keyspaces"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: ListKeyspaces reports a list, got {body}"));
    let summary = summaries
        .iter()
        .find(|summary| summary["keyspaceName"] == json!(name))
        .unwrap_or_else(|| {
            panic!("case {case:?}: the listing holds keyspace {name:?}, got {body}")
        });
    Report::of(summary)
}

/// Both readers of the keyspace, asserted to agree, with the agreed report handed back.
async fn agreeing_report(emulator: &Emulator, case: &str, name: &str) -> Report {
    let got = get_report(emulator, case, name).await;
    let listed = list_report(emulator, case, name).await;
    assert_eq!(
        got, listed,
        "case {case:?}: GetKeyspace and the ListKeyspaces summary report one keyspace, so \
         they report it identically; GetKeyspace said {got:?}, the listing said {listed:?}"
    );
    got
}

/// The rule over a set of keyspaces: every strategy, every Region count the store can
/// hold a list of, a list that repeats a Region, a list that does not begin with the
/// request's own Region, and a keyspace the service manages itself.
#[tokio::test]
async fn every_reader_reports_the_same_replication_regions() {
    struct Case {
        /// Also the keyspace name, except for the service-managed row.
        case: &'static str,
        /// The `replicationSpecification` to create the keyspace with, `Value::Null` for
        /// none at all, and `None` for a keyspace no create of this suite stores.
        specification: Option<Value>,
        strategy: &'static str,
        /// The Regions both readers must report, or `None` for none at all.
        regions: Option<Vec<&'static str>>,
    }

    let cases = vec![
        Case {
            // No replicationSpecification at all.
            case: "implicit_single_region",
            specification: Some(Value::Null),
            strategy: keyspaces::SINGLE_REGION_STRATEGY,
            regions: None,
        },
        Case {
            case: "explicit_single_region",
            specification: Some(json!({"replicationStrategy": "SINGLE_REGION"})),
            strategy: keyspaces::SINGLE_REGION_STRATEGY,
            regions: None,
        },
        Case {
            // A region list under a strategy that is not MULTI_REGION names no Region the
            // keyspace is replicated in, so no reader reports one.
            case: "single_region_with_a_region_list",
            specification: Some(json!({
                "replicationStrategy": "SINGLE_REGION",
                "regionList": [REGION, OTHER_REGION],
            })),
            strategy: keyspaces::SINGLE_REGION_STRATEGY,
            regions: None,
        },
        Case {
            case: "multi_region_pair",
            specification: Some(json!({
                "replicationStrategy": "MULTI_REGION",
                "regionList": [REGION, OTHER_REGION],
            })),
            strategy: keyspaces::MULTI_REGION_STRATEGY,
            regions: Some(vec![REGION, OTHER_REGION]),
        },
        Case {
            case: "multi_region_three",
            specification: Some(json!({
                "replicationStrategy": "MULTI_REGION",
                "regionList": [REGION, OTHER_REGION, THIRD_REGION],
            })),
            strategy: keyspaces::MULTI_REGION_STRATEGY,
            regions: Some(vec![REGION, OTHER_REGION, THIRD_REGION]),
        },
        Case {
            // The list denotes a SET of Regions, each held once, in the order the request
            // first named it.
            case: "multi_region_repeated_entry",
            specification: Some(json!({
                "replicationStrategy": "MULTI_REGION",
                "regionList": [REGION, OTHER_REGION, OTHER_REGION],
            })),
            strategy: keyspaces::MULTI_REGION_STRATEGY,
            regions: Some(vec![REGION, OTHER_REGION]),
        },
        Case {
            // The request's own Region is in the list but not at its head.
            case: "multi_region_own_region_last",
            specification: Some(json!({
                "replicationStrategy": "MULTI_REGION",
                "regionList": [THIRD_REGION, OTHER_REGION, REGION],
            })),
            strategy: keyspaces::MULTI_REGION_STRATEGY,
            regions: Some(vec![THIRD_REGION, OTHER_REGION, REGION]),
        },
        Case {
            // A keyspace Amazon Keyspaces manages itself, which the Region holds before
            // any create: no request chose its replication, and both readers still answer
            // the one rule about it. Not cited by the issue this suite accompanies.
            case: "system_schema_mcs",
            specification: None,
            strategy: keyspaces::SINGLE_REGION_STRATEGY,
            regions: None,
        },
    ];

    let emulator = Emulator::start();

    for case in &cases {
        let Some(specification) = &case.specification else {
            continue;
        };
        let mut request = json!({"keyspaceName": case.case});
        if !specification.is_null() {
            request["replicationSpecification"] = specification.clone();
        }
        assert_ok(case.case, emulator.call("CreateKeyspace", request).await);
    }

    for case in &cases {
        let report = agreeing_report(&emulator, case.case, case.case).await;
        assert_eq!(
            report.replication_strategy,
            json!(case.strategy),
            "case {:?}: the keyspace reads back with the strategy it holds, got {report:?}",
            case.case
        );
        let expected = match &case.regions {
            None => Value::Null,
            Some(regions) => json!(regions),
        };
        assert_eq!(
            report.replication_regions, expected,
            "case {:?}: a replication Region list is reported exactly for a MULTI_REGION \
             keyspace, and is the stored list unchanged; got {report:?}",
            case.case
        );
    }

    // The rule is about the STORED keyspace, so when a widening changes what is stored
    // both readers move together — and keep agreeing.
    let widened = "explicit_single_region";
    tokio::time::sleep(settle_wait()).await;
    assert_ok(
        "widened",
        emulator
            .call(
                "UpdateKeyspace",
                json!({
                    "keyspaceName": widened,
                    "replicationSpecification": {
                        "replicationStrategy": "MULTI_REGION",
                        "regionList": [REGION, OTHER_REGION, THIRD_REGION],
                    },
                }),
            )
            .await,
    );
    let report = agreeing_report(&emulator, "widened", widened).await;
    assert_eq!(
        report.replication_strategy,
        json!(keyspaces::MULTI_REGION_STRATEGY),
        "case \"widened\": the keyspace is MULTI_REGION once it has been widened, got {report:?}"
    );
    assert_eq!(
        report.replication_regions,
        json!([REGION, OTHER_REGION, THIRD_REGION]),
        "case \"widened\": every reader reports the widened Region list, got {report:?}"
    );

    // The keyspaces nothing widened are where they were: a read of one keyspace is not a
    // change to another.
    for case in &cases {
        if case.case == widened {
            continue;
        }
        let report = agreeing_report(&emulator, case.case, case.case).await;
        let expected = match &case.regions {
            None => Value::Null,
            Some(regions) => json!(regions),
        };
        assert_eq!(
            report.replication_regions, expected,
            "case {:?}: widening another keyspace left this one's Region list alone, got \
             {report:?}",
            case.case
        );
    }
}
