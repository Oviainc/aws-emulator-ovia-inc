// SPDX-License-Identifier: CC0-1.0

//! The properties of `AWS::DynamoDB::Table` that Amazon DynamoDB puts into force through a
//! call of its OWN on the created table, read back through that service's own reads once the
//! stack reports itself complete.
//!
//! RULE: a stack creates its resources with the WHOLE configuration their declarations ask
//! for — including every property the owning service applies through a separate call on the
//! resource it has just created — and reports a resource created only once that service's own
//! read reports those settings in force. So a client that provisions a table by stack and
//! then verifies the setting through DynamoDB is answered what the template declared, and a
//! declaration that service refuses fails the resource instead of being dropped from a stack
//! that reports success.
//!
//! The rows differ along what the rule generalises over: WHICH property is declared (each of
//! the four of this type that are calls of their own), the value it declares (the attribute a
//! TTL expires items by, the recovery period, the replication precision), whether the
//! declaration turns the setting ON or asks for it off, and a declaration carrying all of
//! them at once. Each row's expectation is read through the OWNING service's own read
//! (`DescribeTimeToLive`, `DescribeContinuousBackups`, `DescribeContributorInsights`,
//! `DescribeKinesisStreamingDestination`), which is the only thing that says the property was
//! applied rather than trusted to the stack's status.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::dynamodb_table_resources::{describe_table, dynamodb, failure_reasons};
use crate::stack_lifecycle::{
    call, create_stack, describe_stack, member, router, xml_text, ACCOUNT_ID, REGION,
};

/// The `StackStatus` values the rows settle on.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const ROLLBACK_COMPLETE: &str = "ROLLBACK_COMPLETE";

/// How often a row asks the stack whether it has settled, and how long it goes on asking.
///
/// The budget covers the spans DynamoDB owes a new table and a setting it is putting into
/// force several times over — a table reported created carries both, one after the other —
/// so a loaded machine is slower than those spans without the rule being any different, and
/// a row that never settles reports rather than running on.
const POLL_INTERVAL: Duration = Duration::from_millis(50);
const POLLS: usize = 400;

/// The key schema every row's table declares, with the attribute definition behind it.
fn base_properties(table_name: &str) -> Value {
    json!({
        "TableName": table_name,
        "BillingMode": "PAY_PER_REQUEST",
        "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
        "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
    })
}

/// The template one row deploys: its table, declared with the settings the row asks for.
fn template(table_name: &str, settings: &Value) -> String {
    let mut properties = base_properties(table_name);
    let members = properties
        .as_object_mut()
        .expect("the declaration is an object");
    for (property, value) in settings
        .as_object()
        .expect("the row's settings are an object")
    {
        members.insert(property.clone(), value.clone());
    }
    json!({
        "Resources": {"Table": {"Type": "AWS::DynamoDB::Table", "Properties": properties}},
    })
    .to_string()
}

/// The `StackStatus` the stack reports right now, with the answer it was read from.
async fn stack_status(router: &MultiServiceRouter, stack: &str) -> (String, String) {
    let (status, body) = describe_stack(router, stack).await;
    assert_eq!(
        status, 200,
        "DescribeStacks {stack:?}: expected HTTP 200, got {status} with body {body}"
    );
    (xml_text(&body, "StackStatus").unwrap_or_default(), body)
}

/// Poll `stack` until it first reports `wanted`, and answer the last status it reported with
/// the answer that status was read from.
///
/// The FIRST observation of the wanted status is what every row below is stated about: the
/// settings are read straight after it, so nothing is waited out past it.
async fn wait_for_stack_status(
    router: &MultiServiceRouter,
    stack: &str,
    wanted: &str,
) -> (String, String) {
    let mut last = stack_status(router, stack).await;
    for _ in 0..POLLS {
        if last.0 == wanted {
            return last;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
        last = stack_status(router, stack).await;
    }
    last
}

/// One expectation of a row: the DynamoDB read that reports the setting, a JSON Pointer into
/// that read's answer, and the value the declaration asked for.
struct Reported {
    operation: &'static str,
    pointer: &'static str,
    expected: Value,
}

/// One row of the table: a stack, the settings its table declares, and what each owning
/// read must then report.
struct Row {
    case: &'static str,
    stack: &'static str,
    table: &'static str,
    settings: Value,
    reported: Vec<Reported>,
}

/// The ARN of the Kinesis data stream the replicating rows name
/// (`arn:<partition>:kinesis:<region>:<account>:stream/<name>`).
fn stream_arn(name: &str) -> String {
    format!("arn:aws:kinesis:{REGION}:{ACCOUNT_ID}:stream/{name}")
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a table whose declaration enables TTL on expiresAt",
            stack: "ddb-settings-ttl",
            table: "ttl-table",
            settings: json!({
                "TimeToLiveSpecification": {"AttributeName": "expiresAt", "Enabled": true},
            }),
            reported: vec![
                Reported {
                    operation: "DescribeTimeToLive",
                    pointer: "/TimeToLiveDescription/TimeToLiveStatus",
                    expected: json!("ENABLED"),
                },
                Reported {
                    operation: "DescribeTimeToLive",
                    pointer: "/TimeToLiveDescription/AttributeName",
                    expected: json!("expiresAt"),
                },
            ],
        },
        Row {
            case: "TTL over another attribute: the rule is the declaration, not one name",
            stack: "ddb-settings-ttl-other",
            table: "ttl-other-table",
            settings: json!({
                "TimeToLiveSpecification": {"AttributeName": "purge_after", "Enabled": true},
            }),
            reported: vec![
                Reported {
                    operation: "DescribeTimeToLive",
                    pointer: "/TimeToLiveDescription/TimeToLiveStatus",
                    expected: json!("ENABLED"),
                },
                Reported {
                    operation: "DescribeTimeToLive",
                    pointer: "/TimeToLiveDescription/AttributeName",
                    expected: json!("purge_after"),
                },
            ],
        },
        Row {
            case: "a declaration that asks for TTL off: the table is created with it off",
            stack: "ddb-settings-ttl-off",
            table: "ttl-off-table",
            settings: json!({
                "TimeToLiveSpecification": {"AttributeName": "expiresAt", "Enabled": false},
            }),
            reported: vec![Reported {
                operation: "DescribeTimeToLive",
                pointer: "/TimeToLiveDescription/TimeToLiveStatus",
                expected: json!("DISABLED"),
            }],
        },
        Row {
            case: "a table whose declaration asks for continuous backups over 7 days",
            stack: "ddb-settings-pitr",
            table: "pitr-table",
            settings: json!({
                "PointInTimeRecoverySpecification": {"PointInTimeRecoveryEnabled": true,
                                                     "RecoveryPeriodInDays": 7},
            }),
            reported: vec![
                Reported {
                    operation: "DescribeContinuousBackups",
                    pointer: "/ContinuousBackupsDescription/PointInTimeRecoveryDescription/\
                              PointInTimeRecoveryStatus",
                    expected: json!("ENABLED"),
                },
                Reported {
                    operation: "DescribeContinuousBackups",
                    pointer: "/ContinuousBackupsDescription/PointInTimeRecoveryDescription/\
                              RecoveryPeriodInDays",
                    expected: json!(7),
                },
            ],
        },
        Row {
            case: "a table whose declaration turns Contributor Insights on",
            stack: "ddb-settings-insights",
            table: "insights-table",
            settings: json!({"ContributorInsightsSpecification": {"Enabled": true}}),
            reported: vec![Reported {
                operation: "DescribeContributorInsights",
                pointer: "/ContributorInsightsStatus",
                expected: json!("ENABLED"),
            }],
        },
        Row {
            case: "a table whose declaration replicates its changes to a Kinesis stream",
            stack: "ddb-settings-kinesis",
            table: "kinesis-table",
            settings: json!({
                "KinesisStreamSpecification": {
                    "StreamArn": stream_arn("table-changes"),
                    "ApproximateCreationDateTimePrecision": "MICROSECOND",
                },
            }),
            reported: vec![
                Reported {
                    operation: "DescribeKinesisStreamingDestination",
                    pointer: "/KinesisDataStreamDestinations/0/DestinationStatus",
                    expected: json!("ACTIVE"),
                },
                Reported {
                    operation: "DescribeKinesisStreamingDestination",
                    pointer: "/KinesisDataStreamDestinations/0/StreamArn",
                    expected: json!(stream_arn("table-changes")),
                },
                Reported {
                    operation: "DescribeKinesisStreamingDestination",
                    pointer: "/KinesisDataStreamDestinations/0/\
                              ApproximateCreationDateTimePrecision",
                    expected: json!("MICROSECOND"),
                },
            ],
        },
        Row {
            case: "one declaration carrying every one of them at once",
            stack: "ddb-settings-all",
            table: "every-setting-table",
            settings: json!({
                "TimeToLiveSpecification": {"AttributeName": "ttl", "Enabled": true},
                "PointInTimeRecoverySpecification": {"PointInTimeRecoveryEnabled": true},
                "ContributorInsightsSpecification": {"Enabled": true},
                "KinesisStreamSpecification": {"StreamArn": stream_arn("all-changes")},
            }),
            reported: vec![
                Reported {
                    operation: "DescribeTimeToLive",
                    pointer: "/TimeToLiveDescription/TimeToLiveStatus",
                    expected: json!("ENABLED"),
                },
                Reported {
                    operation: "DescribeTimeToLive",
                    pointer: "/TimeToLiveDescription/AttributeName",
                    expected: json!("ttl"),
                },
                Reported {
                    operation: "DescribeContinuousBackups",
                    pointer: "/ContinuousBackupsDescription/PointInTimeRecoveryDescription/\
                              PointInTimeRecoveryStatus",
                    expected: json!("ENABLED"),
                },
                Reported {
                    operation: "DescribeContributorInsights",
                    pointer: "/ContributorInsightsStatus",
                    expected: json!("ENABLED"),
                },
                Reported {
                    operation: "DescribeKinesisStreamingDestination",
                    pointer: "/KinesisDataStreamDestinations/0/DestinationStatus",
                    expected: json!("ACTIVE"),
                },
            ],
        },
    ]
}

/// Every row: the stack completes, and the instant it first reports itself complete the
/// setting its template declared is in force in DynamoDB — read through that service's own
/// read, on the table the template named.
#[tokio::test]
async fn a_declared_table_setting_is_in_force_when_the_stack_completes() {
    for row in rows() {
        let router = router();
        let case = row.case;
        let body = template(row.table, &row.settings);
        let (status, answer) = create_stack(&router, row.stack, &body, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} {answer}"
        );

        let (reported, described) =
            wait_for_stack_status(&router, row.stack, CREATE_COMPLETE).await;
        assert_eq!(
            reported, CREATE_COMPLETE,
            "case {case:?}: expected the stack to reach {CREATE_COMPLETE}, got {reported:?} \
             because {:?} in {described}",
            failure_reasons(&router, row.stack).await
        );

        // The table the template NAMED is the table DynamoDB holds: the settings below are
        // read on that name, which is the name a client addresses the table by.
        assert!(
            describe_table(&router, row.table).await.is_some(),
            "case {case:?}: expected DynamoDB to hold the table {:?} the stack reports \
             created, got no table at all",
            row.table
        );

        for expectation in &row.reported {
            let (status, answered) = dynamodb(
                &router,
                expectation.operation,
                &json!({"TableName": row.table}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from {} on the table the stack created, got \
                 {status} with body {answered}",
                expectation.operation
            );
            let at = expectation.pointer;
            let value = answered.pointer(at).cloned();
            assert_eq!(
                value.as_ref(),
                Some(&expectation.expected),
                "case {case:?}: expected {} to report {:?} at {at}, got {value:?} in {answered}",
                expectation.operation,
                expectation.expected
            );
        }
    }
}

/// A setting DynamoDB REFUSES fails the resource: the stack is rolled back and that service
/// holds no table, rather than a stack reporting a table created without the configuration
/// its declaration asked for.
#[tokio::test]
async fn a_setting_the_owning_service_refuses_fails_the_resource() {
    let router = router();
    let case = "a TTL attribute name longer than the member admits";
    let stack = "ddb-settings-refused";
    let table = "refused-settings-table";
    // `TimeToLiveSpecification$AttributeName` is bounded at 1..255
    // (`smithy_query(query="shape",shape="TimeToLiveSpecification")`), so a longer name is a
    // value that service will not take.
    let settings = json!({
        "TimeToLiveSpecification": {"AttributeName": "e".repeat(256), "Enabled": true},
    });
    let (status, answer) = create_stack(&router, stack, &template(table, &settings), &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} {answer}"
    );
    let (reported, described) = wait_for_stack_status(&router, stack, ROLLBACK_COMPLETE).await;
    assert_eq!(
        reported, ROLLBACK_COMPLETE,
        "case {case:?}: expected the refused setting to fail the resource and roll the stack \
         back, got {reported:?} in {described}"
    );
    // The table the failed create had made is removed again. DynamoDB reports a deleted
    // table `DELETING` until its own span falls due, so the read is taken until that service
    // no longer holds it.
    let mut held = describe_table(&router, table).await;
    for _ in 0..POLLS {
        if held.is_none() {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
        held = describe_table(&router, table).await;
    }
    assert!(
        held.is_none(),
        "case {case:?}: expected DynamoDB to hold no table for a create whose declared \
         setting it refused, got {held:?}"
    );
}

/// The setting is applied to the table a CHANGED declaration names too: a deployment that
/// turns TTL on over a table the stack already has puts it into force through the same call,
/// and one that drops the property turns it off again.
#[tokio::test]
async fn a_changed_declaration_applies_the_setting_to_the_table_the_stack_has() {
    let router = router();
    let case = "TTL turned on, then dropped, over one table";
    let stack = "ddb-settings-updated";
    let table = "updated-settings-table";

    let (status, answer) = create_stack(&router, stack, &template(table, &json!({})), &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} {answer}"
    );
    let (reported, described) = wait_for_stack_status(&router, stack, CREATE_COMPLETE).await;
    assert_eq!(
        reported, CREATE_COMPLETE,
        "case {case:?}: expected the stack to complete, got {reported:?} in {described}"
    );

    // The deployment that turns TTL on: the property is applied to the table that already
    // exists, which that service's own read then reports.
    let enabling = template(
        table,
        &json!({"TimeToLiveSpecification": {"AttributeName": "expiresAt", "Enabled": true}}),
    );
    let (status, answer) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", stack),
            member("TemplateBody", &enabling),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {status} {answer}"
    );
    let (reported, described) = wait_for_stack_status(&router, stack, "UPDATE_COMPLETE").await;
    assert_eq!(
        reported, "UPDATE_COMPLETE",
        "case {case:?}: expected the update to complete, got {reported:?} because {:?} in \
         {described}",
        failure_reasons(&router, stack).await
    );
    let status = wait_for_time_to_live(&router, table, "ENABLED").await;
    assert_eq!(
        status, "ENABLED",
        "case {case:?}: expected DynamoDB to report TTL ENABLED on {table:?} after the \
         deployment that declared it, got {status:?}"
    );

    // And the deployment that DROPS the property asks for TTL off, which the same call
    // expresses over the attribute the setting is in force for.
    let (status, answer) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", stack),
            member("TemplateBody", &template(table, &json!({}))),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the second UpdateStack, got {status} {answer}"
    );
    let (reported, described) = wait_for_stack_status(&router, stack, "UPDATE_COMPLETE").await;
    assert_eq!(
        reported, "UPDATE_COMPLETE",
        "case {case:?}: expected the second update to complete, got {reported:?} because {:?} \
         in {described}",
        failure_reasons(&router, stack).await
    );
    let status = wait_for_time_to_live(&router, table, "DISABLED").await;
    assert_eq!(
        status, "DISABLED",
        "case {case:?}: expected DynamoDB to report TTL DISABLED on {table:?} once the \
         declaration dropped the property, got {status:?}"
    );
}

/// The `TimeToLiveStatus` DynamoDB reports for `table`, waited for until it is `wanted`:
/// turning the setting on or off is asynchronous in that service, so the status it settles on
/// is owed from the instant its own span falls due and no fixed sleep names it.
async fn wait_for_time_to_live(router: &MultiServiceRouter, table: &str, wanted: &str) -> String {
    let mut last = String::new();
    for _ in 0..POLLS {
        let asked = json!({"TableName": table});
        let (_, answered) = dynamodb(router, "DescribeTimeToLive", &asked).await;
        last = answered
            .pointer("/TimeToLiveDescription/TimeToLiveStatus")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if last == wanted {
            return last;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    last
}
