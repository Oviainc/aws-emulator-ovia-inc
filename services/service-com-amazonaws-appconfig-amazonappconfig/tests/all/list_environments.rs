// SPDX-License-Identifier: CC0-1.0

//! `ListEnvironments` as a paginated report over one application's environments.
//!
//! The rule under test: the listing reports exactly the environments stored under the
//! application the path names — every one of them, each exactly once, each as the record
//! a Get of it answers — in an order that is stable across calls, cut into pages of at
//! most `MaxResults` entries, with a `NextToken` handed back only when entries are left
//! over and a token that resumes the traversal where the previous page ended.
//!
//! The tables below span what that rule generalises over: HOW MANY environments the
//! application holds (none, one, several, and one past the default page size), WHICH page
//! size the request asks for (absent, both published bounds of `MaxResults`, and values
//! in between), and WHAT each environment carries (description present or absent,
//! monitors present or absent, a deployment in flight or not). Every assertion names its
//! case and prints the actual value.

use crate::appconfig_workflow::{
    create_application, create_environment, create_profile, json_call, router, send, Answer,
};
use service_com_amazonaws_appconfig_amazonappconfig::state::{
    MAX_LIST_PAGE_SIZE, PREDEFINED_DEPLOYMENT_STRATEGIES,
};
use smithy_server_runtime::MultiServiceRouter;

/// `ListEnvironments` for `application`, with `query` as the raw query string (empty for
/// a request that names neither `MaxResults` nor `NextToken`). The page size and the
/// token are `httpQuery` members named `max_results` and `next_token`.
async fn list(router: &MultiServiceRouter, application: &str, query: &str) -> Answer {
    let uri = if query.is_empty() {
        format!("/applications/{application}/environments")
    } else {
        format!("/applications/{application}/environments?{query}")
    };
    json_call(router, "GET", &uri, &serde_json::Value::Null).await
}

/// The `Items` of a listing, as the JSON array the response carries (an answer with no
/// `Items` at all reads as empty).
fn items(answer: &Answer) -> Vec<serde_json::Value> {
    answer
        .json()
        .get("Items")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `Id` of every entry of a listing, in the order it was served.
fn listed_ids(answer: &Answer) -> Vec<String> {
    items(answer)
        .iter()
        .map(|item| {
            item.get("Id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_else(|| panic!("a listed environment must carry an Id, got {item}"))
                .to_string()
        })
        .collect()
}

/// Walk the whole listing one page at a time and return the ids in the order they were
/// served, asserting the per-page half of the rule as it goes: a page never exceeds the
/// size the request asked for, and a page that carries a `NextToken` is exactly that
/// size, because a traversal is only cut where a page filled up.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    application: &str,
    max_results: Option<i32>,
) -> Vec<String> {
    let page_size = max_results.map_or(MAX_LIST_PAGE_SIZE, |size| size as usize);
    let mut collected: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // Bounded so a listing that never ends its traversal fails instead of hanging: one
    // page per entry the emulator will hold, plus the final empty-tailed page.
    for _ in 0..=(MAX_LIST_PAGE_SIZE * 4 + 2) {
        let mut query: Vec<String> = Vec::new();
        if let Some(size) = max_results {
            query.push(format!("max_results={size}"));
        }
        if let Some(token) = &token {
            query.push(format!("next_token={token}"));
        }
        let answer = list(router, application, &query.join("&")).await;
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

/// The listing reports exactly the environments of the application the path names: every
/// one created under it, none created under another, in an order that does not move
/// between calls.
#[tokio::test]
async fn a_listing_reports_exactly_the_named_applications_environments() {
    let router = router();
    let application = create_application(&router, "listed").await;
    let sibling = create_application(&router, "not-listed").await;

    // Rows differ along the number of environments the application holds, so the listing
    // is asserted while the collection grows from empty.
    let mut created: Vec<String> = Vec::new();
    for (case, name) in [
        ("empty", None),
        ("one environment", Some("alpha")),
        ("two environments", Some("beta")),
        ("three environments", Some("gamma")),
    ] {
        if let Some(name) = name {
            created.push(create_environment(&router, &application, name).await);
            // A sibling application grows at the same time: its environments must never
            // appear in this application's listing.
            create_environment(&router, &sibling, &format!("{name}-elsewhere")).await;
        }

        let answer = list(&router, &application, "").await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected 200, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let mut reported = listed_ids(&answer);
        let mut expected = created.clone();
        reported.sort();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected exactly the environments created under this \
             application {expected:?}, got {reported:?}"
        );

        // Stable order: a second identical call serves the same sequence.
        let again = list(&router, &application, "").await;
        assert_eq!(
            listed_ids(&again),
            listed_ids(&answer),
            "case {case:?}: the order must not move between calls, got {:?} then {:?}",
            listed_ids(&answer),
            listed_ids(&again)
        );
    }

    // The sibling still reports only its own, which is the same rule read from the other
    // side.
    let sibling_listing = list(&router, &sibling, "").await;
    let sibling_ids = listed_ids(&sibling_listing);
    assert_eq!(
        sibling_ids.len(),
        3,
        "case \"sibling application\": expected its own three environments, got {sibling_ids:?}"
    );
    for id in &created {
        assert!(
            !sibling_ids.contains(id),
            "case \"sibling application\": {id:?} belongs to another application, got \
             {sibling_ids:?}"
        );
    }
}

/// Whatever page size a request asks for, the pages of a traversal concatenate to the
/// single-call listing: every environment exactly once, in the same order.
#[tokio::test]
async fn pages_of_every_size_concatenate_to_the_whole_collection() {
    let router = router();

    // Rows differ along the collection size and the requested page size: `MaxResults` is
    // modelled `@range(min: 1, max: 50)`, so 1 and 50 are its published bounds, and
    // `None` is a request that names no page size at all.
    let sizes = [0_usize, 1, 2, 5, 7];
    let page_sizes = [None, Some(1), Some(2), Some(3), Some(50)];

    for size in sizes {
        let application = create_application(&router, &format!("holding-{size}")).await;
        for index in 0..size {
            create_environment(&router, &application, &format!("env-{index}")).await;
        }
        let whole = listed_ids(&list(&router, &application, "").await);
        assert_eq!(
            whole.len(),
            size,
            "case \"{size} environments, one call\": expected {size} entries, got {whole:?}"
        );

        for page_size in page_sizes {
            let case = format!("{size} environments, MaxResults={page_size:?}");
            let walked = walk(&router, &case, &application, page_size).await;
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
                "case {case:?}: no environment may be served twice, got {walked:?}"
            );
        }
    }
}

/// A request naming no page size is cut at the service's own page size, not at the
/// collection: one entry past it is served on a second page.
#[tokio::test]
async fn a_request_naming_no_page_size_is_cut_at_the_services_page_size() {
    let router = router();
    let application = create_application(&router, "one-past-the-page").await;
    for index in 0..=MAX_LIST_PAGE_SIZE {
        create_environment(&router, &application, &format!("env-{index}")).await;
    }

    let first = list(&router, &application, "").await;
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

    let second = list(&router, &application, &format!("next_token={token}")).await;
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
    let application = create_application(&router, "bounded").await;
    for index in 0..3 {
        create_environment(&router, &application, &format!("env-{index}")).await;
    }

    // `MaxResults` is `@range(min: 1, max: 50)`: each bound, and one past each bound.
    for (case, max_results, expected_status) in [
        ("the lower bound, 1", 1, 200),
        ("the upper bound, 50", 50, 200),
        ("one below the lower bound, 0", 0, 400),
        ("one past the upper bound, 51", 51, 400),
    ] {
        let answer = list(&router, &application, &format!("max_results={max_results}")).await;
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

/// An application nobody created holds no environments to report: the listing is 404
/// `ResourceNotFoundException`, the same refusal every other read of that path gives.
#[tokio::test]
async fn an_unknown_application_is_not_found() {
    let router = router();
    let application = create_application(&router, "known").await;
    let environment = create_environment(&router, &application, "known-env").await;

    for (case, path_id) in [
        ("an id nobody minted", "nosuchapp".to_string()),
        ("an environment id in the application slot", environment),
        ("an id of the modelled shape that is free", "zzzzzzz".to_string()),
    ] {
        let answer = list(&router, &path_id, "").await;
        assert_eq!(
            answer.status,
            404,
            "case {case:?}: expected 404, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert!(
            answer.error_code().contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            answer.error_code()
        );
    }
}

/// A `NextToken` this listing did not mint is refused with the modelled
/// `BadRequestException`, rather than silently restarting the traversal.
#[tokio::test]
async fn a_next_token_this_listing_did_not_mint_is_refused() {
    let router = router();
    let application = create_application(&router, "tokens").await;
    for index in 0..3 {
        create_environment(&router, &application, &format!("env-{index}")).await;
        create_profile(&router, &application, &format!("profile-{index}")).await;
    }

    // A token minted by this very listing, as the control: the rule refuses tokens this
    // listing did not issue, not every token.
    let first = list(&router, &application, "max_results=1").await;
    let mine = first
        .string("NextToken")
        .expect("a truncated listing carries a NextToken");
    // A token of the same encoding minted by a DIFFERENT paginated listing of this same
    // service: a cursor is stamped with the listing it belongs to, so this one resumes
    // nothing here.
    let other_listing = json_call(
        &router,
        "GET",
        &format!("/applications/{application}/configurationprofiles?max_results=1"),
        &serde_json::Value::Null,
    )
    .await
    .string("NextToken")
    .expect("a truncated ListConfigurationProfiles carries a NextToken");

    let resumed = list(
        &router,
        &application,
        &format!("max_results=1&next_token={mine}"),
    )
    .await;
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
        ("an environment id passed as a token", "abcdefg".to_string()),
        (
            "a token of the right encoding but no cursor fields",
            "6162".to_string(),
        ),
    ] {
        let answer = list(&router, &application, &format!("next_token={token}")).await;
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

/// Every entry of the listing is the stored record, member for member what a
/// `GetEnvironment` of the same environment answers — including the `State` the clock
/// owes while a deployment is in flight.
#[tokio::test]
async fn each_listed_environment_is_the_record_a_get_of_it_answers() {
    let router = router();
    let application = create_application(&router, "round-trip").await;

    // One CloudWatch alarm monitor. The member names and the `AlarmRoleArn` pattern are
    // the model's own: `smithy_query(query="shape",shape="Monitor")` gives `AlarmArn`
    // (required) and `AlarmRoleArn` (pattern `^((arn):(aws|…):(iam)::\d{12}:role[/].*)$`).
    let monitor = serde_json::json!({
        "AlarmArn": "arn:aws:cloudwatch:us-east-1:123456789012:alarm:appconfig-listed",
        "AlarmRoleArn": "arn:aws:iam::123456789012:role/appconfig-monitor",
    });

    // Rows differ along what the created environment carries: the optional members
    // present or absent, at the published length bounds of `Description` (0..1024) and
    // `Name` (1..64), and with and without monitors.
    let cases: Vec<(&str, serde_json::Value)> = vec![
        ("name only", serde_json::json!({ "Name": "plain" })),
        (
            "name and description",
            serde_json::json!({ "Name": "described", "Description": "a described environment" }),
        ),
        (
            "the empty description at the lower length bound",
            serde_json::json!({ "Name": "empty-description", "Description": "" }),
        ),
        (
            "a monitored environment",
            serde_json::json!({ "Name": "monitored", "Monitors": [monitor] }),
        ),
        (
            "both members at their upper length bounds",
            serde_json::json!({ "Name": "n".repeat(64), "Description": "d".repeat(1024) }),
        ),
    ];

    let mut created: Vec<String> = Vec::new();
    for (case, body) in &cases {
        let answer = json_call(
            &router,
            "POST",
            &format!("/applications/{application}/environments"),
            body,
        )
        .await;
        assert_eq!(
            answer.status,
            201,
            "case {case:?}: CreateEnvironment must answer 201, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        created.push(
            answer
                .string("Id")
                .unwrap_or_else(|| panic!("case {case:?}: CreateEnvironment answered no Id")),
        );
    }

    // A deployment in flight makes its environment `DEPLOYING`, and the listing must
    // report that state as the Get does. The strategy is the longest-running row of the
    // predefined catalog, so the rollout is still in progress when the listing runs.
    let profile = create_profile(&router, &application, "deployed-config").await;
    let hosted = send(
        &router,
        "POST",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        "application/json",
        br#"{"enabled":true}"#.to_vec(),
        &[],
    )
    .await;
    assert_eq!(
        hosted.status,
        201,
        "the configuration to deploy must be hosted first, got {} / {}",
        hosted.status,
        String::from_utf8_lossy(&hosted.body)
    );
    let slowest = PREDEFINED_DEPLOYMENT_STRATEGIES
        .iter()
        .max_by_key(|strategy| {
            strategy.deployment_duration_in_minutes + strategy.final_bake_time_in_minutes
        })
        .expect("the predefined catalog is not empty");
    let deployed = created[0].clone();
    let started = json_call(
        &router,
        "POST",
        &format!("/applications/{application}/environments/{deployed}/deployments"),
        &serde_json::json!({
            "DeploymentStrategyId": slowest.id,
            "ConfigurationProfileId": profile,
            "ConfigurationVersion": "1",
        }),
    )
    .await;
    assert_eq!(
        started.status,
        201,
        "the deployment must start, got {} / {}",
        started.status,
        String::from_utf8_lossy(&started.body)
    );

    let listing = list(&router, &application, "").await;
    let listed = items(&listing);
    assert_eq!(
        listed.len(),
        created.len(),
        "every created environment must be listed, expected {} entries, got {listed:?}",
        created.len()
    );

    for id in &created {
        let entry = listed
            .iter()
            .find(|item| item.get("Id").and_then(serde_json::Value::as_str) == Some(id.as_str()))
            .unwrap_or_else(|| panic!("case {id:?}: the listing must carry this environment"));
        let fetched = json_call(
            &router,
            "GET",
            &format!("/applications/{application}/environments/{id}"),
            &serde_json::Value::Null,
        )
        .await;
        assert_eq!(
            fetched.status,
            200,
            "case {id:?}: GetEnvironment must answer 200, got {} / {}",
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );
        assert_eq!(
            *entry,
            fetched.json(),
            "case {id:?}: the listed entry must be the record GetEnvironment answers {}, got \
             {entry}",
            fetched.json()
        );
        let expected_state = if id == &deployed {
            "DEPLOYING"
        } else {
            "READY_FOR_DEPLOYMENT"
        };
        assert_eq!(
            entry.get("State").and_then(serde_json::Value::as_str),
            Some(expected_state),
            "case {id:?}: expected State {expected_state:?}, got {:?}",
            entry.get("State")
        );
        assert_eq!(
            entry
                .get("ApplicationId")
                .and_then(serde_json::Value::as_str),
            Some(application.as_str()),
            "case {id:?}: every listed environment belongs to the application in the path, \
             got {:?}",
            entry.get("ApplicationId")
        );
    }
}

/// Listing is a report: walking it changes nothing a later caller sees. The collection
/// and every record in it are identical before and after a full paginated walk.
#[tokio::test]
async fn walking_the_listing_changes_nothing_a_later_caller_sees() {
    let router = router();
    let application = create_application(&router, "reporting-only").await;
    for index in 0..4 {
        create_environment(&router, &application, &format!("env-{index}")).await;
    }

    let before = items(&list(&router, &application, "").await);
    for page_size in [Some(1), Some(2), Some(50), None] {
        let case = format!("a walk with MaxResults={page_size:?}");
        walk(&router, &case, &application, page_size).await;
        let after = items(&list(&router, &application, "").await);
        assert_eq!(
            after, before,
            "case {case:?}: a read must not change what a later caller sees, expected \
             {before:?}, got {after:?}"
        );
    }
}
