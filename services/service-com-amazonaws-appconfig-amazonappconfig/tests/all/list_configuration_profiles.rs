// SPDX-License-Identifier: CC0-1.0

//! `ListConfigurationProfiles` as a REPORT over stored state: it answers the
//! configuration profiles of the application it addresses, each summary carrying the
//! values that profile was created with, cut into pages a `NextToken` walks exactly
//! once.
//!
//! Every suite drives real HTTP requests through the router, so the assertions are
//! about the wire behaviour. The rules under test, each asserted over a SET of inputs
//! rather than one recorded call:
//!
//! 1. a summary reports the stored profile — `ApplicationId`, `Id`, `Name`,
//!    `LocationUri`, `Type` as `GetConfigurationProfile` reports them, and
//!    `ValidatorTypes` as the types of the validators the profile holds;
//! 2. a listing reports exactly the profiles of the application it addresses;
//! 3. `MaxResults` bounds a page and `NextToken` resumes after it, so a walk reports
//!    every profile exactly once at any page size;
//! 4. `Type` filters the listing to the profiles whose stored `Type` is that value;
//! 5. an application that does not exist is 404 `ResourceNotFoundException`, and a
//!    `NextToken` this operation did not issue is 400 `BadRequestException`;
//! 6. listing is a read: it changes nothing a later caller sees.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::appconfig_workflow::{
    create_application, create_profile, create_profile_from, json_call, router, Answer,
};

/// The `Type` values the model's own `Type` documentation lists ("When calling this API,
/// enter one of the following values for `Type`: `AWS.AppConfig.FeatureFlags`,
/// `AWS.Freeform`" —
/// `smithy_query(query="doc",shape="ConfigurationProfileSummary",member="Type")`).
const PROFILE_TYPES: [&str; 2] = ["AWS.AppConfig.FeatureFlags", "AWS.Freeform"];

/// The published bounds of `MaxResults` on this operation (`range 1..50`,
/// `smithy_query(query="operation",operation="ListConfigurationProfiles")`).
const MAX_RESULTS_MIN: i64 = 1;
/// See [`MAX_RESULTS_MIN`].
const MAX_RESULTS_MAX: i64 = 50;

/// `GET` the listing of `application`, with the query members in `query`.
async fn list(
    router: &MultiServiceRouter,
    application: &str,
    query: &[(&str, String)],
) -> Answer {
    let mut uri = format!("/applications/{application}/configurationprofiles");
    if !query.is_empty() {
        let rendered: Vec<String> = query
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        uri.push('?');
        uri.push_str(&rendered.join("&"));
    }
    json_call(router, "GET", &uri, &Value::Null).await
}

/// The `Items` of a successful listing.
fn items(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: ListConfigurationProfiles expected 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .json()
        .get("Items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no Items array in {}", answer.json()))
}

/// The `Id` of every summary in `page`.
fn ids(page: &[Value], case: &str) -> Vec<String> {
    page.iter()
        .map(|summary| {
            summary
                .get("Id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| panic!("case {case:?}: a summary carries no Id: {summary}"))
        })
        .collect()
}

/// The created record `CreateConfigurationProfile` reported for `body`, as JSON.
async fn created_profile(
    router: &MultiServiceRouter,
    application: &str,
    case: &str,
    body: &Value,
) -> Value {
    create_profile_from(router, application, case, body)
        .await
        .json()
}

/// One validator of a given `ValidatorType` (`JSON_SCHEMA` | `LAMBDA`, the values the
/// model's `Validator` shape lists).
fn validator(kind: &str, content: &str) -> Value {
    json!({ "Type": kind, "Content": content })
}

/// RULE 1: each summary reports the stored profile, and `ValidatorTypes` reports the
/// types of the validators that profile holds.
#[tokio::test]
async fn a_summary_reports_the_stored_profile_and_its_validator_types() {
    let router = router();
    let application = create_application(&router, "summaries").await;

    // Rows differ along the location kind, the `Type` (both documented values and none
    // at all), the number and types of validators (none, one of each type, two of
    // different types, two of the same type), and the length of `Name` at the published
    // bounds of `LongName` (1..128).
    let long_name = "n".repeat(128);
    let cases: Vec<(&str, Value, Option<Vec<&str>>)> = vec![
        (
            "hosted freeform, no validators",
            json!({ "Name": "hosted-freeform", "LocationUri": "hosted", "Type": "AWS.Freeform" }),
            None,
        ),
        (
            "hosted feature flags, no validators",
            json!({
                "Name": "hosted-flags",
                "LocationUri": "hosted",
                "Type": "AWS.AppConfig.FeatureFlags",
            }),
            None,
        ),
        (
            "no type at all",
            json!({ "Name": "untyped", "LocationUri": "ssm-parameter://my-parameter" }),
            None,
        ),
        (
            "s3 object, one JSON_SCHEMA validator",
            json!({
                "Name": "s3-schema",
                "LocationUri": "s3://amzn-s3-demo-bucket/my-app/config.json",
                "Type": "AWS.Freeform",
                "Validators": [validator("JSON_SCHEMA", "{\"type\":\"object\"}")],
            }),
            Some(vec!["JSON_SCHEMA"]),
        ),
        (
            "secrets manager secret, one LAMBDA validator",
            json!({
                "Name": "secret-lambda",
                "LocationUri": "secretsmanager://my-secret",
                "Type": "AWS.Freeform",
                "Validators": [validator(
                    "LAMBDA",
                    "arn:aws:lambda:us-east-1:000000000000:function:validate",
                )],
            }),
            Some(vec!["LAMBDA"]),
        ),
        (
            "two validators of different types, the modelled maximum",
            json!({
                "Name": "both-validators",
                "LocationUri": "hosted",
                "Type": "AWS.AppConfig.FeatureFlags",
                "Validators": [
                    validator("JSON_SCHEMA", "{\"type\":\"object\"}"),
                    validator("LAMBDA", "arn:aws:lambda:us-east-1:000000000000:function:validate"),
                ],
            }),
            Some(vec!["JSON_SCHEMA", "LAMBDA"]),
        ),
        (
            "two validators of the same type report that type once",
            json!({
                "Name": "same-type-twice",
                "LocationUri": "hosted",
                "Type": "AWS.Freeform",
                "Validators": [
                    validator("LAMBDA", "arn:aws:lambda:us-east-1:000000000000:function:one"),
                    validator("LAMBDA", "arn:aws:lambda:us-east-1:000000000000:function:two"),
                ],
            }),
            Some(vec!["LAMBDA"]),
        ),
        (
            "name at the 1-character bound",
            json!({ "Name": "x", "LocationUri": "hosted", "Type": "AWS.Freeform" }),
            None,
        ),
        (
            "name at the 128-character bound",
            json!({ "Name": long_name, "LocationUri": "hosted", "Type": "AWS.Freeform" }),
            None,
        ),
    ];

    let mut created_by_id = Vec::new();
    for (case, body, _) in &cases {
        let created = created_profile(&router, &application, case, body).await;
        let id = created
            .get("Id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: the created profile carries no Id"))
            .to_string();
        created_by_id.push((id, created));
    }

    let listed = list(&router, &application, &[]).await;
    let page = items(&listed, "every profile");
    assert_eq!(
        page.len(),
        cases.len(),
        "case \"every profile\": expected {} summaries, got {}: {}",
        cases.len(),
        page.len(),
        listed.json()
    );

    for ((case, _, validator_types), (id, created)) in cases.iter().zip(created_by_id.iter()) {
        let summary = page
            .iter()
            .find(|summary| summary.get("Id").and_then(Value::as_str) == Some(id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: no summary for {id}: {}", listed.json()));

        let mut expected = json!({
            "ApplicationId": application,
            "Id": id,
            "Name": created.get("Name").cloned().expect("a created profile has a Name"),
            "LocationUri": created
                .get("LocationUri")
                .cloned()
                .expect("a created profile has a LocationUri"),
        });
        if let Some(profile_type) = created.get("Type") {
            expected["Type"] = profile_type.clone();
        }
        if let Some(validator_types) = validator_types {
            expected["ValidatorTypes"] = Value::from(validator_types.clone());
        }
        assert_eq!(
            summary,
            &expected,
            "case {case:?}: expected {expected}, got {summary}"
        );
    }
}

/// RULE 2: a listing reports exactly the profiles of the application it addresses, and
/// RULE 6: it is a read — repeating it answers the same thing.
#[tokio::test]
async fn a_listing_reports_only_the_profiles_of_the_application_it_addresses() {
    let router = router();

    // Rows differ along how many profiles the application holds: none, one, and several
    // — with a sibling application holding a different count, so a listing that read the
    // whole store rather than one application's subtree fails here.
    let counts: [usize; 4] = [0, 1, 3, 7];
    let mut applications = Vec::new();
    for (index, count) in counts.iter().enumerate() {
        let application = create_application(&router, &format!("app-{index}")).await;
        let mut expected = BTreeSet::new();
        for profile in 0..*count {
            expected.insert(create_profile(&router, &application, &format!("p-{index}-{profile}")).await);
        }
        applications.push((application, expected));
    }

    for (application, expected) in &applications {
        let case = format!("{} profiles", expected.len());
        let answer = list(&router, application, &[]).await;
        let page = items(&answer, &case);
        let reported: BTreeSet<String> = ids(&page, &case).into_iter().collect();
        assert_eq!(
            &reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
        assert!(
            answer.json().get("NextToken").is_none(),
            "case {case:?}: a listing that fits one page carries no NextToken, got {}",
            answer.json()
        );

        // A read changes nothing: the same call answers the same thing.
        let again = list(&router, application, &[]).await;
        assert_eq!(
            again.json(),
            answer.json(),
            "case {case:?}: listing twice must answer the same, got {}",
            again.json()
        );
    }
}

/// RULE 3: `MaxResults` bounds a page, `NextToken` resumes after it, and the walk
/// reports every profile exactly once — at every page size, including the published
/// bounds of `MaxResults`.
#[tokio::test]
async fn a_next_token_walk_reports_every_profile_exactly_once_at_every_page_size() {
    let router = router();

    // Rows span the profile count and, for each, every page size from the published
    // minimum up past the count, plus the published maximum and no `MaxResults` at all.
    for count in [0_usize, 1, 2, 5, 9] {
        let application = create_application(&router, &format!("walk-{count}")).await;
        let mut expected = BTreeSet::new();
        for profile in 0..count {
            expected.insert(create_profile(&router, &application, &format!("w-{profile}")).await);
        }

        let mut page_sizes: Vec<Option<i64>> = (MAX_RESULTS_MIN..=(count as i64 + 1))
            .map(Some)
            .collect();
        page_sizes.push(Some(MAX_RESULTS_MAX));
        page_sizes.push(None);

        for page_size in page_sizes {
            let case = format!("{count} profiles, MaxResults {page_size:?}");
            let mut seen: Vec<String> = Vec::new();
            let mut next_token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut query: Vec<(&str, String)> = Vec::new();
                if let Some(page_size) = page_size {
                    query.push(("max_results", page_size.to_string()));
                }
                if let Some(token) = &next_token {
                    query.push(("next_token", token.clone()));
                }
                let answer = list(&router, &application, &query).await;
                let page = items(&answer, &case);
                if let Some(page_size) = page_size {
                    assert!(
                        page.len() as i64 <= page_size,
                        "case {case:?}: a page must carry at most {page_size} items, got {}",
                        page.len()
                    );
                }
                seen.extend(ids(&page, &case));
                pages += 1;
                assert!(
                    pages <= count + 2,
                    "case {case:?}: pagination did not terminate after {pages} pages"
                );
                next_token = answer.string("NextToken");
                if next_token.is_none() {
                    break;
                }
                assert!(
                    !page.is_empty(),
                    "case {case:?}: a page that resumes must carry items, got {}",
                    answer.json()
                );
            }

            let walked: BTreeSet<String> = seen.iter().cloned().collect();
            assert_eq!(
                walked.len(),
                seen.len(),
                "case {case:?}: the walk reported a profile twice: {seen:?}"
            );
            assert_eq!(
                walked, expected,
                "case {case:?}: expected {expected:?}, got {walked:?}"
            );
        }
    }
}

/// RULE 3, the other side of the published range: `MaxResults` outside `1..50` is
/// refused by the modelled constraint rather than answered.
#[tokio::test]
async fn a_max_results_past_the_published_bounds_is_refused() {
    let router = router();
    let application = create_application(&router, "bad-max-results").await;
    create_profile(&router, &application, "only").await;

    // One past each published bound, and a value that is not a number at all.
    let cases: [(&str, &str); 3] = [
        ("one below the minimum", "0"),
        ("one above the maximum", "51"),
        ("not a number", "many"),
    ];
    for (case, max_results) in cases {
        let answer = list(
            &router,
            &application,
            &[("max_results", max_results.to_string())],
        )
        .await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected 400 for MaxResults={max_results}, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }
}

/// RULE 4: `Type` filters the listing to the profiles whose stored `Type` is that value.
#[tokio::test]
async fn the_type_filter_selects_the_profiles_of_that_type() {
    let router = router();
    let application = create_application(&router, "type-filter").await;

    // Two profiles of each documented `Type`, and one carrying no `Type` at all — which
    // no filter value selects, because the profile stores none.
    let mut by_type: Vec<(&str, BTreeSet<String>)> = Vec::new();
    for profile_type in PROFILE_TYPES {
        let mut ids = BTreeSet::new();
        for index in 0..2 {
            let created = created_profile(
                &router,
                &application,
                profile_type,
                &json!({
                    "Name": format!("{profile_type}-{index}"),
                    "LocationUri": "hosted",
                    "Type": profile_type,
                }),
            )
            .await;
            ids.insert(
                created
                    .get("Id")
                    .and_then(Value::as_str)
                    .expect("a created profile carries an Id")
                    .to_string(),
            );
        }
        by_type.push((profile_type, ids));
    }
    let untyped = created_profile(
        &router,
        &application,
        "untyped",
        &json!({ "Name": "untyped", "LocationUri": "hosted" }),
    )
    .await;
    let untyped_id = untyped
        .get("Id")
        .and_then(Value::as_str)
        .expect("a created profile carries an Id")
        .to_string();

    for (profile_type, expected) in &by_type {
        let case = format!("Type={profile_type}");
        let answer = list(&router, &application, &[("type", (*profile_type).to_string())]).await;
        let page = items(&answer, &case);
        let reported: BTreeSet<String> = ids(&page, &case).into_iter().collect();
        assert_eq!(
            &reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
        assert!(
            !reported.contains(&untyped_id),
            "case {case:?}: a profile with no Type matches no filter value, got {reported:?}"
        );
        for summary in &page {
            assert_eq!(
                summary.get("Type").and_then(Value::as_str),
                Some(*profile_type),
                "case {case:?}: every summary must carry the filtered Type, got {summary}"
            );
        }
    }

    // A `Type` the model's pattern accepts but no profile stores selects nothing, and a
    // filter the request omits selects everything.
    let case = "Type=AWS.Unused";
    let answer = list(&router, &application, &[("type", "AWS.Unused".to_string())]).await;
    let page = items(&answer, case);
    assert!(
        page.is_empty(),
        "case {case:?}: expected no summaries, got {}",
        answer.json()
    );

    let case = "no filter";
    let answer = list(&router, &application, &[]).await;
    let page = items(&answer, case);
    let expected: usize = by_type.iter().map(|(_, ids)| ids.len()).sum::<usize>() + 1;
    assert_eq!(
        page.len(),
        expected,
        "case {case:?}: expected {expected} summaries, got {}",
        page.len()
    );
}

/// RULE 5a: a listing is addressed under its application, so an application that does
/// not exist is 404 `ResourceNotFoundException` — never an empty listing.
#[tokio::test]
async fn listing_the_profiles_of_an_application_that_does_not_exist_is_not_found() {
    let router = router();
    let application = create_application(&router, "not-found").await;
    let profile = create_profile(&router, &application, "existing").await;

    // Rows differ along the shape of the unresolvable id, including a near miss of a
    // real application and the id of a resource that is not an application.
    let near_miss = format!("{application}x");
    let cases: Vec<(&str, String)> = vec![
        ("an id no resource holds", "nosuchapp".to_string()),
        ("a near miss of a real application", near_miss),
        ("the id of a profile, not an application", profile.clone()),
        ("a single character", "a".to_string()),
    ];
    for (case, application_id) in &cases {
        let answer = list(&router, application_id, &[]).await;
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

    // The application that does exist still reports its profile: a refused listing
    // changed nothing.
    let case = "the application that exists";
    let answer = list(&router, &application, &[]).await;
    let page = items(&answer, case);
    assert_eq!(
        ids(&page, case),
        vec![profile],
        "case {case:?}: expected the one created profile, got {}",
        answer.json()
    );
}

/// RULE 5b: a `NextToken` this operation did not issue is 400 `BadRequestException`,
/// and one it did issue resumes the walk it was minted for.
#[tokio::test]
async fn a_next_token_this_operation_did_not_issue_is_a_bad_request() {
    let router = router();
    let application = create_application(&router, "bad-tokens").await;
    for index in 0..3 {
        create_profile(&router, &application, &format!("t-{index}")).await;
    }

    // A token this operation DID issue is accepted, so the rows below are refused for
    // being unminted rather than for the operation refusing every token.
    let first = list(&router, &application, &[("max_results", "1".to_string())]).await;
    let issued = first
        .string("NextToken")
        .expect("three profiles at MaxResults=1 must leave a NextToken");
    let resumed = list(&router, &application, &[("next_token", issued.clone())]).await;
    let page = items(&resumed, "an issued token");
    assert_eq!(
        page.len(),
        2,
        "case \"an issued token\": expected the 2 remaining profiles, got {}",
        resumed.json()
    );

    // Rows differ along HOW the token fails to be one this operation minted: not the
    // codec's alphabet at all, the right alphabet at the wrong length, the codec but
    // not the payload shape, and the payload shape under another listing's name.
    let cases: Vec<(&str, String)> = vec![
        ("not hex at all", "not-a-token".to_string()),
        ("hex of an odd length", "abc".to_string()),
        ("hex with no listing kind in it", hex::encode("abcd")),
        (
            "hex naming another listing",
            hex::encode("ListEnvironments\u{1f}abcd"),
        ),
        (
            "hex naming this listing with no key at all",
            hex::encode("ListConfigurationProfiles"),
        ),
    ];
    for (case, token) in &cases {
        let answer = list(&router, &application, &[("next_token", token.clone())]).await;
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
    }
}
