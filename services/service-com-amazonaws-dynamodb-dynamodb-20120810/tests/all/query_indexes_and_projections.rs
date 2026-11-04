// SPDX-License-Identifier: CC0-1.0

//! A `Query` reads the partition of the schema the request pointed it AT — the table's
//! own key schema, or a secondary index's — and reports of each entry only what the
//! request asked for.
//!
//! The rules these suites pin:
//!
//! * a query naming an `IndexName` is served from that index: it evaluates exactly the
//!   entries that index holds for the partition the key condition names, judged against
//!   the INDEX's key schema, and reports of each the attributes the index projects;
//! * the attributes a reported item carries are the request's projection — a
//!   `ProjectionExpression` list of document paths, with its `ExpressionAttributeNames`
//!   aliases — and a path that reaches inside an attribute reports the value at that
//!   path wrapped in the containers it steps through, on `Query` and on `Scan` alike;
//! * every way such a request can be invalid is the modelled client error: HTTP 400
//!   `ValidationException` with the service's own sentence, never a 501 — an index the
//!   table does not declare, a key condition that misses the index's partition key or
//!   supplies the wrong type for it, `ConsistentRead` against a global index, and a
//!   `Select` that disagrees with the projection beside it.
//!
//! Each suite checks its rule over a SET of inputs: every index kind and projection type
//! the table declares, a composite index key with each comparison its sort key takes,
//! both paging directions, a table of projection forms spanning attribute steps, list
//! indexes, aliases and paths that address nothing, and a table of refusal rows.

use serde_json::{json, Map, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::MAX_PROJECTED_PATHS;

use crate::emulator::{call, describe_when_active, emulator, error_code, message, ok};
use crate::table_scan::{last_evaluated_key, rendered, reported_items};

use smithy_server_runtime::MultiServiceRouter;

/// The table the index suites read. One table name per emulator, and each suite builds
/// its own emulator, so the name is a constant.
const TABLE: &str = "QueryIndexed";

/// The table the projection suite reads: ONE item, so a `Query` of its only partition
/// and a `Scan` of the whole table both report exactly that item and the expectation of
/// each row is a literal.
const SOLO: &str = "QueryProjected";

/// The items the index suites seed, chosen so that each index the table declares covers
/// a DIFFERENT subset of them: one carries no `priority` (so neither priority index
/// covers it), two no `assignee`, and one carries none of the index key attributes at
/// all — which is what makes every secondary index here sparse.
fn stored_items() -> Vec<Value> {
    vec![
        json!({"pk": {"S": "t1"}, "sk": {"S": "a"}, "status": {"S": "OPEN"},
               "priority": {"N": "1"}, "assignee": {"S": "ann"}, "note": {"S": "first"}}),
        json!({"pk": {"S": "t1"}, "sk": {"S": "b"}, "status": {"S": "OPEN"},
               "priority": {"N": "2"}, "assignee": {"S": "bob"}, "note": {"S": "second"}}),
        json!({"pk": {"S": "t2"}, "sk": {"S": "a"}, "status": {"S": "DONE"},
               "priority": {"N": "1"}, "note": {"S": "third"}}),
        json!({"pk": {"S": "t2"}, "sk": {"S": "b"}, "status": {"S": "DONE"},
               "assignee": {"S": "ann"}, "note": {"S": "fourth"}}),
        json!({"pk": {"S": "t3"}, "sk": {"S": "a"}, "note": {"S": "fifth"}}),
    ]
}

/// Create [`TABLE`] with one index of every kind and projection type the service serves,
/// wait out the `CREATING` -> `ACTIVE` transition, write every item of
/// [`stored_items`], and answer the items written.
async fn seeded(router: &MultiServiceRouter) -> Vec<Value> {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": TABLE,
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "S"},
                {"AttributeName": "status", "AttributeType": "S"},
                {"AttributeName": "priority", "AttributeType": "N"},
                {"AttributeName": "assignee", "AttributeType": "S"},
            ],
            "BillingMode": "PAY_PER_REQUEST",
            "GlobalSecondaryIndexes": [
                {
                    "IndexName": "ByStatus",
                    "KeySchema": [{"AttributeName": "status", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "ALL"},
                },
                {
                    "IndexName": "ByStatusPriority",
                    "KeySchema": [
                        {"AttributeName": "status", "KeyType": "HASH"},
                        {"AttributeName": "priority", "KeyType": "RANGE"},
                    ],
                    "Projection": {"ProjectionType": "KEYS_ONLY"},
                },
                {
                    "IndexName": "ByPriority",
                    "KeySchema": [{"AttributeName": "priority", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "ALL"},
                },
            ],
            "LocalSecondaryIndexes": [
                {
                    "IndexName": "ByAssignee",
                    "KeySchema": [
                        {"AttributeName": "pk", "KeyType": "HASH"},
                        {"AttributeName": "assignee", "KeyType": "RANGE"},
                    ],
                    "Projection": {"ProjectionType": "INCLUDE",
                                   "NonKeyAttributes": ["note"]},
                },
            ],
        }),
    )
    .await;
    describe_when_active(router, "the seeded indexed table", TABLE).await;
    let items = stored_items();
    for item in &items {
        ok(
            router,
            "PutItem",
            json!({"TableName": TABLE, "Item": item}),
        )
        .await;
    }
    items
}

/// Create [`SOLO`] and write the one item every projection row is read through: it
/// carries a nested map, a list and an attribute whose name is a DynamoDB reserved word,
/// so a projection can address a map entry, a list element and an aliased name.
async fn seeded_solo(router: &MultiServiceRouter) -> Value {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": SOLO,
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    describe_when_active(router, "the seeded projection table", SOLO).await;
    let item = json!({
        "pk": {"S": "only"},
        "note": {"S": "solo"},
        "status": {"S": "OPEN"},
        "profile": {"M": {"nick": {"S": "ann"}, "city": {"S": "berlin"}}},
        "tags": {"L": [{"S": "red"}, {"S": "blue"}]},
    });
    ok(
        router,
        "PutItem",
        json!({"TableName": SOLO, "Item": item}),
    )
    .await;
    item
}

/// One item narrowed to the attributes an index projects: "Only the index and primary key
/// attributes are projected" for `KEYS_ONLY`, those plus `NonKeyAttributes` for
/// `INCLUDE` (`Projection$ProjectionType`), so a row names the attributes its index
/// holds and the expectation is the stored item narrowed to them.
fn narrowed(item: &Value, names: &[&str]) -> Value {
    let mut narrowed = Map::new();
    for name in names {
        if let Some(value) = item.get(*name) {
            narrowed.insert((*name).to_string(), value.clone());
        }
    }
    Value::Object(narrowed)
}

/// The `note` of every item a response reported, sorted — the handle each row below names
/// its expected selection by.
fn reported_notes(response: &Value) -> Vec<String> {
    let mut notes: Vec<String> = response["Items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|item| item["note"]["S"].as_str().map(ToString::to_string))
        .collect();
    notes.sort();
    notes
}

/// One row of the index table: the index a query names, the key condition it states, the
/// notes of the entries that index holds for it, and the attributes the index projects.
struct IndexCase {
    case: &'static str,
    index: &'static str,
    condition: Value,
    notes: Vec<&'static str>,
    projects: &'static [&'static str],
}

/// Every index the seeded table declares: both kinds, each projection type, a composite
/// index key, and an index keyed by a NUMERIC attribute.
fn index_cases() -> Vec<IndexCase> {
    vec![
        IndexCase {
            case: "a global index keyed by one string attribute, projecting ALL",
            index: "ByStatus",
            condition: json!({
                "KeyConditionExpression": "#s = :s",
                "ExpressionAttributeNames": {"#s": "status"},
                "ExpressionAttributeValues": {":s": {"S": "OPEN"}},
            }),
            notes: vec!["first", "second"],
            projects: &["pk", "sk", "status", "priority", "assignee", "note"],
        },
        IndexCase {
            case: "a composite global index projecting KEYS_ONLY, both key attributes \
                   constrained",
            index: "ByStatusPriority",
            condition: json!({
                "KeyConditionExpression": "#s = :s AND priority = :p",
                "ExpressionAttributeNames": {"#s": "status"},
                "ExpressionAttributeValues": {":s": {"S": "DONE"}, ":p": {"N": "1"}},
            }),
            notes: vec!["third"],
            projects: &["pk", "sk", "status", "priority"],
        },
        IndexCase {
            case: "the same composite index with a BETWEEN over its sort key",
            index: "ByStatusPriority",
            condition: json!({
                "KeyConditionExpression": "#s = :s AND priority BETWEEN :lo AND :hi",
                "ExpressionAttributeNames": {"#s": "status"},
                "ExpressionAttributeValues": {":s": {"S": "OPEN"}, ":lo": {"N": "1"},
                                              ":hi": {"N": "2"}},
            }),
            notes: vec!["first", "second"],
            projects: &["pk", "sk", "status", "priority"],
        },
        IndexCase {
            case: "the same composite index with a > over its sort key",
            index: "ByStatusPriority",
            condition: json!({
                "KeyConditionExpression": "#s = :s AND priority > :p",
                "ExpressionAttributeNames": {"#s": "status"},
                "ExpressionAttributeValues": {":s": {"S": "OPEN"}, ":p": {"N": "1"}},
            }),
            notes: vec!["second"],
            projects: &["pk", "sk", "status", "priority"],
        },
        IndexCase {
            case: "a global index keyed by a numeric attribute",
            index: "ByPriority",
            condition: json!({
                "KeyConditionExpression": "priority = :p",
                "ExpressionAttributeValues": {":p": {"N": "1"}},
            }),
            notes: vec!["first", "third"],
            projects: &["pk", "sk", "status", "priority", "assignee", "note"],
        },
        IndexCase {
            case: "a local index projecting INCLUDE, with begins_with over its sort key",
            index: "ByAssignee",
            condition: json!({
                "KeyConditionExpression": "pk = :t AND begins_with(assignee, :a)",
                "ExpressionAttributeValues": {":t": {"S": "t1"}, ":a": {"S": "a"}},
            }),
            notes: vec!["first"],
            projects: &["pk", "sk", "assignee", "note"],
        },
    ]
}

/// A request body built from a case's base members and its key condition.
fn request(index: Option<&str>, condition: &Value, extra: &Value) -> Value {
    let mut body = Map::new();
    body.insert("TableName".to_string(), json!(TABLE));
    if let Some(index) = index {
        body.insert("IndexName".to_string(), json!(index));
    }
    for (name, value) in condition.as_object().into_iter().flatten() {
        body.insert(name.clone(), value.clone());
    }
    for (name, value) in extra.as_object().into_iter().flatten() {
        body.insert(name.clone(), value.clone());
    }
    Value::Object(body)
}

#[tokio::test]
async fn an_index_query_reads_the_entries_that_index_holds() {
    let router = emulator();
    let items = seeded(&router).await;

    for case in index_cases() {
        let queried = ok(
            &router,
            "Query",
            request(Some(case.index), &case.condition, &json!({})),
        )
        .await;
        let expected: Vec<Value> = items
            .iter()
            .filter(|item| {
                case.notes
                    .contains(&item["note"]["S"].as_str().unwrap_or_default())
            })
            .map(|item| narrowed(item, case.projects))
            .collect();
        assert_eq!(
            reported_items(&queried),
            rendered(&expected),
            "case {:?}: expected the entries {:?} narrowed to {:?}, got {}",
            case.case,
            case.notes,
            case.projects,
            queried
        );
        assert_eq!(
            queried["Count"],
            json!(case.notes.len()),
            "case {:?}: expected Count {}, got {}",
            case.case,
            case.notes.len(),
            queried
        );
        assert_eq!(
            queried["ScannedCount"],
            json!(case.notes.len()),
            "case {:?}: an unfiltered query scans exactly what it answers, got {}",
            case.case,
            queried
        );
        // Every index here is sparse, and a query reads ONE of its partitions: a row
        // that selected the whole table would prove nothing about the index.
        assert!(
            case.notes.len() < items.len(),
            "case {:?}: the fixture must leave at least one item out of the answer, got {} \
             of {}",
            case.case,
            case.notes.len(),
            items.len()
        );
    }
}

#[tokio::test]
async fn an_index_query_pages_its_partition_in_index_key_order() {
    let router = emulator();
    seeded(&router).await;

    // The entries `ByStatusPriority` holds for the OPEN partition, in that index's own
    // order: by its numeric sort key, with the table's primary key breaking the tie.
    let condition = json!({
        "KeyConditionExpression": "#s = :s",
        "ExpressionAttributeNames": {"#s": "status"},
        "ExpressionAttributeValues": {":s": {"S": "OPEN"}},
    });
    for (case, forward, expected) in [
        ("ascending, the index's own order", true, vec!["first", "second"]),
        (
            "descending, the order ScanIndexForward=false reads",
            false,
            vec!["second", "first"],
        ),
    ] {
        // One entry per page, so every page but the last reports a LastEvaluatedKey and
        // the walk has to resume from it.
        let mut walked: Vec<String> = Vec::new();
        let mut start: Option<Value> = None;
        for _ in 0..=expected.len() {
            let mut extra = json!({"Limit": 1, "ScanIndexForward": forward});
            if let Some(key) = start.clone() {
                extra["ExclusiveStartKey"] = key;
            }
            let page = ok(
                &router,
                "Query",
                request(Some("ByStatusPriority"), &condition, &extra),
            )
            .await;
            walked.extend(
                page["Items"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|item| item["sk"]["S"].as_str().map(ToString::to_string)),
            );
            start = last_evaluated_key(&page);
            if let Some(key) = start.as_ref() {
                // An index page is addressed by the index's key AND the table's, which
                // is what makes one entry of an index unique.
                for name in ["status", "priority", "pk", "sk"] {
                    assert!(
                        key.get(name).is_some(),
                        "case {case:?}: an index page's LastEvaluatedKey carries {name:?}, \
                         got {key}"
                    );
                }
            } else {
                break;
            }
        }
        // The notes of the seeded OPEN items, in the order each direction reads them:
        // `first` is (t1, a) and `second` is (t1, b).
        let expected_keys: Vec<String> = expected
            .iter()
            .map(|note| match *note {
                "first" => "a".to_string(),
                _ => "b".to_string(),
            })
            .collect();
        assert_eq!(
            walked, expected_keys,
            "case {case:?}: expected the sort keys {expected_keys:?} in that order, got \
             {walked:?}"
        );
    }
}

#[tokio::test]
async fn an_index_query_bills_its_capacity_to_that_index() {
    let router = emulator();
    seeded(&router).await;

    for (case, index, group) in [
        (
            "a global index is billed under GlobalSecondaryIndexes",
            "ByStatus",
            "GlobalSecondaryIndexes",
        ),
        (
            "a local index is billed under LocalSecondaryIndexes",
            "ByAssignee",
            "LocalSecondaryIndexes",
        ),
    ] {
        let condition = if index == "ByStatus" {
            json!({
                "KeyConditionExpression": "#s = :s",
                "ExpressionAttributeNames": {"#s": "status"},
                "ExpressionAttributeValues": {":s": {"S": "OPEN"}},
            })
        } else {
            json!({
                "KeyConditionExpression": "pk = :t",
                "ExpressionAttributeValues": {":t": {"S": "t1"}},
            })
        };
        let queried = ok(
            &router,
            "Query",
            request(
                Some(index),
                &condition,
                &json!({"ReturnConsumedCapacity": "INDEXES"}),
            ),
        )
        .await;
        assert!(
            queried["ConsumedCapacity"][group][index]["ReadCapacityUnits"]
                .as_f64()
                .is_some_and(|units| units > 0.0),
            "case {case:?}: the units this read spent are reported against {index:?}, got {}",
            queried["ConsumedCapacity"]
        );
    }
}

#[tokio::test]
async fn a_count_only_index_query_reports_the_count_and_no_items() {
    let router = emulator();
    seeded(&router).await;

    let counted = ok(
        &router,
        "Query",
        request(
            Some("ByStatus"),
            &json!({
                "KeyConditionExpression": "#s = :s",
                "ExpressionAttributeNames": {"#s": "status"},
                "ExpressionAttributeValues": {":s": {"S": "DONE"}},
            }),
            &json!({"Select": "COUNT"}),
        ),
    )
    .await;
    assert_eq!(
        counted["Count"],
        json!(2),
        "case COUNT on an index: expected the two DONE entries counted, got {counted}"
    );
    assert!(
        counted.get("Items").is_none_or(Value::is_null),
        "case COUNT on an index: the count is reported rather than the entries, got {counted}"
    );
}

/// One row of the projection table: the projection members a read states, and the one
/// item the read must report.
struct ProjectionCase {
    case: &'static str,
    projection: Value,
    reported: Value,
}

/// The projection forms a read may state, spanning what a document path can be: one
/// attribute, several, an `#alias` (the only way to project a reserved word), a map
/// entry, a map entry behind two aliases, two entries of ONE map, a list element, a
/// mixture, and three paths that address nothing at all.
fn projection_cases() -> Vec<ProjectionCase> {
    vec![
        ProjectionCase {
            case: "one top-level attribute",
            projection: json!({"ProjectionExpression": "pk"}),
            reported: json!({"pk": {"S": "only"}}),
        },
        ProjectionCase {
            case: "two top-level attributes",
            projection: json!({"ProjectionExpression": "pk, note"}),
            reported: json!({"pk": {"S": "only"}, "note": {"S": "solo"}}),
        },
        ProjectionCase {
            case: "a reserved word, reached through an alias",
            projection: json!({"ProjectionExpression": "#s",
                               "ExpressionAttributeNames": {"#s": "status"}}),
            reported: json!({"status": {"S": "OPEN"}}),
        },
        ProjectionCase {
            case: "a path into a map attribute",
            projection: json!({"ProjectionExpression": "profile.nick"}),
            reported: json!({"profile": {"M": {"nick": {"S": "ann"}}}}),
        },
        ProjectionCase {
            case: "the same path with every step behind an alias",
            projection: json!({"ProjectionExpression": "#p.#c",
                               "ExpressionAttributeNames": {"#p": "profile", "#c": "city"}}),
            reported: json!({"profile": {"M": {"city": {"S": "berlin"}}}}),
        },
        ProjectionCase {
            case: "two paths into the same map attribute",
            projection: json!({"ProjectionExpression": "profile.nick, profile.city"}),
            reported: json!({"profile": {"M": {"nick": {"S": "ann"},
                                               "city": {"S": "berlin"}}}}),
        },
        ProjectionCase {
            case: "one element of a list attribute",
            projection: json!({"ProjectionExpression": "tags[1]"}),
            reported: json!({"tags": {"L": [{"S": "blue"}]}}),
        },
        ProjectionCase {
            case: "a map path, a list element and a top-level attribute together",
            projection: json!({"ProjectionExpression": "profile.nick, tags[0], pk"}),
            reported: json!({"profile": {"M": {"nick": {"S": "ann"}}},
                             "tags": {"L": [{"S": "red"}]},
                             "pk": {"S": "only"}}),
        },
        ProjectionCase {
            case: "an attribute the item does not carry",
            projection: json!({"ProjectionExpression": "nowhere"}),
            reported: json!({}),
        },
        ProjectionCase {
            case: "a map entry the item does not carry",
            projection: json!({"ProjectionExpression": "profile.nowhere"}),
            reported: json!({}),
        },
        ProjectionCase {
            case: "a list index past the end of the list",
            projection: json!({"ProjectionExpression": "tags[5]"}),
            reported: json!({}),
        },
        ProjectionCase {
            case: "a path into an attribute that holds no document",
            projection: json!({"ProjectionExpression": "note.deeper"}),
            reported: json!({}),
        },
        ProjectionCase {
            case: "Select SPECIFIC_ATTRIBUTES stated beside the projection",
            projection: json!({"Select": "SPECIFIC_ATTRIBUTES",
                               "ProjectionExpression": "profile.city"}),
            reported: json!({"profile": {"M": {"city": {"S": "berlin"}}}}),
        },
        ProjectionCase {
            case: "the legacy AttributesToGet naming whole attributes",
            projection: json!({"AttributesToGet": ["pk", "note"]}),
            reported: json!({"pk": {"S": "only"}, "note": {"S": "solo"}}),
        },
    ]
}

#[tokio::test]
async fn a_projection_reports_exactly_the_paths_it_names() {
    let router = emulator();
    seeded_solo(&router).await;

    for case in projection_cases() {
        // The same projection through both readers: the member means one thing, so a
        // query of the item's partition and a scan of the whole table report the same
        // narrowed item.
        let mut queried = Map::new();
        queried.insert("TableName".to_string(), json!(SOLO));
        queried.insert(
            "KeyConditionExpression".to_string(),
            json!("pk = :p"),
        );
        queried.insert(
            "ExpressionAttributeValues".to_string(),
            json!({":p": {"S": "only"}}),
        );
        let mut scanned = Map::new();
        scanned.insert("TableName".to_string(), json!(SOLO));
        for (name, value) in case.projection.as_object().into_iter().flatten() {
            queried.insert(name.clone(), value.clone());
            scanned.insert(name.clone(), value.clone());
        }
        let expected = rendered(std::slice::from_ref(&case.reported));
        for (operation, body) in [
            ("Query", Value::Object(queried)),
            ("Scan", Value::Object(scanned)),
        ] {
            let read = ok(&router, operation, body.clone()).await;
            assert_eq!(
                reported_items(&read),
                expected,
                "case {:?} ({operation}): expected the item narrowed to {}, got {}",
                case.case,
                case.reported,
                read
            );
            assert_eq!(
                read["Count"],
                json!(1),
                "case {:?} ({operation}): the one stored item is still counted whatever the \
                 projection reports, got {read}",
                case.case
            );
        }
    }
}

#[tokio::test]
async fn a_projection_is_bounded_in_the_number_of_attributes_it_names() {
    let router = emulator();
    seeded_solo(&router).await;

    // The legacy `AttributesToGet` carries no modelled length, so the ceiling the
    // expression member's token budget implies is what bounds it: AT the bound the read is
    // served, one past it the request is refused, and the refusal states the bound and the
    // count the request sent.
    let names = |count: usize| -> Vec<String> { (0..count).map(|at| format!("a{at}")).collect() };
    let served = ok(
        &router,
        "Scan",
        json!({"TableName": SOLO, "AttributesToGet": names(MAX_PROJECTED_PATHS)}),
    )
    .await;
    assert_eq!(
        served["Count"],
        json!(1),
        "case a projection at the bound: expected the stored item answered, got {served}"
    );
    assert_eq!(
        reported_items(&served),
        rendered(&[json!({})]),
        "case a projection at the bound: the names it lists are on no item, so nothing is \
         reported for them, got {served}"
    );

    for (case, operation, body) in [
        (
            "one past the bound on a scan's legacy member",
            "Scan",
            json!({"TableName": SOLO, "AttributesToGet": names(MAX_PROJECTED_PATHS + 1)}),
        ),
        (
            "one past the bound on a query's legacy member",
            "Query",
            json!({"TableName": SOLO, "KeyConditionExpression": "pk = :p",
                   "ExpressionAttributeValues": {":p": {"S": "only"}},
                   "AttributesToGet": names(MAX_PROJECTED_PATHS + 1)}),
        ),
    ] {
        let (status, refused) = call(&router, operation, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected a modelled ValidationException, got {refused}"
        );
        assert_eq!(
            message(&refused),
            format!(
                "One or more parameter values were invalid: a projection may name at most \
                 {MAX_PROJECTED_PATHS} attributes, got {}",
                MAX_PROJECTED_PATHS + 1
            ),
            "case {case:?}: expected the bound and the count stated, got {:?}",
            message(&refused)
        );
    }
}

/// One row of the refusal table: the request, and the whole message the refusal must
/// carry.
struct RefusalCase {
    case: &'static str,
    operation: &'static str,
    request: Value,
    message: String,
}

/// Every way an index-targeted or projection-carrying read can be invalid, each stated
/// as the client error the service models for it.
fn refusal_cases() -> Vec<RefusalCase> {
    let status_condition = json!({
        "KeyConditionExpression": "#s = :s",
        "ExpressionAttributeNames": {"#s": "status"},
        "ExpressionAttributeValues": {":s": {"S": "OPEN"}},
    });
    vec![
        RefusalCase {
            case: "an index the table does not declare",
            operation: "Query",
            request: request(Some("TaskStatus"), &status_condition, &json!({})),
            message: "The table does not have the specified index: TaskStatus".to_string(),
        },
        RefusalCase {
            case: "the same absent index named by another reader",
            operation: "Scan",
            request: json!({"TableName": TABLE, "IndexName": "TaskStatus"}),
            message: "The table does not have the specified index: TaskStatus".to_string(),
        },
        RefusalCase {
            case: "a key condition that omits the index's partition key",
            operation: "Query",
            request: request(
                Some("ByStatusPriority"),
                &json!({
                    "KeyConditionExpression": "priority = :p",
                    "ExpressionAttributeValues": {":p": {"N": "1"}},
                }),
                &json!({}),
            ),
            message: "Query condition missed key schema element: status".to_string(),
        },
        RefusalCase {
            case: "a key condition that states the table's key instead of the index's",
            operation: "Query",
            request: request(
                Some("ByStatus"),
                &json!({
                    "KeyConditionExpression": "pk = :t",
                    "ExpressionAttributeValues": {":t": {"S": "t1"}},
                }),
                &json!({}),
            ),
            message: "Query condition missed key schema element: status".to_string(),
        },
        RefusalCase {
            case: "a string value for a numeric index key",
            operation: "Query",
            request: request(
                Some("ByPriority"),
                &json!({
                    "KeyConditionExpression": "priority = :p",
                    "ExpressionAttributeValues": {":p": {"S": "1"}},
                }),
                &json!({}),
            ),
            message: "One or more parameter values were invalid: Type mismatch for key \
                      priority expected: N actual: S"
                .to_string(),
        },
        RefusalCase {
            case: "a numeric value for a string index key",
            operation: "Query",
            request: request(
                Some("ByStatus"),
                &json!({
                    "KeyConditionExpression": "#s = :s",
                    "ExpressionAttributeNames": {"#s": "status"},
                    "ExpressionAttributeValues": {":s": {"N": "1"}},
                }),
                &json!({}),
            ),
            message: "One or more parameter values were invalid: Type mismatch for key \
                      status expected: S actual: N"
                .to_string(),
        },
        RefusalCase {
            case: "a strongly consistent read of a global index",
            operation: "Query",
            request: request(
                Some("ByStatus"),
                &status_condition,
                &json!({"ConsistentRead": true}),
            ),
            message: "Consistent reads are not supported on global secondary indexes"
                .to_string(),
        },
        RefusalCase {
            case: "the same flag against a global index on another reader",
            operation: "Scan",
            request: json!({"TableName": TABLE, "IndexName": "ByStatus",
                            "ConsistentRead": true}),
            message: "Consistent reads are not supported on global secondary indexes"
                .to_string(),
        },
        RefusalCase {
            case: "Select COUNT beside a projection",
            operation: "Query",
            request: request(
                None,
                &json!({
                    "KeyConditionExpression": "pk = :t",
                    "ExpressionAttributeValues": {":t": {"S": "t1"}},
                }),
                &json!({"Select": "COUNT", "ProjectionExpression": "note"}),
            ),
            message: "One or more parameter values were invalid: Select type COUNT is not \
                      supported when the attributes to return are listed; the only Select \
                      type that is, is SPECIFIC_ATTRIBUTES"
                .to_string(),
        },
        RefusalCase {
            case: "Select ALL_ATTRIBUTES beside a projection",
            operation: "Query",
            request: request(
                None,
                &json!({
                    "KeyConditionExpression": "pk = :t",
                    "ExpressionAttributeValues": {":t": {"S": "t1"}},
                }),
                &json!({"Select": "ALL_ATTRIBUTES", "ProjectionExpression": "note"}),
            ),
            message: "One or more parameter values were invalid: Select type ALL_ATTRIBUTES \
                      is not supported when the attributes to return are listed; the only \
                      Select type that is, is SPECIFIC_ATTRIBUTES"
                .to_string(),
        },
        RefusalCase {
            case: "Select SPECIFIC_ATTRIBUTES with no projection to list",
            operation: "Query",
            request: request(
                None,
                &json!({
                    "KeyConditionExpression": "pk = :t",
                    "ExpressionAttributeValues": {":t": {"S": "t1"}},
                }),
                &json!({"Select": "SPECIFIC_ATTRIBUTES"}),
            ),
            message: "One or more parameter values were invalid: Select type \
                      SPECIFIC_ATTRIBUTES requires the attributes to be listed in \
                      ProjectionExpression"
                .to_string(),
        },
        RefusalCase {
            case: "Select ALL_ATTRIBUTES from an index that projects only its keys",
            operation: "Query",
            request: request(
                Some("ByStatusPriority"),
                &status_condition,
                &json!({"Select": "ALL_ATTRIBUTES"}),
            ),
            message: "One or more parameter values were invalid: Select type ALL_ATTRIBUTES \
                      is not supported for global secondary index ByStatusPriority because \
                      its projection type is not ALL"
                .to_string(),
        },
        RefusalCase {
            case: "a projection whose path names an undefined placeholder",
            operation: "Query",
            request: request(
                None,
                &json!({
                    "KeyConditionExpression": "pk = :t",
                    "ExpressionAttributeValues": {":t": {"S": "t1"}},
                }),
                &json!({"ProjectionExpression": "note.#u"}),
            ),
            message: "Invalid ProjectionExpression: An expression attribute name used in \
                      the document path is not defined; attribute name: #u"
                .to_string(),
        },
        RefusalCase {
            case: "the same undefined placeholder on another reader",
            operation: "Scan",
            request: json!({"TableName": TABLE, "ProjectionExpression": "note.#u"}),
            message: "Invalid ProjectionExpression: An expression attribute name used in \
                      the document path is not defined; attribute name: #u"
                .to_string(),
        },
        RefusalCase {
            case: "a projection naming a reserved word, written out",
            operation: "Query",
            request: request(
                None,
                &json!({
                    "KeyConditionExpression": "pk = :t",
                    "ExpressionAttributeValues": {":t": {"S": "t1"}},
                }),
                &json!({"ProjectionExpression": "status"}),
            ),
            message: "Invalid ProjectionExpression: Attribute name is a reserved keyword; \
                      reserved keyword: status"
                .to_string(),
        },
        RefusalCase {
            case: "a reserved word inside a projection path",
            operation: "Query",
            request: request(
                None,
                &json!({
                    "KeyConditionExpression": "pk = :t",
                    "ExpressionAttributeValues": {":t": {"S": "t1"}},
                }),
                &json!({"ProjectionExpression": "profile.status"}),
            ),
            message: "Invalid ProjectionExpression: Attribute name is a reserved keyword; \
                      reserved keyword: status"
                .to_string(),
        },
        RefusalCase {
            case: "a projection stated through both the expression and the legacy member",
            operation: "Query",
            request: request(
                None,
                &json!({
                    "KeyConditionExpression": "pk = :t",
                    "ExpressionAttributeValues": {":t": {"S": "t1"}},
                }),
                &json!({"ProjectionExpression": "note", "AttributesToGet": ["note"]}),
            ),
            message: "Can not use both expression and non-expression parameters in the same \
                      request: Non-expression parameters: {AttributesToGet} Expression \
                      parameters: {ProjectionExpression}"
                .to_string(),
        },
    ]
}

#[tokio::test]
async fn an_invalid_indexed_or_projected_read_is_the_modelled_client_error() {
    let router = emulator();
    seeded(&router).await;

    for case in refusal_cases() {
        let (status, body) = call(&router, case.operation, case.request.clone()).await;
        assert_eq!(
            status, 400,
            "case {:?} ({}): expected HTTP 400, got {status} with {body}",
            case.case, case.operation
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {:?} ({}): expected a modelled ValidationException — a 501 tells a client \
             nothing it can act on — got {body}",
            case.case,
            case.operation
        );
        assert_eq!(
            message(&body),
            case.message,
            "case {:?} ({}): expected the message {:?}, got {:?}",
            case.case,
            case.operation,
            case.message,
            message(&body)
        );
        assert!(
            body.get("Items").is_none_or(Value::is_null),
            "case {:?} ({}): a refused read reports no items, got {body}",
            case.case,
            case.operation
        );
    }
}

#[tokio::test]
async fn a_strongly_consistent_read_of_a_local_index_is_served() {
    let router = emulator();
    let items = seeded(&router).await;

    // The flag is refused on a GLOBAL index only: a local index is read from the same
    // partition as the table itself, so the same request against one is served.
    for (case, body) in [
        (
            "a consistent query of a local index",
            request(
                Some("ByAssignee"),
                &json!({
                    "KeyConditionExpression": "pk = :t",
                    "ExpressionAttributeValues": {":t": {"S": "t1"}},
                }),
                &json!({"ConsistentRead": true}),
            ),
        ),
        (
            "a consistent query of the table itself",
            request(
                None,
                &json!({
                    "KeyConditionExpression": "pk = :t",
                    "ExpressionAttributeValues": {":t": {"S": "t1"}},
                }),
                &json!({"ConsistentRead": true}),
            ),
        ),
    ] {
        let queried = ok(&router, "Query", body).await;
        assert_eq!(
            reported_notes(&queried),
            vec!["first".to_string(), "second".to_string()],
            "case {case:?}: expected both t1 items, got {queried}"
        );
        assert!(
            items.len() > 2,
            "case {case:?}: the fixture must hold items outside the partition read, got {}",
            items.len()
        );
    }
}
