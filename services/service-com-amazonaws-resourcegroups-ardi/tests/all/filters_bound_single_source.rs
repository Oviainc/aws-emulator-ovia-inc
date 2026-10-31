// SPDX-License-Identifier: CC0-1.0

//! The per-request `Filters` bound as ONE rule, pinned end to end through the two
//! listings that each used to decide it for themselves — `ListGroups` and
//! `ListTagSyncTasks`.
//!
//! Every request goes through the same `MultiServiceRouter::dispatch` the served
//! emulator uses (the harness is the sibling suite's `Emulator`), so these exercise
//! the whole restJson1 path: routing, boundary validation, the handler, and
//! serialization.
//!
//! The one rule under test, asserted over a TABLE of entry counts crossed with both
//! listings rather than one row: a listing applies at most the number of `Filters`
//! entries the service decides in one place (`state::MAX_FILTERS`) to one call,
//! narrows its answer by every entry at or below it, and refuses a longer list with
//! a `BadRequestException` (400) whose message is the single sentence
//! `state::check_filters` reports for that count — so neither the bound nor the
//! sentence can drift between the two listings, or away from the shared rule both
//! now call. The sibling `filters_bound_rule` suite asserts the same bound holds of
//! every `Filters`-taking listing; what this one adds is the tie from each former
//! copy's wire answer to the shared rule's own value and sentence.

use serde_json::{json, Value};

use service_com_amazonaws_resourcegroups_ardi::state::{check_filters, MAX_FILTERS};

use super::resource_group_lifecycle::{error_code, static_group, Emulator};
use super::tag_sync_tasks::{group_arn, role_arn};

/// The region every request of this suite is signed for, and so the region the
/// fixtures live in.
const REGION: &str = "us-east-1";

/// The group both listings' filtering selects: it is created with a service
/// configuration, and it is the group one of the two tag-sync tasks syncs.
const SELECTED_GROUP: &str = "single-source-selected";

/// The group neither filtering selects: no service configuration, and the other
/// tag-sync task's group — so a filtered answer is a strict subset of what the
/// unfiltered listing covers.
const OTHER_GROUP: &str = "single-source-other";

/// The configuration type [`SELECTED_GROUP`] is created with, which is the value a
/// `configuration-type` entry names.
///
/// Source: the configuration the shared `static_group` fixture sends.
const SELECTED_CONFIGURATION_TYPE: &str = "AWS::EC2::HostManagement";

/// Another configuration type the `configuration-type` filter documents, which no
/// group of this suite is created with: the second spelling of the `ListGroups`
/// filtering carries it beside the selected one, so the entries of a list DIFFER
/// rather than being one entry repeated, and naming a type no group declares
/// narrows the selection no further.
///
/// Source: the `configuration-type` catalog of
/// `smithy_query(query="doc",shape="ListGroupsInput",member="Filters")`.
const OTHER_CONFIGURATION_TYPE: &str = "AWS::CloudFormation::Stack";

/// One of the two listings that used to carry its own copy of the bound: where a
/// filtered call of it goes, the two spellings of the filtering it is driven with,
/// and how a page of it identifies the items it reports.
struct FormerCopy {
    label: &'static str,
    path: &'static str,
    entry: Value,
    alt: Value,
    items_member: &'static str,
    id_member: &'static str,
}

/// The two listings whose copies of the bound were hoisted into the shared rule.
fn former_copies() -> Vec<FormerCopy> {
    vec![
        // `configuration-type` selects the groups whose service configuration
        // declares one of the entry's values; only the selected group has one.
        FormerCopy {
            label: "ListGroups",
            path: "/groups-list",
            entry: json!({
                "Name": "configuration-type",
                "Values": [SELECTED_CONFIGURATION_TYPE],
            }),
            alt: json!({
                "Name": "configuration-type",
                "Values": [SELECTED_CONFIGURATION_TYPE, OTHER_CONFIGURATION_TYPE],
            }),
            items_member: "GroupIdentifiers",
            id_member: "GroupName",
        },
        // A tag-sync task's filter entry names the group whose tasks are listed, in
        // either of the two ways of naming one.
        FormerCopy {
            label: "ListTagSyncTasks",
            path: "/list-tag-sync-tasks",
            entry: json!({ "GroupName": SELECTED_GROUP }),
            alt: json!({ "GroupArn": group_arn(REGION, SELECTED_GROUP) }),
            items_member: "TagSyncTasks",
            id_member: "TaskArn",
        },
    ]
}

/// `count` `Filters` entries for `listing`, alternating its two spellings so a list
/// of several entries carries entries that differ.
fn entries(listing: &FormerCopy, count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| match index % 2 {
                0 => listing.entry.clone(),
                _ => listing.alt.clone(),
            })
            .collect(),
    )
}

/// The identifiers one page carries, in the order the page lists them.
fn page_ids(case: &str, listing: &FormerCopy, page: &Value) -> Vec<String> {
    let items_member = listing.items_member;
    let id_member = listing.id_member;
    page[items_member]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: a page must carry {items_member}, got {page}"))
        .iter()
        .map(|item| {
            item[id_member]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: an item must carry {id_member}, got {item}")
                })
                .to_string()
        })
        .collect()
}

/// The `message` an error response carries.
fn message(case: &str, body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a message, got {body}"))
        .to_string()
}

/// Start one tag-sync task on `group` and answer the ARN the service minted for it.
async fn start_task(emulator: &Emulator, group: &str, tag_key: &str) -> String {
    let started = emulator
        .ok(
            "/start-tag-sync-task",
            &json!({
                "Group": group,
                "RoleArn": role_arn("ResourceGroupsTagSync"),
                "TagKey": tag_key,
                "TagValue": "single-source",
            }),
        )
        .await;
    started["TaskArn"]
        .as_str()
        .unwrap_or_else(|| panic!("StartTagSyncTask must answer a TaskArn, got {started}"))
        .to_string()
}

/// An emulator holding two groups and one tag-sync task on each, and the identifier
/// each listing of [`former_copies`] reports for its filtering.
async fn populated() -> (Emulator, Vec<String>) {
    let emulator = Emulator::new();
    emulator.ok("/groups", &static_group(SELECTED_GROUP)).await;
    emulator.ok("/groups", &json!({ "Name": OTHER_GROUP })).await;
    let selected_task = start_task(&emulator, SELECTED_GROUP, "Selected").await;
    start_task(&emulator, OTHER_GROUP, "Other").await;

    let expected = vec![SELECTED_GROUP.to_string(), selected_task];
    (emulator, expected)
}

/// Both former copies serve every `Filters` list at or below the ONE maximum the
/// service decides, and each answers the selection its entries name — so the bound
/// is the shared rule's value in both, and a bound on the list's LENGTH only.
#[tokio::test]
async fn both_former_copies_serve_every_list_up_to_the_shared_maximum() {
    let (emulator, expected) = populated().await;
    // The counts: the smallest list that filters at all, two in the middle, the
    // entry below the shared maximum, and the maximum itself.
    let counts = [1usize, 2, 5, MAX_FILTERS - 1, MAX_FILTERS];

    for (listing, expected_id) in former_copies().iter().zip(expected) {
        for count in counts {
            let case = format!("{} with {count} Filters entries", listing.label);
            let page = emulator
                .ok(listing.path, &json!({ "Filters": entries(listing, count) }))
                .await;
            let answered = page_ids(&case, listing, &page);
            assert_eq!(
                answered,
                vec![expected_id.clone()],
                "case {case:?}: expected the selection [{expected_id}], got {answered:?}",
            );
        }
    }
}

/// Both former copies refuse a `Filters` list past the shared maximum with the ONE
/// sentence the shared rule reports for that count, byte for byte — which is what
/// neither listing can drift away from while it calls the shared rule.
#[tokio::test]
async fn both_former_copies_refuse_a_longer_list_with_the_shared_sentence() {
    let (emulator, expected) = populated().await;
    // One past the shared maximum, one further past it, and a list far larger than
    // either listing could apply per item.
    for count in [MAX_FILTERS + 1, MAX_FILTERS + 7, 250] {
        let expected_message = check_filters(count).expect_err(
            "a count past the shared maximum must be refused by the shared rule itself",
        );
        for listing in former_copies() {
            let case = format!("{} with {count} Filters entries", listing.label);
            let (status, body) = emulator
                .call(listing.path, &json!({ "Filters": entries(&listing, count) }))
                .await;
            assert_eq!(
                (status, error_code(&body)),
                (400, "BadRequestException"),
                "case {case:?}: expected 400 BadRequestException, got {status} {body}",
            );
            let answered = message(&case, &body);
            assert_eq!(
                answered, expected_message,
                "case {case:?}: expected the shared rule's sentence {expected_message:?}, \
                 got {answered:?}",
            );
        }
    }

    // A refused list listed nothing and changed nothing: both listings still answer
    // the selection their entries name.
    for (listing, expected_id) in former_copies().iter().zip(expected) {
        let case = format!("{} after a refused list", listing.label);
        let page = emulator
            .ok(listing.path, &json!({ "Filters": entries(listing, 1) }))
            .await;
        let answered = page_ids(&case, listing, &page);
        assert_eq!(
            answered,
            vec![expected_id.clone()],
            "case {case:?}: a refused request must change no listing, got {answered:?}",
        );
    }
}
