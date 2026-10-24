// SPDX-License-Identifier: CC0-1.0

//! `StartTagSyncTask` and `GetTagSyncTask`, end to end.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the sibling suite's `Emulator`), so these exercise the
//! whole restJson1 path: routing, boundary validation, the handler, and serialization.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! * starting a tag-sync task PERSISTS it on the group the request addressed and reports
//!   it back from the stored record: the group's own stored ARN and name, what the task
//!   syncs (a tag key-value pair or a resource query, verbatim), the role it was given,
//!   and a task ARN nested inside that group's ARN;
//! * the task ARN is the identifier a later read addresses the task by, so
//!   `GetTagSyncTask` answers the same task for it, with the status the model declares
//!   for a task that is managing resources and the time it was started;
//! * a group identifier that names no stored group is `NotFoundException` (404), a task
//!   ARN that addresses no stored task likewise, and a request that names neither, both,
//!   or half of the two ways a task can sync is `BadRequestException` (400);
//! * reading a task changes nothing: repeated reads answer the same record.

use serde_json::{json, Value};

use super::resource_group_lifecycle::{error_code, Emulator, ACCOUNT};

/// Every value the modelled `QueryType` enum declares, which is the domain of the
/// `Type` a tag-sync task's stored query can carry.
///
/// Source: `smithy_query(query="shape",shape="QueryType")` on this service's model,
/// which answers `["TAG_FILTERS_1_0", "CLOUDFORMATION_STACK_1_0"]`.
const QUERY_TYPE_VALUES: [&str; 2] = ["TAG_FILTERS_1_0", "CLOUDFORMATION_STACK_1_0"];

/// A role ARN shaped like the modelled `RoleArn`
/// (`^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`).
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

/// The ARN every operation of this service reports for the group `name` in `region`.
fn group_arn(region: &str, name: &str) -> String {
    format!("arn:aws:resource-groups:{region}:{ACCOUNT}:group/{name}")
}

/// How a task's two ways of syncing are sent, and what the task then reports.
#[derive(Debug, Clone)]
enum Sync {
    /// `TagKey` and `TagValue`: resources carrying the pair are synced.
    Pair { key: &'static str, value: &'static str },
    /// `ResourceQuery`: resources the query matches are synced.
    Query { query_type: &'static str, query: &'static str },
}

/// One tag-sync task to start: the rows differ in what the task syncs, in the query
/// type and document when it syncs a query, in the tag value's size when it syncs a
/// pair, in the region the group lives in, in the role given, and in the identifier
/// form the group is addressed by — which is what the reporting rule generalises over.
#[derive(Debug, Clone)]
struct TaskCase {
    label: &'static str,
    region: &'static str,
    group: &'static str,
    /// Whether the request addresses the group by its stored ARN rather than by name.
    address_by_arn: bool,
    role: &'static str,
    sync: Sync,
}

fn task_cases() -> Vec<TaskCase> {
    vec![
        TaskCase {
            label: "a tag key-value pair, group addressed by name",
            region: "us-east-1",
            group: "app-one",
            address_by_arn: false,
            role: "ResourceGroupsTagSync",
            sync: Sync::Pair { key: "Stage", value: "Test" },
        },
        TaskCase {
            label: "a tag pair whose value is empty, which the model admits (len 0..256)",
            region: "eu-west-1",
            group: "app.two_2",
            address_by_arn: false,
            role: "tag-sync/with-a-path",
            sync: Sync::Pair { key: "Team", value: "" },
        },
        TaskCase {
            label: "a tag pair addressed by the group's stored ARN",
            region: "ap-southeast-2",
            group: "app-three",
            address_by_arn: true,
            role: "ResourceGroupsTagSync",
            sync: Sync::Pair {
                key: "cost:center",
                value: "platform engineering",
            },
        },
        TaskCase {
            label: "a tag-filters resource query with several filters",
            region: "us-west-2",
            group: "app-four",
            address_by_arn: false,
            role: "AnotherTagSyncRole",
            sync: Sync::Query {
                query_type: "TAG_FILTERS_1_0",
                query: r#"{"ResourceTypeFilters":["AWS::EC2::Instance","AWS::S3::Bucket"],"TagFilters":[{"Key":"Stage","Values":["Test","Prod"]}]}"#,
            },
        },
        TaskCase {
            label: "a cloudformation-stack resource query, group addressed by its ARN",
            region: "us-east-2",
            group: "app-five",
            address_by_arn: true,
            role: "ResourceGroupsTagSync",
            sync: Sync::Query {
                query_type: "CLOUDFORMATION_STACK_1_0",
                query: r#"{"StackIdentifier":"arn:aws:cloudformation:us-east-2:000000000000:stack/demo/0"}"#,
            },
        },
        TaskCase {
            label: "a resource query whose document matches nothing",
            region: "eu-central-1",
            group: "app-six",
            address_by_arn: false,
            role: "ResourceGroupsTagSync",
            sync: Sync::Query {
                query_type: "TAG_FILTERS_1_0",
                query: "{}",
            },
        },
    ]
}

impl TaskCase {
    /// The members this case sends beside `Group` and `RoleArn`.
    fn sync_members(&self) -> Value {
        match &self.sync {
            Sync::Pair { key, value } => json!({ "TagKey": key, "TagValue": value }),
            Sync::Query { query_type, query } => {
                json!({ "ResourceQuery": { "Type": query_type, "Query": query } })
            }
        }
    }

    /// The `StartTagSyncTask` request body for this case.
    fn request(&self) -> Value {
        let identifier = if self.address_by_arn {
            group_arn(self.region, self.group)
        } else {
            self.group.to_string()
        };
        let mut body = json!({ "Group": identifier, "RoleArn": role_arn(self.role) });
        let object = body.as_object_mut().expect("a JSON object");
        for (member, value) in self.sync_members().as_object().expect("a JSON object") {
            object.insert(member.clone(), value.clone());
        }
        body
    }
}

/// Whether `id` is one of the 26-character ids the modelled ARNs are written with
/// (`[a-z0-9]{26}`).
///
/// Source: `smithy_query(query="shape",shape="TagSyncTaskArn")`.
fn is_modelled_id(id: &str) -> bool {
    id.len() == 26
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// The 26-character id qualifying `group_arn` inside `task_arn`, when `task_arn` is
/// shaped like the modelled `TagSyncTaskArn` for a task of that group:
/// `<group arn>/<26 chars>/tag-sync-task/<26 chars>`.
fn application_segment(task_arn: &str, group_arn: &str) -> Option<String> {
    let nested = task_arn.strip_prefix(group_arn)?.strip_prefix('/')?;
    let (application, task) = nested.split_once("/tag-sync-task/")?;
    (is_modelled_id(application) && is_modelled_id(task)).then(|| application.to_string())
}

/// Create the group `case` starts its task on.
async fn create_group(emulator: &Emulator, case: &TaskCase) {
    let (status, created) = emulator
        .call_in(case.region, "/groups", &json!({ "Name": case.group }))
        .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected CreateGroup to answer 200, got {status} {created}",
        case.label,
    );
}

/// Every row's query type is one the model declares, and the table covers both ways a
/// task can sync, so it spans the domain rather than one point of it.
#[test]
fn the_table_spans_both_sync_methods_and_every_declared_query_type() {
    assert!(
        task_cases()
            .iter()
            .any(|case| matches!(case.sync, Sync::Pair { .. })),
        "case \"table\": expected a row syncing a tag key-value pair, got {:?}",
        task_cases().iter().map(|case| case.label).collect::<Vec<_>>(),
    );
    for query_type in QUERY_TYPE_VALUES {
        assert!(
            task_cases().iter().any(|case| matches!(
                case.sync,
                Sync::Query { query_type: declared, .. } if declared == query_type
            )),
            "case {query_type:?}: expected a row syncing a query of that declared type, \
             got {:?}",
            task_cases().iter().map(|case| case.label).collect::<Vec<_>>(),
        );
    }
}

/// A started task is persisted on the addressed group and read back, by its minted
/// task ARN, exactly as it was stored.
#[tokio::test]
async fn a_started_task_is_persisted_and_read_back() {
    let emulator = Emulator::new();
    for case in task_cases() {
        let label = case.label;
        create_group(&emulator, &case).await;
        let (status, started) = emulator
            .call_in(case.region, "/start-tag-sync-task", &case.request())
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected StartTagSyncTask to answer 200, got {status} {started}",
        );

        let arn = group_arn(case.region, case.group);
        let expected_role = role_arn(case.role);
        assert_eq!(
            started["GroupArn"].as_str(),
            Some(arn.as_str()),
            "case {label:?}: expected GroupArn {arn}, got {started}",
        );
        assert_eq!(
            started["GroupName"].as_str(),
            Some(case.group),
            "case {label:?}: expected GroupName {:?}, got {started}",
            case.group,
        );
        assert_eq!(
            started["RoleArn"].as_str(),
            Some(expected_role.as_str()),
            "case {label:?}: expected RoleArn {expected_role}, got {started}",
        );
        for (member, value) in case.sync_members().as_object().expect("a JSON object") {
            assert_eq!(
                &started[member], value,
                "case {label:?}: expected {member} to be reported as {value}, got {started}",
            );
        }
        // The members of the OTHER way of syncing are not reported for a task that
        // does not sync that way.
        for absent in absent_members(&case.sync) {
            assert!(
                started.get(absent).is_none(),
                "case {label:?}: expected no {absent} on a task that does not sync one, \
                 got {started}",
            );
        }
        let task_arn = started["TaskArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: expected a TaskArn, got {started}"))
            .to_string();
        assert!(
            application_segment(&task_arn, &arn).is_some(),
            "case {label:?}: expected a task ARN shaped like \
             <group arn>/<26 chars>/tag-sync-task/<26 chars> under {arn}, got {task_arn:?}",
        );

        // The task ARN is what a later read addresses the task by, and the read
        // reports the stored record: the same fields, the declared status of a task
        // that is managing resources, no error message, and the time it was started.
        let read = emulator
            .call_in(
                case.region,
                "/get-tag-sync-task",
                &json!({ "TaskArn": task_arn }),
            )
            .await;
        assert_eq!(
            read.0, 200,
            "case {label:?}: expected GetTagSyncTask to answer 200, got {} {}",
            read.0, read.1,
        );
        let task = read.1;
        assert_eq!(
            (
                task["GroupArn"].as_str(),
                task["GroupName"].as_str(),
                task["TaskArn"].as_str(),
                task["RoleArn"].as_str(),
                task["Status"].as_str(),
            ),
            (
                Some(arn.as_str()),
                Some(case.group),
                Some(task_arn.as_str()),
                Some(expected_role.as_str()),
                Some("ACTIVE"),
            ),
            "case {label:?}: expected the stored task reported back as ACTIVE, got {task}",
        );
        for (member, value) in case.sync_members().as_object().expect("a JSON object") {
            assert_eq!(
                &task[member], value,
                "case {label:?}: expected the read to report {member} as {value}, got {task}",
            );
        }
        assert!(
            task.get("ErrorMessage").is_none(),
            "case {label:?}: expected no ErrorMessage on a task that did not fail, \
             got {task}",
        );
        assert!(
            task["CreatedAt"].as_f64().is_some_and(|at| at > 0.0),
            "case {label:?}: expected CreatedAt to be a timestamp, got {task}",
        );

        // Reading is not a mutation: a repeated read in the task's own region
        // answers the same record.
        let again = emulator
            .call_in(
                case.region,
                "/get-tag-sync-task",
                &json!({ "TaskArn": task_arn }),
            )
            .await;
        assert_eq!(
            (again.0, &again.1),
            (200, &task),
            "case {label:?}: expected a repeated read to answer the same task, \
             got {} {}",
            again.0,
            again.1,
        );
        // This service is regional, so another region's endpoint sees neither the
        // group nor its tasks.
        let elsewhere = emulator
            .call_in(
                "us-west-1",
                "/get-tag-sync-task",
                &json!({ "TaskArn": task_arn }),
            )
            .await;
        assert_eq!(
            (elsewhere.0, error_code(&elsewhere.1)),
            (404, "NotFoundException"),
            "case {label:?}: expected another region to find no task, got {} {}",
            elsewhere.0,
            elsewhere.1,
        );
        // The group the task was started on, read back through its application-group
        // ARN — the form the task ARN nests the task inside — is the same group.
        let application_group_arn = format!(
            "{arn}/{}",
            application_segment(&task_arn, &arn).expect("a task ARN under the group"),
        );
        let by_application_arn = emulator
            .call_in(
                case.region,
                "/get-group",
                &json!({ "Group": application_group_arn }),
            )
            .await;
        assert_eq!(
            (
                by_application_arn.0,
                by_application_arn.1["Group"]["Name"].as_str()
            ),
            (200, Some(case.group)),
            "case {label:?}: expected {application_group_arn} to resolve to the group, \
             got {} {}",
            by_application_arn.0,
            by_application_arn.1,
        );
        // The group the task was started on is untouched by it.
        let group = emulator
            .call_in(case.region, "/get-group", &json!({ "Group": case.group }))
            .await;
        assert_eq!(
            group.1["Group"]["GroupArn"].as_str(),
            Some(arn.as_str()),
            "case {label:?}: expected the group to still read back as itself, got {}",
            group.1,
        );
    }
}

/// The members a task that syncs one way never reports.
fn absent_members(sync: &Sync) -> &'static [&'static str] {
    match sync {
        Sync::Pair { .. } => &["ResourceQuery"],
        Sync::Query { .. } => &["TagKey", "TagValue"],
    }
}

/// Every task started on one group gets its own ARN, nested under that group's one
/// application segment, and each reads back as itself.
#[tokio::test]
async fn tasks_on_one_group_get_distinct_arns_under_one_application_segment() {
    let emulator = Emulator::new();
    emulator.ok("/groups", &json!({ "Name": "shared-app" })).await;
    let arn = group_arn("us-east-1", "shared-app");
    let requests = [
        json!({ "Group": "shared-app", "RoleArn": role_arn("one"), "TagKey": "A", "TagValue": "1" }),
        json!({ "Group": "shared-app", "RoleArn": role_arn("two"), "TagKey": "A", "TagValue": "1" }),
        json!({
            "Group": arn,
            "RoleArn": role_arn("three"),
            "ResourceQuery": { "Type": "TAG_FILTERS_1_0", "Query": "{}" },
        }),
    ];
    let mut task_arns: Vec<String> = Vec::new();
    let mut segments: Vec<String> = Vec::new();
    for request in &requests {
        let started = emulator.ok("/start-tag-sync-task", request).await;
        let task_arn = started["TaskArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {request}: expected a TaskArn, got {started}"))
            .to_string();
        let segment = application_segment(&task_arn, &arn).unwrap_or_else(|| {
            panic!("case {request}: expected a task ARN under {arn}, got {task_arn:?}")
        });
        assert!(
            !task_arns.contains(&task_arn),
            "case {request}: expected a task ARN of its own, got {task_arn:?} again \
             among {task_arns:?}",
        );
        task_arns.push(task_arn);
        segments.push(segment);
    }
    assert!(
        segments.windows(2).all(|pair| pair[0] == pair[1]),
        "case \"one group\": expected every task of a group to carry the same \
         application segment, got {segments:?}",
    );
    // Each stored task is still addressable by its own ARN, so starting a second one
    // did not replace the first.
    for task_arn in &task_arns {
        let task = emulator
            .ok("/get-tag-sync-task", &json!({ "TaskArn": task_arn }))
            .await;
        assert_eq!(
            task["TaskArn"].as_str(),
            Some(task_arn.as_str()),
            "case {task_arn:?}: expected the task to read back as itself, got {task}",
        );
    }
}

/// A `Group` that names no stored group is `NotFoundException` (404) — including an ARN/// whose group name exists locally but whose region, account or service does not match
/// the stored ARN.
#[tokio::test]
async fn a_group_that_is_not_stored_is_not_found() {
    let emulator = Emulator::new();
    emulator.ok("/groups", &json!({ "Name": "present" })).await;
    let identifiers = [
        ("a name nothing was created under", "absent".to_string()),
        (
            "an ARN for the right name in another region",
            group_arn("eu-west-1", "present"),
        ),
        (
            "an ARN for the right name in another account",
            "arn:aws:resource-groups:us-east-1:111111111111:group/present".to_string(),
        ),
        (
            "an ARN of another service",
            format!("arn:aws:ec2:us-east-1:{ACCOUNT}:group/present"),
        ),
    ];
    for (label, identifier) in identifiers {
        let (status, answered) = emulator
            .call(
                "/start-tag-sync-task",
                &json!({
                    "Group": identifier,
                    "RoleArn": role_arn("ResourceGroupsTagSync"),
                    "TagKey": "Stage",
                    "TagValue": "Test",
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&answered)),
            (404, "NotFoundException"),
            "case {label:?}: expected 404 NotFoundException, got {status} {answered}",
        );
    }
}

/// A request that does not name exactly one way of syncing, or whose query is not a
/// query, is `BadRequestException` (400) and stores nothing.
#[tokio::test]
async fn a_request_that_names_no_single_sync_method_is_bad_request() {
    let emulator = Emulator::new();
    emulator.ok("/groups", &json!({ "Name": "refusals" })).await;
    let role = role_arn("ResourceGroupsTagSync");
    let refused = [
        ("neither a tag pair nor a query", json!({})),
        ("a tag key with no tag value", json!({ "TagKey": "Stage" })),
        ("a tag value with no tag key", json!({ "TagValue": "Test" })),
        (
            "both a tag pair and a query",
            json!({
                "TagKey": "Stage",
                "TagValue": "Test",
                "ResourceQuery": { "Type": "TAG_FILTERS_1_0", "Query": "{}" },
            }),
        ),
        (
            "a query type the model does not declare",
            json!({ "ResourceQuery": { "Type": "TAG_FILTERS_2_0", "Query": "{}" } }),
        ),
        (
            "a query document that is not a JSON object",
            json!({ "ResourceQuery": { "Type": "TAG_FILTERS_1_0", "Query": "[]" } }),
        ),
        (
            "a query document that is not JSON at all",
            json!({ "ResourceQuery": { "Type": "TAG_FILTERS_1_0", "Query": "Stage=Test" } }),
        ),
    ];
    for (label, members) in refused {
        let mut body = json!({ "Group": "refusals", "RoleArn": role });
        let object = body.as_object_mut().expect("a JSON object");
        for (member, value) in members.as_object().expect("a JSON object") {
            object.insert(member.clone(), value.clone());
        }
        let (status, answered) = emulator.call("/start-tag-sync-task", &body).await;
        assert_eq!(
            (status, error_code(&answered)),
            (400, "BadRequestException"),
            "case {label:?} ({body}): expected 400 BadRequestException, \
             got {status} {answered}",
        );
    }
    // The refusals left the group alone, and a valid request on it still succeeds.
    let group = emulator.ok("/get-group", &json!({ "Group": "refusals" })).await;
    assert_eq!(
        group["Group"]["Name"].as_str(),
        Some("refusals"),
        "case \"after the refusals\": expected the group to be untouched, got {group}",
    );
    let started = emulator
        .ok(
            "/start-tag-sync-task",
            &json!({
                "Group": "refusals",
                "RoleArn": role_arn("ResourceGroupsTagSync"),
                "TagKey": "Stage",
                "TagValue": "Test",
            }),
        )
        .await;
    assert!(
        started["TaskArn"].as_str().is_some(),
        "case \"after the refusals\": expected a task to start, got {started}",
    );
}

/// A task ARN that addresses no stored task is `NotFoundException` (404), and one the
/// model's `TagSyncTaskArn` does not admit is refused at the boundary (400).
#[tokio::test]
async fn a_task_arn_that_addresses_nothing_is_not_found() {
    let emulator = Emulator::new();
    emulator.ok("/groups", &json!({ "Name": "has-a-task" })).await;
    let started = emulator
        .ok(
            "/start-tag-sync-task",
            &json!({
                "Group": "has-a-task",
                "RoleArn": role_arn("ResourceGroupsTagSync"),
                "TagKey": "Stage",
                "TagValue": "Test",
            }),
        )
        .await;
    let stored = started["TaskArn"].as_str().expect("a TaskArn").to_string();
    let application = application_segment(&stored, &group_arn("us-east-1", "has-a-task"))
        .expect("a task ARN under the group");
    let other_id = "0123456789abcdefghjkmnpqrs";
    let unknown = [
        (
            "another task id under the same application group",
            format!(
                "{}/{application}/tag-sync-task/{other_id}",
                group_arn("us-east-1", "has-a-task"),
            ),
        ),
        (
            "the same task id under another application segment",
            format!(
                "{}/{other_id}/tag-sync-task/{other_id}",
                group_arn("us-east-1", "has-a-task"),
            ),
        ),
        (
            "a task ARN of a group that does not exist",
            format!(
                "{}/{other_id}/tag-sync-task/{other_id}",
                group_arn("us-east-1", "no-such-group"),
            ),
        ),
        (
            "the stored task's ARN with another region in it",
            format!(
                "{}/{application}/tag-sync-task/{other_id}",
                group_arn("eu-west-1", "has-a-task"),
            ),
        ),
    ];
    for (label, task_arn) in unknown {
        let (status, answered) = emulator
            .call("/get-tag-sync-task", &json!({ "TaskArn": task_arn }))
            .await;
        assert_eq!(
            (status, error_code(&answered)),
            (404, "NotFoundException"),
            "case {label:?} ({task_arn}): expected 404 NotFoundException, \
             got {status} {answered}",
        );
    }
    // The stored task is unaffected by the reads that found nothing.
    let task = emulator
        .ok("/get-tag-sync-task", &json!({ "TaskArn": &stored }))
        .await;
    assert_eq!(
        task["TaskArn"].as_str(),
        Some(stored.as_str()),
        "case \"the stored task\": expected it to still read back, got {task}",
    );

    // An identifier the modelled pattern does not admit never reaches the handler.
    for (label, task_arn) in [
        ("a group ARN rather than a task ARN", group_arn("us-east-1", "has-a-task")),
        ("an id that is not 26 characters", format!("{stored}x")),
        ("a bare name", "has-a-task".to_string()),
    ] {
        let (status, answered) = emulator
            .call("/get-tag-sync-task", &json!({ "TaskArn": task_arn }))
            .await;
        assert_eq!(
            status, 400,
            "case {label:?} ({task_arn}): expected the boundary to refuse it with 400, \
             got {status} {answered}",
        );
    }
}

/// A task can be started only on a group whose name the modelled `TagSyncTaskArn`
/// can carry: that ARN writes the group-name segment as `[a-zA-Z0-9_\.-]{1,150}`,
/// while a group name may be up to 300 characters, so the published bound and one
/// character past it are both exercised.
///
/// Source: `smithy_query(query="shape",shape="TagSyncTaskArn")` and
/// `smithy_query(query="shape",shape="CreateGroupInput")` (`Name`, len 1..300).
#[tokio::test]
async fn a_group_name_a_task_arn_cannot_carry_is_refused() {
    let emulator = Emulator::new();
    let cases = [
        ("a name one character short of the bound", "a".repeat(149), 200),
        ("a name of exactly the bound", "b".repeat(150), 200),
        ("a name one character past the bound", "c".repeat(151), 400),
        ("a name far past the bound", "d".repeat(300), 400),
    ];
    for (label, name, expected) in cases {
        emulator.ok("/groups", &json!({ "Name": name })).await;
        let (status, answered) = emulator
            .call(
                "/start-tag-sync-task",
                &json!({
                    "Group": name,
                    "RoleArn": role_arn("ResourceGroupsTagSync"),
                    "TagKey": "Stage",
                    "TagValue": "Test",
                }),
            )
            .await;
        assert_eq!(
            status, expected,
            "case {label:?} (name of {} characters): expected {expected}, got {status} \
             {answered}",
            name.len(),
        );
        if expected == 200 {
            let task_arn = answered["TaskArn"].as_str().unwrap_or_else(|| {
                panic!("case {label:?}: expected a TaskArn, got {answered}")
            });
            assert!(
                application_segment(task_arn, &group_arn("us-east-1", &name)).is_some(),
                "case {label:?}: expected a task ARN the model admits, got {task_arn:?}",
            );
        } else {
            assert_eq!(
                error_code(&answered),
                "BadRequestException",
                "case {label:?}: expected BadRequestException, got {answered}",
            );
        }
    }
}

/// A `RoleArn` the modelled `RoleArn` does not admit never reaches the handler: the
/// request boundary refuses it (400), so no task is started.
///
/// Source: `smithy_query(query="operation",operation="StartTagSyncTask")` —
/// `RoleArn` is `len 20..2048, pattern
/// ^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`.
#[tokio::test]
async fn a_role_arn_the_model_does_not_admit_is_refused() {
    let emulator = Emulator::new();
    emulator.ok("/groups", &json!({ "Name": "role-shapes" })).await;
    let refused = [
        ("not an ARN at all", "ResourceGroupsTagSync".to_string()),
        ("a role ARN that is too short", "arn:aws:iam::1:role/r".to_string()),
        (
            "an account that is not twelve digits",
            "arn:aws:iam::12345:role/ResourceGroupsTagSync".to_string(),
        ),
        (
            "an ARN of another service",
            format!("arn:aws:resource-groups:us-east-1:{ACCOUNT}:group/role-shapes"),
        ),
        (
            "a role ARN with no role segment",
            format!("arn:aws:iam::{ACCOUNT}:user/someone"),
        ),
    ];
    for (label, role) in refused {
        let (status, answered) = emulator
            .call(
                "/start-tag-sync-task",
                &json!({
                    "Group": "role-shapes",
                    "RoleArn": role,
                    "TagKey": "Stage",
                    "TagValue": "Test",
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {label:?} ({role}): expected 400 from the request boundary, \
             got {status} {answered}",
        );
    }
    // The accepted shape still starts a task on the same group.
    let started = emulator
        .ok(
            "/start-tag-sync-task",
            &json!({
                "Group": "role-shapes",
                "RoleArn": role_arn("ResourceGroupsTagSync"),
                "TagKey": "Stage",
                "TagValue": "Test",
            }),
        )
        .await;
    assert!(
        started["TaskArn"].as_str().is_some(),
        "case \"an admitted role ARN\": expected a task to start, got {started}",
    );
}
