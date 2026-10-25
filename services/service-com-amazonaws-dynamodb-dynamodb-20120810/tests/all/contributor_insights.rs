// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the contributor insights surface:
//! `UpdateContributorInsights` and the two readers it round-trips through,
//! `DescribeContributorInsights` and `ListContributorInsights`.
//!
//! The rules pinned here:
//!
//! 1. every one of the three operations resolves its `TableName` against the stored
//!    tables, and a name that resolves to none is refused with HTTP 400
//!    `ResourceNotFoundException` carrying DynamoDB's missing-table sentence over the
//!    name the request addressed — the same rule for an `IndexName` the stored table
//!    does not carry, which names the table and the index instead;
//! 2. a table or index no update has named reports `DISABLED`, with no mode and no
//!    timestamp: the feature is off until `UpdateContributorInsights` turns it on;
//! 3. turning it on or off is asynchronous, so the call and an immediate read report the
//!    transient status (`ENABLING`/`DISABLING`) and a read after the span reports the
//!    settled one (`ENABLED`/`DISABLED`), with the mode the enable asked for;
//! 4. each resource carries its OWN setting — a table and each of its global secondary
//!    indexes — and `ListContributorInsights` reports one summary per resource, paged by
//!    `MaxResults`/`NextToken`.
//!
//! Every test drives the operations over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use, and each rule is asserted over a
//! SET of inputs — the operation called, why the table is not there, the resource
//! addressed, the mode asked for, the page size — rather than one example.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, emulator, error_code, message, ok};

/// Wait out one transient span, so whatever the clock owes a resource is due.
async fn settle() {
    tokio::time::sleep(
        service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION
            + std::time::Duration::from_millis(150),
    )
    .await;
}

/// Create one on-demand table carrying `indexes` global secondary indexes.
async fn create_table(router: &MultiServiceRouter, name: &str, indexes: &[&str]) -> Value {
    let mut request = json!({
        "TableName": name,
        "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
        "BillingMode": "PAY_PER_REQUEST",
    });
    if !indexes.is_empty() {
        // An index key attribute has to be declared too: "AttributeDefinitions must
        // include the key element(s) of the new index".
        request["AttributeDefinitions"] = json!([
            {"AttributeName": "Part", "AttributeType": "S"},
            {"AttributeName": "Other", "AttributeType": "S"},
        ]);
        request["GlobalSecondaryIndexes"] = Value::Array(
            indexes
                .iter()
                .map(|index| {
                    json!({
                        "IndexName": index,
                        "KeySchema": [{"AttributeName": "Other", "KeyType": "HASH"}],
                        "Projection": {"ProjectionType": "ALL"},
                    })
                })
                .collect(),
        );
    }
    ok(router, "CreateTable", request).await
}

/// One request body for each operation, addressing `table` (and `index` where the
/// operation takes one).
fn request_for(operation: &str, table: &str, index: Option<&str>) -> Value {
    let mut request = json!({"TableName": table});
    if let Some(index) = index {
        if operation != "ListContributorInsights" {
            request["IndexName"] = json!(index);
        }
    }
    if operation == "UpdateContributorInsights" {
        request["ContributorInsightsAction"] = json!("ENABLE");
    }
    request
}

/// Send one `UpdateContributorInsights` and return the parsed response.
async fn set_insights(
    router: &MultiServiceRouter,
    table: &str,
    index: Option<&str>,
    action: &str,
    mode: Option<&str>,
) -> Value {
    let mut request = json!({"TableName": table, "ContributorInsightsAction": action});
    if let Some(index) = index {
        request["IndexName"] = json!(index);
    }
    if let Some(mode) = mode {
        request["ContributorInsightsMode"] = json!(mode);
    }
    ok(router, "UpdateContributorInsights", request).await
}

/// Read one resource's setting back.
async fn describe_insights(router: &MultiServiceRouter, table: &str, index: Option<&str>) -> Value {
    let mut request = json!({"TableName": table});
    if let Some(index) = index {
        request["IndexName"] = json!(index);
    }
    ok(router, "DescribeContributorInsights", request).await
}

/// The three operations of this surface, each of which resolves a `TableName`.
const OPERATIONS: [&str; 3] = [
    "DescribeContributorInsights",
    "ListContributorInsights",
    "UpdateContributorInsights",
];

#[tokio::test]
async fn a_table_that_is_not_there_is_refused_by_every_contributor_insights_operation() {
    let router = emulator();
    create_table(&router, "InsightsPresent", &[]).await;
    create_table(&router, "InsightsRemoved", &[]).await;
    settle().await;
    let described = ok(
        &router,
        "DescribeTable",
        json!({"TableName": "InsightsRemoved"}),
    )
    .await;
    let arn = described["Table"]["TableArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    ok(&router, "DeleteTable", json!({"TableName": "InsightsRemoved"})).await;
    settle().await;

    // Rows differ in why the table does not resolve and in the form it is addressed in:
    // a name never created, a name created then deleted, and the ARN of a table that was
    // never created ("You can also provide the Amazon Resource Name (ARN) of the table in
    // this parameter"). The second column is the table name the refusal names, which is
    // the one the request addressed, bare.
    let absent_arn = arn.replace("InsightsRemoved", "InsightsNeverMade");
    let cases: [(&str, String, &str); 3] = [
        ("never created", "InsightsAbsent".to_string(), "InsightsAbsent"),
        (
            "created then deleted",
            "InsightsRemoved".to_string(),
            "InsightsRemoved",
        ),
        ("addressed by ARN", absent_arn, "InsightsNeverMade"),
    ];

    for (case, addressed, named) in cases {
        for operation in OPERATIONS {
            let (status, body) = call(
                &router,
                operation,
                request_for(operation, &addressed, Some("AnyIndex")),
            )
            .await;
            assert_eq!(
                (status, error_code(&body), message(&body)),
                (
                    400,
                    "ResourceNotFoundException".to_string(),
                    format!("Requested resource not found: Table: {named} not found")
                ),
                "case {case} on {operation}: a table that is not there is refused by name, \
                 got {status} with {body}"
            );
        }
    }

    // The table that IS there is served by all three, so the refusals above are about the
    // table named and not about the operations.
    for operation in OPERATIONS {
        let (status, body) = call(
            &router,
            operation,
            request_for(operation, "InsightsPresent", None),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (200, "<no __type>"),
            "case the present table on {operation}: it is served, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn an_index_the_table_does_not_carry_is_refused_by_name() {
    let router = emulator();
    create_table(&router, "InsightsIndexed", &["Carried.Index"]).await;
    create_table(&router, "InsightsElsewhere", &["Foreign.Index"]).await;
    settle().await;

    // Rows differ in why the index does not resolve: a name no table defines, a name
    // defined on ANOTHER table, and a name that differs from the carried one only in
    // case (index names are matched exactly).
    for (case, index) in [
        ("never defined", "Missing.Index"),
        ("defined on another table", "Foreign.Index"),
        ("the carried name in another case", "carried.index"),
    ] {
        for operation in ["DescribeContributorInsights", "UpdateContributorInsights"] {
            let (status, body) = call(
                &router,
                operation,
                request_for(operation, "InsightsIndexed", Some(index)),
            )
            .await;
            assert_eq!(
                (status, error_code(&body), message(&body)),
                (
                    400,
                    "ResourceNotFoundException".to_string(),
                    format!(
                        "Requested resource not found: Index not found for table \
                         InsightsIndexed: {index}"
                    )
                ),
                "case {case} on {operation}: an index the table does not carry is refused, \
                 got {status} with {body}"
            );
        }
    }

    // The index the table DOES carry is served, so the refusals above are about the index
    // named. Nothing above was stored: the carried index is still off.
    let described = describe_insights(&router, "InsightsIndexed", Some("Carried.Index")).await;
    assert_eq!(
        (
            described["IndexName"].clone(),
            described["ContributorInsightsStatus"].clone(),
        ),
        (json!("Carried.Index"), json!("DISABLED")),
        "case the carried index: it is served and still off, got {described}"
    );
}

#[tokio::test]
async fn an_enable_is_reported_enabling_then_settles_enabled_with_the_mode_it_asked_for() {
    let router = emulator();

    // Rows differ in the resource addressed (a table and one of its indexes) and in the
    // mode the enable names: none at all, and each value the model declares for
    // `ContributorInsightsMode`
    // (`smithy_query(query="shape", shape="ContributorInsightsMode")` gives
    // ACCESSED_AND_THROTTLED_KEYS | THROTTLED_KEYS). The last column is the mode the
    // reads then report. Each row gets a table of its own, so what it reads before its
    // own update is a resource no update has named.
    let cases: [(&str, Option<&str>, Option<&str>, &str); 4] = [
        (
            "the table, no mode named",
            None,
            None,
            "ACCESSED_AND_THROTTLED_KEYS",
        ),
        (
            "the table, the wider mode",
            None,
            Some("ACCESSED_AND_THROTTLED_KEYS"),
            "ACCESSED_AND_THROTTLED_KEYS",
        ),
        (
            "the table, throttled keys only",
            None,
            Some("THROTTLED_KEYS"),
            "THROTTLED_KEYS",
        ),
        (
            "the index, throttled keys only",
            Some("Lifecycle.Index"),
            Some("THROTTLED_KEYS"),
            "THROTTLED_KEYS",
        ),
    ];

    for (position, (case, index, asked, expected_mode)) in cases.into_iter().enumerate() {
        let table = format!("InsightsLifecycle{position}");
        create_table(&router, &table, &["Lifecycle.Index"]).await;
        settle().await;

        // Before any update the resource is off, with no mode and no timestamp.
        let before = describe_insights(&router, &table, index).await;
        assert_eq!(
            (
                before["ContributorInsightsStatus"].clone(),
                before["ContributorInsightsMode"].clone(),
                before["LastUpdateDateTime"].clone(),
            ),
            (json!("DISABLED"), Value::Null, Value::Null),
            "case {case}: an untouched resource is off, got {before}"
        );

        // The enable is asynchronous: the call and an immediate read report ENABLING.
        let updated = set_insights(&router, &table, index, "ENABLE", asked).await;
        let read_back = describe_insights(&router, &table, index).await;
        assert_eq!(
            (
                updated["ContributorInsightsStatus"].clone(),
                updated["ContributorInsightsMode"].clone(),
                read_back["ContributorInsightsStatus"].clone(),
                read_back["ContributorInsightsMode"].clone(),
            ),
            (
                json!("ENABLING"),
                json!(expected_mode),
                json!("ENABLING"),
                json!(expected_mode),
            ),
            "case {case}: the enable is in flight, got {updated} then {read_back}"
        );

        // Once the clock has made the move due the settled status is reported, and the
        // timestamp of that change is not in the future.
        settle().await;
        let settled = describe_insights(&router, &table, index).await;
        let changed_at = settled["LastUpdateDateTime"].as_f64().unwrap_or_default();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs_f64();
        assert!(
            settled["ContributorInsightsStatus"] == json!("ENABLED")
                && settled["ContributorInsightsMode"] == json!(expected_mode)
                && changed_at > 0.0
                && changed_at <= now,
            "case {case}: the enable settled, got {settled} against now {now}"
        );

        // The disable is asynchronous the same way, and leaves the resource tracking
        // nothing once it settles.
        let disabled = set_insights(&router, &table, index, "DISABLE", None).await;
        assert_eq!(
            disabled["ContributorInsightsStatus"].clone(),
            json!("DISABLING"),
            "case {case}: the disable is in flight, got {disabled}"
        );
        settle().await;
        let off = describe_insights(&router, &table, index).await;
        assert_eq!(
            (
                off["ContributorInsightsStatus"].clone(),
                off["ContributorInsightsMode"].clone(),
            ),
            (json!("DISABLED"), Value::Null),
            "case {case}: the disable settled, got {off}"
        );
    }
}

#[tokio::test]
async fn each_resource_carries_its_own_setting_and_the_listing_reports_one_per_resource() {
    let router = emulator();
    create_table(&router, "InsightsListed", &["B.Index", "A.Index"]).await;
    create_table(&router, "InsightsOther", &[]).await;
    settle().await;

    // Only the resource named is changed: enabling one index leaves the table and the
    // other index off. Rows are the resources of one table, and what each is asked for.
    set_insights(&router, "InsightsListed", Some("A.Index"), "ENABLE", None).await;
    settle().await;

    for (case, index, expected) in [
        ("the table itself", None, "DISABLED"),
        ("the index enabled", Some("A.Index"), "ENABLED"),
        ("the index left alone", Some("B.Index"), "DISABLED"),
    ] {
        let described = describe_insights(&router, "InsightsListed", index).await;
        assert_eq!(
            described["ContributorInsightsStatus"].clone(),
            json!(expected),
            "case {case}: only the resource named changed, got {described}"
        );
    }

    // The listing narrowed to one table reports that table and all its indexes, in
    // ascending order, each with the status its own setting is in.
    let listed = ok(
        &router,
        "ListContributorInsights",
        json!({"TableName": "InsightsListed"}),
    )
    .await;
    let summaries = listed["ContributorInsightsSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let reported: Vec<(String, String, String)> = summaries
        .iter()
        .map(|summary| {
            (
                summary["TableName"].as_str().unwrap_or_default().to_string(),
                summary["IndexName"].as_str().unwrap_or_default().to_string(),
                summary["ContributorInsightsStatus"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    assert_eq!(
        reported,
        vec![
            (
                "InsightsListed".to_string(),
                String::new(),
                "DISABLED".to_string()
            ),
            (
                "InsightsListed".to_string(),
                "A.Index".to_string(),
                "ENABLED".to_string()
            ),
            (
                "InsightsListed".to_string(),
                "B.Index".to_string(),
                "DISABLED".to_string()
            ),
        ],
        "case one table: the summaries are the table's and its indexes', got {listed}"
    );
    assert_eq!(
        listed["NextToken"].clone(),
        Value::Null,
        "case one table: a whole listing reports no token, got {listed}"
    );

    // A listing that names no table reports every table of the region, so the other
    // table's summary is in it too.
    let all = ok(&router, "ListContributorInsights", json!({})).await;
    let names: Vec<String> = all["ContributorInsightsSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| {
            format!(
                "{}{}",
                summary["TableName"].as_str().unwrap_or_default(),
                summary["IndexName"]
                    .as_str()
                    .map(|index| format!("/{index}"))
                    .unwrap_or_default()
            )
        })
        .collect();
    assert_eq!(
        names,
        vec![
            "InsightsListed".to_string(),
            "InsightsListed/A.Index".to_string(),
            "InsightsListed/B.Index".to_string(),
            "InsightsOther".to_string(),
        ],
        "case every table: each resource is reported once, got {all}"
    );

    // Paging: rows differ in the page size asked for, and every page together is the
    // whole listing in the same order.
    for (case, page_size) in [("one at a time", 1), ("two at a time", 2), ("all four", 4)] {
        let mut collected: Vec<String> = Vec::new();
        let mut token = Value::Null;
        loop {
            let mut request = json!({"MaxResults": page_size});
            if let Some(next) = token.as_str() {
                request["NextToken"] = json!(next);
            }
            let page = ok(&router, "ListContributorInsights", request).await;
            let entries = page["ContributorInsightsSummaries"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                entries.len() <= page_size,
                "case {case}: a page is no longer than the size asked for, got {page}"
            );
            collected.extend(entries.iter().map(|summary| {
                format!(
                    "{}{}",
                    summary["TableName"].as_str().unwrap_or_default(),
                    summary["IndexName"]
                        .as_str()
                        .map(|index| format!("/{index}"))
                        .unwrap_or_default()
                )
            }));
            token = page["NextToken"].clone();
            if token.is_null() {
                break;
            }
        }
        assert_eq!(
            collected, names,
            "case {case}: the pages together are the whole listing, got {collected:?}"
        );
    }
}

#[tokio::test]
async fn every_status_reported_is_one_the_model_declares() {
    // `ContributorInsightsStatus` is an enum, so what a read reports must be a value from
    // the model's own table — the generated `CONTRIBUTOR_INSIGHTS_STATUS_VALUES` — and
    // not a spelling this emulator invented. Rows span the states a resource can be read
    // in: never touched, enabling, enabled, disabling, disabled.
    use service_com_amazonaws_dynamodb_dynamodb_20120810::ops::types::CONTRIBUTOR_INSIGHTS_STATUS_VALUES;

    let router = emulator();
    create_table(&router, "InsightsDeclared", &["Declared.Index"]).await;
    settle().await;

    let cases: [(&str, Option<(&str, bool)>); 5] = [
        ("never touched", None),
        ("enabling", Some(("ENABLE", false))),
        ("enabled", Some(("ENABLE", true))),
        ("disabling", Some(("DISABLE", false))),
        ("disabled", Some(("DISABLE", true))),
    ];
    for (case, action) in cases {
        if let Some((action, wait)) = action {
            set_insights(&router, "InsightsDeclared", None, action, None).await;
            if wait {
                settle().await;
            }
        }
        for (resource, index) in [("the table", None), ("the index", Some("Declared.Index"))] {
            let described = describe_insights(&router, "InsightsDeclared", index).await;
            let status = described["ContributorInsightsStatus"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                CONTRIBUTOR_INSIGHTS_STATUS_VALUES.contains(&status.as_str()),
                "case {case} on {resource}: the status is a value the model declares, got \
                 {status:?} in {described}"
            );
        }
        let listed = ok(
            &router,
            "ListContributorInsights",
            json!({"TableName": "InsightsDeclared"}),
        )
        .await;
        for summary in listed["ContributorInsightsSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            let status = summary["ContributorInsightsStatus"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                CONTRIBUTOR_INSIGHTS_STATUS_VALUES.contains(&status.as_str()),
                "case {case} in the listing: the status is a value the model declares, got \
                 {status:?} in {listed}"
            );
        }
    }
}
