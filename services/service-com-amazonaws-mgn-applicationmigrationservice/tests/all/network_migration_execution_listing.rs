// SPDX-License-Identifier: CC0-1.0

//! `ListNetworkMigrationExecutions`, end to end over HTTP.
//!
//! The rule under test: a network migration definition is staged one execution when it
//! is created, and the listing reports the executions recorded under the named
//! definition of the caller's account, in creation order, as the modelled
//! `NetworkMigrationExecution` of each stored record, narrowed by the request's
//! `filters` and cut into `maxResults`-sized pages a `nextToken` resumes; a definition
//! the account holds no record of is refused with 404 `ResourceNotFoundException`.
//!
//! Every assertion goes through the router's `dispatch`, so it exercises the same
//! restJson1 deserialize → handle → serialize path a client drives. The staged
//! execution is read back through the listing; further executions, which no modelled
//! operation creates (every `Start*` network-migration operation names an execution
//! that already exists), are seeded straight into the service's store, the way
//! `network_migration_definition_listing.rs` seeds another account's definitions.
//!
//! The harness (`Mgn`, `error_code`, the region and account constants) is the one
//! `application_lifecycle.rs` defines.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_mgn_applicationmigrationservice::state::NetworkMigrationExecution;

use crate::application_lifecycle::{error_code, Mgn, EMULATOR_ACCOUNT, OTHER_ACCOUNT};

/// The modelled `http.uri` of `CreateNetworkMigrationDefinition`
/// (`smithy_query(query="operation",operation="CreateNetworkMigrationDefinition")`),
/// without the leading slash the harness adds.
const CREATE_DEFINITION: &str = "network-migration/CreateNetworkMigrationDefinition";

/// The modelled `http.uri` of `ListNetworkMigrationExecutions`, likewise.
const LIST: &str = "network-migration/ListNetworkMigrationExecutions";

/// Every member the modelled `NetworkMigrationExecution` declares. Source:
/// `smithy_query(query="closure",operation="ListNetworkMigrationExecutions")` →
/// `NetworkMigrationExecution`.
const EXECUTION_MEMBERS: [&str; 8] = [
    "networkMigrationDefinitionID",
    "networkMigrationExecutionID",
    "status",
    "stage",
    "activity",
    "createdAt",
    "updatedAt",
    "tags",
];

/// The model's `ExecutionStatus` values, in the order the model lists them. Source:
/// `smithy_query(query="closure",operation="ListNetworkMigrationExecutions")` →
/// `ExecutionStatus.values`.
const EXECUTION_STATUSES: [&str; 4] = ["PENDING", "STARTED", "SUCCEEDED", "FAILED"];

/// The model's `ExecutionStage` values, likewise (`ExecutionStage.values`).
const EXECUTION_STAGES: [&str; 5] = [
    "MAPPING",
    "CODE_GENERATION",
    "DEPLOY",
    "DEPLOYED_STACKS_DELETION",
    "ANALYZE",
];

/// The model's `ExecutionStageActivity` values, likewise
/// (`ExecutionStageActivity.values`).
const EXECUTION_ACTIVITIES: [&str; 6] = [
    "MAPPING",
    "MAPPING_UPDATE",
    "CODE_GENERATION",
    "DEPLOY",
    "DEPLOYED_STACKS_DELETION",
    "ANALYZE",
];

/// The most ids the modelled `networkMigrationExecutionIDs` filter accepts
/// (`NetworkMigrationExecutionIDsFilter`, len 0..20).
const MAX_FILTER_IDS: usize = 20;

/// The group lengths of the modelled `NetworkMigrationExecutionID` pattern
/// `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`.
const EXECUTION_ID_GROUPS: [usize; 5] = [8, 4, 4, 4, 12];

/// An id in the modelled `NetworkMigrationExecutionID` shape, derived from `seed` so
/// each case names a different execution.
fn execution_id(seed: u32) -> String {
    format!("{seed:08x}-0000-4000-8000-{seed:012x}")
}

/// Whether `id` is in the modelled `NetworkMigrationExecutionID` shape.
fn is_execution_id(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == EXECUTION_ID_GROUPS.len()
        && groups
            .iter()
            .zip(EXECUTION_ID_GROUPS)
            .all(|(group, length)| {
                group.len() == length && group.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        && id.bytes().all(|byte| !byte.is_ascii_uppercase())
}

/// One seeded execution: the case name and the stored record's members.
struct SeedCase {
    case: &'static str,
    status: Option<&'static str>,
    stage: Option<&'static str>,
    activity: Option<&'static str>,
    tags: Vec<(&'static str, &'static str)>,
}

/// The class a listed execution spans: every modelled `ExecutionStatus` value (so the
/// status filter is tested on each), stages and activities from across their catalogs
/// including entries the issue did not cite, tag counts (none, one, several), and an
/// execution carrying none of the optional members at all.
fn seed_cases() -> Vec<SeedCase> {
    vec![
        SeedCase {
            case: "started, code generation, one tag",
            status: Some(EXECUTION_STATUSES[1]),
            stage: Some(EXECUTION_STAGES[1]),
            activity: Some(EXECUTION_ACTIVITIES[2]),
            tags: vec![("env", "dev")],
        },
        SeedCase {
            case: "succeeded, deploy, several tags",
            status: Some(EXECUTION_STATUSES[2]),
            stage: Some(EXECUTION_STAGES[2]),
            activity: Some(EXECUTION_ACTIVITIES[3]),
            tags: vec![("env", "prod"), ("owner", "network-team"), ("tier", "1")],
        },
        SeedCase {
            case: "failed, the last stage of the catalog",
            status: Some(EXECUTION_STATUSES[3]),
            stage: Some(EXECUTION_STAGES[4]),
            activity: Some(EXECUTION_ACTIVITIES[5]),
            tags: vec![("env", "staging")],
        },
        SeedCase {
            case: "started again, a mid-catalog activity",
            status: Some(EXECUTION_STATUSES[1]),
            stage: Some(EXECUTION_STAGES[3]),
            activity: Some(EXECUTION_ACTIVITIES[4]),
            tags: vec![("env", "dev"), ("owner", "network-team")],
        },
        SeedCase {
            case: "pending, first stage, no tags",
            status: Some(EXECUTION_STATUSES[0]),
            stage: Some(EXECUTION_STAGES[0]),
            activity: Some(EXECUTION_ACTIVITIES[0]),
            tags: Vec::new(),
        },
        SeedCase {
            case: "no status, stage, activity or tags",
            status: None,
            stage: None,
            activity: None,
            tags: Vec::new(),
        },
    ]
}

/// A definition of the caller's own account, created through the API, answering its
/// `networkMigrationDefinitionID`.
async fn create_definition(mgn: &Mgn, name: &str) -> String {
    let (status, created) = mgn
        .call(
            CREATE_DEFINITION,
            &json!({
                "name": name,
                "targetS3Configuration": {
                    "s3Bucket": "migration-artifacts",
                    "s3BucketOwner": EMULATOR_ACCOUNT,
                },
                "targetNetwork": {"topology": "ISOLATED_VPC"},
            }),
        )
        .await;
    assert_eq!(
        status, 201,
        "case {name:?}: CreateNetworkMigrationDefinition expected 201, got {status} with {created:?}"
    );
    created["networkMigrationDefinitionID"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no networkMigrationDefinitionID in {created:?}"))
        .to_string()
}

/// The one execution `definition_id` was staged with, read back through the listing.
async fn staged_execution(mgn: &Mgn, definition_id: &str) -> Value {
    let items = list_all(mgn, "the staged execution", definition_id, None, None).await;
    assert_eq!(
        items.len(),
        1,
        "a definition nothing has run must report exactly its one staged execution, got {items:?}"
    );
    items[0].clone()
}

/// Put one execution straight into the store, as if whatever moves an execution on had
/// recorded it, and answer the modelled item a listing must report for it.
fn seed_execution(
    mgn: &Mgn,
    account: &str,
    definition_id: &str,
    id: &str,
    seed: &SeedCase,
    created_at: SystemTime,
) -> Value {
    let updated_at = created_at + Duration::from_secs(60);
    let stored = mgn
        .store()
        .lock()
        .insert_network_migration_execution(NetworkMigrationExecution {
            network_migration_execution_id: id.to_string(),
            network_migration_definition_id: definition_id.to_string(),
            account_id: account.to_string(),
            status: seed.status.map(str::to_string),
            stage: seed.stage.map(str::to_string),
            activity: seed.activity.map(str::to_string),
            created_at: Some(created_at),
            updated_at: Some(updated_at),
            tags: seed
                .tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
        });
    assert!(
        stored,
        "case {:?}: seeding {id:?} must store it (the store bound was not reached)",
        seed.case
    );

    let mut item = json!({
        "networkMigrationDefinitionID": definition_id,
        "networkMigrationExecutionID": id,
        "createdAt": epoch_seconds(created_at),
        "updatedAt": epoch_seconds(updated_at),
    });
    for (member, value) in [
        ("status", seed.status),
        ("stage", seed.stage),
        ("activity", seed.activity),
    ] {
        if let Some(value) = value {
            item[member] = json!(value);
        }
    }
    if !seed.tags.is_empty() {
        item["tags"] = seed
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect::<serde_json::Map<String, Value>>()
            .into();
    }
    item
}

/// An instant as restJson1 renders a `Timestamp` member: epoch seconds.
fn epoch_seconds(at: SystemTime) -> Value {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("a seeded instant is after the epoch");
    json!(since_epoch.as_secs())
}

/// Seed `seed_cases()` under `definition_id`, in order, answering the ids and the items
/// a listing must report.
fn seed_all(mgn: &Mgn, definition_id: &str, first_seed: u32) -> (Vec<String>, Vec<Value>) {
    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_767_225_600);
    let mut ids = Vec::new();
    let mut items = Vec::new();
    for (index, seed) in seed_cases().iter().enumerate() {
        let id = execution_id(first_seed + index as u32);
        let created_at = base + Duration::from_secs(3_600 * index as u64);
        items.push(seed_execution(
            mgn,
            EMULATOR_ACCOUNT,
            definition_id,
            &id,
            seed,
            created_at,
        ));
        ids.push(id);
    }
    (ids, items)
}

/// Every item a listing reports, following `nextToken` to the end, asserting 200 and a
/// page no larger than `max_results` asked for.
async fn list_all(
    mgn: &Mgn,
    case: &str,
    definition_id: &str,
    filters: Option<Value>,
    max_results: Option<u32>,
) -> Vec<Value> {
    let mut collected = Vec::new();
    let mut next_token: Option<String> = None;
    // A page per execution at worst, plus a margin: a listing that keeps handing back a
    // token is a defect, not a reason to loop forever.
    for page_index in 0..64 {
        let mut request = json!({"networkMigrationDefinitionID": definition_id});
        if let Some(filters) = filters.clone() {
            request["filters"] = filters;
        }
        if let Some(max_results) = max_results {
            request["maxResults"] = json!(max_results);
        }
        if let Some(token) = next_token.clone() {
            request["nextToken"] = json!(token);
        }
        let (status, body) = mgn.call(LIST, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListNetworkMigrationExecutions expected 200, got {status} with {body:?}"
        );
        let items = body["items"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no items array in {body:?}"))
            .clone();
        if let Some(limit) = max_results {
            assert!(
                items.len() <= limit as usize,
                "case {case:?}: page {page_index} returned {} items, more than maxResults {limit}",
                items.len()
            );
        }
        collected.extend(items.clone());
        match body["nextToken"].as_str() {
            None => return collected,
            Some(token) => {
                assert!(
                    !items.is_empty(),
                    "case {case:?}: page {page_index} handed back a token with no items: {body:?}"
                );
                next_token = Some(token.to_string());
            }
        }
    }
    panic!("case {case:?}: the listing never stopped handing back a nextToken");
}

/// The `networkMigrationExecutionID` of a listed item.
fn item_id(item: &Value) -> String {
    item["networkMigrationExecutionID"]
        .as_str()
        .unwrap_or_else(|| panic!("no networkMigrationExecutionID in {item:?}"))
        .to_string()
}

/// Creating a definition stages exactly one execution for it: an id in the modelled
/// shape, waiting in `PENDING` at the pipeline's first stage (`MAPPING`) with no
/// activity, timestamped at the definition's own instant — and a read reports the same
/// execution every time rather than staging another.
#[tokio::test]
async fn creating_a_definition_stages_one_pending_execution() {
    let mgn = Mgn::new();
    let mut staged_ids = Vec::new();
    // Two definitions, so the staged executions are per definition rather than one
    // shared record.
    for name in ["plan-one", "plan-two"] {
        let definition = create_definition(&mgn, name).await;
        let staged = staged_execution(&mgn, &definition).await;
        let id = item_id(&staged);
        assert!(
            is_execution_id(&id),
            "case {name:?}: the staged execution's id must be in the modelled shape, got {id:?}"
        );
        assert_eq!(
            staged["networkMigrationDefinitionID"].as_str(),
            Some(definition.as_str()),
            "case {name:?}: the staged execution must name its definition, got {staged:?}"
        );
        assert_eq!(
            staged["status"].as_str(),
            Some(EXECUTION_STATUSES[0]),
            "case {name:?}: a staged execution nothing has started must be {:?}, got {staged:?}",
            EXECUTION_STATUSES[0]
        );
        assert_eq!(
            staged["stage"].as_str(),
            Some(EXECUTION_STAGES[0]),
            "case {name:?}: a staged execution must wait at the pipeline's first stage {:?}, got {staged:?}",
            EXECUTION_STAGES[0]
        );
        assert!(
            staged["activity"].is_null(),
            "case {name:?}: nothing has run, so activity must be absent, got {staged:?}"
        );
        assert_eq!(
            staged["createdAt"], staged["updatedAt"],
            "case {name:?}: nothing has moved the execution on, so its timestamps must agree, got {staged:?}"
        );
        assert!(
            staged["createdAt"].is_number(),
            "case {name:?}: createdAt must be a timestamp, got {staged:?}"
        );

        // Reading again reports the same execution: a read stages nothing.
        let again = staged_execution(&mgn, &definition).await;
        assert_eq!(
            again, staged,
            "case {name:?}: a second read must report the same staged execution, got {again:?}"
        );
        staged_ids.push(id);
    }
    assert_ne!(
        staged_ids[0], staged_ids[1],
        "two definitions must be staged two different executions, got {staged_ids:?}"
    );
}

/// A listing reports the definition's stored executions in creation order, as the
/// modelled record of each, whatever the page size — and carries no member the shape
/// does not declare.
#[tokio::test]
async fn executions_are_listed_in_creation_order_at_every_page_size() {
    let mgn = Mgn::new();
    let definition = create_definition(&mgn, "plan-with-executions").await;
    let staged = staged_execution(&mgn, &definition).await;
    let (_, seeded) = seed_all(&mgn, &definition, 0x1000);

    let mut expected = vec![("the staged execution", staged)];
    expected.extend(
        seed_cases()
            .iter()
            .map(|seed| seed.case)
            .zip(seeded.iter().cloned()),
    );

    // Absent, each bound of the modelled `MaxResultsType` range (1..1000), sizes that
    // split the selection unevenly, and a size past the count.
    for max_results in [None, Some(1), Some(2), Some(4), Some(5), Some(1000)] {
        let case = format!("maxResults {max_results:?}");
        let items = list_all(&mgn, &case, &definition, None, max_results).await;
        assert_eq!(
            items.len(),
            expected.len(),
            "case {case:?}: expected {} executions, got {items:?}",
            expected.len()
        );
        for (index, (seed_case, want)) in expected.iter().enumerate() {
            let got = &items[index];
            assert_eq!(
                got, want,
                "case {case:?} / {seed_case:?}: expected the stored record {want:?}, got {got:?}"
            );
            let unknown: Vec<&String> = got
                .as_object()
                .unwrap_or_else(|| panic!("case {case:?}: item {index} is not an object: {got:?}"))
                .keys()
                .filter(|member| !EXECUTION_MEMBERS.contains(&member.as_str()))
                .collect();
            assert!(
                unknown.is_empty(),
                "case {case:?} / {seed_case:?}: the item carries members the model does not declare: {unknown:?}"
            );
        }
    }
}

/// The `networkMigrationExecutionIDs` and `networkMigrationExecutionStatuses` filters
/// select exactly the executions they name; an absent filter and a present-but-empty
/// list select everything, an id or status nothing was recorded under selects nothing,
/// and two filters narrow together.
#[tokio::test]
async fn the_filters_select_exactly_the_named_executions() {
    let mgn = Mgn::new();
    let definition = create_definition(&mgn, "plan-for-filters").await;
    let staged = item_id(&staged_execution(&mgn, &definition).await);
    let (seeded, _) = seed_all(&mgn, &definition, 0x2000);
    // Every execution of the definition, in creation order: the staged one, then the
    // seeded cases.
    let mut all = vec![staged.clone()];
    all.extend(seeded.iter().cloned());

    let unknown_id = execution_id(0xdead);
    let mut padded: Vec<String> = all.clone();
    let mut filler = 0x9000;
    while padded.len() < MAX_FILTER_IDS {
        padded.push(execution_id(filler));
        filler += 1;
    }
    // The seeded cases by their place in `seed_cases()`: two are `STARTED`, one is
    // `PENDING` beside the staged execution, and one carries no status at all.
    let started = vec![seeded[0].clone(), seeded[3].clone()];
    let pending = vec![staged.clone(), seeded[4].clone()];

    let cases: Vec<(String, Option<Value>, Vec<String>)> = vec![
        ("no filters member".to_string(), None, all.clone()),
        (
            "an empty filters structure".to_string(),
            Some(json!({})),
            all.clone(),
        ),
        (
            "an empty id list".to_string(),
            Some(json!({"networkMigrationExecutionIDs": []})),
            all.clone(),
        ),
        (
            "one id".to_string(),
            Some(json!({"networkMigrationExecutionIDs": [seeded[1]]})),
            vec![seeded[1].clone()],
        ),
        (
            "the staged execution's own id".to_string(),
            Some(json!({"networkMigrationExecutionIDs": [staged]})),
            vec![staged.clone()],
        ),
        (
            "two ids, named out of creation order".to_string(),
            Some(json!({"networkMigrationExecutionIDs": [seeded[2], seeded[0]]})),
            vec![seeded[0].clone(), seeded[2].clone()],
        ),
        (
            "every id".to_string(),
            Some(json!({"networkMigrationExecutionIDs": all.clone()})),
            all.clone(),
        ),
        (
            "one id nothing was recorded under".to_string(),
            Some(json!({"networkMigrationExecutionIDs": [unknown_id]})),
            Vec::new(),
        ),
        (
            "a known id beside an unknown one".to_string(),
            Some(json!({"networkMigrationExecutionIDs": [unknown_id, seeded[0]]})),
            vec![seeded[0].clone()],
        ),
        (
            format!("the filter's modelled maximum of {MAX_FILTER_IDS} ids"),
            Some(json!({"networkMigrationExecutionIDs": padded})),
            all.clone(),
        ),
        (
            "an empty status list".to_string(),
            Some(json!({"networkMigrationExecutionStatuses": []})),
            all.clone(),
        ),
        (
            format!("status {}", EXECUTION_STATUSES[0]),
            Some(json!({"networkMigrationExecutionStatuses": [EXECUTION_STATUSES[0]]})),
            pending.clone(),
        ),
        (
            format!("status {}", EXECUTION_STATUSES[1]),
            Some(json!({"networkMigrationExecutionStatuses": [EXECUTION_STATUSES[1]]})),
            started.clone(),
        ),
        (
            format!("status {}", EXECUTION_STATUSES[2]),
            Some(json!({"networkMigrationExecutionStatuses": [EXECUTION_STATUSES[2]]})),
            vec![seeded[1].clone()],
        ),
        (
            format!("status {}", EXECUTION_STATUSES[3]),
            Some(json!({"networkMigrationExecutionStatuses": [EXECUTION_STATUSES[3]]})),
            vec![seeded[2].clone()],
        ),
        (
            "every status at once".to_string(),
            Some(json!({"networkMigrationExecutionStatuses": EXECUTION_STATUSES})),
            // The execution carrying no status is selected by no status.
            all[..all.len() - 1].to_vec(),
        ),
        (
            "both filters, agreeing".to_string(),
            Some(json!({
                "networkMigrationExecutionIDs": [seeded[0]],
                "networkMigrationExecutionStatuses": [EXECUTION_STATUSES[1]],
            })),
            vec![seeded[0].clone()],
        ),
        (
            "both filters, disagreeing".to_string(),
            Some(json!({
                "networkMigrationExecutionIDs": [seeded[0]],
                "networkMigrationExecutionStatuses": [EXECUTION_STATUSES[2]],
            })),
            Vec::new(),
        ),
    ];

    for (case, filters, want) in cases {
        // Page size 1 as well as the default, so filtering and paging compose.
        for max_results in [None, Some(1)] {
            let case = format!("{case} at maxResults {max_results:?}");
            let items = list_all(&mgn, &case, &definition, filters.clone(), max_results).await;
            let got: Vec<String> = items.iter().map(item_id).collect();
            assert_eq!(
                got, want,
                "case {case:?}: expected executions {want:?}, got {got:?}"
            );
        }
    }
}

/// An execution belongs to one definition of one account: another definition's
/// executions, and another account's, are absent from the listing and not selectable by
/// id.
#[tokio::test]
async fn only_this_definitions_executions_of_this_account_are_listed() {
    let mgn = Mgn::new();
    let definition = create_definition(&mgn, "plan-under-test").await;
    let staged = item_id(&staged_execution(&mgn, &definition).await);
    let other_definition = create_definition(&mgn, "plan-next-door").await;
    let other_staged = item_id(&staged_execution(&mgn, &other_definition).await);
    let (seeded, _) = seed_all(&mgn, &definition, 0x3000);
    let mut all = vec![staged.clone()];
    all.extend(seeded.iter().cloned());

    // Another account's execution, recorded under THIS definition's id: the account is
    // what keeps it invisible here.
    let foreign = execution_id(0x5000);
    seed_execution(
        &mgn,
        OTHER_ACCOUNT,
        &definition,
        &foreign,
        &SeedCase {
            case: "another account's execution",
            status: Some(EXECUTION_STATUSES[1]),
            stage: Some(EXECUTION_STAGES[0]),
            activity: Some(EXECUTION_ACTIVITIES[0]),
            tags: Vec::new(),
        },
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_767_225_600),
    );

    let items = list_all(&mgn, "one definition's executions", &definition, None, None).await;
    let got: Vec<String> = items.iter().map(item_id).collect();
    assert_eq!(
        got, all,
        "a listing must report only this definition's own executions, got {got:?}"
    );

    for (case, id) in [
        ("another definition's staged execution", &other_staged),
        ("another account's execution", &foreign),
    ] {
        let items = list_all(
            &mgn,
            case,
            &definition,
            Some(json!({"networkMigrationExecutionIDs": [id]})),
            None,
        )
        .await;
        assert!(
            items.is_empty(),
            "case {case:?}: {id:?} must not be selectable here, got {items:?}"
        );
    }

    // The neighbouring definition reports its own staged execution, so the executions
    // are partitioned rather than hidden.
    let items = list_all(
        &mgn,
        "the neighbouring definition",
        &other_definition,
        None,
        None,
    )
    .await;
    let got: Vec<String> = items.iter().map(item_id).collect();
    assert_eq!(
        got,
        vec![other_staged.clone()],
        "the neighbouring definition must report its own execution, got {got:?}"
    );
}

/// A `networkMigrationDefinitionID` the account holds no record of is refused with 404
/// `ResourceNotFoundException` naming the definition, not answered with an empty page.
#[tokio::test]
async fn an_unknown_definition_is_refused() {
    let mgn = Mgn::new();
    let known = create_definition(&mgn, "plan-that-exists").await;

    // Ids inside the modelled `^nmd-[0-9a-zA-Z]{17}$` pattern, differing in their body
    // characters: all digits, all letters, mixed case.
    for unknown in [
        "nmd-00000000000000000",
        "nmd-zzzzzzzzzzzzzzzzz",
        "nmd-0A1b2C3d4E5f6G7h8",
    ] {
        let (status, body) = mgn
            .call(LIST, &json!({"networkMigrationDefinitionID": unknown}))
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (404, "ResourceNotFoundException"),
            "case {unknown:?}: expected 404 ResourceNotFoundException, got {status} with {body:?}"
        );
        assert_eq!(
            body["resourceId"].as_str(),
            Some(unknown),
            "case {unknown:?}: the refusal must name the requested id, got {body:?}"
        );
        assert_eq!(
            body["resourceType"].as_str(),
            Some("NetworkMigrationDefinition"),
            "case {unknown:?}: the refusal must name the resource kind, got {body:?}"
        );
    }

    // The definition that does exist is still listable, so the refusal is about the id
    // and not about the operation.
    let (status, body) = mgn
        .call(LIST, &json!({"networkMigrationDefinitionID": known}))
        .await;
    assert_eq!(
        status, 200,
        "a known definition expected 200, got {status} with {body:?}"
    );
}

/// A `nextToken` this operation did not issue, or one whose anchor execution the
/// selection no longer holds, cannot be resumed: it is refused with a 400
/// `ValidationException` rather than served as some other page. An empty token is the
/// first page, as the modelled length 0..2048 allows.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let mgn = Mgn::new();
    let definition = create_definition(&mgn, "plan-for-tokens").await;
    let staged = item_id(&staged_execution(&mgn, &definition).await);
    let (seeded, _) = seed_all(&mgn, &definition, 0x6000);
    let mut all = vec![staged];
    all.extend(seeded.iter().cloned());

    let unresumable = [
        "not-a-token".to_string(),
        "0".to_string(),
        execution_id(0x6000),
        format!("mgn-network-migration-definitions-{definition}"),
        // This operation's own marker, but anchored on an execution the selection does
        // not hold.
        format!("mgn-network-migration-executions-{}", execution_id(0xbeef)),
    ];
    for token in unresumable {
        let (status, body) = mgn
            .call(
                LIST,
                &json!({"networkMigrationDefinitionID": definition, "nextToken": token}),
            )
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "ValidationException"),
            "case {token:?}: an unresumable nextToken must be refused with 400 ValidationException, got {status} with {body:?}"
        );
    }

    // An empty token names no anchor, so it is the first page rather than a refusal.
    let (status, first) = mgn
        .call(
            LIST,
            &json!({
                "networkMigrationDefinitionID": definition,
                "nextToken": "",
                "maxResults": 2,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "an empty nextToken expected 200, got {status} with {first:?}"
    );
    assert_eq!(
        first["items"].as_array().map(Vec::len),
        Some(2),
        "an empty nextToken must answer the first page, got {first:?}"
    );

    // A token the operation issued resumes where the page stopped, and an execution
    // appears on exactly one page.
    let resumed = list_all(&mgn, "following the issued tokens", &definition, None, Some(2)).await;
    let got: Vec<String> = resumed.iter().map(item_id).collect();
    assert_eq!(
        got, all,
        "following tokens must report every execution exactly once, got {got:?}"
    );
}

/// Listing is a read: it changes nothing a later caller sees. Two identical listings,
/// and a listing at a different page size, answer the same items, and the store holds
/// the same executions afterwards.
#[tokio::test]
async fn listing_does_not_change_what_a_later_caller_sees() {
    let mgn = Mgn::new();
    let definition = create_definition(&mgn, "plan-read-twice").await;
    let staged = staged_execution(&mgn, &definition).await;
    let (_, seeded) = seed_all(&mgn, &definition, 0x7000);
    let mut expected = vec![staged];
    expected.extend(seeded.iter().cloned());

    let first = list_all(&mgn, "first listing", &definition, None, Some(2)).await;
    let again = list_all(&mgn, "second listing", &definition, None, Some(3)).await;
    assert_eq!(
        again, first,
        "two listings of an unchanged store must answer the same items, got {again:?}"
    );
    assert_eq!(
        first, expected,
        "a listing must report the stored records, got {first:?}"
    );
    assert_eq!(
        mgn.store().lock().network_migration_execution_order.len(),
        expected.len(),
        "listing must not add or drop a stored execution"
    );
}
