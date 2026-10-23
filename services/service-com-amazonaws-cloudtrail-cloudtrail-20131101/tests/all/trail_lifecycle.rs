// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CloudTrail trail lifecycle.
//!
//! Every case goes over the wire: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: CloudTrail_20131101.<Operation>` through the same
//! `MultiServiceRouter` the production router serves, so what is asserted here is
//! what a client sees — including the HTTP status and the error code.
//!
//! The rules under test, each grounded in this service's Smithy model:
//!
//! * a created trail is persisted and every later read reports the stored values;
//! * a trail identifier is a bare name (the caller's Region) or a trail ARN (the
//!   ARN's Region), and a malformed one is refused as the form it was written in;
//! * a trail name must satisfy the rules the model documents for
//!   `CreateTrailRequest$Name`;
//! * `DescribeTrails` reports the caller's Region plus, unless
//!   `includeShadowTrails` is false, the multi-Region trails homed elsewhere;
//! * `StartLogging` / `StopLogging` move the logging flag, are `@idempotent`, and
//!   only run in the trail's home Region.

use serde_json::{json, Value};

// The emulator-under-test harness is shared with this crate's other suites; see
// `tests/all/emulator.rs`.
use super::emulator::{Emulator, HOME_REGION};

/// The trail names a reply's `trailList` / `Trails` carries, in order.
fn names(list: &Value, member: &str) -> Vec<String> {
    list.get(member)
        .and_then(Value::as_array)
        .map(|trails| {
            trails
                .iter()
                .map(|trail| {
                    trail
                        .get("Name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The issue's happy path, end to end: create, read back, drive logging, list,
/// delete, and confirm the trail is gone.
#[tokio::test]
async fn a_trail_lives_through_create_read_logging_and_delete() {
    let emulator = Emulator::new();
    let request = json!({
        "Name": "demo-trail",
        "S3BucketName": "my-bucket",
        "IsMultiRegionTrail": true,
        "EnableLogFileValidation": true,
    });

    let created = emulator.call("CreateTrail", &request).await;
    let created = created.expect_ok("CreateTrail").clone();
    assert_eq!(
        created["TrailARN"],
        json!("arn:aws:cloudtrail:us-east-1:000000000000:trail/demo-trail"),
        "case CreateTrail: the minted trail ARN is wrong, got {created:?}"
    );
    assert_eq!(
        (
            &created["Name"],
            &created["S3BucketName"],
            &created["IsMultiRegionTrail"],
            &created["LogFileValidationEnabled"],
        ),
        (
            &json!("demo-trail"),
            &json!("my-bucket"),
            &json!(true),
            &json!(true),
        ),
        "case CreateTrail: the response does not echo the request, got {created:?}"
    );

    // The stored trail, read back through GetTrail.
    let fetched = emulator.call("GetTrail", &json!({"Name": "demo-trail"})).await;
    let trail = fetched.expect_ok("GetTrail")["Trail"].clone();
    assert_eq!(
        trail["TrailARN"], created["TrailARN"],
        "case GetTrail: the trail read back is not the one created, got {trail:?}"
    );
    assert_eq!(
        trail["HomeRegion"],
        json!(HOME_REGION),
        "case GetTrail: expected HomeRegion {HOME_REGION}, got {trail:?}"
    );

    // A fresh trail is not logging.
    let status = emulator
        .call("GetTrailStatus", &json!({"Name": "demo-trail"}))
        .await;
    let status = status.expect_ok("GetTrailStatus").clone();
    assert_eq!(
        status["IsLogging"],
        json!(false),
        "case GetTrailStatus after create: expected IsLogging false, got {status:?}"
    );
    assert!(
        status.get("StartLoggingTime").is_none(),
        "case GetTrailStatus after create: expected no StartLoggingTime, got {status:?}"
    );

    emulator
        .call("StartLogging", &json!({"Name": "demo-trail"}))
        .await
        .expect_ok("StartLogging");
    let status = emulator
        .call("GetTrailStatus", &json!({"Name": "demo-trail"}))
        .await;
    let started = status.expect_ok("GetTrailStatus after StartLogging").clone();
    assert_eq!(
        started["IsLogging"],
        json!(true),
        "case GetTrailStatus after StartLogging: expected IsLogging true, got {started:?}"
    );
    let start_logging_time = started["StartLoggingTime"].as_f64();
    assert!(
        start_logging_time.is_some_and(|t| t > 0.0),
        "case GetTrailStatus after StartLogging: expected a StartLoggingTime, got {started:?}"
    );
    assert!(
        started["TimeLoggingStarted"]
            .as_str()
            .is_some_and(|t| t.ends_with('Z')),
        "case GetTrailStatus after StartLogging: expected an ISO 8601 TimeLoggingStarted, \
         got {started:?}"
    );

    // StartLogging is `@idempotent`: a second call leaves the start instant alone.
    emulator
        .call("StartLogging", &json!({"Name": "demo-trail"}))
        .await
        .expect_ok("StartLogging twice");
    let again = emulator
        .call("GetTrailStatus", &json!({"Name": "demo-trail"}))
        .await;
    let again = again.expect_ok("GetTrailStatus after StartLogging twice").clone();
    assert_eq!(
        again["StartLoggingTime"], started["StartLoggingTime"],
        "case StartLogging twice: the start instant moved, got {again:?}"
    );

    // DescribeTrails reports it, and filtering by another name filters it out.
    let described = emulator.call("DescribeTrails", &json!({})).await;
    assert_eq!(
        names(described.expect_ok("DescribeTrails"), "trailList"),
        vec!["demo-trail".to_string()],
        "case DescribeTrails: expected the created trail, got {described:?}"
    );
    let filtered = emulator
        .call("DescribeTrails", &json!({"trailNameList": ["other-trail"]}))
        .await;
    assert_eq!(
        names(filtered.expect_ok("DescribeTrails filtered"), "trailList"),
        Vec::<String>::new(),
        "case DescribeTrails filtered by another name: expected nothing, got {filtered:?}"
    );

    emulator
        .call("StopLogging", &json!({"Name": "demo-trail"}))
        .await
        .expect_ok("StopLogging");
    let stopped = emulator
        .call("GetTrailStatus", &json!({"Name": "demo-trail"}))
        .await;
    let stopped = stopped.expect_ok("GetTrailStatus after StopLogging").clone();
    assert_eq!(
        stopped["IsLogging"],
        json!(false),
        "case GetTrailStatus after StopLogging: expected IsLogging false, got {stopped:?}"
    );
    assert!(
        stopped["StopLoggingTime"].as_f64().is_some_and(|t| t > 0.0),
        "case GetTrailStatus after StopLogging: expected a StopLoggingTime, got {stopped:?}"
    );
    assert_eq!(
        stopped["StartLoggingTime"], started["StartLoggingTime"],
        "case GetTrailStatus after StopLogging: the start instant moved, got {stopped:?}"
    );

    // The name is taken: a second create is a conflict, not an overwrite.
    emulator
        .call("CreateTrail", &json!({"Name": "demo-trail", "S3BucketName": "other-bucket"}))
        .await
        .expect_error("CreateTrail twice", 400, "TrailAlreadyExistsException");
    let unchanged = emulator.call("GetTrail", &json!({"Name": "demo-trail"})).await;
    assert_eq!(
        unchanged.expect_ok("GetTrail after the conflict")["Trail"]["S3BucketName"],
        json!("my-bucket"),
        "case CreateTrail twice: the stored trail was overwritten, got {unchanged:?}"
    );

    emulator
        .call("DeleteTrail", &json!({"Name": "demo-trail"}))
        .await
        .expect_ok("DeleteTrail");
    for operation in ["GetTrail", "GetTrailStatus", "DeleteTrail", "StartLogging", "StopLogging"] {
        emulator
            .call(operation, &json!({"Name": "demo-trail"}))
            .await
            .expect_error(
                &format!("{operation} after DeleteTrail"),
                404,
                "TrailNotFoundException",
            );
    }
    let empty = emulator.call("DescribeTrails", &json!({})).await;
    assert_eq!(
        names(empty.expect_ok("DescribeTrails after DeleteTrail"), "trailList"),
        Vec::<String>::new(),
        "case DescribeTrails after DeleteTrail: expected nothing, got {empty:?}"
    );
}

/// Whatever configuration a trail is created with, the response and every later
/// read report the stored values — the rows differ in which optional members are
/// present, in the form of `SnsTopicName` (bare name vs ARN), and in each boolean's
/// value, including the members whose absence takes a documented default.
#[tokio::test]
async fn a_created_trail_reports_the_configuration_it_was_given() {
    let emulator = Emulator::new();
    // (case, request, expected members of the CreateTrail response)
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "only the required members",
            json!({"Name": "minimal", "S3BucketName": "bucket-a"}),
            json!({
                "Name": "minimal",
                "S3BucketName": "bucket-a",
                // Documented defaults: multi-Region, validation and organization
                // trail off, recursive logging on, global service events delivered.
                "IsMultiRegionTrail": false,
                "LogFileValidationEnabled": false,
                "IsOrganizationTrail": false,
                "RecursiveLogging": true,
                "IncludeGlobalServiceEvents": true,
                "TrailARN": "arn:aws:cloudtrail:us-east-1:000000000000:trail/minimal",
            }),
        ),
        (
            "every flag turned the other way",
            json!({
                "Name": "all.flags_set-1",
                "S3BucketName": "bucket-b",
                "S3KeyPrefix": "logs/prod",
                "IncludeGlobalServiceEvents": false,
                "IsMultiRegionTrail": true,
                "EnableLogFileValidation": true,
                "IsOrganizationTrail": true,
                "RecursiveLogging": false,
                "KmsKeyId": "alias/MyAliasName",
                "CloudWatchLogsLogGroupArn":
                    "arn:aws:logs:us-east-1:000000000000:log-group:ct:*",
                "CloudWatchLogsRoleArn": "arn:aws:iam::000000000000:role/ct",
            }),
            json!({
                "Name": "all.flags_set-1",
                "S3BucketName": "bucket-b",
                "S3KeyPrefix": "logs/prod",
                "IncludeGlobalServiceEvents": false,
                "IsMultiRegionTrail": true,
                "LogFileValidationEnabled": true,
                "IsOrganizationTrail": true,
                "RecursiveLogging": false,
                "KmsKeyId": "alias/MyAliasName",
                "CloudWatchLogsLogGroupArn":
                    "arn:aws:logs:us-east-1:000000000000:log-group:ct:*",
                "CloudWatchLogsRoleArn": "arn:aws:iam::000000000000:role/ct",
                "TrailARN": "arn:aws:cloudtrail:us-east-1:000000000000:trail/all.flags_set-1",
            }),
        ),
        (
            "an SNS topic given by name is completed into an ARN",
            json!({"Name": "sns-by-name", "S3BucketName": "bucket-c", "SnsTopicName": "MyTopic"}),
            json!({
                "Name": "sns-by-name",
                "SnsTopicARN": "arn:aws:sns:us-east-1:000000000000:MyTopic",
            }),
        ),
        (
            "an SNS topic given as an ARN is kept",
            json!({
                "Name": "sns-by-arn",
                "S3BucketName": "bucket-d",
                "SnsTopicName": "arn:aws:sns:eu-west-1:000000000000:OtherTopic",
            }),
            json!({
                "Name": "sns-by-arn",
                "SnsTopicARN": "arn:aws:sns:eu-west-1:000000000000:OtherTopic",
            }),
        ),
        (
            "tags at the documented limit of 50",
            json!({
                "Name": "fifty-tags",
                "S3BucketName": "bucket-e",
                "TagsList": (0..50)
                    .map(|i| json!({"Key": format!("k{i}"), "Value": format!("v{i}")}))
                    .collect::<Vec<Value>>(),
            }),
            json!({"Name": "fifty-tags"}),
        ),
    ];

    for (case, request, expected) in rows {
        let created = emulator.call("CreateTrail", &request).await;
        let created = created.expect_ok(case).clone();
        for (member, want) in expected.as_object().expect("an object of expectations") {
            assert_eq!(
                &created[member], want,
                "case {case}: expected {member} = {want}, got {created:?}"
            );
        }
        // `SnsTopicName` is deprecated in the model in favour of `SnsTopicARN`.
        assert!(
            created.get("SnsTopicName").is_none(),
            "case {case}: the deprecated SnsTopicName was reported, got {created:?}"
        );

        // Everything the create reported is what a later read reports.
        let name = request["Name"].as_str().expect("a name");
        let fetched = emulator.call("GetTrail", &json!({"Name": name})).await;
        let trail = fetched.expect_ok(case)["Trail"].clone();
        for (member, want) in created.as_object().expect("an object") {
            assert_eq!(
                &trail[member], want,
                "case {case}: GetTrail reports {member} = {} where CreateTrail \
                 reported {want}, got {trail:?}",
                &trail[member]
            );
        }
        // Neither selector kind has been put on this trail.
        assert_eq!(
            (&trail["HasCustomEventSelectors"], &trail["HasInsightSelectors"]),
            (&json!(false), &json!(false)),
            "case {case}: expected no selectors on a fresh trail, got {trail:?}"
        );
    }
}

/// A trail name is accepted or refused by the rules the model documents for
/// `CreateTrailRequest$Name`, at every published bound and one past it.
#[tokio::test]
async fn trail_names_follow_the_documented_name_rules() {
    let emulator = Emulator::new();
    // (name, is it valid) — the invalid rows each break exactly one documented
    // rule; `my-_namespace` and `my--namespace` are the model's own examples.
    let rows: Vec<(String, bool)> = vec![
        ("abc".to_string(), true),                 // the documented minimum, 3
        ("ab".to_string(), false),                 // one under it
        ("a".repeat(128), true),                   // the documented maximum, 128
        ("a".repeat(129), false),                  // one over it
        ("a.b_c-d9".to_string(), true),            // every allowed punctuation
        ("my-_namespace".to_string(), false),      // adjacent punctuation
        ("my--namespace".to_string(), false),      // adjacent punctuation
        ("-leading".to_string(), false),           // must start with a letter or number
        ("trailing-".to_string(), false),          // must end with a letter or number
        ("has space".to_string(), false),          // ASCII letters/numbers/._- only
        ("has!punctuation".to_string(), false),    // ASCII letters/numbers/._- only
        ("hás-accent".to_string(), false),         // ASCII only
        ("192.168.5.4".to_string(), false),        // IP address format
        ("1.2.3.4.5".to_string(), true),           // not an IP address
        ("0123456789".to_string(), true),          // digits only is fine
    ];

    for (name, valid) in rows {
        let request = json!({"Name": name, "S3BucketName": "bucket-names"});
        let reply = emulator.call("CreateTrail", &request).await;
        if valid {
            reply.expect_ok(&format!("CreateTrail {name:?}"));
        } else {
            reply.expect_error(
                &format!("CreateTrail {name:?}"),
                400,
                "InvalidTrailNameException",
            );
        }
    }

    // The same rule governs every operation that takes a trail identifier.
    for operation in [
        "GetTrail",
        "GetTrailStatus",
        "StartLogging",
        "StopLogging",
        "DeleteTrail",
    ] {
        emulator
            .call(operation, &json!({"Name": "my--namespace"}))
            .await
            .expect_error(
                &format!("{operation} with an invalid name"),
                400,
                "InvalidTrailNameException",
            );
    }
    emulator
        .call("DescribeTrails", &json!({"trailNameList": ["my--namespace"]}))
        .await
        .expect_error(
            "DescribeTrails with an invalid name",
            400,
            "InvalidTrailNameException",
        );
}

/// Every operation that takes a trail identifier accepts the trail's ARN as well as
/// its name, and refuses a value shaped like an ARN that is not a trail ARN.
#[tokio::test]
async fn a_trail_identifier_is_a_name_or_a_trail_arn() {
    let emulator = Emulator::new();
    emulator
        .call(
            "CreateTrail",
            &json!({"Name": "by-arn", "S3BucketName": "bucket-arn"}),
        )
        .await
        .expect_ok("CreateTrail");
    let arn = "arn:aws:cloudtrail:us-east-1:000000000000:trail/by-arn";

    for operation in ["GetTrail", "GetTrailStatus", "StartLogging", "StopLogging"] {
        emulator
            .call(operation, &json!({"Name": arn}))
            .await
            .expect_ok(&format!("{operation} by ARN"));
    }
    let described = emulator
        .call("DescribeTrails", &json!({"trailNameList": [arn]}))
        .await;
    assert_eq!(
        names(described.expect_ok("DescribeTrails by ARN"), "trailList"),
        vec!["by-arn".to_string()],
        "case DescribeTrails by ARN: expected the trail, got {described:?}"
    );

    // Not a trail ARN: the resource kind, the service, the field count and the
    // embedded name are each read.
    let malformed = [
        "arn:aws:cloudtrail:us-east-1:000000000000:eventdatastore/by-arn",
        "arn:aws:cloudtrail:us-east-1:000000000000:trail/my--namespace",
        "arn:aws:s3:::my-bucket",
        "arn:aws:cloudtrail:us-east-1:trail/by-arn",
        "arn:aws:cloudtrail::000000000000:trail/by-arn",
        "arn:",
    ];
    for identifier in malformed {
        for operation in ["GetTrail", "GetTrailStatus", "DeleteTrail"] {
            emulator
                .call(operation, &json!({"Name": identifier}))
                .await
                .expect_error(
                    &format!("{operation} with {identifier}"),
                    400,
                    "CloudTrailARNInvalidException",
                );
        }
    }

    // A well-formed trail ARN naming no stored trail is a not-found, not a refusal.
    emulator
        .call(
            "GetTrail",
            &json!({"Name": "arn:aws:cloudtrail:us-east-1:000000000000:trail/absent"}),
        )
        .await
        .expect_error("GetTrail with an unknown trail ARN", 404, "TrailNotFoundException");
}

/// A trail belongs to the Region it was created in: a bare name names a trail in
/// the caller's own Region, an ARN names the Region it carries, `DescribeTrails`
/// adds the multi-Region trails homed elsewhere unless the caller declines them,
/// and logging is driven only from the home Region.
#[tokio::test]
async fn region_decides_which_trails_a_caller_sees_and_can_drive() {
    let emulator = Emulator::new();
    emulator
        .call_in(
            "CreateTrail",
            "us-west-2",
            &json!({"Name": "single-region", "S3BucketName": "bucket-w"}),
        )
        .await
        .expect_ok("CreateTrail single-region in us-west-2");
    emulator
        .call_in(
            "CreateTrail",
            "us-west-2",
            &json!({"Name": "multi-region", "S3BucketName": "bucket-m", "IsMultiRegionTrail": true}),
        )
        .await
        .expect_ok("CreateTrail multi-region in us-west-2");

    // Home Region: both trails.
    let home = emulator.call_in("DescribeTrails", "us-west-2", &json!({})).await;
    assert_eq!(
        names(home.expect_ok("DescribeTrails in us-west-2"), "trailList"),
        vec!["multi-region".to_string(), "single-region".to_string()],
        "case DescribeTrails in the home Region: expected both trails, got {home:?}"
    );

    // Another Region: the multi-Region trail's shadow only, and nothing at all once
    // shadow trails are declined.
    let shadow = emulator.call_in("DescribeTrails", "eu-west-1", &json!({})).await;
    assert_eq!(
        names(shadow.expect_ok("DescribeTrails in eu-west-1"), "trailList"),
        vec!["multi-region".to_string()],
        "case DescribeTrails in another Region: expected the shadow trail only, got {shadow:?}"
    );
    let no_shadow = emulator
        .call_in("DescribeTrails", "eu-west-1", &json!({"includeShadowTrails": false}))
        .await;
    assert_eq!(
        names(
            no_shadow.expect_ok("DescribeTrails without shadow trails"),
            "trailList"
        ),
        Vec::<String>::new(),
        "case DescribeTrails with includeShadowTrails false: expected nothing, got {no_shadow:?}"
    );

    // A bare name in another Region names no trail; the ARN names it.
    emulator
        .call_in("GetTrail", "eu-west-1", &json!({"Name": "multi-region"}))
        .await
        .expect_error(
            "GetTrail by bare name from another Region",
            404,
            "TrailNotFoundException",
        );
    let by_arn = emulator
        .call_in(
            "GetTrail",
            "eu-west-1",
            &json!({"Name": "arn:aws:cloudtrail:us-west-2:000000000000:trail/multi-region"}),
        )
        .await;
    assert_eq!(
        by_arn.expect_ok("GetTrail by ARN from another Region")["Trail"]["HomeRegion"],
        json!("us-west-2"),
        "case GetTrail by ARN from another Region: expected the home Region, got {by_arn:?}"
    );

    // Logging and deletion run in the home Region only.
    for operation in ["StartLogging", "StopLogging", "DeleteTrail"] {
        emulator
            .call_in(
                operation,
                "eu-west-1",
                &json!({"Name": "arn:aws:cloudtrail:us-west-2:000000000000:trail/multi-region"}),
            )
            .await
            .expect_error(
                &format!("{operation} from another Region"),
                400,
                "InvalidHomeRegionException",
            );
    }
    emulator
        .call_in("StartLogging", "us-west-2", &json!({"Name": "multi-region"}))
        .await
        .expect_ok("StartLogging in the home Region");
}

/// `ListTrails` reports every trail in the account, each with its own home Region,
/// and hands out a `NextToken` for the trails past one page.
#[tokio::test]
async fn list_trails_pages_through_every_trail_in_the_account() {
    let emulator = Emulator::new();
    let page_size =
        service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::LIST_TRAILS_PAGE_SIZE;
    let total = page_size + 1;
    // Zero-padded so the name order is the numeric order.
    let expected: Vec<String> = (0..total).map(|i| format!("trail-{i:04}")).collect();
    for (index, name) in expected.iter().enumerate() {
        // Half the trails are created in another Region: `ListTrails` is
        // account-wide, unlike `DescribeTrails`.
        let region = if index % 2 == 0 { HOME_REGION } else { "eu-west-1" };
        emulator
            .call_in(
                "CreateTrail",
                region,
                &json!({"Name": name, "S3BucketName": "bucket-list"}),
            )
            .await
            .expect_ok(&format!("CreateTrail {name}"));
    }

    let first = emulator.call("ListTrails", &json!({})).await;
    let first = first.expect_ok("ListTrails page 1").clone();
    assert_eq!(
        names(&first, "Trails"),
        expected[..page_size].to_vec(),
        "case ListTrails page 1: expected the first {page_size} trails, got {first:?}"
    );
    let token = first["NextToken"].as_str().map(str::to_string);
    assert!(
        token.is_some(),
        "case ListTrails page 1: expected a token for the trails past this page, got {first:?}"
    );

    let second = emulator
        .call("ListTrails", &json!({"NextToken": token}))
        .await;
    let second = second.expect_ok("ListTrails page 2").clone();
    assert_eq!(
        names(&second, "Trails"),
        expected[page_size..].to_vec(),
        "case ListTrails page 2: expected the remaining trails, got {second:?}"
    );
    assert!(
        second.get("NextToken").is_none(),
        "case ListTrails page 2: expected no token on the last page, got {second:?}"
    );

    // Each entry carries the ARN and home Region of its own trail.
    let entry = second["Trails"][0].clone();
    assert_eq!(
        (&entry["Name"], &entry["HomeRegion"], &entry["TrailARN"]),
        (
            &json!(expected[page_size]),
            &json!(HOME_REGION),
            &json!(format!(
                "arn:aws:cloudtrail:{HOME_REGION}:000000000000:trail/{}",
                expected[page_size]
            ))
        ),
        "case ListTrails page 2: the entry does not describe its trail, got {entry:?}"
    );
}

/// A trail name belongs to its Region: the same name may be created in two
/// Regions, each trail is reachable only from its own Region or by its own ARN, and
/// deleting one leaves the other.
#[tokio::test]
async fn one_name_can_be_a_trail_in_each_region() {
    let emulator = Emulator::new();
    let regions = ["us-east-1", "eu-west-1", "ap-south-1"];
    for region in regions {
        let created = emulator
            .call_in(
                "CreateTrail",
                region,
                &json!({"Name": "shared-name", "S3BucketName": format!("bucket-{region}")}),
            )
            .await;
        let created = created.expect_ok(&format!("CreateTrail in {region}")).clone();
        assert_eq!(
            created["TrailARN"],
            json!(format!(
                "arn:aws:cloudtrail:{region}:000000000000:trail/shared-name"
            )),
            "case CreateTrail in {region}: expected a Region-qualified ARN, got {created:?}"
        );
    }

    // Each Region sees its own trail, by bare name and by its own ARN.
    for region in regions {
        let fetched = emulator
            .call_in("GetTrail", region, &json!({"Name": "shared-name"}))
            .await;
        let trail = fetched.expect_ok(&format!("GetTrail in {region}"))["Trail"].clone();
        assert_eq!(
            (&trail["HomeRegion"], &trail["S3BucketName"]),
            (&json!(region), &json!(format!("bucket-{region}"))),
            "case GetTrail in {region}: expected that Region's own trail, got {trail:?}"
        );
        let by_arn = emulator
            .call_in(
                "GetTrail",
                "us-east-1",
                &json!({
                    "Name": format!("arn:aws:cloudtrail:{region}:000000000000:trail/shared-name"),
                }),
            )
            .await;
        assert_eq!(
            by_arn.expect_ok(&format!("GetTrail {region} by ARN"))["Trail"]["S3BucketName"],
            json!(format!("bucket-{region}")),
            "case GetTrail {region} by ARN: expected that Region's own trail, got {by_arn:?}"
        );
    }

    // ListTrails is account-wide, so it carries all three.
    let listed = emulator.call("ListTrails", &json!({})).await;
    let listed = listed.expect_ok("ListTrails").clone();
    let homes: Vec<&str> = listed["Trails"]
        .as_array()
        .expect("an array of trails")
        .iter()
        .map(|trail| trail["HomeRegion"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        homes,
        vec!["ap-south-1", "eu-west-1", "us-east-1"],
        "case ListTrails: expected one trail per Region, got {listed:?}"
    );

    // Deleting one Region's trail leaves the others.
    emulator
        .call_in("DeleteTrail", "eu-west-1", &json!({"Name": "shared-name"}))
        .await
        .expect_ok("DeleteTrail in eu-west-1");
    emulator
        .call_in("GetTrail", "eu-west-1", &json!({"Name": "shared-name"}))
        .await
        .expect_error("GetTrail after the delete", 404, "TrailNotFoundException");
    for region in ["us-east-1", "ap-south-1"] {
        emulator
            .call_in("GetTrail", region, &json!({"Name": "shared-name"}))
            .await
            .expect_ok(&format!("GetTrail in {region} after the delete"));
    }
}

/// The registry is bounded: once it holds [`TRAIL_QUOTA`] trails, `CreateTrail`
/// refuses with the modelled `MaximumNumberOfTrailsExceededException` (HTTP 403)
/// until a trail is deleted.
#[tokio::test]
async fn the_trail_registry_is_bounded() {
    let emulator = Emulator::new();
    let quota = service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::TRAIL_QUOTA;
    for index in 0..quota {
        let name = format!("quota-{index:06}");
        emulator
            .call("CreateTrail", &json!({"Name": name, "S3BucketName": "bucket-quota"}))
            .await
            .expect_ok(&format!("CreateTrail {name}"));
    }

    // One past the bound, in the same Region and in another one: the bound is on
    // the store, not on a Region's share of it.
    for region in [HOME_REGION, "eu-west-1"] {
        emulator
            .call_in(
                "CreateTrail",
                region,
                &json!({"Name": "one-too-many", "S3BucketName": "bucket-quota"}),
            )
            .await
            .expect_error(
                &format!("CreateTrail past the quota in {region}"),
                403,
                "MaximumNumberOfTrailsExceededException",
            );
    }

    // Deleting a trail makes room again.
    emulator
        .call("DeleteTrail", &json!({"Name": "quota-000000"}))
        .await
        .expect_ok("DeleteTrail to make room");
    emulator
        .call(
            "CreateTrail",
            &json!({"Name": "one-too-many", "S3BucketName": "bucket-quota"}),
        )
        .await
        .expect_ok("CreateTrail after making room");
}

/// `CreateTrail` refuses the request members that break a documented bound, at the
/// bound and one past it, and persists nothing when it does.
#[tokio::test]
async fn create_trail_refuses_values_outside_their_documented_bounds() {
    let emulator = Emulator::new();
    let state = service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::S3_KEY_PREFIX_MAX_LEN;
    let topic = service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::SNS_TOPIC_NAME_MAX_LEN;
    let tags = service_com_amazonaws_cloudtrail_cloudtrail_20131101::state::TAGS_PER_RESOURCE_LIMIT;
    let tag_list = |count: usize| -> Value {
        (0..count)
            .map(|i| json!({"Key": format!("k{i}"), "Value": "v"}))
            .collect::<Vec<Value>>()
            .into()
    };

    // (case, request, the error code expected, or "" for a success)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "S3KeyPrefix at the maximum",
            json!({"Name": "prefix-ok", "S3BucketName": "b", "S3KeyPrefix": "p".repeat(state)}),
            "",
        ),
        (
            "S3KeyPrefix one over the maximum",
            json!({"Name": "prefix-long", "S3BucketName": "b", "S3KeyPrefix": "p".repeat(state + 1)}),
            "InvalidS3PrefixException",
        ),
        (
            "SnsTopicName at the maximum",
            json!({"Name": "topic-ok", "S3BucketName": "b", "SnsTopicName": "t".repeat(topic)}),
            "",
        ),
        (
            "SnsTopicName one over the maximum",
            json!({"Name": "topic-long", "S3BucketName": "b", "SnsTopicName": "t".repeat(topic + 1)}),
            "InvalidSnsTopicNameException",
        ),
        (
            "tags one over the limit",
            json!({"Name": "tags-many", "S3BucketName": "b", "TagsList": tag_list(tags + 1)}),
            "TagsLimitExceededException",
        ),
        (
            "a duplicated tag key",
            json!({
                "Name": "tags-dup",
                "S3BucketName": "b",
                "TagsList": [{"Key": "k", "Value": "1"}, {"Key": "k", "Value": "2"}],
            }),
            "InvalidTagParameterException",
        ),
    ];

    for (case, request, expected) in rows {
        let reply = emulator.call("CreateTrail", &request).await;
        if expected.is_empty() {
            reply.expect_ok(case);
        } else {
            reply.expect_error(case, 400, expected);
            // A refused create stores nothing.
            let name = request["Name"].as_str().expect("a name");
            emulator
                .call("GetTrail", &json!({"Name": name}))
                .await
                .expect_error(
                    &format!("{case}: GetTrail after the refusal"),
                    404,
                    "TrailNotFoundException",
                );
        }
    }
}
