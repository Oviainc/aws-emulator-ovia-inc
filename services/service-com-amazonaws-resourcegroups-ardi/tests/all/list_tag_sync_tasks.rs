// SPDX-License-Identifier: CC0-1.0

//! `ListTagSyncTasks`, end to end.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the sibling suite's `Emulator`), so these exercise the
//! whole restJson1 path: routing, boundary validation, the handler, and serialization.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! * a listing reports one item per tag-sync task STORED in the region the request
//!   addressed, computed from the same record a single read of that task reports, and
//!   no task of another region;
//! * each `Filters` entry names one application group — by its ARN or by its name, in
//!   either the plain or the qualified form of each — and the listing reports the tasks
//!   of every group its entries name;
//! * a listing is ordered and pages through every selected task exactly once whatever
//!   the page size, and it reports: the stored tasks are unchanged afterwards;
//! * a `Filters` entry that names no group at all, a `NextToken` no listing issued, and
//!   a `MaxResults` outside the modelled range are each refused with 400.

use serde_json::{json, Value};

use super::resource_group_lifecycle::{error_code, is_modelled_token, Emulator};
use super::tag_sync_tasks::{application_segment, group_arn, role_arn};

/// One tag-sync task the fixtures start. The rows differ in the region their group
/// lives in, in how many tasks one group carries, in the role given and in what the
/// task syncs — which is what the listing rule generalises over, since a listing
/// reports whatever the store holds.
struct TaskFixture {
    label: &'static str,
    region: &'static str,
    group: &'static str,
    role: &'static str,
    /// The members naming what the task syncs: a `TagKey`/`TagValue` pair or a
    /// `ResourceQuery`.
    sync: fn() -> Value,
}

fn fixtures() -> Vec<TaskFixture> {
    vec![
        TaskFixture {
            label: "a tag pair on the first group",
            region: "us-east-1",
            group: "app-one",
            role: "ResourceGroupsTagSync",
            sync: || json!({ "TagKey": "Stage", "TagValue": "Test" }),
        },
        TaskFixture {
            label: "a tag pair whose value is empty, on the same group",
            region: "us-east-1",
            group: "app-one",
            role: "tag-sync/with-a-path",
            sync: || json!({ "TagKey": "Team", "TagValue": "" }),
        },
        TaskFixture {
            label: "a tag-filters query, on the same group again",
            region: "us-east-1",
            group: "app-one",
            role: "AnotherTagSyncRole",
            sync: || {
                json!({
                    "ResourceQuery": {
                        "Type": "TAG_FILTERS_1_0",
                        "Query": "{\"ResourceTypeFilters\":[\"AWS::EC2::Instance\"],\"TagFilters\":[{\"Key\":\"Stage\",\"Values\":[\"Test\"]}]}"
                    }
                })
            },
        },
        TaskFixture {
            label: "a cloudformation-stack query on a second group of the same region",
            region: "us-east-1",
            group: "app.two_2",
            role: "ResourceGroupsTagSync",
            sync: || {
                json!({
                    "ResourceQuery": {
                        "Type": "CLOUDFORMATION_STACK_1_0",
                        "Query": "{\"StackIdentifier\":\"arn:aws:cloudformation:us-east-1:000000000000:stack/demo/0\"}"
                    }
                })
            },
        },
        TaskFixture {
            label: "a tag pair on a group of ANOTHER region, under the first group's name",
            region: "eu-west-1",
            group: "app-one",
            role: "ResourceGroupsTagSync",
            sync: || json!({ "TagKey": "Stage", "TagValue": "Prod" }),
        },
        TaskFixture {
            label: "a second task in that other region",
            region: "eu-west-1",
            group: "app-one",
            role: "ResourceGroupsTagSync",
            sync: || json!({ "TagKey": "Owner", "TagValue": "sre@example.com" }),
        },
    ]
}

/// A group of the fixtures that carries NO task: its name selects an empty listing
/// rather than every task.
const GROUP_WITHOUT_TASKS: &str = "no-tasks";

/// One started task, as the fixtures left it in the store.
struct Started {
    label: &'static str,
    region: &'static str,
    group: &'static str,
    task_arn: String,
    /// The 26-character id qualifying the group's name inside the task ARN, which is
    /// what the group's application-group ARN and its qualified name are written with.
    application: String,
}

/// An emulator holding every fixture task, and the record of what was started.
async fn populated() -> (Emulator, Vec<Started>) {
    let emulator = Emulator::new();
    let mut started = Vec::new();
    for region in ["us-east-1", "eu-west-1"] {
        for group in ["app-one", "app.two_2", GROUP_WITHOUT_TASKS] {
            emulator
                .ok_in(region, "/groups", &json!({ "Name": group }))
                .await;
        }
    }
    for fixture in fixtures() {
        let mut body = json!({
            "Group": fixture.group,
            "RoleArn": role_arn(fixture.role),
        });
        let object = body.as_object_mut().expect("a JSON object");
        for (member, value) in (fixture.sync)().as_object().expect("a JSON object") {
            object.insert(member.clone(), value.clone());
        }
        let answered = emulator
            .ok_in(fixture.region, "/start-tag-sync-task", &body)
            .await;
        let task_arn = answered["TaskArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: expected a TaskArn, got {answered}", fixture.label))
            .to_string();
        let arn = group_arn(fixture.region, fixture.group);
        let application = application_segment(&task_arn, &arn).unwrap_or_else(|| {
            panic!(
                "case {:?}: expected a task ARN under {arn}, got {task_arn:?}",
                fixture.label,
            )
        });
        started.push(Started {
            label: fixture.label,
            region: fixture.region,
            group: fixture.group,
            task_arn,
            application,
        });
    }
    (emulator, started)
}

/// The `TagSyncTasks` items one `ListTagSyncTasks` call reports, requiring the
/// modelled 200 success rather than any refusal.
async fn listed(emulator: &Emulator, region: &str, body: &Value) -> Vec<Value> {
    let page = emulator
        .ok_in(region, "/list-tag-sync-tasks", body)
        .await;
    page["TagSyncTasks"]
        .as_array()
        .unwrap_or_else(|| panic!("case {body} in {region}: expected TagSyncTasks, got {page}"))
        .clone()
}

/// The `TaskArn` of each item of a page.
fn task_arns(items: &[Value]) -> Vec<String> {
    items
        .iter()
        .map(|item| {
            item["TaskArn"]
                .as_str()
                .unwrap_or_else(|| panic!("expected a TaskArn on {item}"))
                .to_string()
        })
        .collect()
}

/// The task ARNs the fixtures stored in `region`, in the order a listing reports
/// them (task-ARN order).
fn stored_in(started: &[Started], region: &str) -> Vec<String> {
    let mut arns: Vec<String> = started
        .iter()
        .filter(|task| task.region == region)
        .map(|task| task.task_arn.clone())
        .collect();
    arns.sort();
    arns
}

/// An unfiltered listing reports every task STORED in the region the request
/// addressed — each item exactly what a single read of that task reports — and no
/// task of another region.
#[tokio::test]
async fn an_unfiltered_listing_reports_every_stored_task_of_the_region() {
    let (emulator, started) = populated().await;
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let items = listed(&emulator, region, &json!({})).await;
        let answered = task_arns(&items);
        let expected = stored_in(&started, region);
        assert_eq!(
            answered, expected,
            "case {region:?}: expected the region's stored tasks in task-ARN order, \
             got {answered:?}",
        );
        // Every item is the record a single read of that task reports: the listing is
        // computed from the same stored task, so the two cannot disagree.
        for item in &items {
            let task_arn = item["TaskArn"].as_str().expect("a TaskArn");
            let read = emulator
                .ok_in(region, "/get-tag-sync-task", &json!({ "TaskArn": task_arn }))
                .await;
            assert_eq!(
                item, &read,
                "case {task_arn:?}: expected the listed item to match the single read, \
                 got {item} and {read}",
            );
            assert_eq!(
                item["Status"].as_str(),
                Some("ACTIVE"),
                "case {task_arn:?}: expected the modelled status of a task that is \
                 managing resources, got {item}",
            );
            assert!(
                item["CreatedAt"].is_number(),
                "case {task_arn:?}: expected the stored creation time, got {item}",
            );
        }
    }
}

/// A `Filters` entry selects the tasks of the group it names, in each form this
/// service reads a group identifier in, and entries are alternatives: a listing
/// reports the tasks of every group its entries name.
#[tokio::test]
async fn filters_select_the_tasks_of_the_groups_they_name() {
    let (emulator, started) = populated().await;
    let region = "us-east-1";
    let one: Vec<String> = stored_in(&started, region)
        .into_iter()
        .filter(|arn| {
            started
                .iter()
                .any(|task| &task.task_arn == arn && task.group == "app-one")
        })
        .collect();
    let two: Vec<String> = stored_in(&started, region)
        .into_iter()
        .filter(|arn| {
            started
                .iter()
                .any(|task| &task.task_arn == arn && task.group == "app.two_2")
        })
        .collect();
    assert_eq!(
        one.len(),
        3,
        "case \"the fixtures\": expected three tasks on the first group, got {one:?}",
    );
    let application = started
        .iter()
        .find(|task| task.region == region && task.group == "app-one")
        .map(|task| task.application.clone())
        .expect("a task on the first group");

    // (label, the whole Filters list, the task ARNs it must select)
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "the group's bare name",
            json!([{ "GroupName": "app-one" }]),
            one.clone(),
        ),
        (
            "the group's stored ARN",
            json!([{ "GroupArn": group_arn(region, "app-one") }]),
            one.clone(),
        ),
        (
            "the QUALIFIED form of the group's name, which the modelled GroupName admits",
            json!([{ "GroupName": format!("app-one/{application}") }]),
            one.clone(),
        ),
        (
            "the group's application-group ARN, the form the task ARN nests the task in",
            json!([{ "GroupArn": format!("{}/{application}", group_arn(region, "app-one")) }]),
            one.clone(),
        ),
        (
            "both ways of naming the same group in one entry",
            json!([{ "GroupArn": group_arn(region, "app-one"), "GroupName": "app-one" }]),
            one.clone(),
        ),
        (
            "an entry whose two members name DIFFERENT groups, which is no group",
            json!([{ "GroupArn": group_arn(region, "app-one"), "GroupName": "app.two_2" }]),
            Vec::new(),
        ),
        (
            "the group's ARN sent in GroupName, which the member does not model",
            json!([{ "GroupName": group_arn(region, "app-one") }]),
            Vec::new(),
        ),
        (
            "the group's qualified NAME sent in GroupName, beside its ARN",
            json!([{
                "GroupArn": group_arn(region, "app-one"),
                "GroupName": format!("app-one/{application}"),
            }]),
            one.clone(),
        ),
        (
            "the second group of the region",
            json!([{ "GroupName": "app.two_2" }]),
            two.clone(),
        ),
        (
            "two entries, which are alternatives",
            json!([{ "GroupName": "app-one" }, { "GroupName": "app.two_2" }]),
            stored_in(&started, region),
        ),
        (
            "a group of the region that carries no task",
            json!([{ "GroupName": GROUP_WITHOUT_TASKS }]),
            Vec::new(),
        ),
        (
            "a name no group of the region was created under",
            json!([{ "GroupName": "absent" }]),
            Vec::new(),
        ),
        (
            "the same group's ARN in ANOTHER region",
            json!([{ "GroupArn": group_arn("eu-west-1", "app-one") }]),
            Vec::new(),
        ),
        ("no filters at all", json!([]), stored_in(&started, region)),
    ];
    for (label, filters, expected) in cases {
        let answered = task_arns(&listed(&emulator, region, &json!({ "Filters": filters })).await);
        assert_eq!(
            answered, expected,
            "case {label:?} ({filters}): expected {expected:?}, got {answered:?}",
        );
    }

    // The same filter, addressed to the OTHER region, selects that region's tasks:
    // the group name is the same in both, and each region answers its own.
    let other = task_arns(
        &listed(
            &emulator,
            "eu-west-1",
            &json!({ "Filters": [{ "GroupName": "app-one" }] }),
        )
        .await,
    );
    assert_eq!(
        other,
        stored_in(&started, "eu-west-1"),
        "case \"the same group name in another region\": expected that region's tasks, \
         got {other:?}",
    );
}

/// A listing pages through the selected tasks exactly once, in task-ARN order,
/// whatever the page size — and it reports: the stored tasks are unchanged after
/// every page.
#[tokio::test]
async fn a_listing_pages_in_task_arn_order_and_changes_nothing() {
    let (emulator, started) = populated().await;
    let region = "us-east-1";
    let expected = stored_in(&started, region);
    // The modelled `MaxResults` range is 1..50, so the page sizes span one item, a
    // partial page, exactly the stored count, and the modelled maximum.
    for page_size in [1usize, 2, 3, expected.len(), 50] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            // `MaxResults` and `NextToken` are members of this operation's BODY: the
            // model binds neither to the query string.
            let mut body = json!({ "MaxResults": page_size });
            if let Some(resume) = &token {
                body.as_object_mut()
                    .expect("a JSON object")
                    .insert("NextToken".to_string(), json!(resume));
            }
            let page = emulator.ok_in(region, "/list-tag-sync-tasks", &body).await;
            let arns = task_arns(
                page["TagSyncTasks"]
                    .as_array()
                    .unwrap_or_else(|| panic!("case page_size={page_size}: expected TagSyncTasks, got {page}")),
            );
            assert!(
                arns.len() <= page_size,
                "case page_size={page_size}: a page carried {} items, got {page}",
                arns.len(),
            );
            seen.extend(arns);
            token = page["NextToken"].as_str().map(str::to_string);
            if let Some(resume) = &token {
                assert!(
                    is_modelled_token(resume),
                    "case page_size={page_size}: NextToken {resume:?} is not the modelled form",
                );
            }
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case page_size={page_size}: paging did not terminate, saw {seen:?}",
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "case page_size={page_size}: expected every stored task once each in \
             task-ARN order, got {seen:?}",
        );
    }

    // Listing reports; it does not change what a later caller sees, in any region.
    for task in &started {
        let read = emulator
            .ok_in(
                task.region,
                "/get-tag-sync-task",
                &json!({ "TaskArn": &task.task_arn }),
            )
            .await;
        assert_eq!(
            read["TaskArn"].as_str(),
            Some(task.task_arn.as_str()),
            "case {:?}: expected the task to survive the listings, got {read}",
            task.label,
        );
    }
    let after = task_arns(&listed(&emulator, region, &json!({})).await);
    assert_eq!(
        after, expected,
        "case \"after the listings\": expected the same tasks, got {after:?}",
    );
}

/// The most `Filters` entries one call is served with: a request at the bound is
/// answered, and one past it is refused.
///
/// Not an AWS quota — it is this emulator's bound on the work one request asks for,
/// since every entry is applied to every task of the region.
const MAX_FILTERS: usize = 10;

/// A request this operation cannot answer is refused with `BadRequestException`
/// (400), and a value outside a modelled constraint never reaches the handler.
#[tokio::test]
async fn a_request_the_listing_cannot_answer_is_refused() {
    let (emulator, started) = populated().await;
    let region = "us-east-1";
    let entry = json!({ "GroupName": "app-one" });
    // A token ANOTHER listing of this service issued: it names a resume point in that
    // listing, so it cannot resume this one.
    let groups_page = emulator
        .ok_in(region, "/groups-list?maxResults=1", &json!({}))
        .await;
    let other_listings_token = groups_page["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("expected ListGroups to issue a NextToken, got {groups_page}"))
        .to_string();
    let refused: Vec<(&str, Value)> = vec![
        (
            "a Filters entry that names no group at all",
            json!({ "Filters": [{}] }),
        ),
        (
            "an entry that names no group beside one that does",
            json!({ "Filters": [entry.clone(), {}] }),
        ),
        (
            "a NextToken no listing issued",
            json!({ "NextToken": "bm90LWEtdG9rZW4=" }),
        ),
        (
            "a NextToken issued by ANOTHER listing of this service",
            json!({ "NextToken": other_listings_token }),
        ),
        (
            "more Filters entries than one call applies",
            json!({ "Filters": Value::Array(vec![entry.clone(); MAX_FILTERS + 1]) }),
        ),
        // Outside the modelled `MaxResults` range 1..50 — refused at the boundary,
        // with the same modelled code.
        ("a MaxResults of zero", json!({ "MaxResults": 0 })),
        ("a MaxResults past the modelled maximum", json!({ "MaxResults": 51 })),
        ("a negative MaxResults", json!({ "MaxResults": -1 })),
    ];
    for (label, body) in refused {
        let (status, answered) = emulator
            .call_in(region, "/list-tag-sync-tasks", &body)
            .await;
        assert_eq!(
            (status, error_code(&answered)),
            (400, "BadRequestException"),
            "case {label:?} ({body}): expected 400 BadRequestException, \
             got {status} {answered}",
        );
    }

    // A request AT the filter bound is answered, and the refusals changed nothing.
    let answered = task_arns(
        &listed(
            &emulator,
            region,
            &json!({ "Filters": Value::Array(vec![entry; MAX_FILTERS]) }),
        )
        .await,
    );
    let on_the_group: Vec<String> = stored_in(&started, region)
        .into_iter()
        .filter(|arn| {
            started
                .iter()
                .any(|task| &task.task_arn == arn && task.group == "app-one")
        })
        .collect();
    assert_eq!(
        answered, on_the_group,
        "case \"{MAX_FILTERS} filters\": expected the group's tasks, got {answered:?}",
    );
    let every = task_arns(&listed(&emulator, region, &json!({})).await);
    assert_eq!(
        every,
        stored_in(&started, region),
        "case \"after the refusals\": expected every stored task, got {every:?}",
    );
}
