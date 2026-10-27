// SPDX-License-Identifier: CC0-1.0

//! What `UpdateContributorInsights` WRITES, over and above the enable/disable lifecycle
//! its sibling suite pins (`crate::contributor_insights`, whose helpers this one drives
//! the operations through).
//!
//! The rules pinned here:
//!
//! 1. the call resolves `TableName` in either form that member accepts — a bare name or
//!    the table's ARN ("You can also provide the Amazon Resource Name (ARN) of the table
//!    in this parameter", `UpdateContributorInsights$TableName`, length 1..1024) — writes
//!    the setting of the one resource it resolves to, and answers naming the STORED table
//!    bare (`UpdateContributorInsightsOutput$TableName`, length 3..255, pattern
//!    `^[a-zA-Z0-9_.-]+$`), echoing `IndexName` exactly when the request named one;
//! 2. every accepted call REPLACES that resource's setting and starts the asynchronous
//!    change again, so the call answers the transient status with the mode it just wrote
//!    and a read one span later reports the settled status and that same mode — the last
//!    accepted call's, whatever the resource was set to before it; a `DISABLE` leaves the
//!    resource tracking no mode at all;
//! 3. a call reaches only the resource it names, in the region its endpoint names: a
//!    same-named table in another region, another table in the same region and the
//!    indexes the call did not name all keep the setting they had, and each region's
//!    listing reports only its own resources.
//!
//! Each rule is asserted over a SET of inputs — the form the table is addressed in, the
//! resource addressed, the mode asked for, the order the calls arrive in, and the region
//! they are signed for — rather than one example. Every call goes over the wire (`POST /`
//! with `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use.

use std::collections::HashMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::contributor_insights::{
    create_table, create_table_in, describe_insights, describe_insights_in, set_insights,
    set_insights_in, settle,
};
use crate::emulator::{emulator, ok, ok_in, REGION};

/// A second region to sign for, which is not [`REGION`]: the scope test needs two.
const OTHER_REGION: &str = "us-west-2";

/// The member a response omits reads as `Null`, whether the body left it out or carried
/// it null.
fn reported(body: &Value, member: &str) -> Value {
    body.get(member).cloned().unwrap_or(Value::Null)
}

/// A member a read is expected to report, or `Null` where the response omits it.
fn or_null(value: Option<&str>) -> Value {
    value.map_or(Value::Null, |value| json!(value))
}

/// The ARN of a stored table — the other form its name may be given in.
async fn table_arn(router: &MultiServiceRouter, table: &str) -> String {
    let described = ok(router, "DescribeTable", json!({"TableName": table})).await;
    described["Table"]["TableArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// How one resource of one region is named in the expectation map the scope test grows.
fn resource_key(region: &str, table: &str, index: Option<&str>) -> String {
    format!("{region}/{table}/{}", index.unwrap_or("<the table>"))
}

/// One row of the sequence in [`each_accepted_call_replaces_the_setting`]: the case, the
/// action asked for, the mode asked for, and the mode a read reports once that call has
/// settled (`None` where the setting then tracks none).
type Step = (
    &'static str,
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
);

/// One row of the table in [`the_call_writes_the_resource_it_names_whatever_form_it_is_addressed_in`]:
/// the case, the form the table is addressed in, the index named beside it, the mode
/// asked for and the mode the call and the reads then report.
type AddressedCase = (
    &'static str,
    String,
    Option<&'static str>,
    Option<&'static str>,
    &'static str,
);

/// One row of the table in [`a_call_reaches_only_the_resource_and_the_region_it_names`]:
/// the case, the region the call is signed for, the table and index it names, and the
/// mode it asks for.
type ScopedCall = (
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
    &'static str,
);

#[tokio::test]
async fn the_call_writes_the_resource_it_names_whatever_form_it_is_addressed_in() {
    let router = emulator();
    create_table(&router, "InsightsAddressed", &["Addressed.Index"]).await;
    settle().await;
    let arn = table_arn(&router, "InsightsAddressed").await;
    assert!(
        arn.ends_with("table/InsightsAddressed"),
        "case the stored table: it has an ARN to address it by, got {arn:?}"
    );

    // Rows differ in the form the table is addressed in, the resource addressed and the
    // mode asked for — the two values `ContributorInsightsMode` declares
    // (`smithy_query(query="shape", shape="ContributorInsightsMode")` gives
    // ACCESSED_AND_THROTTLED_KEYS | THROTTLED_KEYS) and naming none at all. The last
    // column is the mode the call and the reads then report.
    let bare = "InsightsAddressed".to_string();
    let cases: [AddressedCase; 5] = [
        (
            "the table by bare name, no mode named",
            bare.clone(),
            None,
            None,
            "ACCESSED_AND_THROTTLED_KEYS",
        ),
        (
            "the table by its ARN, throttled keys only",
            arn.clone(),
            None,
            Some("THROTTLED_KEYS"),
            "THROTTLED_KEYS",
        ),
        (
            "the table by its ARN, no mode named",
            arn.clone(),
            None,
            None,
            "ACCESSED_AND_THROTTLED_KEYS",
        ),
        (
            "the index by bare name, the wider mode",
            bare.clone(),
            Some("Addressed.Index"),
            Some("ACCESSED_AND_THROTTLED_KEYS"),
            "ACCESSED_AND_THROTTLED_KEYS",
        ),
        (
            "the index by its table's ARN, throttled keys only",
            arn.clone(),
            Some("Addressed.Index"),
            Some("THROTTLED_KEYS"),
            "THROTTLED_KEYS",
        ),
    ];

    for (case, addressed, index, asked, expected_mode) in cases {
        let answered = set_insights(&router, &addressed, index, "ENABLE", asked).await;
        assert_eq!(
            (
                reported(&answered, "TableName"),
                reported(&answered, "IndexName"),
                reported(&answered, "ContributorInsightsStatus"),
                reported(&answered, "ContributorInsightsMode"),
            ),
            (
                json!("InsightsAddressed"),
                or_null(index),
                json!("ENABLING"),
                json!(expected_mode),
            ),
            "case {case}: the call names the stored table bare, echoes only an index the \
             request named, and reports what it wrote, got {answered}"
        );

        // The write landed on the one resource BOTH forms name: a read addressed by the
        // other form reports it, still in flight, and reports the settled status a span
        // later.
        let other_form = if addressed == bare {
            arn.clone()
        } else {
            bare.clone()
        };
        let read_back = describe_insights(&router, &other_form, index).await;
        assert_eq!(
            (
                reported(&read_back, "ContributorInsightsStatus"),
                reported(&read_back, "ContributorInsightsMode"),
            ),
            (json!("ENABLING"), json!(expected_mode)),
            "case {case}: the setting the call wrote is read back through the other form of \
             the name, got {read_back}"
        );
        settle().await;
        let settled = describe_insights(&router, &other_form, index).await;
        assert_eq!(
            (
                reported(&settled, "ContributorInsightsStatus"),
                reported(&settled, "ContributorInsightsMode"),
            ),
            (json!("ENABLED"), json!(expected_mode)),
            "case {case}: the enable settles with the mode it was written with, got {settled}"
        );
    }
}

#[tokio::test]
async fn each_accepted_call_replaces_the_setting() {
    let router = emulator();
    create_table(&router, "InsightsReplaced", &["Replaced.Index"]).await;
    settle().await;

    // Rows are the calls in the order they arrive, which is what the rule generalises
    // over: a first enable naming no mode, a narrowing, a widening, a disable, the same
    // disable again, and an enable after it.
    let steps: [Step; 6] = [
        (
            "the first enable, no mode named",
            "ENABLE",
            None,
            Some("ACCESSED_AND_THROTTLED_KEYS"),
        ),
        (
            "narrowed to throttled keys",
            "ENABLE",
            Some("THROTTLED_KEYS"),
            Some("THROTTLED_KEYS"),
        ),
        (
            "widened to all access again",
            "ENABLE",
            Some("ACCESSED_AND_THROTTLED_KEYS"),
            Some("ACCESSED_AND_THROTTLED_KEYS"),
        ),
        ("disabled", "DISABLE", None, None),
        ("disabled a second time", "DISABLE", None, None),
        (
            "enabled again after the disable",
            "ENABLE",
            Some("THROTTLED_KEYS"),
            Some("THROTTLED_KEYS"),
        ),
    ];

    // Both kinds of resource one table carries, each carrying its own setting: the whole
    // sequence is walked over each.
    for (resource, index) in [("the table", None), ("its index", Some("Replaced.Index"))] {
        for (case, action, asked, settled_mode) in steps {
            let enabling = action == "ENABLE";
            let in_flight = if enabling { "ENABLING" } else { "DISABLING" };
            let settled_status = if enabling { "ENABLED" } else { "DISABLED" };

            let answered = set_insights(&router, "InsightsReplaced", index, action, asked).await;
            let read_in_flight = describe_insights(&router, "InsightsReplaced", index).await;
            assert_eq!(
                (
                    reported(&answered, "ContributorInsightsStatus"),
                    reported(&answered, "ContributorInsightsMode"),
                    reported(&read_in_flight, "ContributorInsightsStatus"),
                    reported(&read_in_flight, "ContributorInsightsMode"),
                ),
                (
                    json!(in_flight),
                    or_null(settled_mode),
                    json!(in_flight),
                    or_null(settled_mode),
                ),
                "case {resource}, {case}: the call answers the transient status with the mode \
                 it wrote and a read straight after agrees, got {answered} then \
                 {read_in_flight}"
            );

            settle().await;
            let settled = describe_insights(&router, "InsightsReplaced", index).await;
            assert_eq!(
                (
                    reported(&settled, "ContributorInsightsStatus"),
                    reported(&settled, "ContributorInsightsMode"),
                ),
                (json!(settled_status), or_null(settled_mode)),
                "case {resource}, {case}: the settled setting is the last accepted call's, got \
                 {settled}"
            );
        }
    }
}

#[tokio::test]
async fn a_call_reaches_only_the_resource_and_the_region_it_names() {
    let router = emulator();
    // A table name is unique within ONE region, so the same name is two tables across
    // two regions; a second table in the first region and two indexes each give the
    // calls below neighbours they must not reach.
    let indexes = ["Scoped.One", "Scoped.Two"];
    create_table_in(&router, REGION, "InsightsScoped", &indexes).await;
    create_table_in(&router, OTHER_REGION, "InsightsScoped", &indexes).await;
    create_table_in(&router, REGION, "InsightsSibling", &indexes).await;
    settle().await;

    // Every resource the two regions hold, which is what each accepted call is checked
    // against.
    let resources: [(&str, &str, Option<&str>); 9] = [
        (REGION, "InsightsScoped", None),
        (REGION, "InsightsScoped", Some("Scoped.One")),
        (REGION, "InsightsScoped", Some("Scoped.Two")),
        (REGION, "InsightsSibling", None),
        (REGION, "InsightsSibling", Some("Scoped.One")),
        (REGION, "InsightsSibling", Some("Scoped.Two")),
        (OTHER_REGION, "InsightsScoped", None),
        (OTHER_REGION, "InsightsScoped", Some("Scoped.One")),
        (OTHER_REGION, "InsightsScoped", Some("Scoped.Two")),
    ];

    // Rows are the resource one call names and the mode it asks for: the table in the
    // signed region, one of its indexes, the same-named table in the other region, and
    // that region's index of the same name as the one already enabled.
    let calls: [ScopedCall; 4] = [
        (
            "the table in the signed region",
            REGION,
            "InsightsScoped",
            None,
            "THROTTLED_KEYS",
        ),
        (
            "one of that table's indexes",
            REGION,
            "InsightsScoped",
            Some("Scoped.One"),
            "ACCESSED_AND_THROTTLED_KEYS",
        ),
        (
            "the same-named table in the other region",
            OTHER_REGION,
            "InsightsScoped",
            None,
            "ACCESSED_AND_THROTTLED_KEYS",
        ),
        (
            "that other region's index of the same name",
            OTHER_REGION,
            "InsightsScoped",
            Some("Scoped.One"),
            "THROTTLED_KEYS",
        ),
    ];

    // What every resource is expected to report, grown one accepted call at a time: a
    // resource is in here exactly when a call has named it.
    let mut expected: HashMap<String, &str> = HashMap::new();
    for (case, region, table, index, mode) in calls {
        set_insights_in(&router, region, table, index, "ENABLE", Some(mode)).await;
        settle().await;
        expected.insert(resource_key(region, table, index), mode);

        for (read_region, read_table, read_index) in resources {
            let described =
                describe_insights_in(&router, read_region, read_table, read_index).await;
            let mode_in_force = expected
                .get(&resource_key(read_region, read_table, read_index))
                .copied();
            let status = if mode_in_force.is_some() {
                "ENABLED"
            } else {
                "DISABLED"
            };
            assert_eq!(
                (
                    reported(&described, "ContributorInsightsStatus"),
                    reported(&described, "ContributorInsightsMode"),
                ),
                (json!(status), or_null(mode_in_force)),
                "case after {case}: {} reports only what a call named it, got {described}",
                resource_key(read_region, read_table, read_index)
            );
        }
    }

    // The listing of each region reports exactly the resources that region's calls
    // turned on, so the writes above never crossed into another region's tables.
    for region in [REGION, OTHER_REGION] {
        let listed = ok_in(&router, region, "ListContributorInsights", json!({})).await;
        let mut reported_resources: Vec<String> = listed["ContributorInsightsSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|summary| {
                resource_key(
                    region,
                    summary["TableName"].as_str().unwrap_or_default(),
                    summary["IndexName"].as_str(),
                )
            })
            .collect();
        reported_resources.sort();
        let mut enabled_here: Vec<String> = expected
            .keys()
            .filter(|key| key.starts_with(&format!("{region}/")))
            .cloned()
            .collect();
        enabled_here.sort();
        assert_eq!(
            reported_resources, enabled_here,
            "case the listing of {region}: it reports that region's enabled resources and no \
             other, got {listed}"
        );
    }
}
