// SPDX-License-Identifier: CC0-1.0

//! `DeleteGroup` against the tag-sync tasks nested inside the group it deletes, end
//! to end.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the lifecycle suite's `Emulator`), so these
//! exercise the whole restJson1 path: routing, boundary validation, the handler,
//! and serialization.
//!
//! The rule under test, asserted over a TABLE of inputs rather than one row: a
//! tag-sync task is a resource nested inside the group it syncs into (the modelled
//! `TagSyncTaskArn` writes it as that group's own ARN followed by
//! `/tag-sync-task/<id>`), so it cannot outlive that group — a delete that
//! succeeds takes the deleted group's tasks with it, leaving no reader reporting a
//! task of a group that is gone and handing no later group of the same name the
//! tasks of its predecessor, while a delete that is REFUSED takes none; the tasks
//! of every other group, including a same-named group of another region, are
//! untouched either way.
//!
//! The rows differ in the region the group lives in, in how many tasks it carries
//! (none, one, several), in what each task syncs (a tag key-value pair or a
//! resource query), in the group's service configuration (none, an application
//! group whose ARN is qualified by its application id, a host-management group)
//! and in which identifier form the delete addresses the group by — the last being
//! what the rule generalises over most directly.

use serde_json::{json, Value};

use super::resource_group_lifecycle::{error_code, Emulator};
use super::tag_sync_tasks::{group_arn, role_arn};

/// The region whose groups and tasks every row leaves alone: no row uses it, so a
/// task of a same-named group stored here must survive every delete under test.
const OTHER_REGION: &str = "ap-south-1";

/// The name of the group every row also creates beside its own, in the same
/// region, so a delete can be shown to take ONLY the addressed group's tasks.
const BYSTANDER: &str = "bystander-group";

/// The fixed sentence this service answers when a task ARN addresses no stored
/// tag-sync task, which is the message a reader of a deleted group's task gets —
/// the same one an ARN nothing was ever minted for gets.
const TASK_NOT_FOUND_MESSAGE: &str = "The specified tag-sync task does not exist";

/// How a task under test syncs: the two alternatives `StartTagSyncTask` models
/// (`TagKey` with `TagValue`, or `ResourceQuery`).
#[derive(Debug, Clone, Copy)]
enum Sync {
    Pair { key: &'static str, value: &'static str },
    Query { query_type: &'static str, query: &'static str },
}

impl Sync {
    /// The members this way of syncing contributes to a `StartTagSyncTask` request.
    fn members(self) -> Value {
        match self {
            Sync::Pair { key, value } => json!({ "TagKey": key, "TagValue": value }),
            Sync::Query { query_type, query } => {
                json!({ "ResourceQuery": { "Type": query_type, "Query": query } })
            }
        }
    }
}

/// Which of the identifier forms `DeleteGroup` accepts the row addresses its group
/// by. The group a request resolves to must not depend on the form, so the
/// deletion of its tasks must not either.
#[derive(Debug, Clone, Copy)]
enum AddressedBy {
    /// The group's name in `Group`.
    Name,
    /// The group's name in the deprecated `GroupName`.
    DeprecatedGroupName,
    /// The group's stored ARN in `Group`, as `CreateGroup` reported it.
    Arn,
}

/// One row: a group, its configuration, the tasks started on it, and the way the
/// delete addresses it.
#[derive(Debug, Clone)]
struct DeleteCase {
    label: &'static str,
    region: &'static str,
    group: &'static str,
    /// The `Configuration` member of the create request, or `Value::Null` for a
    /// create that sends none.
    configuration: Value,
    tasks: Vec<Sync>,
    addressed_by: AddressedBy,
}

/// The configuration item types below are AWS service-configuration vocabulary
/// from the "Service configurations for resource groups" reference
/// (<https://docs.aws.amazon.com/ARG/latest/APIReference/about-slg.html>), which is
/// what `GroupConfigurationItem$Type` is documented against.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            label: "a group carrying no task at all, deleted by name",
            region: "us-east-1",
            group: "no-tasks",
            configuration: Value::Null,
            tasks: Vec::new(),
            addressed_by: AddressedBy::Name,
        },
        DeleteCase {
            label: "one task syncing a tag pair, deleted by name",
            region: "us-east-1",
            group: "one-task",
            configuration: Value::Null,
            tasks: vec![Sync::Pair { key: "Stage", value: "Test" }],
            addressed_by: AddressedBy::Name,
        },
        DeleteCase {
            label: "one task syncing a tag pair whose value is empty, deleted by ARN",
            region: "eu-west-1",
            group: "empty.value_1",
            configuration: Value::Null,
            tasks: vec![Sync::Pair { key: "Team", value: "" }],
            addressed_by: AddressedBy::Arn,
        },
        DeleteCase {
            label: "three tasks of both sync kinds, deleted by the deprecated GroupName",
            region: "us-west-2",
            group: "three-tasks",
            configuration: json!([{ "Type": "AWS::EC2::HostManagement" }]),
            tasks: vec![
                Sync::Pair { key: "cost:center", value: "platform engineering" },
                Sync::Query {
                    query_type: "TAG_FILTERS_1_0",
                    query: r#"{"ResourceTypeFilters":["AWS::EC2::Instance"],"TagFilters":[{"Key":"Stage","Values":["Test"]}]}"#,
                },
                Sync::Query {
                    query_type: "CLOUDFORMATION_STACK_1_0",
                    query: r#"{"StackIdentifier":"arn:aws:cloudformation:us-west-2:000000000000:stack/probe/1234"}"#,
                },
            ],
            addressed_by: AddressedBy::DeprecatedGroupName,
        },
        DeleteCase {
            label: "an application group, whose stored ARN is qualified, deleted by that ARN",
            region: "ap-southeast-2",
            group: "application-group",
            configuration: json!([{ "Type": "AWS::ResourceGroups::ApplicationGroup" }]),
            tasks: vec![
                Sync::Pair { key: "Stage", value: "Prod" },
                Sync::Query {
                    query_type: "TAG_FILTERS_1_0",
                    query: r#"{"ResourceTypeFilters":"AWS::AllSupported"}"#,
                },
            ],
            addressed_by: AddressedBy::Arn,
        },
        DeleteCase {
            label: "two tasks on a host-management group, deleted by name",
            region: "us-east-2",
            group: "host-management",
            configuration: json!([{ "Type": "AWS::EC2::HostManagement" }]),
            tasks: vec![
                Sync::Pair { key: "Stage", value: "Dev" },
                Sync::Pair { key: "Stage", value: "Dev" },
            ],
            addressed_by: AddressedBy::Name,
        },
    ]
}

/// Create the group `name` in `region` with `configuration`, and return its stored
/// ARN as `CreateGroup` reported it.
async fn create_group(
    emulator: &Emulator,
    case: &str,
    region: &str,
    name: &str,
    configuration: &Value,
) -> String {
    let mut request = json!({ "Name": name });
    if !configuration.is_null() {
        request["Configuration"] = configuration.clone();
    }
    let created = emulator.ok_in(region, "/groups", &request).await;
    created["Group"]["GroupArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateGroup returned no GroupArn: {created}"))
        .to_string()
}

/// Start a task on `group` in `region` and return the ARN it was minted with.
async fn start_task(
    emulator: &Emulator,
    case: &str,
    region: &str,
    group: &str,
    sync: Sync,
    role: &str,
) -> String {
    let mut request = json!({ "Group": group, "RoleArn": role_arn(role) });
    for (member, value) in sync.members().as_object().expect("a JSON object") {
        request[member.clone()] = value.clone();
    }
    let started = emulator
        .ok_in(region, "/start-tag-sync-task", &request)
        .await;
    started["TaskArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: StartTagSyncTask returned no TaskArn: {started}"))
        .to_string()
}

/// The task ARNs `ListTagSyncTasks` reports in `region`, in the order the listing
/// hands them back.
async fn listed_task_arns(emulator: &Emulator, case: &str, region: &str) -> Vec<String> {
    let listed = emulator
        .ok_in(region, "/list-tag-sync-tasks", &json!({}))
        .await;
    listed["TagSyncTasks"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected TagSyncTasks, got {listed}"))
        .iter()
        .map(|task| {
            task["TaskArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a TaskArn, got {task}"))
                .to_string()
        })
        .collect()
}

/// Assert that `task_arn` addresses no task in `region`, with the service's fixed
/// not-found wording.
async fn assert_task_is_gone(emulator: &Emulator, case: &str, region: &str, task_arn: &str) {
    let (status, answered) = emulator
        .call_in(region, "/get-tag-sync-task", &json!({ "TaskArn": task_arn }))
        .await;
    assert_eq!(
        (status, error_code(&answered)),
        (404, "NotFoundException"),
        "case {case:?}: expected GetTagSyncTask on {task_arn} to answer 404 \
         NotFoundException, got {status} {answered}",
    );
    assert_eq!(
        answered["Message"].as_str(),
        Some(TASK_NOT_FOUND_MESSAGE),
        "case {case:?}: expected the fixed not-found message for {task_arn}, got {answered}",
    );
}

/// The rule: a delete that succeeds takes the deleted group's tag-sync tasks with
/// it, and takes no other group's.
#[tokio::test]
async fn deleting_a_group_deletes_the_tag_sync_tasks_nested_inside_it() {
    for case in delete_cases() {
        let label = case.label;
        let emulator = Emulator::new();
        let arn = create_group(&emulator, label, case.region, case.group, &case.configuration).await;

        // Its own tasks.
        let mut task_arns: Vec<String> = Vec::new();
        for (at, sync) in case.tasks.iter().enumerate() {
            let task_arn =
                start_task(&emulator, label, case.region, case.group, *sync, "TagSync").await;
            assert!(
                task_arn.starts_with(&arn),
                "case {label:?}: expected task {at} to be nested inside the group ARN {arn}, \
                 got {task_arn}",
            );
            task_arns.push(task_arn);
        }
        // A second group of the same region, whose task must survive.
        create_group(&emulator, label, case.region, BYSTANDER, &Value::Null).await;
        let bystander_task = start_task(
            &emulator,
            label,
            case.region,
            BYSTANDER,
            Sync::Pair { key: "Keep", value: "me" },
            "Bystander",
        )
        .await;
        // A SAME-NAMED group of another region, whose task must likewise survive:
        // this service is regional, so the name alone does not identify a group.
        create_group(&emulator, label, OTHER_REGION, case.group, &Value::Null).await;
        let elsewhere_task = start_task(
            &emulator,
            label,
            OTHER_REGION,
            case.group,
            Sync::Pair { key: "Keep", value: "me" },
            "Elsewhere",
        )
        .await;

        let request = match case.addressed_by {
            AddressedBy::Name => json!({ "Group": case.group }),
            AddressedBy::DeprecatedGroupName => json!({ "GroupName": case.group }),
            AddressedBy::Arn => json!({ "Group": arn }),
        };
        let (status, deleted) = emulator
            .call_in(case.region, "/delete-group", &request)
            .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected DeleteGroup {request} to answer 200, got {status} {deleted}",
        );

        // The group is gone...
        let (status, read) = emulator
            .call_in(case.region, "/get-group", &json!({ "Group": case.group }))
            .await;
        assert_eq!(
            (status, error_code(&read)),
            (404, "NotFoundException"),
            "case {label:?}: expected the deleted group to be gone, got {status} {read}",
        );
        // ...and so is every task that was nested inside it, on the reader that
        // addresses one task and on the listing alike.
        for task_arn in &task_arns {
            assert_task_is_gone(&emulator, label, case.region, task_arn).await;
        }
        assert_eq!(
            listed_task_arns(&emulator, label, case.region).await,
            vec![bystander_task.clone()],
            "case {label:?}: expected only the bystander group's task {bystander_task} to \
             survive in {}",
            case.region,
        );
        // The other region's same-named group kept its task, which still reports
        // that group.
        let surviving = emulator
            .ok_in(
                OTHER_REGION,
                "/get-tag-sync-task",
                &json!({ "TaskArn": &elsewhere_task }),
            )
            .await;
        assert_eq!(
            (
                surviving["GroupName"].as_str(),
                surviving["GroupArn"].as_str(),
            ),
            (
                Some(case.group),
                Some(group_arn(OTHER_REGION, case.group).as_str()),
            ),
            "case {label:?}: expected the other region's task to keep reporting its own \
             group, got {surviving}",
        );

        // A group created again under the same name inherits nothing: the deleted
        // group's task ARNs still address no task, and the new group has none.
        create_group(&emulator, label, case.region, case.group, &case.configuration).await;
        for task_arn in &task_arns {
            assert_task_is_gone(&emulator, label, case.region, task_arn).await;
        }
        let listed = emulator
            .ok_in(
                case.region,
                "/list-tag-sync-tasks",
                &json!({ "Filters": [{ "GroupName": case.group }] }),
            )
            .await;
        assert_eq!(
            listed["TagSyncTasks"].as_array().map(Vec::len),
            Some(0),
            "case {label:?}: expected the re-created group to carry no task, got {listed}",
        );
    }
}

/// The other half of the rule: a delete that is REFUSED takes no task. A group
/// whose stored configuration sets `deletion-protection` to `UNLESS_EMPTY` while it
/// still contains resources is refused, and keeps every task nested inside it.
#[tokio::test]
async fn a_refused_delete_keeps_the_group_s_tag_sync_tasks() {
    // The `deletion-protection` parameter and its `UNLESS_EMPTY` value are AWS
    // service-configuration vocabulary from the "Service configurations for
    // resource groups" reference, which `GroupConfigurationParameter$Name` is
    // documented against.
    let protecting = json!([
        { "Type": "AWS::EC2::HostManagement" },
        {
            "Type": "AWS::ResourceGroups::Generic",
            "Parameters": [{ "Name": "deletion-protection", "Values": ["UNLESS_EMPTY"] }]
        }
    ]);
    let emulator = Emulator::new();
    let case = "a protected group that still contains a resource";
    create_group(&emulator, case, "us-east-1", "guarded", &protecting).await;
    emulator
        .ok(
            "/group-resources",
            &json!({
                "Group": "guarded",
                "ResourceArns": ["arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0"]
            }),
        )
        .await;
    let mut task_arns: Vec<String> = Vec::new();
    for sync in [
        Sync::Pair { key: "Stage", value: "Prod" },
        Sync::Query {
            query_type: "TAG_FILTERS_1_0",
            query: r#"{"ResourceTypeFilters":["AWS::EC2::Instance"]}"#,
        },
    ] {
        task_arns.push(start_task(&emulator, case, "us-east-1", "guarded", sync, "TagSync").await);
    }
    task_arns.sort();

    let (status, refused) = emulator
        .call("/delete-group", &json!({ "Group": "guarded" }))
        .await;
    assert_eq!(
        (status, error_code(&refused)),
        (400, "BadRequestException"),
        "case {case:?}: expected DeleteGroup to be refused with 400 BadRequestException, \
         got {status} {refused}",
    );
    // Nothing was destroyed: the group reads back, and so does every task.
    let read = emulator.ok("/get-group", &json!({ "Group": "guarded" })).await;
    assert_eq!(
        read["Group"]["Name"].as_str(),
        Some("guarded"),
        "case {case:?}: expected the refused delete to leave the group, got {read}",
    );
    for task_arn in &task_arns {
        let task = emulator
            .ok("/get-tag-sync-task", &json!({ "TaskArn": task_arn }))
            .await;
        assert_eq!(
            task["TaskArn"].as_str(),
            Some(task_arn.as_str()),
            "case {case:?}: expected {task_arn} to survive the refused delete, got {task}",
        );
    }
    let mut listed = listed_task_arns(&emulator, case, "us-east-1").await;
    listed.sort();
    assert_eq!(
        listed, task_arns,
        "case {case:?}: expected the refused delete to keep every task {task_arns:?}, \
         got {listed:?}",
    );

    // Once the group holds nothing it deletes, and the tasks go with it then.
    emulator
        .ok(
            "/ungroup-resources",
            &json!({
                "Group": "guarded",
                "ResourceArns": ["arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0"]
            }),
        )
        .await;
    emulator.ok("/delete-group", &json!({ "Group": "guarded" })).await;
    for task_arn in &task_arns {
        assert_task_is_gone(&emulator, case, "us-east-1", task_arn).await;
    }
    assert_eq!(
        listed_task_arns(&emulator, case, "us-east-1").await,
        Vec::<String>::new(),
        "case {case:?}: expected the successful delete to take every task",
    );
}
