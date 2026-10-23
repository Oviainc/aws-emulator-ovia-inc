// SPDX-License-Identifier: CC0-1.0

//! `UpdateApplication` as a PATCH over the stored application.
//!
//! The rule under test: a member the request carries replaces the stored one, a member it
//! omits leaves the stored one exactly as it was, and the answer — plus every later read —
//! comes out of the updated record. `Id` is not a writable member, and neither are the
//! application's tags nor the environments and configuration profiles it owns, none of
//! which `UpdateApplicationRequest` carries.
//!
//! The tables span what the rule generalises over: WHICH subset of the writable members a
//! request carries, and the VALUE each one carries — both published length bounds
//! (`Name` len 1..64, `Description` len 0..1024, from
//! `smithy_query(query="operation",operation="UpdateApplication")`), one character past
//! each, and values that are empty, non-ASCII or URI-significant. Every assertion names
//! its case and prints the actual value.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{
    create_application, create_environment, create_profile, is_modelled_id, json_call, router,
    Answer,
};
use crate::resource_tags::{arn, filler, list_tags, tag};

/// The application every row starts from: both writable members are already set, so a row
/// that omits one is distinguishable from a row that clears it.
const BASELINE_NAME: &str = "the application as created";
/// See [`BASELINE_NAME`].
const BASELINE_DESCRIPTION: &str = "the description as created";

/// What a case expects an application to hold afterwards.
#[derive(Clone, Debug)]
struct Expected {
    name: String,
    description: Option<String>,
}

impl Expected {
    fn new(name: &str, description: Option<&str>) -> Self {
        Self {
            name: name.to_string(),
            description: description.map(str::to_string),
        }
    }

    fn with_name(&self, name: &str) -> Self {
        Self {
            name: name.to_string(),
            ..self.clone()
        }
    }

    fn with_description(&self, description: &str) -> Self {
        Self {
            description: Some(description.to_string()),
            ..self.clone()
        }
    }
}

/// `PATCH /applications/{ApplicationId}`.
async fn update(router: &MultiServiceRouter, application: &str, patch: &Value) -> Answer {
    json_call(router, "PATCH", &format!("/applications/{application}"), patch).await
}

/// `GET /applications/{ApplicationId}`.
async fn get(router: &MultiServiceRouter, application: &str) -> Answer {
    json_call(
        router,
        "GET",
        &format!("/applications/{application}"),
        &Value::Null,
    )
    .await
}

/// Assert that `answer` is a 200 describing `expected` under the id the request named.
fn assert_application(case: &str, source: &str, answer: &Answer, id: &str, expected: &Expected) {
    assert_eq!(
        answer.status,
        200,
        "case {case:?} ({source}): expected 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let body = answer.json();
    assert_eq!(
        body.get("Name").and_then(Value::as_str),
        Some(expected.name.as_str()),
        "case {case:?} ({source}): expected Name {:?}, got {:?}",
        expected.name,
        body.get("Name")
    );
    assert_eq!(
        body.get("Description").and_then(Value::as_str),
        expected.description.as_deref(),
        "case {case:?} ({source}): expected Description {:?}, got {:?}",
        expected.description,
        body.get("Description")
    );
    assert_eq!(
        body.get("Id").and_then(Value::as_str),
        Some(id),
        "case {case:?} ({source}): Id is not writable, expected {id:?}, got {:?}",
        body.get("Id")
    );
    assert!(
        is_modelled_id(id),
        "case {case:?} ({source}): Id must match the modelled ^[a-z0-9]{{4,7}}$, got {id:?}"
    );
}

/// Read the application back through `GetApplication` and assert the same expectation: the
/// update was persisted, not merely reported.
async fn assert_round_trip(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    expected: &Expected,
) {
    let fetched = get(router, id).await;
    assert_application(case, "GetApplication", &fetched, id, expected);
}

/// Create the baseline application (both writable members set) and answer its minted id.
async fn create_baseline(router: &MultiServiceRouter) -> String {
    let created = json_call(
        router,
        "POST",
        "/applications",
        &json!({ "Name": BASELINE_NAME, "Description": BASELINE_DESCRIPTION }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "the application under test must be created, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    created
        .string("Id")
        .expect("CreateApplication answered no Id")
}

/// Each patch writes the members it carries, leaves the rest alone, and the record it
/// leaves behind is what `GetApplication` answers.
#[tokio::test]
async fn update_application_replaces_only_the_members_the_request_carries() {
    let router = router();
    let id = create_baseline(&router).await;

    // The published bounds of the two writable members: `Name` len 1..64 and
    // `Description` len 0..1024.
    let longest_name = filler(64, 3);
    let longest_description = filler(1024, 7);

    // Rows differ along which members the patch carries and the value each carries. Each
    // row's expectation is relative to the row before it, which is what makes "an omitted
    // member keeps its stored value" observable.
    let mut expected = Expected::new(BASELINE_NAME, Some(BASELINE_DESCRIPTION));
    let cases: Vec<(&str, Value, Expected)> = vec![
        (
            "name only",
            json!({ "Name": "renamed" }),
            expected.with_name("renamed"),
        ),
        (
            "description only",
            json!({ "Description": "a replacement description" }),
            expected
                .with_name("renamed")
                .with_description("a replacement description"),
        ),
        (
            "no writable member at all",
            json!({}),
            expected
                .with_name("renamed")
                .with_description("a replacement description"),
        ),
        (
            "description replaced by the empty string at its lower length bound",
            json!({ "Description": "" }),
            expected.with_name("renamed").with_description(""),
        ),
        (
            "name at the one-character lower bound",
            json!({ "Name": "a" }),
            expected.with_name("a").with_description(""),
        ),
        (
            "a non-ASCII name and description",
            json!({ "Name": "приложение-配置", "Description": "描述 — ünïcödé" }),
            Expected::new("приложение-配置", Some("描述 — ünïcödé")),
        ),
        (
            "URI-significant characters in both members",
            json!({ "Name": "a/b?c#d=e&f", "Description": "100% ok/maybe?" }),
            Expected::new("a/b?c#d=e&f", Some("100% ok/maybe?")),
        ),
        (
            "both members at their upper length bounds",
            json!({ "Name": longest_name, "Description": longest_description }),
            Expected::new(&longest_name, Some(&longest_description)),
        ),
        (
            "name rewritten over the longest description, which is left alone",
            json!({ "Name": "back to short" }),
            Expected::new("back to short", Some(&longest_description)),
        ),
    ];

    for (case, patch, after) in cases {
        let updated = update(&router, &id, &patch).await;
        assert_application(case, "UpdateApplication", &updated, &id, &after);
        assert_round_trip(&router, case, &id, &after).await;
        expected = after;
    }

    // The last row is still what the record holds once the table is done.
    assert_round_trip(&router, "final state", &id, &expected).await;
}

/// An update writes only the two members the request carries: the application's tags and
/// the environments and configuration profiles it owns are not members of
/// `UpdateApplicationRequest`, so a patch leaves every one of them where it was.
#[tokio::test]
async fn update_application_touches_nothing_the_request_does_not_carry() {
    let router = router();
    let id = create_baseline(&router).await;
    let environment = create_environment(&router, &id, "owned-environment").await;
    let profile = create_profile(&router, &id, "owned-profile").await;

    let application_arn = arn(&format!("application/{id}"));
    let tagged = tag(
        &router,
        &application_arn,
        &json!({ "owner": "platform", "tier": "gold" }),
    )
    .await;
    assert_eq!(
        tagged.status,
        204,
        "the application under test must be tagged first, got {} / {}",
        tagged.status,
        String::from_utf8_lossy(&tagged.body)
    );

    // Rows differ along which writable members the patch carries; none of them can reach
    // the tags or the owned resources.
    for (case, patch) in [
        ("name only", json!({ "Name": "renamed once" })),
        ("description only", json!({ "Description": "redescribed" })),
        (
            "both members",
            json!({ "Name": "renamed twice", "Description": "redescribed twice" }),
        ),
        ("no member at all", json!({})),
    ] {
        let updated = update(&router, &id, &patch).await;
        assert_eq!(
            updated.status,
            200,
            "case {case:?}: expected 200, got {} / {}",
            updated.status,
            String::from_utf8_lossy(&updated.body)
        );

        let tags = list_tags(&router, &application_arn).await;
        assert_eq!(
            tags.json().get("Tags").cloned().unwrap_or(Value::Null),
            json!({ "owner": "platform", "tier": "gold" }),
            "case {case:?}: tags are not writable through UpdateApplication, got {:?}",
            tags.json().get("Tags")
        );

        let read_environment = json_call(
            &router,
            "GET",
            &format!("/applications/{id}/environments/{environment}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            read_environment.string("Name").as_deref(),
            Some("owned-environment"),
            "case {case:?}: the owned environment is untouched, got {} / {}",
            read_environment.status,
            String::from_utf8_lossy(&read_environment.body)
        );

        let read_profile = json_call(
            &router,
            "GET",
            &format!("/applications/{id}/configurationprofiles/{profile}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            read_profile.string("Name").as_deref(),
            Some("owned-profile"),
            "case {case:?}: the owned configuration profile is untouched, got {} / {}",
            read_profile.status,
            String::from_utf8_lossy(&read_profile.body)
        );
    }
}

/// A value past a published bound is refused with HTTP 400 and writes nothing: the record
/// the refused request named still holds what it held before.
#[tokio::test]
async fn update_application_refuses_a_value_past_a_modelled_bound_without_writing() {
    let router = router();
    let id = create_baseline(&router).await;
    let expected = Expected::new(BASELINE_NAME, Some(BASELINE_DESCRIPTION));

    // One past each bound the model publishes: `Name` max 64 and min 1, `Description`
    // max 1024.
    for (case, patch) in [
        (
            "name one character past the upper bound",
            json!({ "Name": filler(65, 1) }),
        ),
        (
            "name one character below the lower bound",
            json!({ "Name": "" }),
        ),
        (
            "description one character past the upper bound",
            json!({ "Description": filler(1025, 2) }),
        ),
        (
            "both members past their upper bounds",
            json!({ "Name": filler(100, 4), "Description": filler(2048, 5) }),
        ),
    ] {
        let refused = update(&router, &id, &patch).await;
        assert_eq!(
            refused.status,
            400,
            "case {case:?}: expected 400, got {} / {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert!(
            refused.error_code().contains("BadRequestException"),
            "case {case:?}: expected the modelled BadRequestException, got {:?}",
            refused.error_code()
        );
        assert_round_trip(&router, case, &id, &expected).await;
    }
}

/// `UpdateApplication` patches a record and never creates one: an id no application
/// answers to is HTTP 404 `ResourceNotFoundException`, and nothing is stored for it.
#[tokio::test]
async fn update_application_is_not_found_for_an_id_no_application_answers_to() {
    let router = router();
    let known = create_application(&router, "the known one").await;
    let environment = create_environment(&router, &known, "not-an-application").await;

    // Rows differ along WHY the id names no application: never minted, the right shape
    // but unused, and an id that belongs to another resource kind in the same tree.
    for (case, id) in [
        ("an id the service never minted", "nosuchapp".to_string()),
        ("a vacant id of the modelled shape", "zzzz99".to_string()),
        ("an environment id used as an application id", environment),
    ] {
        let answer = update(&router, &id, &json!({ "Name": "should not be stored" })).await;
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
        // The refused id is still unknown to a reader: the refusal created nothing.
        let fetched = get(&router, &id).await;
        assert_eq!(
            fetched.status,
            404,
            "case {case:?}: the refused update must store nothing, GetApplication got {} / {}",
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );
    }

    // The application that does exist is untouched by every refusal above.
    assert_round_trip(
        &router,
        "the known application after the refusals",
        &known,
        &Expected::new("the known one", None),
    )
    .await;
}

/// Successive updates are not a conflict — the operation models none — and the last write
/// is what the record holds, for every row of the sequence.
#[tokio::test]
async fn the_last_update_wins() {
    let router = router();
    let id = create_baseline(&router).await;

    for case in ["first writer", "second writer", "third writer"] {
        let patch = json!({ "Name": case, "Description": format!("set by {case}") });
        let updated = update(&router, &id, &patch).await;
        let expected = Expected::new(case, Some(&format!("set by {case}")));
        assert_application(case, "UpdateApplication", &updated, &id, &expected);
        assert_round_trip(&router, case, &id, &expected).await;
    }
}
