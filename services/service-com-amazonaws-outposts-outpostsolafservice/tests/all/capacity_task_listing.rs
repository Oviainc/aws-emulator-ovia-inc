// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListCapacityTasks`, driven over the wire through the same
//! front door as the rest of this crate's suites (whose harness it reuses).
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. `ListCapacityTasks` reports every capacity task of the region the request is
//!    addressed to, exactly once, in creation order, and each one as the summary of
//!    ITS OWN stored record: every member the modelled `CapacityTaskSummary` carries
//!    is the value the task's own read reports, and the five members a read carries
//!    that a summary does not are absent from it.
//! 2. The list is cut into pages of at most `MaxResults` with a `NextToken` that
//!    resumes exactly where the previous page stopped — for any page size — and a
//!    page that exhausts the list issues no token.
//! 3. A `NextToken` this service did not issue for THIS list is a bad request (400
//!    `ValidationException`), never a silently truncated or empty page; so are a page
//!    size outside the modelled range and a status outside the modelled enum.
//! 4. Each filter narrows the list on one member of a task, several filters narrow
//!    together, an absent or empty filter narrows nothing, and a filter that is a
//!    list of values accepts a task matching any one of them. A filter is a filter
//!    and not an identifier the request is addressed to, so one naming an Outpost
//!    that holds no task reports no task rather than failing.
//! 5. A capacity task is a task of its Outpost, and an Outpost belongs to one region,
//!    so each region's list reports that region's tasks and no other's.
//! 6. The list reports the status each task holds at the instant it is asked for —
//!    the clock-driven progression, exactly as a read reports it — and listing
//!    changes nothing about any task.

use std::collections::{BTreeSet, VecDeque};

use serde_json::{json, Map, Value};

use service_com_amazonaws_outposts_outpostsolafservice::ops::types::CAPACITY_TASK_STATUS_VALUES;
use service_com_amazonaws_outposts_outpostsolafservice::state::{
    CapacityTaskRecord, CAPACITY_TASK_COMPLETED, CAPACITY_TASK_REQUESTED,
};

use crate::capacity_task_lifecycle::{
    an_order, capacity_route, install, install_capacity, pool, settle_capacity_task, task_route,
    INSTALLED,
};
use crate::site_and_outpost_lifecycle::{
    query_value, refusal, text, Outposts, Refusal, ABSENT_OUTPOST_ID, HOME_REGION,
};

/// The route `ListCapacityTasks` is bound to: an ACCOUNT-level collection, not one
/// nested under an Outpost — `GET /capacity/tasks`, from
/// `smithy_query(query="operation", operation="ListCapacityTasks")`.
const TASKS_ROUTE: &str = "/capacity/tasks";

/// The eight members the modelled `CapacityTaskSummary` carries, from
/// `smithy_query(query="shape", shape="CapacityTaskSummary")`.
const SUMMARY_MEMBERS: [&str; 8] = [
    "CapacityTaskId",
    "OutpostId",
    "OrderId",
    "AssetId",
    "CapacityTaskStatus",
    "CreationDate",
    "CompletionDate",
    "LastModifiedDate",
];

/// The five members `GetCapacityTask` reports that a `CapacityTaskSummary` does not,
/// from the same two shapes: the task's own request and outcome detail, which the
/// model keeps on the read alone.
const NOT_IN_SUMMARY: [&str; 5] = [
    "DryRun",
    "Failed",
    "InstancesToExclude",
    "RequestedInstancePools",
    "TaskActionOnBlockingInstances",
];

/// `GET /capacity/tasks` addressed to `region`, returning the whole output.
async fn list_in(outposts: &Outposts, region: &str, query: &str) -> Value {
    outposts
        .ok_in(region, "GET", &format!("{TASKS_ROUTE}{query}"), None)
        .await
}

/// `GET /capacity/tasks` in [`HOME_REGION`], returning the whole output.
async fn list(outposts: &Outposts, query: &str) -> Value {
    list_in(outposts, HOME_REGION, query).await
}

/// The `CapacityTaskSummary` list an output carries, which is always present — the
/// modelled `CapacityTaskList` allows the empty list, so "no tasks" is `[]`.
fn summaries(output: &Value) -> Vec<Value> {
    output
        .get("CapacityTasks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("ListCapacityTasks answers a CapacityTasks list, got {output}"))
}

/// The `CapacityTaskId` of every summary an output carries, in the order it reported
/// them.
fn ids(output: &Value) -> Vec<String> {
    summaries(output)
        .iter()
        .map(|summary| text(summary, "CapacityTaskId"))
        .collect()
}

/// What a `GetCapacityTask` answer looks like once narrowed to the members a
/// `CapacityTaskSummary` carries: the summary a list has to report for that task.
fn summary_of(read: &Value) -> Value {
    let mut narrowed = Map::new();
    for member in SUMMARY_MEMBERS {
        if let Some(value) = read.get(member) {
            narrowed.insert(member.to_string(), value.clone());
        }
    }
    Value::Object(narrowed)
}

/// One `StartCapacityTask` row: the case, the Outpost the task runs on and the
/// `CapacityTaskId` the start minted.
struct Started {
    case: &'static str,
    outpost_id: String,
    capacity_task_id: String,
}

/// A set of started capacity tasks spanning what a task generalises over: how many
/// instance pools it requests (none, one, several), whether it names an order, an
/// asset, instances to exclude or an action on blocking instances, and both values of
/// the modelled `TaskActionOnBlockingInstances` enum.
///
/// Each task runs on an Outpost of its own, because only one capacity task is active
/// on an Outpost at a time (`crate::capacity_task_lifecycle`), and they are started in
/// this order — which is the creation order the list has to report them in.
async fn started_tasks(outposts: &Outposts) -> Vec<Started> {
    let rows: Vec<(&'static str, Value)> = vec![
        ("one instance pool", json!({ "InstancePools": [pool("m5.4xlarge", 1)] })),
        ("no instance pools at all", json!({ "InstancePools": [] })),
        (
            "several pools and an action on blocking instances",
            json!({
                "InstancePools": [pool("m5.4xlarge", 2), pool("c5.2xlarge", 1)],
                "TaskActionOnBlockingInstances": "WAIT_FOR_EVACUATION",
            }),
        ),
        (
            "an order of the account and instances to exclude",
            json!({
                "InstancePools": [pool("m5.24xlarge", 1)],
                "OrderId": Value::Null,
                "InstancesToExclude": { "Instances": ["i-0123456789abcdef0"] },
                "TaskActionOnBlockingInstances": "FAIL_TASK",
            }),
        ),
        (
            "hardware installed on the Outpost",
            json!({
                "InstancePools": [pool("c5.4xlarge", 3)],
                "AssetId": INSTALLED[0],
            }),
        ),
    ];

    let mut started: Vec<Started> = Vec::new();
    for (index, (case, request)) in rows.into_iter().enumerate() {
        let (outpost_id, _) = outposts
            .site_and_outpost_in_service(&format!("task-listing-{index}"))
            .await;
        let mut request = request;
        // The two optional references a task carries name OTHER resources, so a row
        // that names one is made to resolve first: the asset is recorded as hardware
        // of this Outpost, and the order is one placed against it.
        if let Some(asset_id) = request.get("AssetId").and_then(Value::as_str) {
            install(outposts, &outpost_id, &[asset_id], case);
        }
        if request.get("OrderId").is_some() {
            request["OrderId"] = json!(an_order(outposts, &outpost_id).await);
        }
        let answered = outposts
            .ok("POST", &capacity_route(&outpost_id), Some(&request))
            .await;
        started.push(Started {
            case,
            outpost_id,
            capacity_task_id: text(&answered, "CapacityTaskId"),
        });
    }
    started
}

/// `GetCapacityTask` on one started task.
async fn read(outposts: &Outposts, task: &Started) -> Value {
    outposts
        .ok(
            "GET",
            &task_route(&task.outpost_id, &task.capacity_task_id),
            None,
        )
        .await
}

/// Record a capacity task of `outpost_id` in `status` through the store, and answer
/// the `CapacityTaskId` it is keyed by.
///
/// The modelled `CapacityTaskStatus` declares seven values and the operations this
/// emulator implements produce three of them, so a case that needs a task in one of
/// the other four records it the way [`crate::outpost_assets`] records hardware:
/// through the store, which is the seam a case uses for state no call of this API
/// creates. The record takes the store's next creation sequence, so a seeded task
/// takes its place in the one creation order the list is reported in.
fn seed(outposts: &Outposts, outpost_id: &str, status: &str, index: usize) -> String {
    let now = std::time::SystemTime::now();
    let store = outposts.store();
    let mut store = store.lock();
    let sequence = store.created;
    store.created += 1;
    let capacity_task_id = format!("cap-{:017x}", index + 1);
    store.capacity_tasks.insert(
        capacity_task_id.clone(),
        CapacityTaskRecord {
            sequence,
            capacity_task_id: capacity_task_id.clone(),
            outpost_id: outpost_id.to_string(),
            order_id: None,
            asset_id: None,
            requested_instance_pools: Vec::new(),
            instances_to_exclude: None,
            dry_run: false,
            task_action_on_blocking_instances: None,
            status: status.to_string(),
            failed: None,
            created_at: now,
            last_modified_at: now,
            completed_at: (status == CAPACITY_TASK_COMPLETED).then_some(now),
            // Nothing is owed: the status is the one this record is recorded in.
            pending: VecDeque::new(),
        },
    );
    capacity_task_id
}

/// Rule 1: every stored capacity task is reported, once, in creation order, as the
/// summary of its own record — and listing is a read, so repeating it answers the
/// same thing and the task's own route still answers the same record.
#[tokio::test]
async fn list_capacity_tasks_reports_every_stored_task_as_its_own_summary() {
    let outposts = Outposts::new();

    // A region nothing was created in lists nothing, as the empty list the modelled
    // list allows rather than as an absent member.
    let empty = list(&outposts, "").await;
    assert!(
        summaries(&empty).is_empty() && empty.get("NextToken").is_none(),
        "with no capacity tasks stored, ListCapacityTasks answers an empty list and no \
         NextToken, got {empty}"
    );

    let started = started_tasks(&outposts).await;
    // Every task is let finish first, so the three clock-driven members are settled
    // and the list and the read cannot be straddling a transition. Rule 6 states what
    // a list reports while a task is still moving.
    settle_capacity_task();

    let listed = list(&outposts, "").await;
    assert_eq!(
        ids(&listed),
        started
            .iter()
            .map(|task| task.capacity_task_id.clone())
            .collect::<Vec<String>>(),
        "ListCapacityTasks must report every stored task exactly once in creation order, \
         got {listed}"
    );

    for (index, task) in started.iter().enumerate() {
        let case = task.case;
        let summary = &summaries(&listed)[index];
        let read = read(&outposts, task).await;
        assert_eq!(
            summary,
            &summary_of(&read),
            "case {case:?}: the summary must be what GetCapacityTask reports for the \
             members a summary carries, got {summary}"
        );
        for omitted in NOT_IN_SUMMARY {
            assert!(
                summary.get(omitted).is_none(),
                "case {case:?}: {omitted} is no member of a CapacityTaskSummary, got {summary}"
            );
        }
        // And the summary carries nothing the two shapes do not account for.
        let members: BTreeSet<&str> = summary
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: a summary is an object, got {summary}"))
            .keys()
            .map(String::as_str)
            .collect();
        let modelled: BTreeSet<&str> = SUMMARY_MEMBERS.into_iter().collect();
        assert!(
            members.is_subset(&modelled),
            "case {case:?}: a summary carries only modelled members, got {summary}"
        );
    }

    // Listing reports; it does not change what a later caller sees.
    let again = list(&outposts, "").await;
    assert_eq!(
        again, listed,
        "a second ListCapacityTasks must answer exactly what the first did, got {again}"
    );
    for task in &started {
        let case = task.case;
        let summary = summaries(&listed)
            .into_iter()
            .find(|summary| text(summary, "CapacityTaskId") == task.capacity_task_id)
            .unwrap_or_else(|| panic!("case {case:?}: the list reports the stored task"));
        let read = read(&outposts, task).await;
        assert_eq!(
            summary,
            summary_of(&read),
            "case {case:?}: listing must not change the task its own route answers"
        );
    }
}

/// Rule 2: for EVERY page size, paging the list from the first `NextToken` to the
/// last yields exactly the stored tasks, once each, in the same stable order.
///
/// The page sizes span one task per page, an exact divisor of the count, a size that
/// leaves a short last page, the count itself, and more than the count — so a token
/// that duplicated or skipped a task at any boundary would show up. A token naming a
/// boundary past the end of the list is a page with nothing in it and no further
/// token.
#[tokio::test]
async fn paging_the_list_covers_every_capacity_task_exactly_once() {
    let outposts = Outposts::new();
    let expected: Vec<String> = started_tasks(&outposts)
        .await
        .into_iter()
        .map(|task| task.capacity_task_id)
        .collect();

    let count = expected.len();
    for size in [1usize, 2, 3, count, count + 4] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let query = match &token {
                Some(token) => format!("?MaxResults={size}&NextToken={}", query_value(token)),
                None => format!("?MaxResults={size}"),
            };
            let page = list(&outposts, &query).await;
            let page_ids = ids(&page);
            assert!(
                page_ids.len() <= size,
                "case size {size}: a page holds at most MaxResults tasks, got {page}"
            );
            collected.extend(page_ids);
            pages += 1;
            assert!(
                pages <= count + 1,
                "case size {size}: paging must terminate, got {collected:?}"
            );
            match page.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            collected, expected,
            "case size {size}: paging must report every task once, in the list's order"
        );
        assert_eq!(
            pages,
            count.div_ceil(size),
            "case size {size}: a list of {count} tasks served {size} at a time takes {} \
             pages, got {pages}",
            count.div_ceil(size)
        );
    }

    // A boundary past the end of the list: nothing left to report, and no token.
    let past_the_end = list(
        &outposts,
        &format!(
            "?NextToken={}",
            query_value(&format!("{}##capacityTasks", count + 100))
        ),
    )
    .await;
    assert!(
        summaries(&past_the_end).is_empty() && past_the_end.get("NextToken").is_none(),
        "a token past the end of the list is an empty page with no token, got {past_the_end}"
    );
}

/// Rule 3: a `NextToken` this service did not issue for THIS list, a page size
/// outside the modelled `range(1..1000)`, and a status outside the modelled
/// `CapacityTaskStatus` enum are all refused as bad requests.
#[tokio::test]
async fn a_foreign_page_token_or_an_unmodelled_filter_value_is_refused() {
    let outposts = Outposts::new();
    let (outpost_id, _) = outposts
        .site_and_outpost_in_service("task-listing-refusals")
        .await;
    let _ = outposts
        .ok(
            "POST",
            &capacity_route(&outpost_id),
            Some(&json!({ "InstancePools": [pool("m5.4xlarge", 1)] })),
        )
        .await;

    let cases: Vec<Refusal> = vec![
        refusal(
            "a token stamped for ListSites",
            "GET",
            format!("{TASKS_ROUTE}?NextToken=1%23%23sites"),
            400,
            "ValidationException",
        ),
        refusal(
            "a token stamped for ListQuotes",
            "GET",
            format!("{TASKS_ROUTE}?NextToken=1%23%23quotes"),
            400,
            "ValidationException",
        ),
        refusal(
            "a token stamped for ListAssets",
            "GET",
            format!("{TASKS_ROUTE}?NextToken=1%23%23assets"),
            400,
            "ValidationException",
        ),
        refusal(
            "a token whose boundary is not a number",
            "GET",
            format!("{TASKS_ROUTE}?NextToken=abc%23%23capacityTasks"),
            400,
            "ValidationException",
        ),
        refusal(
            "a page size below the modelled minimum",
            "GET",
            format!("{TASKS_ROUTE}?MaxResults=0"),
            400,
            "ValidationException",
        ),
        refusal(
            "a page size above the modelled maximum",
            "GET",
            format!("{TASKS_ROUTE}?MaxResults=1001"),
            400,
            "ValidationException",
        ),
        refusal(
            "a status outside the modelled enum",
            "GET",
            format!("{TASKS_ROUTE}?CapacityTaskStatusFilter=RUNNING"),
            400,
            "ValidationException",
        ),
        refusal(
            "an Outpost filter that is no Outpost identifier",
            "GET",
            format!("{TASKS_ROUTE}?OutpostIdentifierFilter=op-not-an-identifier"),
            400,
            "ValidationException",
        ),
    ];
    outposts.expect_refusals(&cases).await;

    // Both published bounds of the page-size range are served, not refused.
    for size in [1, 1000] {
        let page = list(&outposts, &format!("?MaxResults={size}")).await;
        assert_eq!(
            summaries(&page).len(),
            1,
            "case MaxResults={size}: the one stored task is reported, got {page}"
        );
    }

    // And the stamp cuts both ways: a token ListCapacityTasks issued is not a
    // boundary of another list.
    let replayed = outposts
        .call("GET", "/quotes?NextToken=1%23%23capacityTasks", None)
        .await;
    assert_eq!(
        (replayed.status, replayed.error_code.as_str()),
        (400, "ValidationException"),
        "a ListCapacityTasks token replayed on ListQuotes is refused, got {} {} {}",
        replayed.status,
        replayed.error_code,
        replayed.body
    );
}

/// Rule 4: each filter narrows the list, several filters narrow together, and a
/// filter naming an Outpost with no task reports no task.
///
/// The rows span both identifier forms the Outpost filter accepts, an Outpost that
/// holds no task and one the store does not hold at all, every value of the modelled
/// `CapacityTaskStatus` enum on its own, several values together, every value at
/// once, and the two filters combined — both where they overlap and where they do
/// not.
#[tokio::test]
async fn the_filters_narrow_the_list() {
    let outposts = Outposts::new();
    let (first, first_arn) = outposts
        .site_and_outpost_in_service("task-filter-first")
        .await;
    let (second, _) = outposts
        .site_and_outpost_in_service("task-filter-second")
        .await;
    let (idle, _) = outposts
        .site_and_outpost_in_service("task-filter-idle")
        .await;

    // One task per modelled status, spread across the two Outposts that hold tasks,
    // so no row can pass by selecting everything.
    let mut held: Vec<(String, String, &'static str)> = Vec::new();
    for (index, status) in CAPACITY_TASK_STATUS_VALUES.iter().enumerate() {
        let outpost_id = match index % 2 {
            0 => &first,
            _ => &second,
        };
        let capacity_task_id = seed(&outposts, outpost_id, status, index);
        held.push((capacity_task_id, outpost_id.clone(), status));
    }

    // What the wire must answer for the filter that states the same selection, in the
    // creation order the tasks were recorded in.
    let select = |outpost_id: Option<&str>, statuses: &[&str]| -> Vec<String> {
        held.iter()
            .filter(|(_, on, status)| {
                outpost_id.is_none_or(|wanted| on == wanted)
                    && (statuses.is_empty() || statuses.contains(status))
            })
            .map(|(capacity_task_id, _, _)| capacity_task_id.clone())
            .collect()
    };

    let first_status = CAPACITY_TASK_STATUS_VALUES[0];
    let second_status = CAPACITY_TASK_STATUS_VALUES[1];
    let every_status: String = CAPACITY_TASK_STATUS_VALUES
        .iter()
        .map(|status| format!("CapacityTaskStatusFilter={status}"))
        .collect::<Vec<String>>()
        .join("&");

    let mut cases: Vec<(String, String, Vec<String>)> = vec![
        ("no filter at all".to_string(), String::new(), select(None, &[])),
        (
            "by bare Outpost id".to_string(),
            format!("?OutpostIdentifierFilter={first}"),
            select(Some(&first), &[]),
        ),
        (
            "by the same Outpost's ARN".to_string(),
            format!("?OutpostIdentifierFilter={}", query_value(&first_arn)),
            select(Some(&first), &[]),
        ),
        (
            "by the other Outpost".to_string(),
            format!("?OutpostIdentifierFilter={second}"),
            select(Some(&second), &[]),
        ),
        (
            "an Outpost that holds no task".to_string(),
            format!("?OutpostIdentifierFilter={idle}"),
            Vec::new(),
        ),
        (
            "an Outpost the store does not hold".to_string(),
            format!("?OutpostIdentifierFilter={ABSENT_OUTPOST_ID}"),
            Vec::new(),
        ),
        (
            "two statuses at once".to_string(),
            format!(
                "?CapacityTaskStatusFilter={first_status}&CapacityTaskStatusFilter={second_status}"
            ),
            select(None, &[first_status, second_status]),
        ),
        (
            "every modelled status at once".to_string(),
            format!("?{every_status}"),
            select(None, &[]),
        ),
        (
            "both filters, where they overlap".to_string(),
            format!("?OutpostIdentifierFilter={first}&CapacityTaskStatusFilter={first_status}"),
            select(Some(&first), &[first_status]),
        ),
        (
            "both filters, where they overlap in nothing".to_string(),
            format!("?OutpostIdentifierFilter={second}&CapacityTaskStatusFilter={first_status}"),
            select(Some(&second), &[first_status]),
        ),
    ];
    // Every value of the enum on its own: each selects exactly the task recorded in
    // it, whichever Outpost holds it.
    for status in CAPACITY_TASK_STATUS_VALUES {
        cases.push((
            format!("only {status}"),
            format!("?CapacityTaskStatusFilter={status}"),
            select(None, &[status]),
        ));
    }

    for (case, query, expected) in &cases {
        let listed = ids(&list(&outposts, query).await);
        assert_eq!(
            &listed, expected,
            "case {case:?}: {query:?} must report exactly {expected:?}, got {listed:?}"
        );
    }

    // The overlap row is not vacuous: the filters really do narrow to fewer tasks
    // than either of them alone.
    let both = select(Some(&first), &[first_status]);
    assert!(
        both.len() == 1 && both.len() < select(Some(&first), &[]).len(),
        "the combined filters must narrow to one of this Outpost's several tasks, got {both:?}"
    );
}

/// Rule 5: each region's list reports that region's capacity tasks and no other's.
#[tokio::test]
async fn each_region_lists_only_its_own_capacity_tasks() {
    let outposts = Outposts::new();
    // The counts differ per region so a list answering another region's inventory
    // cannot pass by coincidence.
    let plan: Vec<(&str, usize)> = vec![("us-east-1", 3), ("eu-west-1", 1), ("ap-south-1", 2)];

    let mut expected: Vec<(&str, BTreeSet<String>)> = Vec::new();
    for (region, count) in &plan {
        let mut task_ids: BTreeSet<String> = BTreeSet::new();
        for index in 0..*count {
            let name = format!("{region}-task-listing-{index}");
            let site_id = outposts.site_id_in(region, &format!("{name}-site")).await;
            let outpost = outposts
                .create_outpost_in(region, &json!({ "Name": name, "SiteId": site_id }))
                .await;
            let outpost_id = text(&outpost, "OutpostId");
            // A capacity task is refused on an Outpost whose hardware is not in
            // service ([`crate::precondition_gates`]).
            crate::site_and_outpost_lifecycle::mark_outpost_active(&outposts, &outpost_id);
            let answered = outposts
                .ok_in(
                    region,
                    "POST",
                    &capacity_route(&outpost_id),
                    Some(&json!({ "InstancePools": [pool("m5.4xlarge", 1)] })),
                )
                .await;
            task_ids.insert(text(&answered, "CapacityTaskId"));
        }
        expected.push((region, task_ids));
    }

    for (region, task_ids) in &expected {
        let listed: BTreeSet<String> = ids(&list_in(&outposts, region, "").await)
            .into_iter()
            .collect();
        assert_eq!(
            &listed, task_ids,
            "case {region}: ListCapacityTasks in {region} reports exactly that region's \
             tasks, got {listed:?}"
        );
    }
}

/// Rule 6: the list reports the status each task holds at the instant it is asked
/// for, and reports it exactly as the task's own read does.
///
/// A task this emulator starts is still `REQUESTED` when the start answers and
/// `COMPLETED` once its progression has fallen due, so the two rows are the two sides
/// of that span — and listing never moves a task itself, which the read after each
/// list is what states.
#[tokio::test]
async fn the_list_reports_the_status_each_task_holds_now() {
    let outposts = Outposts::new();
    let (outpost_id, _) = outposts
        .site_and_outpost_in_service("task-listing-clock")
        .await;
    // Hardware that can hold the configuration the task below asks for, so the task
    // this row follows across the span is one that COMPLETES.
    install_capacity(&outposts, &outpost_id, &INSTALLED[..1], "task-listing-clock");
    let answered = outposts
        .ok(
            "POST",
            &capacity_route(&outpost_id),
            Some(&json!({ "InstancePools": [pool("m5.4xlarge", 1)] })),
        )
        .await;
    let task = Started {
        case: "a task on the clock",
        outpost_id,
        capacity_task_id: text(&answered, "CapacityTaskId"),
    };

    for (case, expected) in [
        ("straight after the start", CAPACITY_TASK_REQUESTED),
        ("once the progression has fallen due", CAPACITY_TASK_COMPLETED),
    ] {
        if expected == CAPACITY_TASK_COMPLETED {
            settle_capacity_task();
        }
        let listed = list(&outposts, "").await;
        let summary = summaries(&listed)
            .into_iter()
            .find(|summary| text(summary, "CapacityTaskId") == task.capacity_task_id)
            .unwrap_or_else(|| panic!("case {case:?}: the list reports the started task"));
        let status = text(&summary, "CapacityTaskStatus");
        assert_eq!(
            status, expected,
            "case {case:?}: the list must report the status the task holds now \
             ({expected}), got {status:?}"
        );
        // Listing materialised what the clock owed and nothing else: the task's own
        // read answers the same record, status included.
        let read = read(&outposts, &task).await;
        assert_eq!(
            summary,
            summary_of(&read),
            "case {case:?}: the summary and the read must agree member for member"
        );
    }
}
