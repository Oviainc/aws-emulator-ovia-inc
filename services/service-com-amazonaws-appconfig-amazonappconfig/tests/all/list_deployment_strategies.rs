// SPDX-License-Identifier: CC0-1.0

//! `ListDeploymentStrategies` as a paginated report over the account's deployment-strategy
//! catalogue.
//!
//! The rule under test: the listing reports exactly the deployment strategies the account
//! can deploy with — AppConfig's predefined strategies plus every strategy this account
//! created — each one exactly once and as the record a `GetDeploymentStrategy` of its id
//! answers, in an order that is stable across calls, cut into pages of at most
//! `MaxResults` entries, with a `NextToken` handed back only when entries are left over
//! and a token that resumes the traversal where the previous page ended. Being a report,
//! walking it changes nothing a later caller sees.
//!
//! The tables below span what that rule generalises over: HOW MANY strategies the account
//! created (none, one, several, and enough to run one past the service's page size), WHICH
//! page size the request asks for (absent, both published bounds of `MaxResults`, and
//! values in between), and WHAT each strategy carries (each `GrowthType`, each
//! `ReplicateTo`, a description present or absent, and the published bounds of the
//! duration and growth-factor members). Every assertion names its case and prints the
//! actual value.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::state::{
    MAX_LIST_PAGE_SIZE, PREDEFINED_DEPLOYMENT_STRATEGIES,
};

use super::appconfig_workflow::{create_application, create_environment, json_call, router, Answer};

/// The collection the model binds `ListDeploymentStrategies` to
/// (`GET /deploymentstrategies`, from
/// `smithy_query(query="operation",operation="ListDeploymentStrategies")`).
const STRATEGIES: &str = "/deploymentstrategies";

/// `ListDeploymentStrategies`, with `query` as the raw query string (empty for a request
/// that names neither `MaxResults` nor `NextToken`). Both are `httpQuery` members, named
/// `max_results` and `next_token`.
async fn list(router: &MultiServiceRouter, query: &str) -> Answer {
    let uri = if query.is_empty() {
        STRATEGIES.to_string()
    } else {
        format!("{STRATEGIES}?{query}")
    };
    json_call(router, "GET", &uri, &Value::Null).await
}

/// `GetDeploymentStrategy` for one id.
async fn get_strategy(router: &MultiServiceRouter, id: &str) -> Answer {
    json_call(router, "GET", &format!("{STRATEGIES}/{id}"), &Value::Null).await
}

/// The `Items` of a listing, as the JSON array the response carries (an answer with no
/// `Items` at all reads as empty).
fn items(answer: &Answer) -> Vec<Value> {
    answer
        .json()
        .get("Items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `Id` of every entry of a listing, in the order it was served.
fn listed_ids(answer: &Answer) -> Vec<String> {
    items(answer)
        .iter()
        .map(|item| {
            item.get("Id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("a listed strategy must carry an Id, got {item}"))
                .to_string()
        })
        .collect()
}

/// A numeric member of a listed entry.
fn number(case: &str, entry: &Value, member: &str) -> f64 {
    entry
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: expected a numeric {member}, got {entry}"))
}

/// Create one deployment strategy and answer its minted id.
async fn create_strategy(router: &MultiServiceRouter, case: &str, body: &Value) -> String {
    let created = json_call(router, "POST", STRATEGIES, body).await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: CreateDeploymentStrategy expected 201, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    created
        .string("Id")
        .unwrap_or_else(|| panic!("case {case:?}: CreateDeploymentStrategy answered no Id"))
}

/// A create request whose criteria vary with `index`, for a suite that needs a collection
/// of a given size rather than one specific strategy. Every value stays inside the
/// published bounds `MinutesBetween0And24Hours` (range 0..1440) and `GrowthFactor`
/// (range 1..100).
fn varied_request(index: usize) -> Value {
    json!({
        "Name": format!("created-{index}"),
        "DeploymentDurationInMinutes": (index as i64 * 7) % 1441,
        "FinalBakeTimeInMinutes": (index as i64 * 3) % 1441,
        "GrowthFactor": 1 + (index as i64 * 9) % 100,
        "GrowthType": if index.is_multiple_of(2) { "LINEAR" } else { "EXPONENTIAL" },
    })
}

/// Walk the whole listing one page at a time and return the ids in the order they were
/// served, asserting the per-page half of the rule as it goes: a page never exceeds the
/// size the request asked for, and a page that carries a `NextToken` is exactly that size,
/// because a traversal is only cut where a page filled up.
async fn walk(router: &MultiServiceRouter, case: &str, max_results: Option<i32>) -> Vec<String> {
    let page_size = max_results.map_or(MAX_LIST_PAGE_SIZE, |size| size as usize);
    let mut collected: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // Bounded so a listing that never ends its traversal fails instead of hanging.
    for _ in 0..=(MAX_LIST_PAGE_SIZE * 4 + 2) {
        let mut query: Vec<String> = Vec::new();
        if let Some(size) = max_results {
            query.push(format!("max_results={size}"));
        }
        if let Some(token) = &token {
            query.push(format!("next_token={token}"));
        }
        let answer = list(router, &query.join("&")).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected 200, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let page = listed_ids(&answer);
        assert!(
            page.len() <= page_size,
            "case {case:?}: a page holds at most {page_size} entries, got {page:?}"
        );
        token = answer.string("NextToken");
        if token.is_some() {
            assert_eq!(
                page.len(),
                page_size,
                "case {case:?}: a page carrying a NextToken is a full page of {page_size}, \
                 got {page:?}"
            );
        }
        collected.extend(page);
        if token.is_none() {
            return collected;
        }
    }
    panic!("case {case:?}: the paginated walk did not terminate");
}

/// An account that created nothing still has a catalogue: AppConfig's predefined
/// strategies, each reported with the criteria it is published with and with its id as its
/// name.
#[tokio::test]
async fn the_catalogue_reports_every_predefined_strategy() {
    let router = router();

    let answer = list(&router, "").await;
    assert_eq!(
        answer.status,
        200,
        "case \"an account that created nothing\": expected 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let listed = items(&answer);
    assert_eq!(
        listed.len(),
        PREDEFINED_DEPLOYMENT_STRATEGIES.len(),
        "case \"an account that created nothing\": expected the {} predefined strategies, \
         got {listed:?}",
        PREDEFINED_DEPLOYMENT_STRATEGIES.len()
    );

    // The catalogue is reported in the order the published table carries it.
    let published: Vec<String> = PREDEFINED_DEPLOYMENT_STRATEGIES
        .iter()
        .map(|strategy| strategy.id.to_string())
        .collect();
    assert_eq!(
        listed_ids(&answer),
        published,
        "case \"an account that created nothing\": expected the published order \
         {published:?}, got {:?}",
        listed_ids(&answer)
    );

    // Rows are the whole predefined catalogue
    // (`state::PREDEFINED_DEPLOYMENT_STRATEGIES`, the AWS AppConfig User Guide's
    // "Predefined deployment strategies" table), so every entry is asserted, not only the
    // one a caller happens to deploy with.
    for predefined in PREDEFINED_DEPLOYMENT_STRATEGIES {
        let case = predefined.id;
        let entry = listed
            .iter()
            .find(|item| item.get("Id").and_then(Value::as_str) == Some(predefined.id))
            .unwrap_or_else(|| {
                panic!("case {case:?}: the catalogue must carry this strategy, got {listed:?}")
            });
        assert_eq!(
            entry.get("Name").and_then(Value::as_str),
            Some(predefined.id),
            "case {case:?}: a predefined strategy's name is its id, got {:?}",
            entry.get("Name")
        );
        assert_eq!(
            number(case, entry, "DeploymentDurationInMinutes"),
            f64::from(predefined.deployment_duration_in_minutes),
            "case {case:?}: expected the published deployment duration {}, got {entry}",
            predefined.deployment_duration_in_minutes
        );
        assert_eq!(
            number(case, entry, "FinalBakeTimeInMinutes"),
            f64::from(predefined.final_bake_time_in_minutes),
            "case {case:?}: expected the published bake time {}, got {entry}",
            predefined.final_bake_time_in_minutes
        );
        assert_eq!(
            entry.get("GrowthType").and_then(Value::as_str),
            Some(predefined.growth_type),
            "case {case:?}: expected the published growth type {:?}, got {:?}",
            predefined.growth_type,
            entry.get("GrowthType")
        );
        assert!(
            (number(case, entry, "GrowthFactor") - f64::from(predefined.growth_factor)).abs()
                < 1e-6,
            "case {case:?}: expected the published growth factor {}, got {entry}",
            predefined.growth_factor
        );
        assert_eq!(
            entry.get("ReplicateTo").and_then(Value::as_str),
            Some("NONE"),
            "case {case:?}: a predefined strategy is saved nowhere else, got {:?}",
            entry.get("ReplicateTo")
        );
        assert_eq!(
            entry.get("Description").and_then(Value::as_str),
            Some(predefined.description),
            "case {case:?}: expected the published description {:?}, got {:?}",
            predefined.description,
            entry.get("Description")
        );

        // The listed entry is the record a Get of it answers.
        let fetched = get_strategy(&router, predefined.id).await;
        assert_eq!(
            fetched.status,
            200,
            "case {case:?}: GetDeploymentStrategy expected 200, got {} / {}",
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );
        assert_eq!(
            *entry,
            fetched.json(),
            "case {case:?}: the listed entry must be the record GetDeploymentStrategy \
             answers {}, got {entry}",
            fetched.json()
        );
    }

    // Stable order: a second identical call serves the same sequence.
    let again = list(&router, "").await;
    assert_eq!(
        listed_ids(&again),
        listed_ids(&answer),
        "case \"two identical calls\": the order must not move between calls, got {:?} \
         then {:?}",
        listed_ids(&answer),
        listed_ids(&again)
    );
}

/// Every created strategy joins the catalogue, as the record a Get of it answers, while
/// the predefined ones stay in it.
#[tokio::test]
async fn created_strategies_join_the_catalogue() {
    let router = router();

    // Rows differ along what the created strategy carries: each `GrowthType` the model
    // lists, each `ReplicateTo` value, a description present and absent, and the published
    // bounds of `MinutesBetween0And24Hours` (range 0..1440) and `GrowthFactor`
    // (range 1..100) — bounds from
    // `smithy_query(query="operation",operation="CreateDeploymentStrategy")`.
    let rows: Vec<(&str, Value)> = vec![
        (
            "every member stated, linear growth",
            json!({
                "Name": "linear-stated",
                "Description": "a flat rollout",
                "DeploymentDurationInMinutes": 30,
                "FinalBakeTimeInMinutes": 10,
                "GrowthFactor": 20,
                "GrowthType": "LINEAR",
                "ReplicateTo": "NONE",
            }),
        ),
        (
            "exponential growth, no description",
            json!({
                "Name": "exponential",
                "DeploymentDurationInMinutes": 20,
                "FinalBakeTimeInMinutes": 10,
                "GrowthFactor": 10,
                "GrowthType": "EXPONENTIAL",
            }),
        ),
        (
            "replicated to an SSM document",
            json!({
                "Name": "replicated",
                "DeploymentDurationInMinutes": 5,
                "GrowthFactor": 50,
                "ReplicateTo": "SSM_DOCUMENT",
            }),
        ),
        (
            "every member at its published lower bound",
            json!({
                "Name": "l",
                "DeploymentDurationInMinutes": 0,
                "FinalBakeTimeInMinutes": 0,
                "GrowthFactor": 1,
            }),
        ),
        (
            "every member at its published upper bound",
            json!({
                "Name": "u".repeat(64),
                "DeploymentDurationInMinutes": 1440,
                "FinalBakeTimeInMinutes": 1440,
                "GrowthFactor": 100,
            }),
        ),
    ];

    let predefined: Vec<String> = PREDEFINED_DEPLOYMENT_STRATEGIES
        .iter()
        .map(|strategy| strategy.id.to_string())
        .collect();
    let mut created: Vec<String> = Vec::new();

    for (case, request) in &rows {
        created.push(create_strategy(&router, case, request).await);

        let answer = list(&router, "").await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected 200, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let reported = listed_ids(&answer);
        // The predefined strategies in published order, then this account's own in
        // ascending id order.
        let mut own = created.clone();
        own.sort();
        let expected: Vec<String> = predefined.iter().cloned().chain(own).collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the predefined catalogue plus every created strategy \
             {expected:?}, got {reported:?}"
        );

        // Every created strategy in the listing is the record a Get of it answers — which
        // is where the criteria the create persisted are checked.
        for id in &created {
            let entry = items(&answer)
                .into_iter()
                .find(|item| item.get("Id").and_then(Value::as_str) == Some(id.as_str()))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the catalogue must carry created strategy {id:?}")
                });
            let fetched = get_strategy(&router, id).await;
            assert_eq!(
                fetched.status,
                200,
                "case {case:?}: GetDeploymentStrategy {id:?} expected 200, got {} / {}",
                fetched.status,
                String::from_utf8_lossy(&fetched.body)
            );
            assert_eq!(
                entry,
                fetched.json(),
                "case {case:?}: the listed entry for {id:?} must be the record \
                 GetDeploymentStrategy answers {}, got {entry}",
                fetched.json()
            );
        }
    }

    // The row at the upper bound of `Name` must have been reported with that whole name:
    // the listing reports the stored record, not a shortened one.
    let longest = items(&list(&router, "").await)
        .into_iter()
        .find(|item| {
            item.get("Name").and_then(Value::as_str).map(str::len) == Some(64)
                && item.get("DeploymentDurationInMinutes").and_then(Value::as_i64) == Some(1440)
        })
        .unwrap_or_else(|| {
            panic!("case \"every member at its published upper bound\": the row must be listed")
        });
    assert_eq!(
        longest.get("Name").and_then(Value::as_str),
        Some("u".repeat(64).as_str()),
        "case \"every member at its published upper bound\": expected the stored name, got {:?}",
        longest.get("Name")
    );
}

/// Whatever page size a request asks for, the pages of a traversal concatenate to the
/// single-call listing: every strategy exactly once, in the same order.
#[tokio::test]
async fn pages_of_every_size_concatenate_to_the_whole_catalogue() {
    // Rows differ along the number of created strategies and the requested page size:
    // `MaxResults` is modelled `@range(min: 1, max: 50)`, so 1 and 50 are its published
    // bounds, and `None` is a request that names no page size at all.
    let sizes = [0_usize, 1, 2, 5, 7];
    let page_sizes = [None, Some(1), Some(2), Some(3), Some(50)];

    for size in sizes {
        // A router per row, so each row's catalogue is exactly the predefined strategies
        // plus the strategies that row created.
        let router = router();
        for index in 0..size {
            create_strategy(
                &router,
                &format!("{size} created strategies, row {index}"),
                &varied_request(index),
            )
            .await;
        }
        let whole = listed_ids(&list(&router, "").await);
        let expected = size + PREDEFINED_DEPLOYMENT_STRATEGIES.len();
        assert_eq!(
            whole.len(),
            expected,
            "case \"{size} created strategies, one call\": expected {expected} entries, \
             got {whole:?}"
        );

        for page_size in page_sizes {
            let case = format!("{size} created strategies, MaxResults={page_size:?}");
            let walked = walk(&router, &case, page_size).await;
            assert_eq!(
                walked, whole,
                "case {case:?}: the pages must concatenate to the whole listing {whole:?}, \
                 got {walked:?}"
            );
            let mut unique = walked.clone();
            unique.sort();
            unique.dedup();
            assert_eq!(
                unique.len(),
                walked.len(),
                "case {case:?}: no strategy may be served twice, got {walked:?}"
            );
        }
    }
}

/// A request naming no page size is cut at the service's own page size, not at the
/// catalogue: one entry past it is served on a second page.
#[tokio::test]
async fn a_request_naming_no_page_size_is_cut_at_the_services_page_size() {
    let router = router();
    // One strategy more than a full page, counting the predefined ones the catalogue
    // already holds.
    for index in 0..=(MAX_LIST_PAGE_SIZE - PREDEFINED_DEPLOYMENT_STRATEGIES.len()) {
        create_strategy(
            &router,
            &format!("one past the page size, row {index}"),
            &varied_request(index),
        )
        .await;
    }

    let first = list(&router, "").await;
    let first_ids = listed_ids(&first);
    assert_eq!(
        first_ids.len(),
        MAX_LIST_PAGE_SIZE,
        "case \"one past the page size\": expected a full page of {MAX_LIST_PAGE_SIZE}, \
         got {} entries",
        first_ids.len()
    );
    let token = first.string("NextToken").unwrap_or_else(|| {
        panic!(
            "case \"one past the page size\": a truncated listing carries a NextToken, got {}",
            String::from_utf8_lossy(&first.body)
        )
    });

    let second = list(&router, &format!("next_token={token}")).await;
    let second_ids = listed_ids(&second);
    assert_eq!(
        second_ids.len(),
        1,
        "case \"one past the page size\": the second page holds the one left-over entry, \
         got {second_ids:?}"
    );
    assert!(
        second.string("NextToken").is_none(),
        "case \"one past the page size\": the last page ends the traversal, got {:?}",
        second.string("NextToken")
    );
    assert!(
        !first_ids.contains(&second_ids[0]),
        "case \"one past the page size\": the second page repeats nothing from the first, \
         got {second_ids:?} after {first_ids:?}"
    );
}

/// A page size outside the modelled range is refused with 400 and reports nothing; the
/// published bounds themselves are accepted.
#[tokio::test]
async fn a_page_size_outside_the_modelled_range_is_refused() {
    let router = router();
    for index in 0..3 {
        create_strategy(&router, &format!("bounded, row {index}"), &varied_request(index)).await;
    }

    // `MaxResults` is `@range(min: 1, max: 50)`: each bound, and one past each bound.
    for (case, max_results, expected_status) in [
        ("the lower bound, 1", 1, 200),
        ("the upper bound, 50", 50, 200),
        ("one below the lower bound, 0", 0, 400),
        ("one past the upper bound, 51", 51, 400),
    ] {
        let answer = list(&router, &format!("max_results={max_results}")).await;
        assert_eq!(
            answer.status,
            expected_status,
            "case {case:?}: expected {expected_status}, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        if expected_status == 400 {
            assert!(
                answer.error_code().contains("ValidationException"),
                "case {case:?}: expected ValidationException, got {:?}",
                answer.error_code()
            );
            assert!(
                items(&answer).is_empty(),
                "case {case:?}: a refused request reports nothing, got {:?}",
                items(&answer)
            );
        }
    }
}

/// A `NextToken` this listing did not mint is refused with the modelled
/// `BadRequestException`, rather than silently restarting the traversal.
#[tokio::test]
async fn a_next_token_this_listing_did_not_mint_is_refused() {
    let router = router();
    for index in 0..3 {
        create_strategy(&router, &format!("tokens, row {index}"), &varied_request(index)).await;
    }

    // A token minted by this very listing, as the control: the rule refuses tokens this
    // listing did not issue, not every token.
    let first = list(&router, "max_results=1").await;
    let mine = first
        .string("NextToken")
        .expect("a truncated listing carries a NextToken");
    // A token of the same encoding minted by a DIFFERENT paginated listing of this same
    // service: a cursor is stamped with the listing it belongs to, so this one resumes
    // nothing here.
    let application = create_application(&router, "another-listing").await;
    for index in 0..2 {
        create_environment(&router, &application, &format!("env-{index}")).await;
    }
    let other_listing = json_call(
        &router,
        "GET",
        &format!("/applications/{application}/environments?max_results=1"),
        &Value::Null,
    )
    .await
    .string("NextToken")
    .expect("a truncated ListEnvironments carries a NextToken");

    let resumed = list(&router, &format!("max_results=1&next_token={mine}")).await;
    assert_eq!(
        resumed.status,
        200,
        "case \"the token this listing minted\": expected 200, got {} / {}",
        resumed.status,
        String::from_utf8_lossy(&resumed.body)
    );

    // Rows differ along HOW the token fails to be one of this listing's: another
    // listing's cursor, not this service's encoding at all, a mangled copy of a real one,
    // and values a client might have invented.
    for (case, token) in [
        ("a cursor minted by another listing", other_listing),
        ("not this service's encoding", "not-a-token".to_string()),
        (
            "a mangled copy of a real token",
            mine[..mine.len() - 1].to_string(),
        ),
        ("a strategy id passed as a token", "abcdefg".to_string()),
        (
            "a token of the right encoding but no cursor fields",
            "6162".to_string(),
        ),
    ] {
        let answer = list(&router, &format!("next_token={token}")).await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected 400, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            answer.error_code().contains("BadRequestException"),
            "case {case:?}: expected BadRequestException, got {:?}",
            answer.error_code()
        );
        assert!(
            items(&answer).is_empty(),
            "case {case:?}: a refused request reports nothing, got {:?}",
            items(&answer)
        );
    }
}

/// Listing is a report: walking it changes nothing a later caller sees. The catalogue and
/// every record in it are identical before and after a full paginated walk.
#[tokio::test]
async fn walking_the_listing_changes_nothing_a_later_caller_sees() {
    let router = router();
    for index in 0..4 {
        create_strategy(
            &router,
            &format!("reporting only, row {index}"),
            &varied_request(index),
        )
        .await;
    }

    let before = items(&list(&router, "").await);
    let before_records: Vec<Value> = {
        let mut records = Vec::new();
        for id in listed_ids(&list(&router, "").await) {
            records.push(get_strategy(&router, &id).await.json());
        }
        records
    };

    for page_size in [Some(1), Some(2), Some(50), None] {
        let case = format!("a walk with MaxResults={page_size:?}");
        walk(&router, &case, page_size).await;
        let after = items(&list(&router, "").await);
        assert_eq!(
            after, before,
            "case {case:?}: a read must not change what a later caller sees, expected \
             {before:?}, got {after:?}"
        );
        let mut after_records = Vec::new();
        for id in listed_ids(&list(&router, "").await) {
            after_records.push(get_strategy(&router, &id).await.json());
        }
        assert_eq!(
            after_records, before_records,
            "case {case:?}: a read must not change any strategy's record, expected \
             {before_records:?}, got {after_records:?}"
        );
    }
}
