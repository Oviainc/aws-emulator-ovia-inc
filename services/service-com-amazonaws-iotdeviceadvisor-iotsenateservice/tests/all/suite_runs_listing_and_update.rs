// SPDX-License-Identifier: CC0-1.0

//! `ListSuiteRuns`, `UpdateSuiteDefinition`, and the templates this service's refusals
//! carry, over the wire.
//!
//! Every case drives real HTTP requests through the same `MultiServiceRouter` the emulator
//! serves on a socket (the harness `suite_lifecycle` already owns), so what is asserted is
//! the service's wire behaviour.
//!
//! The tables span the classes the rules generalise over rather than the one request the
//! issue cited: every combination of the two `ListSuiteRuns` filters including ones that
//! match nothing, `maxResults` at each published bound and one past each, pagination
//! tokens of several malformed shapes against BOTH listings, configurations differing in
//! name, `rootGroup`, protocol and device list for the update round-trip, and every
//! lookup in this service that can fail to find what it was asked for.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_iotdeviceadvisor_iotsenateservice as iotdeviceadvisor;

use super::suite_lifecycle::{thing_arn, Emulator, ROLE_ARN};

/// The region every case below signs for, so the ARNs and the endpoint are the published
/// shape for one region throughout.
const REGION: &str = "us-east-1";

/// Put one FINISHED run of `(definition id, version)` straight into the store and hand
/// back its id.
///
/// A fixture, not a behaviour: the service serialises runs of one definition, so reaching
/// several stored runs over the wire would mean waiting out a transition span per run.
/// What is under test is what the LISTING reports about a store that holds them, so the
/// seeded statuses are ones this service's own lifecycle produces (`ERROR` for a run no
/// test runner could execute, `STOPPED` for one a caller stopped) and no transition is
/// left owed.
fn seed_finished_run(
    emulator: &Emulator,
    definition_id: &str,
    version: &str,
    status: &str,
    age: Duration,
) -> String {
    let state = iotdeviceadvisor::state::service_state(&*emulator.engine)
        .expect("the service's store is reachable from the shared engine");
    let mut store = state.lock();
    let id = iotdeviceadvisor::state::new_uuid();
    let created_at = SystemTime::now() - age;
    store.runs.insert(
        id.clone(),
        iotdeviceadvisor::state::SuiteRun {
            id: id.clone(),
            arn: iotdeviceadvisor::state::arn(REGION, &format!("suiterun/{definition_id}/{id}")),
            suite_definition_id: definition_id.to_string(),
            suite_definition_version: version.to_string(),
            configuration: iotdeviceadvisor::ops::types::SuiteRunConfiguration::default(),
            status: status.to_string(),
            created_at,
            started_at: Some(created_at),
            end_at: Some(created_at + Duration::from_secs(1)),
            error_reason: None,
            endpoint: iotdeviceadvisor::state::endpoint_for_region(REGION),
            next_transition_at: None,
        },
    );
    id
}

/// Create one suite definition named `name` and hand back its id.
async fn definition_with_name(emulator: &Emulator, name: &str) -> String {
    let created = emulator
        .create_definition(
            &json!({
                "suiteDefinitionName": name,
                "rootGroup": "g",
                "devicePermissionRoleArn": ROLE_ARN,
                "devices": [{ "thingArn": thing_arn(0) }],
            }),
            REGION,
        )
        .await;
    assert_eq!(
        created.status, 200,
        "setup: creating {name:?} failed with {} {}",
        created.status, created.body
    );
    created.string("suiteDefinitionId")
}

/// The run ids one `ListSuiteRuns` answer carries, in the order it listed them.
fn listed_ids(answer: &Value) -> Vec<String> {
    answer["suiteRunsList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            entry["suiteRunId"]
                .as_str()
                .unwrap_or_else(|| panic!("every entry carries a suiteRunId, got {entry}"))
                .to_string()
        })
        .collect()
}

/// `ListSuiteRuns` projects the stored runs, narrows them by each filter the caller names,
/// and answers an EMPTY list — never an error — for a filter that matches nothing.
#[tokio::test]
async fn listing_suite_runs_filters_the_stored_runs_and_never_errors_on_an_unmatched_filter() {
    let emulator = Emulator::new();

    let first = definition_with_name(&emulator, "listing-suite-one").await;
    let second = definition_with_name(&emulator, "listing-suite-two").await;

    // A second version of the first definition, so the version filter has two values to
    // tell apart.
    let updated = emulator
        .call(
            "PATCH",
            &format!("/suiteDefinitions/{first}"),
            REGION,
            Some(json!({
                "suiteDefinitionConfiguration": {
                    "suiteDefinitionName": "listing-suite-one-v2",
                    "rootGroup": "g2",
                    "devicePermissionRoleArn": ROLE_ARN,
                },
            })),
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "setup: the update failed with {} {}",
        updated.status, updated.body
    );

    let first_v1_error =
        seed_finished_run(&emulator, &first, "v1", "ERROR", Duration::from_secs(300));
    let first_v1_stopped = seed_finished_run(
        &emulator,
        &first,
        "v1",
        "STOPPED",
        Duration::from_secs(200),
    );
    let first_v2_error =
        seed_finished_run(&emulator, &first, "v2", "ERROR", Duration::from_secs(100));
    let second_v1_error =
        seed_finished_run(&emulator, &second, "v1", "ERROR", Duration::from_secs(50));

    // One run started for real, so the listing is asserted over a run the service minted
    // itself and not only over seeded ones. It runs the definition's latest version (v2).
    let started = emulator
        .call(
            "POST",
            &format!("/suiteDefinitions/{first}/suiteRuns"),
            REGION,
            Some(json!({
                "suiteRunConfiguration": { "primaryDevice": { "thingArn": thing_arn(1) } },
            })),
        )
        .await;
    assert_eq!(
        started.status, 200,
        "setup: StartSuiteRun failed with {} {}",
        started.status, started.body
    );
    let live_run = started.string("suiteRunId");

    let mut all = vec![
        first_v1_error.clone(),
        first_v1_stopped.clone(),
        first_v2_error.clone(),
        second_v1_error.clone(),
        live_run.clone(),
    ];
    all.sort();
    let mut of_first = vec![
        first_v1_error.clone(),
        first_v1_stopped.clone(),
        first_v2_error.clone(),
        live_run.clone(),
    ];
    of_first.sort();
    let mut first_v1 = vec![first_v1_error.clone(), first_v1_stopped.clone()];
    first_v1.sort();
    let mut first_v2 = vec![first_v2_error.clone(), live_run.clone()];
    first_v2.sort();

    // (case, query string, the ids the answer must carry)
    let rows: Vec<(String, String, Vec<String>)> = vec![
        ("no filter".to_string(), String::new(), all),
        (
            "by definition, four runs".to_string(),
            format!("?suiteDefinitionId={first}"),
            of_first.clone(),
        ),
        (
            "by definition, one run".to_string(),
            format!("?suiteDefinitionId={second}"),
            vec![second_v1_error.clone()],
        ),
        (
            "by definition and its first version".to_string(),
            format!("?suiteDefinitionId={first}&suiteDefinitionVersion=v1"),
            first_v1,
        ),
        (
            "by definition and its latest version".to_string(),
            format!("?suiteDefinitionId={first}&suiteDefinitionVersion=v2"),
            first_v2,
        ),
        (
            "a version filter that matches nothing".to_string(),
            format!("?suiteDefinitionId={first}&suiteDefinitionVersion=does-not-exist-99"),
            Vec::new(),
        ),
        (
            "a definition filter that matches nothing".to_string(),
            "?suiteDefinitionId=11111111-2222-3333-4444-555555555555".to_string(),
            Vec::new(),
        ),
        (
            "a definition that holds no runs and a version filter".to_string(),
            format!("?suiteDefinitionId={second}&suiteDefinitionVersion=v2"),
            Vec::new(),
        ),
    ];

    for (case, query, expected) in rows {
        let answer = emulator
            .call("GET", &format!("/suiteRuns{query}"), REGION, None)
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a listing is never an error, got {} {}",
            answer.status, answer.body
        );
        assert!(
            answer.member("suiteRunsList").is_array(),
            "case {case:?}: suiteRunsList must be present, got {}",
            answer.body
        );
        let mut listed = listed_ids(&answer.body);
        listed.sort();
        assert_eq!(
            listed, expected,
            "case {case:?}: unexpected suiteRunsList, got {}",
            answer.body
        );
        assert!(
            answer.member("nextToken").is_null(),
            "case {case:?}: one page holds every match, so no nextToken is owed, got {}",
            answer.body
        );

        for entry in answer
            .member("suiteRunsList")
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            let run_id = entry["suiteRunId"].as_str().unwrap_or_default().to_string();
            let definition_id = entry["suiteDefinitionId"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let expected_name = if definition_id == first {
                "listing-suite-one-v2"
            } else {
                "listing-suite-two"
            };
            assert_eq!(
                entry["suiteDefinitionName"].as_str(),
                Some(expected_name),
                "case {case:?}: run {run_id} must carry the definition's stored name, \
                 got {entry}"
            );
            assert!(
                entry["createdAt"].is_number(),
                "case {case:?}: run {run_id} has no createdAt, got {entry}"
            );
            let status = entry["status"].as_str().unwrap_or_default();
            assert!(
                iotdeviceadvisor::ops::types::STATUS_VALUES.contains(&status),
                "case {case:?}: run {run_id} reports {status:?}, which is not a modelled \
                 Status"
            );
            assert!(
                entry["passed"].is_null() && entry["failed"].is_null(),
                "case {case:?}: no test case ran, so no passed/failed count is reported, \
                 got {entry}"
            );
            assert!(
                entry["suiteDefinitionVersion"]
                    .as_str()
                    .is_some_and(|version| version == "v1" || version == "v2"),
                "case {case:?}: run {run_id} must report the version it runs, got {entry}"
            );
        }
    }

    // The live run is the one the service minted, and it is reported as its stored status.
    let live = emulator
        .call(
            "GET",
            &format!("/suiteRuns?suiteDefinitionId={first}"),
            REGION,
            None,
        )
        .await;
    let live_entry = live
        .member("suiteRunsList")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|entry| entry["suiteRunId"].as_str() == Some(live_run.as_str()))
        .unwrap_or_else(|| panic!("the started run must be listed, got {}", live.body));
    assert_eq!(
        live_entry["status"].as_str(),
        Some("PENDING"),
        "case \"the run just started\": a run read straight after the start is PENDING, \
         got {live_entry}"
    );
    assert!(
        live_entry["endAt"].is_null(),
        "case \"the run just started\": a non-terminal run has no endAt, got {live_entry}"
    );
}

/// `ListSuiteRuns` pages with a terminating `nextToken`, for `maxResults` at each
/// published bound and in between, and refuses one past each bound.
#[tokio::test]
async fn listing_suite_runs_pages_to_the_end() {
    let emulator = Emulator::new();
    let definition = definition_with_name(&emulator, "paging-suite").await;

    let mut seeded: Vec<String> = (0..7)
        .map(|index| {
            seed_finished_run(
                &emulator,
                &definition,
                "v1",
                if index % 2 == 0 { "ERROR" } else { "STOPPED" },
                Duration::from_secs(600 - index * 10),
            )
        })
        .collect();
    seeded.sort();

    // `maxResults` is `range 1..50`: both published bounds, and sizes between them.
    for page_size in [1usize, 2, 3, 50] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let query = match &token {
                Some(token) => format!(
                    "/suiteRuns?suiteDefinitionId={definition}&maxResults={page_size}\
                     &nextToken={token}"
                ),
                None => {
                    format!("/suiteRuns?suiteDefinitionId={definition}&maxResults={page_size}")
                }
            };
            let page = emulator.call("GET", &query, REGION, None).await;
            assert_eq!(
                page.status, 200,
                "case maxResults={page_size}: expected HTTP 200, got {} {}",
                page.status, page.body
            );
            let ids = listed_ids(&page.body);
            assert!(
                ids.len() <= page_size,
                "case maxResults={page_size}: a page held {} entries",
                ids.len()
            );
            seen.extend(ids);
            pages += 1;
            assert!(
                pages <= 10,
                "case maxResults={page_size}: paging did not terminate"
            );
            match page.member("nextToken").as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            seen.len(),
            "case maxResults={page_size}: paging listed a run twice: {seen:?}"
        );
        assert_eq!(
            sorted, seeded,
            "case maxResults={page_size}: paging must yield every run exactly once"
        );
        let expected_pages = seeded.len().div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case maxResults={page_size}: expected {expected_pages} pages, got {pages}"
        );
    }

    // One past each published bound: the modelled `range 1..50` refuses both.
    for out_of_range in ["0", "51"] {
        let refused = emulator
            .call(
                "GET",
                &format!("/suiteRuns?suiteDefinitionId={definition}&maxResults={out_of_range}"),
                REGION,
                None,
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (400, Some("ValidationException")),
            "case maxResults={out_of_range}: a value outside range 1..50 must be refused, \
             got {} {}",
            refused.status,
            refused.body
        );
    }
}

/// A `nextToken` this service cannot have issued is refused with the modelled
/// `ValidationException`, by EVERY listing — while a token it did issue, and the empty
/// token the model's `len 0..2000` permits, are accepted.
#[tokio::test]
async fn a_next_token_this_service_never_issued_is_refused_by_every_listing() {
    let emulator = Emulator::new();
    let definition = definition_with_name(&emulator, "token-suite").await;
    // A second definition, so a one-entry page of EITHER listing has a next page to
    // resume at.
    definition_with_name(&emulator, "token-suite-two").await;
    for index in 0..3 {
        seed_finished_run(
            &emulator,
            &definition,
            "v1",
            "ERROR",
            Duration::from_secs(90 - index * 10),
        );
    }

    // Tokens of several malformed shapes: the recorded one, a word, a uuid-shaped value
    // that is not hex, an upper-case uuid, a truncated uuid, one group too many, and a
    // token at the modelled maximum length.
    let malformed = [
        "sim2real-behav-not-a-real-pagination-token",
        "not-a-token",
        "gggggggg-hhhh-iiii-jjjj-kkkkkkkkkkkk",
        "11111111-2222-3333-4444-5555555555AA",
        "11111111-2222-3333-4444-55555555",
        "11111111-2222-3333-4444-555555555555-6666",
        "11111111222233334444555555555555",
        &"t".repeat(2000),
    ];

    for listing in ["/suiteRuns", "/suiteDefinitions"] {
        for token in malformed {
            let case = format!("{listing} with token {token:.48?}");
            let refused = emulator
                .call("GET", &format!("{listing}?nextToken={token}"), REGION, None)
                .await;
            assert_eq!(
                (refused.status, refused.error_code.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?}: a token this service never issued must be a 400 \
                 ValidationException, got {} {}",
                refused.status,
                refused.body
            );
            assert!(
                refused
                    .member("message")
                    .as_str()
                    .is_some_and(|message| message.contains("nextToken")),
                "case {case:?}: the refusal must name the member it refused, got {}",
                refused.body
            );
        }

        // The empty token carries no position, so it is the same request as naming none.
        let empty = emulator
            .call("GET", &format!("{listing}?nextToken="), REGION, None)
            .await;
        assert_eq!(
            empty.status, 200,
            "case {listing:?} with the empty token: expected HTTP 200, got {} {}",
            empty.status, empty.body
        );

        // A token this service DID issue resumes the listing.
        let first_page = emulator
            .call("GET", &format!("{listing}?maxResults=1"), REGION, None)
            .await;
        let issued = first_page
            .member("nextToken")
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {listing:?}: expected a nextToken on a one-entry page, got {}",
                    first_page.body
                )
            })
            .to_string();
        let resumed = emulator
            .call(
                "GET",
                &format!("{listing}?maxResults=1&nextToken={issued}"),
                REGION,
                None,
            )
            .await;
        assert_eq!(
            resumed.status, 200,
            "case {listing:?} with a service-issued token: expected HTTP 200, got {} {}",
            resumed.status, resumed.body
        );
    }

    // `ListSuiteRunsRequest$suiteDefinitionVersion`: "Must be passed along with
    // suiteDefinitionId" — a version filter alone names no suite.
    let orphan_version = emulator
        .call("GET", "/suiteRuns?suiteDefinitionVersion=v1", REGION, None)
        .await;
    assert_eq!(
        (orphan_version.status, orphan_version.error_code.as_deref()),
        (400, Some("ValidationException")),
        "case \"a version filter without a definition id\": expected a 400 \
         ValidationException, got {} {}",
        orphan_version.status,
        orphan_version.body
    );
}

/// The configurations the update round-trip runs over: rows differing in name, `rootGroup`
/// form, protocol (source: the `Protocol` enum, re-exported as `PROTOCOL_VALUES`), device
/// list size and the optional booleans.
fn update_rows() -> Vec<(String, Value, Value)> {
    let protocols = iotdeviceadvisor::ops::types::PROTOCOL_VALUES;
    vec![
        (
            "rename only".to_string(),
            json!({
                "suiteDefinitionName": "before-rename",
                "rootGroup": "g",
                "devicePermissionRoleArn": ROLE_ARN,
            }),
            json!({
                "suiteDefinitionName": "after-rename",
                "rootGroup": "g",
                "devicePermissionRoleArn": ROLE_ARN,
            }),
        ),
        (
            "new rootGroup and protocol".to_string(),
            json!({
                "suiteDefinitionName": "protocol-change",
                "rootGroup": "root/group/one",
                "devicePermissionRoleArn": ROLE_ARN,
                "protocol": protocols[0],
            }),
            json!({
                "suiteDefinitionName": "protocol-change",
                "rootGroup": "root/group/two",
                "devicePermissionRoleArn": ROLE_ARN,
                // The last `Protocol` the model enumerates, which the issue did not cite.
                "protocol": protocols[protocols.len() - 1],
            }),
        ),
        (
            "devices added up to the modelled maximum".to_string(),
            json!({
                "suiteDefinitionName": "device-growth",
                "rootGroup": "g",
                "devicePermissionRoleArn": ROLE_ARN,
                "devices": [],
            }),
            json!({
                "suiteDefinitionName": "device-growth",
                "rootGroup": "g",
                "devicePermissionRoleArn": ROLE_ARN,
                "isLongDurationTest": true,
                "intendedForQualification": false,
                // `DeviceUnderTestList` is `len 0..2`: two devices is the maximum.
                "devices": [
                    { "thingArn": thing_arn(3) },
                    { "deviceRoleArn": ROLE_ARN },
                ],
            }),
        ),
        (
            "becomes a qualification suite with an empty rootGroup".to_string(),
            json!({
                "suiteDefinitionName": "qualification-change",
                "rootGroup": "g",
                "devicePermissionRoleArn": ROLE_ARN,
            }),
            json!({
                "suiteDefinitionName": "qualification-change",
                "rootGroup": "",
                "intendedForQualification": true,
                "devicePermissionRoleArn": ROLE_ARN,
            }),
        ),
        (
            // `SuiteDefinitionName` is `len 1..256`: both published bounds.
            "renamed to the longest legal name".to_string(),
            json!({
                "suiteDefinitionName": "a",
                "rootGroup": "g",
                "devicePermissionRoleArn": ROLE_ARN,
            }),
            json!({
                "suiteDefinitionName": "n".repeat(256),
                "rootGroup": "g",
                "devicePermissionRoleArn": ROLE_ARN,
            }),
        ),
    ]
}

/// An update records a NEW configuration version, and every later read observes it: the
/// new version is the latest, the earlier version stays readable at its own label (serving
/// the one live configuration), and the identity of the definition does not move.
#[tokio::test]
async fn an_update_records_a_new_version_a_following_read_observes() {
    let emulator = Emulator::new();

    for (case, initial, replacement) in update_rows() {
        let created = emulator.create_definition(&initial, REGION).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: setup create failed with {} {}",
            created.status, created.body
        );
        let id = created.string("suiteDefinitionId");
        let arn = created.string("suiteDefinitionArn");
        let created_at = created
            .member("createdAt")
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: expected a createdAt timestamp"));

        let updated = emulator
            .call(
                "PATCH",
                &format!("/suiteDefinitions/{id}"),
                REGION,
                Some(json!({ "suiteDefinitionConfiguration": replacement })),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateSuiteDefinition, got {} {}",
            updated.status, updated.body
        );
        assert_eq!(
            updated.string("suiteDefinitionId"),
            id,
            "case {case:?}: an update must not move the definition's id"
        );
        assert_eq!(
            updated.string("suiteDefinitionArn"),
            arn,
            "case {case:?}: an update must not move the definition's ARN"
        );
        assert_eq!(
            updated.string("suiteDefinitionVersion"),
            "v2",
            "case {case:?}: an update records the next version, got {}",
            updated.body
        );
        assert_eq!(
            updated.member("suiteDefinitionName"),
            &replacement["suiteDefinitionName"],
            "case {case:?}: the name must come from the configuration just stored, got {}",
            updated.body
        );
        // For the one live record both timestamps are last-write stamps: an update moves
        // createdAt past the one the create reported.
        let updated_created_at = updated
            .member("createdAt")
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: expected a createdAt timestamp"));
        assert!(
            updated_created_at > created_at,
            "case {case:?}: an update must advance createdAt past {created_at}, got \
             {updated_created_at}"
        );
        assert!(
            updated.member("lastUpdatedAt").is_number(),
            "case {case:?}: expected a lastUpdatedAt timestamp, got {}",
            updated.body
        );

        let read = emulator
            .call("GET", &format!("/suiteDefinitions/{id}"), REGION, None)
            .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetSuiteDefinition, got {} {}",
            read.status, read.body
        );
        assert_eq!(
            read.string("latestVersion"),
            "v2",
            "case {case:?}: the version the update minted is the latest, got {}",
            read.body
        );
        assert_eq!(
            read.member("suiteDefinitionConfiguration"),
            &replacement,
            "case {case:?}: an unqualified read answers the configuration just stored"
        );

        // The version the create minted is still readable — and answers the one LIVE
        // configuration, not the content it was created with.
        let old = emulator
            .call(
                "GET",
                &format!("/suiteDefinitions/{id}?suiteDefinitionVersion=v1"),
                REGION,
                None,
            )
            .await;
        assert_eq!(
            old.member("suiteDefinitionConfiguration"),
            &replacement,
            "case {case:?}: version v1 must answer the CURRENT configuration (a definition \
             keeps one live configuration), got {}",
            old.body
        );

        // A second update records a third version.
        let again = emulator
            .call(
                "PATCH",
                &format!("/suiteDefinitions/{id}"),
                REGION,
                Some(json!({ "suiteDefinitionConfiguration": initial })),
            )
            .await;
        assert_eq!(
            again.string("suiteDefinitionVersion"),
            "v3",
            "case {case:?}: each update records the next version, got {}",
            again.body
        );

        // And the listing projects the name of the latest version.
        let listed = emulator
            .call("GET", "/suiteDefinitions?maxResults=50", REGION, None)
            .await;
        let entry = listed
            .member("suiteDefinitionInformationList")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|entry| entry["suiteDefinitionId"].as_str() == Some(id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: the definition must be listed"));
        assert_eq!(
            entry["suiteDefinitionName"],
            initial["suiteDefinitionName"],
            "case {case:?}: the listing projects the latest version's name, got {entry}"
        );
    }
}

/// Every lookup of something the store does not hold is refused with AWS's template,
/// `Unable to find resource with <member>: <value>`, built from the request.
#[tokio::test]
async fn a_missing_resource_is_refused_with_the_aws_not_found_template() {
    let emulator = Emulator::new();
    let known = definition_with_name(&emulator, "not-found-suite").await;
    let known_run = seed_finished_run(&emulator, &known, "v1", "ERROR", Duration::from_secs(10));

    // A definition created and then deleted: a read after the delete must be refused the
    // same way as one of an id that never existed.
    let deleted = definition_with_name(&emulator, "deleted-suite").await;
    let delete = emulator
        .call("DELETE", &format!("/suiteDefinitions/{deleted}"), REGION, None)
        .await;
    assert_eq!(
        delete.status, 200,
        "setup: the delete failed with {} {}",
        delete.status, delete.body
    );

    let update_body = Some(json!({
        "suiteDefinitionConfiguration": {
            "suiteDefinitionName": "x",
            "rootGroup": "",
            "intendedForQualification": true,
            "devicePermissionRoleArn": ROLE_ARN,
        },
    }));
    // A `UUID` is `len 12..36`: a 36-character one and one at the shortest legal length.
    let never_created = "1e8542c9-cb2a-4172-ae95-04aeb8a757da";
    let shortest = "abcdefghijkl";

    // (case, method, path, body, the message the refusal must carry)
    let rows: Vec<(String, &str, String, Option<Value>, String)> = vec![
        (
            "update of an id that was never created".to_string(),
            "PATCH",
            format!("/suiteDefinitions/{never_created}"),
            update_body.clone(),
            format!("Unable to find resource with suiteDefinitionId: {never_created}"),
        ),
        (
            "update of an id at the shortest legal length".to_string(),
            "PATCH",
            format!("/suiteDefinitions/{shortest}"),
            update_body.clone(),
            format!("Unable to find resource with suiteDefinitionId: {shortest}"),
        ),
        (
            "update of a deleted id".to_string(),
            "PATCH",
            format!("/suiteDefinitions/{deleted}"),
            update_body.clone(),
            format!("Unable to find resource with suiteDefinitionId: {deleted}"),
        ),
        (
            "read of an id that was never created".to_string(),
            "GET",
            format!("/suiteDefinitions/{never_created}"),
            None,
            format!("Unable to find resource with suiteDefinitionId: {never_created}"),
        ),
        (
            "read of a deleted id".to_string(),
            "GET",
            format!("/suiteDefinitions/{deleted}"),
            None,
            format!("Unable to find resource with suiteDefinitionId: {deleted}"),
        ),
        (
            "read of a version the definition never had".to_string(),
            "GET",
            format!("/suiteDefinitions/{known}?suiteDefinitionVersion=v9"),
            None,
            "Unable to find resource with suiteDefinitionVersion: v9".to_string(),
        ),
        (
            "read of a run that was never started".to_string(),
            "GET",
            format!("/suiteDefinitions/{known}/suiteRuns/{never_created}"),
            None,
            format!("Unable to find resource with suiteRunId: {never_created}"),
        ),
        (
            "read of a known run under the wrong definition".to_string(),
            "GET",
            format!("/suiteDefinitions/{never_created}/suiteRuns/{known_run}"),
            None,
            format!("Unable to find resource with suiteRunId: {known_run}"),
        ),
        (
            "stop of a run that was never started".to_string(),
            "POST",
            format!("/suiteDefinitions/{known}/suiteRuns/{never_created}/stop"),
            None,
            format!("Unable to find resource with suiteRunId: {never_created}"),
        ),
    ];

    for (case, method, path, body, expected_message) in rows {
        let answer = emulator.call(method, &path, REGION, body).await;
        assert_eq!(
            (answer.status, answer.error_code.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?}: expected a 404 ResourceNotFoundException, got {} {:?} {}",
            answer.status,
            answer.error_code,
            answer.body
        );
        assert_eq!(
            answer.member("message").as_str(),
            Some(expected_message.as_str()),
            "case {case:?}: expected the AWS not-found template, got {}",
            answer.body
        );
    }

    // The same rule holds where the operation models no not-found error: `StartSuiteRun`
    // declares only `ValidationException` (`smithy_query` lists no
    // `ResourceNotFoundException` for it), so the code is that one — and the message is
    // still built from the request the same way.
    let start = emulator
        .call(
            "POST",
            &format!("/suiteDefinitions/{never_created}/suiteRuns"),
            REGION,
            Some(json!({
                "suiteRunConfiguration": { "primaryDevice": { "thingArn": thing_arn(2) } },
            })),
        )
        .await;
    assert_eq!(
        (start.status, start.error_code.as_deref()),
        (400, Some("ValidationException")),
        "case \"start on an unknown definition\": expected the modelled client-input \
         error, got {} {}",
        start.status,
        start.body
    );
    assert_eq!(
        start.member("message").as_str(),
        Some(format!("Unable to find resource with suiteDefinitionId: {never_created}").as_str()),
        "case \"start on an unknown definition\": expected the AWS not-found template, \
         got {}",
        start.body
    );
}

/// An empty `rootGroup` is accepted exactly when the suite is intended for qualification,
/// and the refusal carries AWS's template — for EVERY operation that takes a
/// `suiteDefinitionConfiguration`, create and update alike.
#[tokio::test]
async fn an_empty_root_group_is_refused_with_the_aws_template_by_create_and_update() {
    let emulator = Emulator::new();

    // AWS's template for this rejection, recorded verbatim.
    let expected_message = "Invalid Request: rootGroup cannot be empty for non-qualification \
                            test suite";

    // (case, rootGroup, intendedForQualification, expected status)
    let rows: Vec<(&str, &str, Option<bool>, u16)> = vec![
        ("empty rootGroup, not qualification", "", Some(false), 400),
        ("empty rootGroup, flag absent", "", None, 400),
        ("empty rootGroup, qualification", "", Some(true), 200),
        ("non-empty rootGroup, not qualification", "g", Some(false), 200),
        ("non-empty rootGroup, flag absent", "g", None, 200),
        ("non-empty rootGroup, qualification", "g", Some(true), 200),
    ];

    for (case, root_group, qualification, expected) in rows {
        let mut configuration = json!({
            "suiteDefinitionName": format!("root-group-rule-{case}"),
            "rootGroup": root_group,
            "devicePermissionRoleArn": ROLE_ARN,
        });
        if let Some(qualification) = qualification {
            configuration["intendedForQualification"] = json!(qualification);
        }

        // CreateSuiteDefinition.
        let created = emulator.create_definition(&configuration, REGION).await;
        assert_eq!(
            created.status, expected,
            "case {case:?} on CreateSuiteDefinition: expected HTTP {expected}, got {} {}",
            created.status, created.body
        );
        if expected == 400 {
            assert_eq!(
                (created.status, created.error_code.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?} on CreateSuiteDefinition: expected a ValidationException, \
                 got {}",
                created.body
            );
            assert_eq!(
                created.member("message").as_str(),
                Some(expected_message),
                "case {case:?} on CreateSuiteDefinition: expected AWS's template, got {}",
                created.body
            );
        }

        // UpdateSuiteDefinition, on a definition that already exists: the SAME rule, so
        // the same refusal and the same message.
        let subject = definition_with_name(&emulator, &format!("update-subject-{case}")).await;
        let updated = emulator
            .call(
                "PATCH",
                &format!("/suiteDefinitions/{subject}"),
                REGION,
                Some(json!({ "suiteDefinitionConfiguration": configuration })),
            )
            .await;
        assert_eq!(
            updated.status, expected,
            "case {case:?} on UpdateSuiteDefinition: expected HTTP {expected}, got {} {}",
            updated.status, updated.body
        );
        if expected == 400 {
            assert_eq!(
                (updated.status, updated.error_code.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?} on UpdateSuiteDefinition: expected a ValidationException, \
                 got {}",
                updated.body
            );
            assert_eq!(
                updated.member("message").as_str(),
                Some(expected_message),
                "case {case:?} on UpdateSuiteDefinition: expected AWS's template, got {}",
                updated.body
            );
            // A refused update left the stored definition where it was.
            let read = emulator
                .call(
                    "GET",
                    &format!("/suiteDefinitions/{subject}"),
                    REGION,
                    None,
                )
                .await;
            assert_eq!(
                read.string("latestVersion"),
                "v1",
                "case {case:?}: a refused update must not record a version, got {}",
                read.body
            );
        }
    }
}
