// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the saved-query-definition loop: `PutQueryDefinition` /
//! `DescribeQueryDefinitions` / `DeleteQueryDefinition`.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The rules these suites hold the service to:
//!
//! * a put with no `queryDefinitionId` SAVES a new definition under an id CloudWatch
//!   generates, and every member the put named is what a read of that definition reports;
//! * a put WITH a `queryDefinitionId` replaces that definition's values wholesale — "no
//!   current values are retained from the current query definition" — and keeps its id;
//! * `DescribeQueryDefinitions` reports the stored definitions, filtered by name prefix
//!   and by query language, one `maxResults`-sized page at a time;
//! * `DeleteQueryDefinition` removes exactly the definition its id names;
//! * a definition that is not stored cannot be updated or deleted;
//! * a create past the ceiling on what this emulator stores is refused with the modelled
//!   `LimitExceededException`, while an update of a definition already stored is not.
//!
//! The tables span the classes those rules generalise over (query language including the
//! omitted one, the presence and size of `logGroupNames` and `parameters`, name prefixes,
//! page size, the published `maxResults` bounds); any single definition a report happens
//! to mention is one row.

use serde_json::{json, Value};
use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::MAX_QUERY_DEFINITIONS;

use crate::harness::{now_ms, Logs};

/// The values `queryLanguage` takes. Source: the model's `QueryLanguage` enum
/// (`CWLI` | `PPL` | `SQL`).
const QUERY_LANGUAGES: [&str; 3] = ["CWLI", "PPL", "SQL"];

/// The language a definition saved without a `queryLanguage` is at: `CWLI`, how the
/// model's `QueryLanguage` spells Logs Insights QL — the language whose queries
/// `PutQueryDefinition$parameters` ("supported only for Logs Insights QL queries") are
/// accepted for on a request that names no language at all.
const DEFAULT_QUERY_LANGUAGE: &str = QUERY_LANGUAGES[0];

/// The published bounds of `DescribeQueryDefinitions$maxResults`. Source: the
/// `QueryListMaxResults` shape, `range 1..1000`.
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 1_000;

/// The code `PutQueryDefinition`, `DescribeQueryDefinitions` and
/// `DeleteQueryDefinition` declare for a request they refuse: each lists
/// `InvalidParameterException` and no `ValidationException`.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The code those three operations declare for a resource that is not stored.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The code `PutQueryDefinition` declares for a create it cannot store: "You have reached
/// the maximum number of resources that can be created."
const LIMIT_EXCEEDED: &str = "LimitExceededException";

/// A `clientToken` the model admits: `ClientToken` is `@length(36..128)` with
/// `@pattern ^\S{36,128}$`, so a 36-character token carrying no whitespace.
const CLIENT_TOKEN: &str = "11111111-1111-4111-8111-111111111111";
const OTHER_CLIENT_TOKEN: &str = "22222222-2222-4222-8222-222222222222";
const UPDATE_CLIENT_TOKEN: &str = "33333333-3333-4333-8333-333333333333";

/// One query parameter a definition can carry, as `PutQueryDefinition$parameters` models
/// one: a required `name` plus an optional default value and description.
#[derive(Debug, Clone, Copy)]
struct Param {
    name: &'static str,
    default_value: Option<&'static str>,
    description: Option<&'static str>,
}

impl Param {
    fn body(&self) -> Value {
        let mut body = json!({"name": self.name});
        if let Some(default_value) = self.default_value {
            body["defaultValue"] = json!(default_value);
        }
        if let Some(description) = self.description {
            body["description"] = json!(description);
        }
        body
    }
}

/// One put row: everything the request names, so the assertions can be derived from it.
#[derive(Debug, Clone)]
struct Row {
    name: &'static str,
    query_string: &'static str,
    /// `None` for a row whose request names no `queryLanguage` at all.
    language: Option<&'static str>,
    /// `None` for a row whose request names no `logGroupNames` at all.
    log_groups: Option<&'static [&'static str]>,
    /// `None` for a row whose request names no `parameters` at all.
    parameters: Option<&'static [Param]>,
}

impl Row {
    /// The `PutQueryDefinition` request body this row describes: a member the row leaves
    /// out is left out of the request too.
    fn put_body(&self) -> Value {
        let mut body = json!({"name": self.name, "queryString": self.query_string});
        if let Some(language) = self.language {
            body["queryLanguage"] = json!(language);
        }
        if let Some(groups) = self.log_groups {
            body["logGroupNames"] = json!(groups);
        }
        if let Some(parameters) = self.parameters {
            body["parameters"] = json!(parameters
                .iter()
                .map(Param::body)
                .collect::<Vec<Value>>());
        }
        body
    }

    /// The language this row's definition is stored at: what it named, or the default for
    /// a row that named none.
    fn expected_language(&self) -> &'static str {
        self.language.unwrap_or(DEFAULT_QUERY_LANGUAGE)
    }
}

/// The table: one row per class the round-trip rule generalises over — each query
/// language and the omitted one, a definition limited to no / one / several log groups,
/// and no / one / several query parameters with and without their optional members.
fn round_trip_rows() -> Vec<Row> {
    vec![
        Row {
            name: "errors/unscoped",
            query_string: "fields @timestamp, @message | sort @timestamp desc | limit 20",
            language: None,
            log_groups: None,
            parameters: None,
        },
        Row {
            name: "errors/one-group",
            query_string: "filter @message like /ERROR/ | stats count(*)",
            language: Some("CWLI"),
            log_groups: Some(&["/aws/lambda/alpha"]),
            parameters: Some(&[Param {
                name: "threshold",
                default_value: Some("500"),
                description: None,
            }]),
        },
        Row {
            name: "latency/three-groups",
            query_string: "source = logs | where latency > {{floor}} | head 5",
            language: Some("PPL"),
            log_groups: Some(&[
                "/aws/lambda/alpha",
                "/aws/lambda/beta",
                "/aws/apigateway/gamma",
            ]),
            parameters: None,
        },
        Row {
            name: "latency/sql",
            query_string: "SELECT @message FROM `/aws/lambda/alpha` LIMIT 1",
            language: Some("SQL"),
            log_groups: None,
            parameters: Some(&[
                Param {
                    name: "floor",
                    default_value: None,
                    description: Some("the slowest request to ignore"),
                },
                Param {
                    name: "window_minutes",
                    default_value: Some("15"),
                    description: Some("how far back to look"),
                },
            ]),
        },
    ]
}

/// Save one definition and answer the `queryDefinitionId` the service generated for it.
async fn put(logs: &Logs, body: &Value) -> String {
    let response = logs.ok("PutQueryDefinition", body).await;
    response
        .get("queryDefinitionId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {body}: a put answers a queryDefinitionId, got {response}"))
        .to_string()
}

/// Every definition one `DescribeQueryDefinitions` request reports, walking its pages.
async fn describe(logs: &Logs, request: &Value) -> Vec<Value> {
    let mut found = Vec::new();
    let mut token: Option<String> = None;
    for page_index in 0..256 {
        let mut body = request.clone();
        if let Some(token) = &token {
            body["nextToken"] = json!(token);
        }
        let page = logs.ok("DescribeQueryDefinitions", &body).await;
        let definitions = page
            .get("queryDefinitions")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!("case {request} page {page_index}: a page carries queryDefinitions, got {page}")
            })
            .clone();
        found.extend(definitions);
        match page.get("nextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => return found,
        }
    }
    panic!("case {request}: pagination did not terminate, collected {found:?}");
}

/// Every stored definition, unfiltered.
async fn all_definitions(logs: &Logs) -> Vec<Value> {
    describe(logs, &json!({})).await
}

/// The one definition stored under `id`, or `None` once it is gone.
async fn definition(logs: &Logs, id: &str) -> Option<Value> {
    let mut matching: Vec<Value> = all_definitions(logs)
        .await
        .into_iter()
        .filter(|found| member(found, "queryDefinitionId") == Some(id.to_string()))
        .collect();
    assert!(
        matching.len() <= 1,
        "case {id}: at most one definition is reported under one id, got {matching:?}"
    );
    matching.pop()
}

/// The `queryDefinitionId`s one describe reports, in the order it reported them.
fn ids(definitions: &[Value]) -> Vec<String> {
    definitions
        .iter()
        .map(|found| {
            member(found, "queryDefinitionId")
                .unwrap_or_else(|| panic!("a reported definition carries an id, got {found}"))
        })
        .collect()
}

/// One string member of a reported definition, or `None` when it is absent.
fn member(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(ToString::to_string)
}

/// Assert the definition reported under `id` is exactly what `row` named.
fn assert_matches_row(case: &str, found: &Value, row: &Row) {
    assert_eq!(
        member(found, "name").as_deref(),
        Some(row.name),
        "case {case}: name must be echoed, got {found}"
    );
    assert_eq!(
        member(found, "queryString").as_deref(),
        Some(row.query_string),
        "case {case}: queryString must be echoed, got {found}"
    );
    assert_eq!(
        member(found, "queryLanguage").as_deref(),
        Some(row.expected_language()),
        "case {case}: the stored query language is reported, got {found}"
    );
    let reported_groups = found
        .get("logGroupNames")
        .and_then(Value::as_array)
        .map(|groups| {
            groups
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect::<Vec<String>>()
        });
    let expected_groups = row.log_groups.map(|groups| {
        groups
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<String>>()
    });
    assert_eq!(
        reported_groups, expected_groups,
        "case {case}: logGroupNames is reported only when the put named any, got {found}"
    );
    let expected_parameters = row.parameters.map(|parameters| {
        parameters
            .iter()
            .map(Param::body)
            .collect::<Vec<Value>>()
    });
    let reported_parameters = found.get("parameters").and_then(Value::as_array).cloned();
    assert_eq!(
        reported_parameters, expected_parameters,
        "case {case}: parameters is reported only when the put named any, got {found}"
    );
}

/// Every accepted put is readable back through `DescribeQueryDefinitions` with the
/// members it named, an id CloudWatch generated for it, and a live `lastModified` — over
/// the whole class of query languages, log-group lists and parameter lists.
#[tokio::test]
async fn put_is_readable_back_through_describe() {
    let logs = Logs::new();
    let before = now_ms();

    let mut saved = Vec::new();
    for row in round_trip_rows() {
        let id = put(&logs, &row.put_body()).await;
        assert!(
            id.split('-').map(str::len).eq([8usize, 4, 4, 4, 12]),
            "case {row:?}: the generated queryDefinitionId is a UUID, got {id:?}"
        );
        saved.push((row, id));
    }
    let after = now_ms();

    // Every id is distinct: a definition is identified by the id CloudWatch generated,
    // never by its name.
    let minted: std::collections::BTreeSet<&String> = saved.iter().map(|(_, id)| id).collect();
    assert_eq!(
        minted.len(),
        saved.len(),
        "every saved definition gets its own id, got {saved:?}"
    );

    for (row, id) in &saved {
        let case = format!("{row:?}");
        let found = definition(&logs, id)
            .await
            .unwrap_or_else(|| panic!("case {case}: the saved definition must be reported"));
        assert_matches_row(&case, &found, row);
        let last_modified = found
            .get("lastModified")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("case {case}: a definition carries lastModified, got {found}"));
        assert!(
            (before..=after).contains(&last_modified),
            "case {case}: lastModified must be the wall clock of the put, \
             expected {before}..={after}, got {last_modified}"
        );
    }

    // Two creates that name the same thing are two definitions, not one.
    let row = round_trip_rows().remove(0);
    let first = put(&logs, &row.put_body()).await;
    let second = put(&logs, &row.put_body()).await;
    assert_ne!(
        first, second,
        "case {row:?}: a second create of the same name is its own definition"
    );
}

/// An update — a put that names a `queryDefinitionId` — replaces every value and retains
/// none, and keeps the definition's id. Each row is updated to the NEXT row of the table,
/// so the class of transitions (language changed, log groups dropped and added, parameters
/// dropped and added) is covered rather than one rename.
#[tokio::test]
async fn an_update_replaces_every_value_and_retains_none() {
    let rows = round_trip_rows();
    for (index, before) in rows.iter().enumerate() {
        let after = &rows[(index + 1) % rows.len()];
        let case = format!("{before:?} -> {after:?}");

        let logs = Logs::new();
        let id = put(&logs, &before.put_body()).await;

        let mut update = after.put_body();
        update["queryDefinitionId"] = json!(id);
        let updated = put(&logs, &update).await;
        assert_eq!(
            updated, id,
            "case {case}: an update keeps the definition's id"
        );

        let stored = all_definitions(&logs).await;
        assert_eq!(
            stored.len(),
            1,
            "case {case}: an update stores no second definition, got {stored:?}"
        );
        assert_matches_row(&case, &stored[0], after);
    }
}

/// `DescribeQueryDefinitions` filters by name prefix and by query language, and a request
/// that names neither reports every stored definition.
#[tokio::test]
async fn describe_filters_by_name_prefix_and_query_language() {
    let logs = Logs::new();
    let rows = round_trip_rows();
    for row in &rows {
        put(&logs, &row.put_body()).await;
    }

    assert_eq!(
        all_definitions(&logs).await.len(),
        rows.len(),
        "an unfiltered describe reports every stored definition"
    );

    // Prefixes spanning the class: a shared folder prefix matching several rows, a whole
    // name matching one, and a prefix matching none.
    for prefix in ["errors/", "latency/", "latency/sql", "errors/unscoped", "nothing"] {
        let expected: Vec<&Row> = rows
            .iter()
            .filter(|row| row.name.starts_with(prefix))
            .collect();
        let found = describe(&logs, &json!({"queryDefinitionNamePrefix": prefix})).await;
        let mut found_names: Vec<String> = found.iter().filter_map(|f| member(f, "name")).collect();
        found_names.sort();
        let mut expected_names: Vec<String> =
            expected.iter().map(|row| row.name.to_string()).collect();
        expected_names.sort();
        assert_eq!(
            found_names, expected_names,
            "case prefix {prefix:?}: only the definitions whose names start with it are reported"
        );
    }

    // Each language of the model's enum, including the one the default put is at.
    for language in QUERY_LANGUAGES {
        let expected: Vec<String> = rows
            .iter()
            .filter(|row| row.expected_language() == language)
            .map(|row| row.name.to_string())
            .collect();
        let found = describe(&logs, &json!({"queryLanguage": language})).await;
        let mut found_names: Vec<String> = found.iter().filter_map(|f| member(f, "name")).collect();
        found_names.sort();
        let mut expected_names = expected;
        expected_names.sort();
        assert_eq!(
            found_names, expected_names,
            "case language {language}: only the definitions using it are reported"
        );
    }

    // Both filters together narrow to their intersection.
    let found = describe(
        &logs,
        &json!({"queryDefinitionNamePrefix": "errors/", "queryLanguage": DEFAULT_QUERY_LANGUAGE}),
    )
    .await;
    let mut found_names: Vec<String> = found.iter().filter_map(|f| member(f, "name")).collect();
    found_names.sort();
    assert_eq!(
        found_names,
        vec!["errors/one-group".to_string(), "errors/unscoped".to_string()],
        "the two filters narrow to their intersection"
    );
}

/// A page holds at most `maxResults` definitions, its `nextToken` resumes where it ended,
/// and walking the pages reports every definition exactly once — at each page size, and at
/// the published bounds of the member. A value outside those bounds is refused.
#[tokio::test]
async fn describe_pages_at_max_results() {
    let logs = Logs::new();
    // Enough definitions that the small page sizes below need several pages.
    let names: Vec<String> = (0..7).map(|index| format!("paged/{index}")).collect();
    for name in &names {
        put(
            &logs,
            &json!({"name": name, "queryString": "fields @message"}),
        )
        .await;
    }
    let every_id = ids(&all_definitions(&logs).await);
    assert_eq!(
        every_id.len(),
        names.len(),
        "every put is stored, got {every_id:?}"
    );

    for max_results in [MAX_RESULTS_MIN, 2, 3, 6, 7, 8, MAX_RESULTS_MAX] {
        let mut walked = Vec::new();
        let mut token: Option<String> = None;
        let page_limit = usize::try_from(max_results).expect("a positive page size");
        for page_index in 0..64 {
            let mut body = json!({"maxResults": max_results});
            if let Some(token) = &token {
                body["nextToken"] = json!(token);
            }
            let page = logs.ok("DescribeQueryDefinitions", &body).await;
            let definitions = page
                .get("queryDefinitions")
                .and_then(Value::as_array)
                .unwrap_or_else(|| {
                    panic!("case maxResults {max_results} page {page_index}: expected a page, got {page}")
                })
                .clone();
            assert!(
                definitions.len() <= page_limit,
                "case maxResults {max_results} page {page_index}: expected at most {max_results} \
                 definitions, got {}",
                definitions.len()
            );
            walked.extend(ids(&definitions));
            match page.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, every_id,
            "case maxResults {max_results}: the pages report every definition exactly once, \
             in the order an unpaged describe reports them"
        );
    }

    // One past each published bound is refused, with the code this operation declares.
    for max_results in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        let code = logs
            .err(
                "DescribeQueryDefinitions",
                &json!({"maxResults": max_results}),
            )
            .await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case maxResults {max_results}: a value outside range {MAX_RESULTS_MIN}..{MAX_RESULTS_MAX} is refused"
        );
    }

    // A token this service did not mint is refused rather than read as a position.
    let code = logs
        .err(
            "DescribeQueryDefinitions",
            &json!({"nextToken": "not-a-token"}),
        )
        .await;
    assert_eq!(
        code, INVALID_PARAMETER,
        "a nextToken this service did not mint is refused"
    );
}

/// `DeleteQueryDefinition` removes exactly the definition its id names, and a definition
/// that is not stored can neither be deleted nor updated.
#[tokio::test]
async fn delete_removes_only_the_definition_it_names() {
    let logs = Logs::new();
    let rows = round_trip_rows();
    let mut saved = Vec::new();
    for row in &rows {
        saved.push((row.clone(), put(&logs, &row.put_body()).await));
    }

    for (index, (row, id)) in saved.iter().enumerate() {
        let case = format!("{row:?}");
        let response = logs
            .ok("DeleteQueryDefinition", &json!({"queryDefinitionId": id}))
            .await;
        assert_eq!(
            response.get("success").and_then(Value::as_bool),
            Some(true),
            "case {case}: a delete that removed the definition reports success, got {response}"
        );
        assert!(
            definition(&logs, id).await.is_none(),
            "case {case}: the deleted definition is gone"
        );

        // Every definition this delete did not name is still reported.
        let remaining = ids(&all_definitions(&logs).await);
        let expected: Vec<String> = saved
            .iter()
            .skip(index + 1)
            .map(|(_, other)| other.clone())
            .collect();
        let mut remaining_sorted = remaining.clone();
        remaining_sorted.sort();
        let mut expected_sorted = expected.clone();
        expected_sorted.sort();
        assert_eq!(
            remaining_sorted, expected_sorted,
            "case {case}: a delete removes only the definition it names"
        );

        // Deleting the same definition twice is a refusal, not a second success.
        let code = logs
            .err("DeleteQueryDefinition", &json!({"queryDefinitionId": id}))
            .await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case}: a definition that is not stored cannot be deleted"
        );
    }

    // Updating an id nothing is stored under is the same refusal, over the forms an id
    // takes: a well-formed id that was never minted, and one that was deleted above.
    let absent = [
        "00000000-0000-4000-8000-000000000000".to_string(),
        saved[0].1.clone(),
    ];
    for id in absent {
        let code = logs
            .err(
                "PutQueryDefinition",
                &json!({
                    "queryDefinitionId": id,
                    "name": "errors/unscoped",
                    "queryString": "fields @message",
                }),
            )
            .await;
        assert_eq!(
            code, NOT_FOUND,
            "case {id}: a definition that is not stored cannot be updated"
        );
    }
    assert!(
        all_definitions(&logs).await.is_empty(),
        "a refused update stores nothing"
    );
}

/// The ceiling on what this emulator stores is on the definitions HELD, not on the calls
/// made: a create past it is refused with the modelled `LimitExceededException`, while an
/// update of a definition already stored still succeeds, and deleting one makes room
/// again.
#[tokio::test]
async fn the_store_ceiling_refuses_a_create_but_not_an_update() {
    let logs = Logs::new();
    let mut last = String::new();
    for index in 0..MAX_QUERY_DEFINITIONS {
        last = put(
            &logs,
            &json!({"name": format!("capped/{index}"), "queryString": "fields @message"}),
        )
        .await;
    }

    let overflow = json!({"name": "capped/overflow", "queryString": "fields @message"});
    let code = logs.err("PutQueryDefinition", &overflow).await;
    assert_eq!(
        code, LIMIT_EXCEEDED,
        "case at the ceiling: a create past {MAX_QUERY_DEFINITIONS} definitions is refused"
    );
    assert_eq!(
        all_definitions(&logs).await.len(),
        MAX_QUERY_DEFINITIONS,
        "case at the ceiling: a refused create stores nothing"
    );

    let mut update = json!({"name": "capped/renamed", "queryString": "fields @timestamp"});
    update["queryDefinitionId"] = json!(last);
    let updated = put(&logs, &update).await;
    assert_eq!(
        updated, last,
        "case at the ceiling: an update adds no definition, so it is not refused"
    );

    logs.ok(
        "DeleteQueryDefinition",
        &json!({"queryDefinitionId": last}),
    )
    .await;
    let after_delete = put(&logs, &overflow).await;
    assert_ne!(
        after_delete, last,
        "case below the ceiling: deleting a definition makes room for a create of its own"
    );
    assert_eq!(
        all_definitions(&logs).await.len(),
        MAX_QUERY_DEFINITIONS,
        "case below the ceiling: the store is back at the ceiling, never past it"
    );
}

/// `clientToken` is an idempotency token: a create retried under the same token answers
/// with the definition the first call saved instead of saving a second one, while a
/// different token (or none at all) saves its own definition.
#[tokio::test]
async fn a_retried_create_under_one_client_token_saves_one_definition() {
    let logs = Logs::new();
    let row = round_trip_rows().remove(1);

    let mut body = row.put_body();
    body["clientToken"] = json!(CLIENT_TOKEN);
    let first = put(&logs, &body).await;
    let retried = put(&logs, &body).await;
    assert_eq!(
        retried, first,
        "case {row:?}: the same clientToken answers with the definition already saved"
    );
    assert_eq!(
        all_definitions(&logs).await.len(),
        1,
        "case {row:?}: a retry under one clientToken saves one definition"
    );

    let mut other = row.put_body();
    other["clientToken"] = json!(OTHER_CLIENT_TOKEN);
    let second = put(&logs, &other).await;
    assert_ne!(
        second, first,
        "case {row:?}: another clientToken saves its own definition"
    );

    let untokened = put(&logs, &row.put_body()).await;
    assert!(
        untokened != first && untokened != second,
        "case {row:?}: a create with no clientToken saves its own definition, got {untokened}"
    );
    assert_eq!(
        all_definitions(&logs).await.len(),
        3,
        "case {row:?}: three distinct creates are three definitions"
    );

    // A token an UPDATE carried identifies no create: the update keeps the definition's
    // id, and a later create under that token is its own definition, while the token the
    // first create carried still answers the definition that create minted.
    let mut update = row.put_body();
    update["queryDefinitionId"] = json!(first);
    update["clientToken"] = json!(UPDATE_CLIENT_TOKEN);
    assert_eq!(
        put(&logs, &update).await,
        first,
        "case {row:?}: an update keeps the definition's id whatever token it carries"
    );

    let mut create_under_update_token = row.put_body();
    create_under_update_token["clientToken"] = json!(UPDATE_CLIENT_TOKEN);
    let fourth = put(&logs, &create_under_update_token).await;
    assert!(
        fourth != first && fourth != second && fourth != untokened,
        "case {row:?}: a token an update carried de-duplicates no create, got {fourth}"
    );
    assert_eq!(
        put(&logs, &body).await,
        first,
        "case {row:?}: the create's own token still answers the definition it minted"
    );
    assert_eq!(
        all_definitions(&logs).await.len(),
        4,
        "case {row:?}: four distinct creates are four definitions"
    );
}
