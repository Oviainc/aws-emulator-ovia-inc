// SPDX-License-Identifier: CC0-1.0

//! `ListExperimentRunEvents` — the timeline of one experiment run — end to end over HTTP.
//!
//! RULE: the listing reports the events this service RECORDED on the addressed run, one
//! entry per action a call performed on it, MOST RECENT FIRST, each entry reporting the
//! type of action, who triggered it, when it occurred and the run's exposure
//! and overrides at that moment; the entries are cut into pages of at most `MaxResults`,
//! resumed only by a `NextToken` this listing itself minted; and an application, a
//! definition or a run number that addresses nothing is HTTP 404 rather than an empty
//! timeline.
//!
//! The tables span what the rule generalises over rather than one recorded request: WHICH
//! optional members the start of the run carried and their values at the published bounds
//! (`Description` len 0..1024, `NullablePercentage` range 0..100, from
//! `smithy_query(query="operation", operation="StartExperimentRun")`), how many treatment
//! overrides it assigned, whether the run is addressed by ids or by names, WHICH page size
//! the request asks for (absent, both published bounds of `MaxResults` and values between
//! them, and one past each bound), WHERE a `NextToken` came from, and WHICH level of the
//! path — application, definition or run number — resolves to nothing.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::appconfig_workflow::{create_application, json_call, router, Answer};
use crate::experiment_definitions::{create_definition, flag, scope, treatment, Scope};
use crate::resource_tags::{filler, pct};

/// The values `ExperimentRunEventType` accepts
/// (`smithy_query(query="shape", shape="ExperimentRunEventType")`). Every entry a listing
/// reports must name one of them, and the entry a start writes names the first.
/// `pub(crate)` because more than one suite in this target reads a run's timeline.
pub(crate) const EVENT_TYPES: &[&str] = &[
    "RUN_STARTED",
    "EXPOSURE_UPDATED",
    "OVERRIDES_UPDATED",
    "RUN_STOPPED",
];

/// The values `TriggeredBy` accepts
/// (`smithy_query(query="shape", shape="TriggeredBy")`). An action a caller performed is
/// triggered by `USER`.
const TRIGGERED_BY: &[&str] = &["USER", "APPCONFIG", "CLOUDWATCH_ALARM", "INTERNAL_ERROR"];

/// The published bounds of `MaxResults` on this operation, range 1..50
/// (`smithy_query(query="operation", operation="ListExperimentRunEvents")`).
const MIN_MAX_RESULTS: i64 = 1;
/// See [`MIN_MAX_RESULTS`].
const MAX_MAX_RESULTS: i64 = 50;

/// `GET .../experimentruns/{Run}/events`, with `query` as the raw query string (empty for
/// a request that names neither `MaxResults` nor `NextToken`; both are `httpQuery`
/// members, named `max_results` and `next_token`). `pub(crate)` because more than one
/// suite in this target reads a run's timeline.
pub(crate) async fn list_events(
    router: &MultiServiceRouter,
    application: &str,
    definition: &str,
    run: i64,
    query: &str,
) -> Answer {
    let path = format!(
        "/applications/{}/experimentdefinitions/{}/experimentruns/{run}/events",
        pct(application),
        pct(definition)
    );
    let uri = if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    };
    json_call(router, "GET", &uri, &Value::Null).await
}

/// `POST .../experimentruns`.
async fn start_run(
    router: &MultiServiceRouter,
    application: &str,
    definition: &str,
    body: &Value,
) -> Answer {
    json_call(
        router,
        "POST",
        &format!(
            "/applications/{}/experimentdefinitions/{}/experimentruns",
            pct(application),
            pct(definition)
        ),
        body,
    )
    .await
}

/// `GET .../experimentruns/{Run}` — the reader that proves the listing reports the stored
/// run rather than anything the listing itself derived.
async fn get_run(
    router: &MultiServiceRouter,
    application: &str,
    definition: &str,
    run: i64,
) -> Answer {
    json_call(
        router,
        "GET",
        &format!(
            "/applications/{}/experimentdefinitions/{}/experimentruns/{run}",
            pct(application),
            pct(definition)
        ),
        &Value::Null,
    )
    .await
}

/// A JSON object from `(key, value)` pairs — the wire form of the inline
/// `TreatmentOverrides`.
fn string_map<'pair>(pairs: impl IntoIterator<Item = (&'pair str, &'pair str)>) -> Value {
    Value::Object(
        pairs
            .into_iter()
            .map(|(key, value)| (key.to_string(), Value::from(value)))
            .collect::<Map<String, Value>>(),
    )
}

/// Create one experiment definition under `scope` and answer its minted id.
async fn definition_id(router: &MultiServiceRouter, scope: &Scope, name: &str) -> String {
    let answer = create_definition(
        router,
        &scope.application,
        &json!({
            "Name": name,
            "ConfigurationProfileIdentifier": scope.profile,
            "EnvironmentIdentifier": scope.environment,
            "FlagKey": "checkout-flow",
            "AudienceRule": "attribute(\"tier\") == \"beta\"",
            "Treatments": [treatment(50.0, Some("the new flow"), flag(true))],
            "Control": treatment(50.0, Some("today's experience"), flag(false)),
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "case {name:?}: the definition under test must be created, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .json()
        .get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {name:?}: the create answered no Id"))
        .to_string()
}

/// The `Items` of a listing (an answer with no `Items` at all reads as empty).
fn items(answer: &Answer) -> Vec<Value> {
    answer
        .json()
        .get("Items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Assert a 200 listing and answer its entries.
fn listed(case: &str, answer: &Answer) -> Vec<Value> {
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: expected 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    items(answer)
}

/// Assert a refusal: the status the operation models for it and its wire error code.
fn assert_refused(case: &str, refused: &Answer, expected_status: u16, expected_code: &str) {
    assert_eq!(
        refused.status,
        expected_status,
        "case {case:?}: expected {expected_status}, got {} / {}",
        refused.status,
        String::from_utf8_lossy(&refused.body)
    );
    assert!(
        refused.error_code().contains(expected_code),
        "case {case:?}: expected {expected_code}, got {:?}",
        refused.error_code()
    );
}

/// Assert the one entry a started run's timeline holds against the run the start wrote.
///
/// `started` is the `StartExperimentRun` document, so every value compared here comes from
/// the stored run rather than from this test's own expectations.
fn assert_run_started_entry(case: &str, entry: &Value, started: &Value) {
    let event_type = entry.get("EventType").and_then(Value::as_str);
    assert!(
        event_type.is_some_and(|value| EVENT_TYPES.contains(&value)),
        "case {case:?}: expected one of {EVENT_TYPES:?}, got {event_type:?}"
    );
    assert_eq!(
        event_type,
        Some("RUN_STARTED"),
        "case {case:?}: expected the start of the run to be recorded as RUN_STARTED, got \
         {event_type:?}"
    );
    let triggered_by = entry.get("TriggeredBy").and_then(Value::as_str);
    assert!(
        triggered_by.is_some_and(|value| TRIGGERED_BY.contains(&value)),
        "case {case:?}: expected one of {TRIGGERED_BY:?}, got {triggered_by:?}"
    );
    assert_eq!(
        triggered_by,
        Some("USER"),
        "case {case:?}: a start a caller asked for is triggered by USER, got {triggered_by:?}"
    );
    assert_eq!(
        entry.get("OccurredAt"),
        started.get("StartedAt"),
        "case {case:?}: expected OccurredAt to be the run's StartedAt {:?}, got {:?}",
        started.get("StartedAt"),
        entry.get("OccurredAt")
    );
    // "The exposure percentage at the time of the event" and "The treatment overrides at
    // the time of the event" (`smithy_query(query="shape", shape="ExperimentRunEvent")`),
    // which for the start of the run is what the start stored — the share of the audience
    // the run began delivering to, and the assignments it began with where it carried any.
    for member in ["ExposurePercentage", "TreatmentOverrides"] {
        assert_eq!(
            entry.get(member),
            started.get(member),
            "case {case:?}: expected {member} {:?}, got {:?}",
            started.get(member),
            entry.get(member)
        );
    }
    assert!(
        entry
            .get("Description")
            .and_then(Value::as_str)
            .is_some_and(|text| !text.is_empty()),
        "case {case:?}: expected a description of the event, got {:?}",
        entry.get("Description")
    );
    // Nothing in this service deploys a configuration on an experiment run's behalf, so
    // the entry names no deployment rather than one no `GetDeployment` could report.
    assert_eq!(
        entry.get("AssociatedDeployment"),
        None,
        "case {case:?}: expected no AssociatedDeployment, got {:?}",
        entry.get("AssociatedDeployment")
    );
}

/// A started run's timeline reports exactly the one action performed on it so far, read
/// out of the run the start persisted — whatever the start carried, and however the run is
/// addressed.
#[tokio::test]
async fn the_timeline_of_a_started_run_reports_what_the_start_recorded() {
    let router = router();
    let scope = scope(&router, "timeline").await;

    // Rows differ along WHICH optional members the start carried and their values at the
    // published bounds, and along how many overrides it assigned.
    let longest_description = filler(1024, 7);
    let rows: Vec<(&str, Value)> = vec![
        ("nothing optional", json!({})),
        (
            "a description at the lower length bound",
            json!({ "Description": "" }),
        ),
        (
            "a description at the upper length bound",
            json!({ "Description": longest_description.clone() }),
        ),
        (
            "an exposure at the lower range bound",
            json!({ "ExposurePercentage": 0.0 }),
        ),
        (
            "a fractional exposure",
            json!({ "ExposurePercentage": 12.5 }),
        ),
        (
            "an exposure at the upper range bound",
            json!({ "ExposurePercentage": 100.0 }),
        ),
        (
            "one treatment override",
            json!({ "TreatmentOverrides": { "Inline": string_map([("entity-1", "T1")]) } }),
        ),
        (
            "several treatment overrides beside every other member",
            json!({
                "Description": "overridden assignment",
                "ExposurePercentage": 37.5,
                "TreatmentOverrides": { "Inline": string_map([
                    ("entity-1", "T1"),
                    ("entity-2", "C"),
                    ("entity-3", "T1"),
                ]) },
            }),
        ),
    ];

    for (index, (case, body)) in rows.iter().enumerate() {
        // One definition per row: a definition delivers one run at a time.
        let name = format!("timeline-{index}");
        let id = definition_id(&router, &scope, &name).await;
        let created = start_run(&router, &scope.application, &id, body).await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: the run under test must start, got {} / {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let started = created.json();

        let answer = list_events(&router, &scope.application, &id, 1, "").await;
        let entries = listed(case, &answer);
        assert_eq!(
            entries.len(),
            1,
            "case {case:?}: one action has been performed on the run, so its timeline holds \
             one entry, got {entries:?}"
        );
        assert_run_started_entry(case, &entries[0], &started);
        assert_eq!(
            answer.json().get("NextToken"),
            None,
            "case {case:?}: a timeline served whole leaves nothing to resume, got {:?}",
            answer.json().get("NextToken")
        );

        // The identifiers are the resource's "ID or name", so the same run is reached
        // either way and answers the same timeline. `scope` names its application
        // `app-<label>`.
        let by_name = list_events(&router, "app-timeline", &name, 1, "").await;
        assert_eq!(
            listed(case, &by_name),
            entries,
            "case {case:?}: expected the same timeline when addressed by name, got {:?}",
            listed(case, &by_name)
        );

        // Reporting only: listing the timeline twice answers the same thing, and the run
        // itself is unchanged by having been read.
        let again = list_events(&router, &scope.application, &id, 1, "").await;
        assert_eq!(
            listed(case, &again),
            entries,
            "case {case:?}: a second listing must report the same timeline, got {:?}",
            listed(case, &again)
        );
        let read = get_run(&router, &scope.application, &id, 1).await;
        assert_eq!(
            read.json(),
            started,
            "case {case:?}: expected the run to be unchanged by the listing, got {:?}",
            read.json()
        );
    }
}

/// Each run's timeline is its own: a listing never reports another run's entries.
#[tokio::test]
async fn each_run_reports_only_its_own_timeline() {
    let router = router();
    let here = scope(&router, "own").await;
    let elsewhere = scope(&router, "own-other").await;

    // Rows differ along which application holds the definition, and along the exposure
    // that tells one run's entry from another's.
    let rows: Vec<(&str, &Scope, &str, f64)> = vec![
        ("the first definition of its application", &here, "own-one", 10.0),
        ("a second definition of the same application", &here, "own-two", 20.0),
        ("a definition of another application", &elsewhere, "own-far", 30.0),
    ];

    let mut started: Vec<(&str, String, String, f64)> = Vec::new();
    for (case, scope, name, exposure) in &rows {
        let id = definition_id(&router, scope, name).await;
        let created = start_run(
            &router,
            &scope.application,
            &id,
            &json!({ "ExposurePercentage": exposure }),
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: the run under test must start, got {} / {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        started.push((case, scope.application.clone(), id, *exposure));
    }

    for (case, application, id, exposure) in &started {
        let entries = listed(
            case,
            &list_events(&router, application, id, 1, "").await,
        );
        assert_eq!(
            entries.len(),
            1,
            "case {case:?}: expected one entry on this run's timeline, got {entries:?}"
        );
        assert_eq!(
            entries[0].get("ExposurePercentage").and_then(Value::as_f64),
            Some(*exposure),
            "case {case:?}: expected this run's own exposure {exposure}, got {:?}",
            entries[0].get("ExposurePercentage")
        );
    }
}

/// A page carries at most the entries the request asked for, and the only token that
/// resumes the walk is one this listing minted: a page size outside the modelled range is
/// refused before the handler runs, and a foreign or malformed token is a bad request
/// rather than the first page over again.
#[tokio::test]
async fn a_page_is_bounded_and_only_this_listings_own_token_resumes_it() {
    let router = router();
    let scope = scope(&router, "pages").await;
    let id = definition_id(&router, &scope, "pages").await;
    let created = start_run(&router, &scope.application, &id, &json!({})).await;
    assert_eq!(
        created.status,
        201,
        "the run under test must start, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );

    // Rows differ along WHICH page size the request asks for: absent, both published
    // bounds, and values in between.
    for max_results in [None, Some(MIN_MAX_RESULTS), Some(2), Some(25), Some(MAX_MAX_RESULTS)] {
        let case = format!("max_results {max_results:?}");
        let query = max_results.map_or(String::new(), |size| format!("max_results={size}"));
        let answer = list_events(&router, &scope.application, &id, 1, &query).await;
        let entries = listed(&case, &answer);
        let allowed = max_results.unwrap_or(MAX_MAX_RESULTS) as usize;
        assert!(
            entries.len() <= allowed,
            "case {case:?}: a page carries at most {allowed} entries, got {}",
            entries.len()
        );
        assert_eq!(
            entries.len(),
            1,
            "case {case:?}: the whole timeline fits in one page, got {entries:?}"
        );
        assert_eq!(
            answer.json().get("NextToken"),
            None,
            "case {case:?}: nothing is left over, so no token comes back, got {:?}",
            answer.json().get("NextToken")
        );
    }

    // One past each published bound is outside the modelled range, so it is refused as a
    // bad request rather than served as some other page size.
    for size in [MIN_MAX_RESULTS - 1, MAX_MAX_RESULTS + 1] {
        let case = format!("max_results {size}, outside the modelled range");
        let refused =
            list_events(&router, &scope.application, &id, 1, &format!("max_results={size}")).await;
        assert_eq!(
            refused.status,
            400,
            "case {case:?}: expected 400, got {} / {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
    }

    // A token this listing did not mint: one that is not a token at all, and one another
    // listing of this service handed out.
    let second_application = create_application(&router, "app-pages-other").await;
    assert!(
        !second_application.is_empty(),
        "the second application must be created so another listing has a page to cut"
    );
    let applications = json_call(&router, "GET", "/applications?max_results=1", &Value::Null).await;
    let foreign_token = applications
        .json()
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
        .expect("ListApplications must hand back a token while applications are left over");
    for (case, token) in [
        ("a token that is not one this service minted", "zzz".to_string()),
        ("a token minted by another listing", foreign_token),
    ] {
        let refused = list_events(
            &router,
            &scope.application,
            &id,
            1,
            &format!("next_token={token}"),
        )
        .await;
        assert_refused(case, &refused, 400, "BadRequestException");
    }
}

/// A level of the path that addresses nothing is HTTP 404, not an empty timeline; a run
/// number below the modelled range is refused before the handler runs.
#[tokio::test]
async fn a_path_that_addresses_nothing_is_not_found() {
    let router = router();
    let scope = scope(&router, "misses").await;
    let other_application = create_application(&router, "app-misses-elsewhere").await;
    let id = definition_id(&router, &scope, "misses").await;
    let created = start_run(&router, &scope.application, &id, &json!({})).await;
    assert_eq!(
        created.status,
        201,
        "the run under test must start, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );

    // Rows differ along WHICH level of the path resolves to nothing.
    for (case, application, definition) in [
        (
            "an application no resource answers to",
            "nosuchapp".to_string(),
            id.clone(),
        ),
        (
            "a definition name nothing holds",
            scope.application.clone(),
            "no-such-definition".to_string(),
        ),
        (
            "a definition addressed under the wrong application",
            other_application.clone(),
            id.clone(),
        ),
    ] {
        let refused = list_events(&router, &application, &definition, 1, "").await;
        assert_refused(case, &refused, 404, "ResourceNotFoundException");
    }

    // Rows differ along WHICH run number the definition never issued.
    for (case, run) in [
        ("a run number never issued", 7_i64),
        ("the largest run number the model's Integer holds", 2_147_483_647),
    ] {
        let refused = list_events(&router, &scope.application, &id, run, "").await;
        assert_refused(case, &refused, 404, "ResourceNotFoundException");
    }

    // A definition that was never started holds no run, so not even the first number
    // addresses anything.
    let unstarted = definition_id(&router, &scope, "misses-unstarted").await;
    let refused = list_events(&router, &scope.application, &unstarted, 1, "").await;
    assert_refused(
        "the first run number of a definition nothing started",
        &refused,
        404,
        "ResourceNotFoundException",
    );

    // `Run` is a `PositiveInteger` (range 1..), so one below that bound is a bad request
    // rather than a lookup.
    let case = "a run number below the modelled range";
    let refused = list_events(&router, &scope.application, &id, 0, "").await;
    assert_eq!(
        refused.status,
        400,
        "case {case:?}: expected 400, got {} / {}",
        refused.status,
        String::from_utf8_lossy(&refused.body)
    );
}
