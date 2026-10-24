// SPDX-License-Identifier: CC0-1.0

//! `DeleteStoredQuery`, end to end over the wire.
//!
//! Three rules are pinned here, each over a table of inputs rather than one case:
//!
//! 1. deleting a stored query removes THAT query from every path that reports it —
//!    `GetStoredQuery` no longer finds it, `ListStoredQueries` no longer lists it, its
//!    ARN resolves to no tag set — and leaves every other stored query alone;
//! 2. a `QueryName` the account holds no query under is refused with the modelled
//!    `ResourceNotFoundException` (HTTP 400) quoting the name the request carried, and
//!    persists nothing; a second delete of the same name is that same refusal;
//! 3. a deleted name is free again: saving under it creates a NEW query with a fresh
//!    service-assigned identity, and the slot it gave back is free under the Region's
//!    stored-query quota.
//!
//! The harness (`Emulator`) is the one `config_setup_and_rule_loop` already builds;
//! every case drives the emulator the way a client does, through the
//! `MultiServiceRouter`.

use serde_json::{json, Value};

use crate::config_setup_and_rule_loop::Emulator;

/// The published maximum length of a `QueryName` (`StoredQuery$QueryName`, length
/// 1..64) from `smithy_query(query="operation", operation="DeleteStoredQuery")` on this
/// service's model.
const QUERY_NAME_MAX: usize = 64;

/// The stored-query quota for one account in one Region (`PutStoredQuery`: "You can
/// create upto 300 queries in a single Amazon Web Services account and a single Amazon
/// Web Services Region").
const STORED_QUERIES_QUOTA: usize = 300;

/// A minimal query expression; its text is never asserted on, only saved.
const SELECT_BUCKETS: &str =
    "SELECT resourceId, resourceType WHERE resourceType = 'AWS::S3::Bucket'";

/// Save `name` and return the `QueryArn` the service answered with. `tags` are carried
/// only when the list is not empty, so a save without tags sends no `Tags` member.
async fn save(
    emulator: &Emulator,
    name: &str,
    description: Option<&str>,
    expression: &str,
    tags: &[(&str, &str)],
) -> String {
    let mut query = json!({ "QueryName": name, "Expression": expression });
    if let Some(description) = description {
        query["Description"] = json!(description);
    }
    let mut body = json!({ "StoredQuery": query });
    if !tags.is_empty() {
        body["Tags"] = Value::Array(
            tags.iter()
                .map(|(key, value)| json!({ "Key": key, "Value": value }))
                .collect(),
        );
    }
    let saved = emulator.ok("PutStoredQuery", &body).await;
    saved["QueryArn"]
        .as_str()
        .unwrap_or_else(|| panic!("PutStoredQuery must answer a QueryArn, got {saved}"))
        .to_string()
}

/// The `QueryId` `GetStoredQuery` reports for `name`.
async fn query_id(emulator: &Emulator, name: &str) -> String {
    let got = emulator
        .ok("GetStoredQuery", &json!({ "QueryName": name }))
        .await;
    got["StoredQuery"]["QueryId"]
        .as_str()
        .unwrap_or_else(|| panic!("GetStoredQuery must answer a QueryId, got {got}"))
        .to_string()
}

/// Every `QueryName` `ListStoredQueries` reports, following the token chain so the
/// answer is the whole account's set and not just its first page.
async fn listed_names(emulator: &Emulator) -> Vec<String> {
    let mut names = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({});
        if let Some(token) = &token {
            body["NextToken"] = json!(token);
        }
        let page = emulator.ok("ListStoredQueries", &body).await;
        for entry in page["StoredQueryMetadata"].as_array().into_iter().flatten() {
            names.push(
                entry["QueryName"]
                    .as_str()
                    .unwrap_or_else(|| panic!("a listed query must carry a QueryName, got {entry}"))
                    .to_string(),
            );
        }
        token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            return names;
        }
    }
}

/// Call `operation` expecting a 400 refusal with the error code `code`, and return the
/// message it carried.
async fn refusal(emulator: &Emulator, case: &str, operation: &str, body: &Value, code: &str) -> String {
    let (status, parsed) = emulator.call(operation, body).await;
    let answered = parsed
        .get("__type")
        .and_then(Value::as_str)
        .map(|wire| wire.rsplit('#').next().unwrap_or(wire))
        .unwrap_or("<no __type>");
    assert_eq!(
        (status, answered),
        (400, code),
        "case {case:?}: expected HTTP 400 {code}, got {status} with body {parsed}"
    );
    parsed
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a message in {parsed}"))
        .to_string()
}

/// How Config words the `ResourceNotFoundException` of a request that named a stored
/// query the account holds none of: the message is BUILT FROM THE REQUEST, quoting the
/// `QueryName` the caller sent, so it is asserted in full rather than sampled.
fn not_found_message(name: &str) -> String {
    format!("The query '{name}' provided in the request is invalid. Please check the query name.")
}

/// The names of the two queries every row saves beside the one it deletes, so the
/// delete can be shown to touch only what it named.
const BYSTANDERS: [&str; 2] = ["bystander-one", "bystander_two"];

#[tokio::test]
async fn deleting_a_stored_query_removes_only_that_query() {
    // Rows differ along the shape of the deleted query: the characters its name is
    // built from (letters, digits, hyphen, underscore, mixed case), its length up to
    // the published maximum, whether it carries a description, the text its members
    // carry (ASCII, non-Latin scripts, emoji), and whether it was tagged.
    struct Row {
        case: &'static str,
        name: String,
        description: Option<&'static str>,
        expression: &'static str,
        tags: Vec<(&'static str, &'static str)>,
    }

    let rows = vec![
        Row {
            case: "plain ASCII, no description, untagged",
            name: "ascii-query".to_string(),
            description: None,
            expression: SELECT_BUCKETS,
            tags: Vec::new(),
        },
        Row {
            case: "digits and underscores in the name, with a description",
            name: "query_42_09".to_string(),
            description: Some("the forty-second query"),
            expression: "SELECT resourceId WHERE resourceType = 'AWS::EC2::Instance'",
            tags: Vec::new(),
        },
        Row {
            case: "mixed case in the name, tagged",
            name: "MixedCase-Query".to_string(),
            description: Some("mixed case"),
            expression: SELECT_BUCKETS,
            tags: vec![("owner", "platform"), ("env", "prod")],
        },
        Row {
            case: "non-Latin scripts and emoji in the members, tagged",
            name: "unicode-query".to_string(),
            description: Some("ресурсы 資源 🪣"),
            expression: "SELECT resourceId /* 資源 🚀 */",
            tags: vec![("владелец", "платформа 🧑‍🚀")],
        },
        Row {
            case: "a one-character name, the published minimum length",
            name: "q".to_string(),
            description: None,
            expression: "*",
            tags: vec![("k", "")],
        },
        Row {
            case: "a name at the published maximum length",
            name: "n".repeat(QUERY_NAME_MAX),
            description: Some("the longest name the model allows"),
            expression: SELECT_BUCKETS,
            tags: Vec::new(),
        },
    ];

    for row in rows {
        let case = row.case;
        let emulator = Emulator::new();
        let mut bystander_arns = Vec::new();
        for bystander in BYSTANDERS {
            bystander_arns.push(save(&emulator, bystander, None, SELECT_BUCKETS, &[]).await);
        }
        let arn = save(
            &emulator,
            &row.name,
            row.description,
            row.expression,
            &row.tags,
        )
        .await;

        let deleted = emulator
            .ok("DeleteStoredQuery", &json!({ "QueryName": row.name }))
            .await;
        assert!(
            deleted.as_object().is_none_or(|fields| fields.is_empty()),
            "case {case:?}: DeleteStoredQuery answers an empty response, got {deleted}"
        );

        // Gone from the read path that reports one query...
        let message = refusal(
            &emulator,
            case,
            "GetStoredQuery",
            &json!({ "QueryName": row.name }),
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            message,
            not_found_message(&row.name),
            "case {case:?}: expected the not-found message built from the request, got \
             {message:?}"
        );

        // ...from the one that reports them all...
        let names = listed_names(&emulator).await;
        assert!(
            !names.contains(&row.name),
            "case {case:?}: expected {:?} to be gone from the listing, got {names:?}",
            row.name
        );

        // ...and its ARN now names no resource, so the tags that lived on it are gone
        // with it.
        refusal(
            &emulator,
            case,
            "ListTagsForResource",
            &json!({ "ResourceArn": arn }),
            "ResourceNotFoundException",
        )
        .await;

        // Only the named query went: every other query is readable, with the identity
        // and the tags it was created with.
        for (bystander, bystander_arn) in BYSTANDERS.iter().zip(&bystander_arns) {
            let still_there = emulator
                .ok("GetStoredQuery", &json!({ "QueryName": bystander }))
                .await;
            assert_eq!(
                still_there["StoredQuery"]["QueryArn"].as_str(),
                Some(bystander_arn.as_str()),
                "case {case:?}: deleting {:?} must leave {bystander:?} unchanged, got \
                 {still_there}",
                row.name
            );
        }
        let mut expected_rest: Vec<String> =
            BYSTANDERS.iter().map(|name| name.to_string()).collect();
        expected_rest.sort();
        let mut listed = names;
        listed.sort();
        assert_eq!(
            listed, expected_rest,
            "case {case:?}: expected exactly the queries that were not deleted, got \
             {listed:?}"
        );
    }
}

#[tokio::test]
async fn deleting_a_name_the_account_holds_no_query_under_is_not_found() {
    // Rows differ along WHY the name holds no query: it was never saved, it was already
    // deleted, or it differs from a saved name only in case or in one separator
    // character — a stored query is keyed by its exact name.
    let saved_name = "held-query";
    let longest_absent = "a".repeat(QUERY_NAME_MAX);
    let rows: Vec<(&str, String, bool)> = vec![
        ("a name nobody ever saved", "never-saved".to_string(), false),
        (
            "a name at the published maximum length, never saved",
            longest_absent.clone(),
            false,
        ),
        (
            "a name differing from a saved one only in case",
            "HELD-QUERY".to_string(),
            false,
        ),
        (
            "a name differing from a saved one by one separator",
            "held_query".to_string(),
            false,
        ),
        (
            "a name whose query was deleted a moment ago",
            "deleted-query".to_string(),
            true,
        ),
        (
            "a one-character name, the published minimum length",
            "z".to_string(),
            false,
        ),
    ];

    for (case, name, save_then_delete) in rows {
        let emulator = Emulator::new();
        let witness_arn = save(&emulator, saved_name, None, SELECT_BUCKETS, &[]).await;
        if save_then_delete {
            save(&emulator, &name, None, SELECT_BUCKETS, &[]).await;
            emulator
                .ok("DeleteStoredQuery", &json!({ "QueryName": name }))
                .await;
        }

        let message = refusal(
            &emulator,
            case,
            "DeleteStoredQuery",
            &json!({ "QueryName": name }),
            "ResourceNotFoundException",
        )
        .await;
        assert_eq!(
            message,
            not_found_message(&name),
            "case {case:?}: expected the not-found message built from the request, got \
             {message:?}"
        );

        // A refused delete persists nothing: the query the account does hold is intact,
        // and the absent name is still absent.
        let witness = emulator
            .ok("GetStoredQuery", &json!({ "QueryName": saved_name }))
            .await;
        assert_eq!(
            witness["StoredQuery"]["QueryArn"].as_str(),
            Some(witness_arn.as_str()),
            "case {case:?}: a refused delete must leave {saved_name:?} unchanged, got \
             {witness}"
        );
        let names = listed_names(&emulator).await;
        assert_eq!(
            names,
            vec![saved_name.to_string()],
            "case {case:?}: expected only {saved_name:?} to be held, got {names:?}"
        );
    }
}

#[tokio::test]
async fn a_deleted_name_is_free_to_be_saved_again() {
    // Rows differ along the name reused and the expression the second save carries, so
    // the new query is distinguishable from the one that was deleted.
    let rows = [
        ("plain ASCII", "reused-query", "SELECT resourceId"),
        ("underscores and digits", "reused_2", "SELECT awsRegion"),
        (
            "non-Latin text in the expression",
            "reused-unicode",
            "SELECT resourceId /* 再利用 ♻️ */",
        ),
    ];

    for (case, name, expression) in rows {
        let emulator = Emulator::new();
        let first_arn = save(&emulator, name, Some("the first"), SELECT_BUCKETS, &[]).await;
        let first_id = query_id(&emulator, name).await;
        emulator
            .ok("DeleteStoredQuery", &json!({ "QueryName": name }))
            .await;

        // A save under a freed name CREATES a query: the service assigns it a new
        // identity rather than resurrecting the deleted one's.
        let second_arn = save(&emulator, name, None, expression, &[]).await;
        let second_id = query_id(&emulator, name).await;
        assert_ne!(
            second_id, first_id,
            "case {case:?}: a query saved under a freed name is a new resource, so its \
             QueryId must differ from the deleted query's ({first_id:?})"
        );
        assert_ne!(
            second_arn, first_arn,
            "case {case:?}: the new query's ARN carries its own id, so it must differ \
             from {first_arn:?}"
        );
        let read_back = emulator
            .ok("GetStoredQuery", &json!({ "QueryName": name }))
            .await;
        let stored = &read_back["StoredQuery"];
        assert_eq!(
            stored["Expression"].as_str(),
            Some(expression),
            "case {case:?}: expected the re-saved expression, got {read_back}"
        );
        assert_eq!(
            stored["QueryArn"].as_str(),
            Some(second_arn.as_str()),
            "case {case:?}: expected the ARN the second save answered, got {read_back}"
        );
        // The deleted query carried a description and the new one carries none, so a
        // reused name must not inherit the deleted query's members.
        assert!(
            stored.get("Description").is_none(),
            "case {case:?}: a query saved without a description must carry none, got \
             {read_back}"
        );
    }
}

#[tokio::test]
async fn a_delete_frees_a_slot_under_the_regions_stored_query_quota() {
    let emulator = Emulator::new();
    for index in 0..STORED_QUERIES_QUOTA {
        save(
            &emulator,
            &format!("query-{index:03}"),
            None,
            SELECT_BUCKETS,
            &[],
        )
        .await;
    }
    refusal(
        &emulator,
        "one past the quota",
        "PutStoredQuery",
        &json!({ "StoredQuery": { "QueryName": "one-too-many", "Expression": SELECT_BUCKETS } }),
        "ValidationException",
    )
    .await;

    emulator
        .ok("DeleteStoredQuery", &json!({ "QueryName": "query-000" }))
        .await;

    // The deleted query gave its slot back, so the save that was refused a moment ago
    // now succeeds.
    let arn = save(&emulator, "one-too-many", None, SELECT_BUCKETS, &[]).await;
    assert!(
        arn.contains(":stored-query/one-too-many/query-"),
        "case \"after a delete\": expected a query ARN for the new query, got {arn:?}"
    );
    let names = listed_names(&emulator).await;
    assert_eq!(
        names.len(),
        STORED_QUERIES_QUOTA,
        "case \"after a delete\": expected the account to hold {STORED_QUERIES_QUOTA} \
         queries, got {}",
        names.len()
    );
    assert!(
        !names.contains(&"query-000".to_string()),
        "case \"after a delete\": expected the deleted query to be gone from the \
         listing of {} queries",
        names.len()
    );
}
