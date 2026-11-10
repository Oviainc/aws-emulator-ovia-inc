// SPDX-License-Identifier: CC0-1.0

//! Directory assessments, driven end to end over HTTP: `StartADAssessment`,
//! `DescribeADAssessment`, `ListADAssessments` and `DeleteADAssessment`.
//!
//! Every case goes through the front door a client uses — `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! through [`crate::harness`] — so nothing here can pass on a response that was not
//! computed from persisted state.
//!
//! The cases assert RULES over sets of inputs rather than one recorded call: the
//! assessment record over a table of start requests that differ in input form
//! (`AssessmentConfiguration` vs `DirectoryId`), in DNS-IP/instance-id pairing order,
//! in managed-node id form (`i-` vs `mi-`), in resource-id width (8 vs 17 hex digits)
//! and in whether a security group was given; the exactly-one-input rule over every
//! combination of the two mutually exclusive members; the clock-owed status transition
//! over both input forms; pagination over a table of page sizes spanning 1, several,
//! exactly the record count and past it; the delete over an in-flight and a settled
//! record; and the account quota at its published bound and one past it.

use std::collections::BTreeSet;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::PAST_ONE_SPAN;
use crate::harness::{call, ok, refused, service, string};

/// A password satisfying the modelled `Password` pattern, so the `CreateDirectory`
/// calls a `DirectoryId`-form assessment needs are well formed. Never asserted on.
const PASSWORD: &str = "Passw0rd!2345";

/// The status an assessment carries while the run it started is still owed by the
/// clock: an `Assessment$Status` value ("Valid values include SUCCESS, FAILED, PENDING,
/// and IN_PROGRESS").
const STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// The terminal status an assessment reaches here, from the same value list: this
/// emulator reaches no self-managed AD environment, so the run it starts cannot be
/// performed and ends in the modelled failure status with a reason that says so.
const STATUS_FAILED: &str = "FAILED";

/// The `ReportType` of a caller-started assessment, from `Assessment$ReportType`'s
/// documentation: "Valid values are CUSTOMER and SYSTEM."
const REPORT_TYPE_CUSTOMER: &str = "CUSTOMER";

/// The account quota `StartADAssessment`'s documentation states: "Your Amazon Web
/// Services account has a limit of 100 CUSTOMER directory assessments."
const ASSESSMENT_QUOTA: usize = 100;

/// The modelled `AssessmentId` pattern, `^da-[0-9a-f]{18}$`, as an assertion.
fn assert_assessment_id(id: &str, case: &str) {
    let suffix = id.strip_prefix("da-").unwrap_or_default();
    assert!(
        suffix.len() == 18
            && suffix
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "case {case:?}: expected an AssessmentId matching ^da-[0-9a-f]{{18}}$, got {id:?}"
    );
}

/// `StartADAssessment` with `body`, asserting it was accepted and returning the id.
async fn start(router: &MultiServiceRouter, body: Value, case: &str) -> String {
    let answer = ok(router, "StartADAssessment", body, case).await;
    let id = string(&answer, "AssessmentId", case);
    assert_assessment_id(&id, case);
    id
}

/// The `Assessment` `DescribeADAssessment` reports for `assessment_id`, with the
/// `AssessmentReports` list checked to be present and empty: no validation ran, so
/// there are no per-domain-controller outcomes to report.
async fn describe(router: &MultiServiceRouter, assessment_id: &str, case: &str) -> Value {
    let answer = ok(
        router,
        "DescribeADAssessment",
        json!({ "AssessmentId": assessment_id }),
        case,
    )
    .await;
    let reports = answer["AssessmentReports"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected AssessmentReports, got {answer}"));
    assert!(
        reports.is_empty(),
        "case {case:?}: expected no validation reports for a run nothing performed, got {reports:?}"
    );
    answer["Assessment"].clone()
}

/// The `Assessments` summaries of a `ListADAssessments` answer.
fn summaries(body: &Value, case: &str) -> Vec<Value> {
    body["Assessments"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected Assessments, got {body}"))
        .clone()
}

/// Every stored assessment, unfiltered and unpaginated.
async fn list_all(router: &MultiServiceRouter, case: &str) -> Vec<Value> {
    let answer = ok(router, "ListADAssessments", json!({}), case).await;
    summaries(&answer, case)
}

/// A well-formed `AssessmentConfiguration`, as the `DnsName`/`CustomerDnsIps`/
/// `InstanceIds` trio the model requires.
fn configuration(dns_name: &str, dns_ips: [&str; 2], instance_ids: [&str; 2]) -> Value {
    json!({
        "DnsName": dns_name,
        "CustomerDnsIps": [dns_ips[0], dns_ips[1]],
        "InstanceIds": [instance_ids[0], instance_ids[1]],
        "VpcSettings": {
            "VpcId": "vpc-0a1b2c3d",
            "SubnetIds": ["subnet-0a1b2c3d", "subnet-1a2b3c4d"]
        }
    })
}

/// One row of the start table: a request and what the stored assessment must report.
struct StartCase {
    name: &'static str,
    body: Value,
    expect: Vec<(&'static str, Value)>,
}

/// Create a directory with VPC settings, returning its id — the subject of a
/// `DirectoryId`-form assessment.
async fn directory_with_vpc_settings(
    router: &MultiServiceRouter,
    name: &str,
    case: &str,
) -> String {
    let answer = ok(
        router,
        "CreateDirectory",
        json!({
            "Name": name,
            "Password": PASSWORD,
            "Size": "Small",
            "VpcSettings": {
                "VpcId": "vpc-09f8e7d6c5b4a3210",
                "SubnetIds": ["subnet-09f8e7d6c5b4a3210", "subnet-01234567890abcdef"]
            }
        }),
        case,
    )
    .await;
    string(&answer, "DirectoryId", case)
}

#[tokio::test]
async fn a_started_assessment_is_reported_from_persisted_state() {
    let router = service();
    let directory_id = directory_with_vpc_settings(&router, "corp.example.com", "directory subject").await;

    // The rows differ along everything `StartADAssessment` persists: which of the two
    // mutually exclusive input forms was used, the pairing order of the DNS IPs and the
    // managed node ids, the managed node id form (`i-` and `mi-`), the resource id
    // width (8 and 17 hex digits) and whether a security group was given.
    let cases = vec![
        StartCase {
            name: "configuration, ips and instances in matching order",
            body: json!({
                "AssessmentConfiguration": configuration(
                    "selfmanaged.example.com",
                    ["10.24.34.100", "10.24.34.200"],
                    ["i-10243410", "i-10243420"],
                )
            }),
            expect: vec![
                ("DnsName", json!("selfmanaged.example.com")),
                ("CustomerDnsIps", json!(["10.24.34.100", "10.24.34.200"])),
                ("SelfManagedInstanceIds", json!(["i-10243410", "i-10243420"])),
                ("VpcId", json!("vpc-0a1b2c3d")),
                ("SubnetIds", json!(["subnet-0a1b2c3d", "subnet-1a2b3c4d"])),
            ],
        },
        StartCase {
            name: "configuration, both lists in the reversed pairing",
            body: json!({
                "AssessmentConfiguration": configuration(
                    "reversed.example.org",
                    ["10.24.34.200", "10.24.34.100"],
                    ["i-10243420", "i-10243410"],
                )
            }),
            expect: vec![
                // The one-to-one positional correspondence the operation's
                // documentation requires is preserved exactly as sent, not sorted.
                ("CustomerDnsIps", json!(["10.24.34.200", "10.24.34.100"])),
                ("SelfManagedInstanceIds", json!(["i-10243420", "i-10243410"])),
            ],
        },
        StartCase {
            name: "configuration with mi- nodes, 17-hex ids and a security group",
            body: json!({
                "AssessmentConfiguration": {
                    "DnsName": "hybrid-ad.example.net",
                    "CustomerDnsIps": ["192.168.0.2", "192.168.1.2"],
                    "InstanceIds": ["mi-0a1b2c3d4e5f67890", "mi-00112233445566778"],
                    "VpcSettings": {
                        "VpcId": "vpc-0a1b2c3d4e5f67890",
                        "SubnetIds": ["subnet-0a1b2c3d4e5f67890"]
                    },
                    "SecurityGroupIds": ["sg-0a1b2c3d4e5f67890"]
                }
            }),
            expect: vec![
                ("DnsName", json!("hybrid-ad.example.net")),
                (
                    "SelfManagedInstanceIds",
                    json!(["mi-0a1b2c3d4e5f67890", "mi-00112233445566778"]),
                ),
                ("VpcId", json!("vpc-0a1b2c3d4e5f67890")),
                ("SubnetIds", json!(["subnet-0a1b2c3d4e5f67890"])),
                ("SecurityGroupIds", json!(["sg-0a1b2c3d4e5f67890"])),
            ],
        },
        StartCase {
            name: "existing directory",
            body: json!({ "DirectoryId": directory_id.clone() }),
            expect: vec![
                ("DirectoryId", json!(directory_id.clone())),
                // The directory's own fully qualified name and VPC settings are what
                // the assessment of an existing directory is run against.
                ("DnsName", json!("corp.example.com")),
                ("VpcId", json!("vpc-09f8e7d6c5b4a3210")),
                (
                    "SubnetIds",
                    json!(["subnet-09f8e7d6c5b4a3210", "subnet-01234567890abcdef"]),
                ),
            ],
        },
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for case in &cases {
        let assessment_id = start(&router, case.body.clone(), case.name).await;
        assert!(
            minted.insert(assessment_id.clone()),
            "case {:?}: expected a fresh AssessmentId, got the already-minted {assessment_id}",
            case.name
        );

        let described = describe(&router, &assessment_id, case.name).await;
        assert_eq!(
            described["AssessmentId"],
            json!(assessment_id),
            "case {:?}: expected the assessment to report its own id, got {described}",
            case.name
        );
        for (member, expected) in &case.expect {
            assert_eq!(
                described[member], *expected,
                "case {:?}: expected {member} to be {expected}, got {described}",
                case.name
            );
        }
        assert_eq!(
            described["ReportType"],
            json!(REPORT_TYPE_CUSTOMER),
            "case {:?}: expected a caller-started assessment to be the CUSTOMER kind, got {described}",
            case.name
        );
        // The run is asynchronous, so the call answers with it still in flight.
        assert_eq!(
            described["Status"],
            json!(STATUS_IN_PROGRESS),
            "case {:?}: expected the transient status, got {described}",
            case.name
        );
        assert!(
            described["StartTime"].is_number(),
            "case {:?}: expected a StartTime timestamp, got {described}",
            case.name
        );
        // Members this emulator holds no record behind are absent, never invented.
        for absent in ["StatusReason", "StatusCode", "Version"] {
            assert!(
                described[absent].is_null(),
                "case {:?}: expected no {absent} while the run is in flight, got {described}",
                case.name
            );
        }
        if case.body["DirectoryId"].is_null() {
            assert!(
                described["DirectoryId"].is_null(),
                "case {:?}: expected no DirectoryId for a configuration-form assessment, got {described}",
                case.name
            );
        }
    }

    // Every assessment started above is in the inventory the listing reports, in
    // creation order, and each summary agrees with the detailed record.
    let listed = list_all(&router, "inventory").await;
    let listed_ids: Vec<String> = listed
        .iter()
        .map(|summary| string(summary, "AssessmentId", "inventory"))
        .collect();
    assert_eq!(
        listed_ids.len(),
        cases.len(),
        "case {:?}: expected every started assessment listed, got {listed:?}",
        "inventory"
    );
    assert_eq!(
        listed_ids.iter().cloned().collect::<BTreeSet<String>>(),
        minted,
        "case {:?}: expected the listing to report exactly the minted ids, got {listed_ids:?}",
        "inventory"
    );
    for summary in &listed {
        let assessment_id = string(summary, "AssessmentId", "inventory");
        let described = describe(&router, &assessment_id, "inventory").await;
        // `AssessmentSummary` is the same record "without detailed validation results",
        // so every member the two shapes share reports the same value.
        for member in [
            "AssessmentId",
            "DirectoryId",
            "DnsName",
            "StartTime",
            "LastUpdateDateTime",
            "Status",
            "CustomerDnsIps",
            "ReportType",
        ] {
            assert_eq!(
                summary[member], described[member],
                "case {:?}: expected the summary's {member} to match the described record, \
                 got {summary} and {described}",
                assessment_id
            );
        }
    }
}

#[tokio::test]
async fn the_status_transition_the_start_owes_falls_due_on_the_clock() {
    let router = service();
    let directory_id = directory_with_vpc_settings(&router, "clock.example.com", "clock subject").await;

    // Both input forms owe the same transition, so both rows are checked.
    let cases = [
        (
            "configuration",
            json!({
                "AssessmentConfiguration": configuration(
                    "clock.example.net",
                    ["10.0.0.10", "10.0.1.10"],
                    ["i-0a1b2c3d", "i-1a2b3c4d"],
                )
            }),
        ),
        (
            "existing directory",
            json!({ "DirectoryId": directory_id.clone() }),
        ),
    ];

    for (case, body) in &cases {
        let assessment_id = start(&router, body.clone(), case).await;
        let fresh = describe(&router, &assessment_id, case).await;
        assert_eq!(
            fresh["Status"],
            json!(STATUS_IN_PROGRESS),
            "case {case:?}: expected the transient status immediately after the start, got {fresh}"
        );

        tokio::time::sleep(PAST_ONE_SPAN).await;

        let settled = describe(&router, &assessment_id, case).await;
        assert_eq!(
            settled["Status"],
            json!(STATUS_FAILED),
            "case {case:?}: expected the terminal status the clock owed, got {settled}"
        );
        let reason = settled["StatusReason"].as_str().unwrap_or_default();
        assert!(
            !reason.is_empty(),
            "case {case:?}: expected a StatusReason explaining the failure, got {settled}"
        );
        let started = fresh["StartTime"].as_f64().unwrap_or_default();
        let updated = settled["LastUpdateDateTime"].as_f64().unwrap_or_default();
        assert!(
            updated >= started,
            "case {case:?}: expected LastUpdateDateTime ({updated}) at or after StartTime ({started})"
        );
        // Reading twice does not move the status on again, and the configuration the
        // start persisted is unchanged by the transition.
        let again = describe(&router, &assessment_id, case).await;
        assert_eq!(
            again, settled,
            "case {case:?}: expected a second read to report the same record, got {again}"
        );
    }
}

#[tokio::test]
async fn a_start_names_exactly_one_subject() {
    let router = service();
    let directory_id = directory_with_vpc_settings(&router, "exactly.example.com", "one subject").await;
    let config = configuration(
        "exactly.example.net",
        ["10.1.0.10", "10.1.1.10"],
        ["i-0a1b2c3d", "i-1a2b3c4d"],
    );

    // "You must provide exactly one DirectoryId or AssessmentConfiguration": the whole
    // combination table of the two mutually exclusive members.
    let cases = [
        ("neither member", json!({}), "InvalidParameterException"),
        (
            "both members",
            json!({ "DirectoryId": directory_id.clone(), "AssessmentConfiguration": config.clone() }),
            "InvalidParameterException",
        ),
        (
            // Pattern-valid, so the boundary validator passes it, but it names no
            // stored directory.
            "a directory that does not exist",
            json!({ "DirectoryId": "d-0123456789" }),
            "DirectoryDoesNotExistException",
        ),
    ];
    for (case, body, expected) in &cases {
        refused(&router, "StartADAssessment", body.clone(), expected, case).await;
    }

    // Neither refusal persisted anything: only the one accepted start is stored.
    let accepted = start(
        &router,
        json!({ "AssessmentConfiguration": config }),
        "the accepted start",
    )
    .await;
    let listed = list_all(&router, "after the refusals").await;
    let ids: Vec<String> = listed
        .iter()
        .map(|summary| string(summary, "AssessmentId", "after the refusals"))
        .collect();
    assert_eq!(
        ids,
        vec![accepted],
        "case {:?}: expected only the accepted start to be stored, got {listed:?}",
        "after the refusals"
    );
}

#[tokio::test]
async fn describing_an_unknown_assessment_is_refused() {
    let router = service();
    // Pattern-valid `AssessmentId`s (`^da-[0-9a-f]{18}$`) that name no stored
    // assessment: the lowest and the highest the pattern admits, and one in between.
    let cases = [
        "da-000000000000000000",
        "da-0123456789abcdef01",
        "da-ffffffffffffffffff",
    ];
    for case in &cases {
        refused(
            &router,
            "DescribeADAssessment",
            json!({ "AssessmentId": case }),
            "EntityDoesNotExistException",
            case,
        )
        .await;
    }
}

#[tokio::test]
async fn the_listing_pages_over_every_assessment_in_creation_order() {
    let router = service();

    // Four assessments, so a page size of 1, 2, 4 (exactly the record count) and 5
    // (past it) all have something to say.
    let mut expected_order: Vec<String> = Vec::new();
    for index in 0..4u8 {
        let case = format!("assessment {index}");
        let id = start(
            &router,
            json!({
                "AssessmentConfiguration": configuration(
                    &format!("page{index}.example.com"),
                    ["10.2.0.10", "10.2.1.10"],
                    ["i-0a1b2c3d", "i-1a2b3c4d"],
                )
            }),
            &case,
        )
        .await;
        expected_order.push(id);
    }

    for limit in [1usize, 2, 4, 5] {
        let case = format!("Limit {limit}");
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({ "Limit": limit });
            if let Some(token) = next_token.as_deref() {
                body["NextToken"] = json!(token);
            }
            let answer = ok(&router, "ListADAssessments", body, &case).await;
            let page = summaries(&answer, &case);
            assert!(
                page.len() <= limit,
                "case {case:?}: expected at most {limit} summaries per page, got {page:?}"
            );
            walked.extend(
                page.iter()
                    .map(|summary| string(summary, "AssessmentId", &case)),
            );
            pages += 1;
            assert!(
                pages <= expected_order.len() + 1,
                "case {case:?}: expected pagination to terminate, walked {walked:?}"
            );
            match answer["NextToken"].as_str() {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, expected_order,
            "case {case:?}: expected every assessment once, in creation order"
        );
    }

    // A token this listing did not issue is refused rather than served as a page of
    // it — including one minted by the other paginated listing of this service.
    let foreign = ok(
        &router,
        "DescribeDirectories",
        json!({ "Limit": 1 }),
        "a foreign token",
    )
    .await;
    let mut bad_tokens = vec!["not-a-token".to_string(), "6e6f70".to_string()];
    if let Some(token) = foreign["NextToken"].as_str() {
        bad_tokens.push(token.to_string());
    }
    for token in &bad_tokens {
        refused(
            &router,
            "ListADAssessments",
            json!({ "NextToken": token }),
            "InvalidParameterException",
            token,
        )
        .await;
    }
}

#[tokio::test]
async fn the_listing_narrows_to_one_directory() {
    let router = service();
    let first = directory_with_vpc_settings(&router, "first.example.com", "filter subjects").await;
    let second = directory_with_vpc_settings(&router, "second.example.com", "filter subjects").await;

    let for_first = start(
        &router,
        json!({ "DirectoryId": first.clone() }),
        "first directory",
    )
    .await;
    let for_second = start(
        &router,
        json!({ "DirectoryId": second.clone() }),
        "second directory",
    )
    .await;
    let unattached = start(
        &router,
        json!({
            "AssessmentConfiguration": configuration(
                "unattached.example.com",
                ["10.3.0.10", "10.3.1.10"],
                ["i-0a1b2c3d", "i-1a2b3c4d"],
            )
        }),
        "no directory",
    )
    .await;

    // Each filter reports exactly the assessments of that directory; an unfiltered
    // listing reports all three, including the one attached to no directory.
    let cases = [
        (first.clone(), vec![for_first.clone()]),
        (second.clone(), vec![for_second.clone()]),
    ];
    for (directory_id, expected) in &cases {
        let answer = ok(
            &router,
            "ListADAssessments",
            json!({ "DirectoryId": directory_id }),
            directory_id,
        )
        .await;
        let ids: Vec<String> = summaries(&answer, directory_id)
            .iter()
            .map(|summary| string(summary, "AssessmentId", directory_id))
            .collect();
        assert_eq!(
            &ids, expected,
            "case {directory_id:?}: expected only that directory's assessments, got {answer}"
        );
    }

    let all: Vec<String> = list_all(&router, "unfiltered")
        .await
        .iter()
        .map(|summary| string(summary, "AssessmentId", "unfiltered"))
        .collect();
    assert_eq!(
        all,
        vec![for_first, for_second, unattached],
        "case {:?}: expected every assessment, in creation order",
        "unfiltered"
    );

    // A filter naming no stored directory is reported, not served as an empty page.
    refused(
        &router,
        "ListADAssessments",
        json!({ "DirectoryId": "d-0123456789" }),
        "DirectoryDoesNotExistException",
        "a directory that does not exist",
    )
    .await;
}

#[tokio::test]
async fn deleting_an_assessment_removes_it_from_the_inventory() {
    let router = service();
    let directory_id = directory_with_vpc_settings(&router, "deletes.example.com", "delete subjects").await;

    // Three assessments spanning both input forms, so the one deleted is removed from
    // the middle of the listing rather than from an end.
    let first = start(
        &router,
        json!({
            "AssessmentConfiguration": configuration(
                "delete-first.example.com",
                ["10.6.0.10", "10.6.1.10"],
                ["i-0a1b2c3d", "i-1a2b3c4d"],
            )
        }),
        "first",
    )
    .await;
    let middle = start(
        &router,
        json!({ "DirectoryId": directory_id.clone() }),
        "middle",
    )
    .await;
    let last = start(
        &router,
        json!({
            "AssessmentConfiguration": configuration(
                "delete-last.example.com",
                ["10.6.2.10", "10.6.3.10"],
                ["mi-0a1b2c3d", "mi-1a2b3c4d"],
            )
        }),
        "last",
    )
    .await;

    // "You can delete customer-created assessments even if they are in progress": this
    // one is deleted while its clock-owed transition is still pending.
    let answer = ok(
        &router,
        "DeleteADAssessment",
        json!({ "AssessmentId": middle }),
        "an in-progress assessment",
    )
    .await;
    assert_eq!(
        answer["AssessmentId"],
        json!(middle),
        "case {:?}: expected the deleted id reported back, got {answer}",
        "an in-progress assessment"
    );

    // Gone from both readers, and the rest of the inventory is untouched.
    refused(
        &router,
        "DescribeADAssessment",
        json!({ "AssessmentId": middle }),
        "EntityDoesNotExistException",
        "the deleted assessment",
    )
    .await;
    let remaining: Vec<String> = list_all(&router, "after the delete")
        .await
        .iter()
        .map(|summary| string(summary, "AssessmentId", "after the delete"))
        .collect();
    assert_eq!(
        remaining,
        vec![first.clone(), last.clone()],
        "case {:?}: expected the surviving assessments, in creation order",
        "after the delete"
    );

    // A second delete of the same id, and a delete of an id that never existed, are
    // both reported rather than silently accepted.
    for case in [middle.as_str(), "da-abcdefabcdefabcdef"] {
        refused(
            &router,
            "DeleteADAssessment",
            json!({ "AssessmentId": case }),
            "EntityDoesNotExistException",
            case,
        )
        .await;
    }

    // A settled assessment deletes the same way an in-flight one does.
    tokio::time::sleep(PAST_ONE_SPAN).await;
    let settled = describe(&router, &last, "a settled assessment").await;
    assert_eq!(
        settled["Status"],
        json!(STATUS_FAILED),
        "case {:?}: expected the terminal status before the delete, got {settled}",
        "a settled assessment"
    );
    ok(
        &router,
        "DeleteADAssessment",
        json!({ "AssessmentId": last }),
        "a settled assessment",
    )
    .await;
    let left: Vec<String> = list_all(&router, "after both deletes")
        .await
        .iter()
        .map(|summary| string(summary, "AssessmentId", "after both deletes"))
        .collect();
    assert_eq!(
        left,
        vec![first],
        "case {:?}: expected only the undeleted assessment, got {left:?}",
        "after both deletes"
    );
}

#[tokio::test]
async fn the_account_quota_bounds_the_assessments_that_can_be_started() {
    let router = service();

    // Up to the published bound every start is accepted...
    for index in 0..ASSESSMENT_QUOTA {
        let case = format!("assessment {index} of {ASSESSMENT_QUOTA}");
        start(
            &router,
            json!({
                "AssessmentConfiguration": configuration(
                    &format!("quota{index}.example.com"),
                    ["10.4.0.10", "10.4.1.10"],
                    ["i-0a1b2c3d", "i-1a2b3c4d"],
                )
            }),
            &case,
        )
        .await;
    }

    // ... and the one past it is refused.
    let past_the_bound = format!("assessment {} of {ASSESSMENT_QUOTA}", ASSESSMENT_QUOTA + 1);
    refused(
        &router,
        "StartADAssessment",
        json!({
            "AssessmentConfiguration": configuration(
                "past-the-bound.example.com",
                ["10.4.0.10", "10.4.1.10"],
                ["i-0a1b2c3d", "i-1a2b3c4d"],
            )
        }),
        "ADAssessmentLimitExceededException",
        &past_the_bound,
    )
    .await;

    // The refusal persisted nothing: the inventory still holds exactly the bound.
    let answer = ok(
        &router,
        "ListADAssessments",
        json!({ "Limit": ASSESSMENT_QUOTA }),
        "at the bound",
    )
    .await;
    assert_eq!(
        summaries(&answer, "at the bound").len(),
        ASSESSMENT_QUOTA,
        "case {:?}: expected exactly the bound stored, got {answer}",
        "at the bound"
    );

    // "Delete assessments to free up capacity before trying again": one delete makes
    // room for exactly one more start.
    let freed = string(
        &summaries(&answer, "at the bound")[0],
        "AssessmentId",
        "freeing capacity",
    );
    ok(
        &router,
        "DeleteADAssessment",
        json!({ "AssessmentId": freed }),
        "freeing capacity",
    )
    .await;
    start(
        &router,
        json!({
            "AssessmentConfiguration": configuration(
                "after-the-delete.example.com",
                ["10.4.0.10", "10.4.1.10"],
                ["i-0a1b2c3d", "i-1a2b3c4d"],
            )
        }),
        "freeing capacity",
    )
    .await;
    refused(
        &router,
        "StartADAssessment",
        json!({
            "AssessmentConfiguration": configuration(
                "past-the-bound-again.example.com",
                ["10.4.0.10", "10.4.1.10"],
                ["i-0a1b2c3d", "i-1a2b3c4d"],
            )
        }),
        "ADAssessmentLimitExceededException",
        "back at the bound",
    )
    .await;
}

#[tokio::test]
async fn a_malformed_start_is_refused_by_the_boundary() {
    let router = service();

    // The modelled constraints of `AssessmentConfiguration`: `CustomerDnsIps` and
    // `InstanceIds` are lists of exactly 2, `SecurityGroupIds` of exactly 1, and each
    // member has its own pattern. A request breaking one is refused BEFORE the handler
    // runs, so nothing is persisted.
    let cases = [
        (
            "one DNS IP",
            json!({
                "AssessmentConfiguration": {
                    "DnsName": "short.example.com",
                    "CustomerDnsIps": ["10.5.0.10"],
                    "InstanceIds": ["i-0a1b2c3d", "i-1a2b3c4d"],
                    "VpcSettings": { "VpcId": "vpc-0a1b2c3d", "SubnetIds": ["subnet-0a1b2c3d"] }
                }
            }),
        ),
        (
            "three managed nodes",
            json!({
                "AssessmentConfiguration": {
                    "DnsName": "long.example.com",
                    "CustomerDnsIps": ["10.5.0.10", "10.5.1.10"],
                    "InstanceIds": ["i-0a1b2c3d", "i-1a2b3c4d", "i-2a3b4c5d"],
                    "VpcSettings": { "VpcId": "vpc-0a1b2c3d", "SubnetIds": ["subnet-0a1b2c3d"] }
                }
            }),
        ),
        (
            "a DNS IP that is not an address",
            json!({
                "AssessmentConfiguration": {
                    "DnsName": "bad-ip.example.com",
                    "CustomerDnsIps": ["10.5.0.10", "999.1.1.1"],
                    "InstanceIds": ["i-0a1b2c3d", "i-1a2b3c4d"],
                    "VpcSettings": { "VpcId": "vpc-0a1b2c3d", "SubnetIds": ["subnet-0a1b2c3d"] }
                }
            }),
        ),
        (
            "a VPC id of the wrong width",
            json!({
                "AssessmentConfiguration": {
                    "DnsName": "bad-vpc.example.com",
                    "CustomerDnsIps": ["10.5.0.10", "10.5.1.10"],
                    "InstanceIds": ["i-0a1b2c3d", "i-1a2b3c4d"],
                    "VpcSettings": { "VpcId": "vpc-0a1b2c", "SubnetIds": ["subnet-0a1b2c3d"] }
                }
            }),
        ),
    ];
    for (case, body) in &cases {
        let (status, parsed) = call(&router, "StartADAssessment", body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a malformed configuration to be refused, got {status} with {parsed}"
        );
    }

    let listed = list_all(&router, "after the malformed starts").await;
    assert!(
        listed.is_empty(),
        "case {:?}: expected a refused start to persist nothing, got {listed:?}",
        "after the malformed starts"
    );
}
