// SPDX-License-Identifier: CC0-1.0

//! How m2 words a request it will not serve, driven over the wire.
//!
//! Four rules, each about the whole service rather than about the one operation a
//! recording happened to cover, and each checked as a TABLE whose rows differ along what
//! the rule generalises over:
//!
//! 1. **A clause about a LIST member states the list the caller sent** — its elements in
//!    order, comma-separated, in brackets — because the message is the only place the
//!    refused input is reported back (m2's `ValidationException` carries no `fieldList`
//!    for a boundary refusal), so a caller has to be able to read its own values out of
//!    it. Rows: the two operations that bind `EntityNameList` and the one that binds
//!    `IdentifierList` (`smithy_query(query="shape",shape="EntityNameList,IdentifierList")`
//!    — both `@length(min:1,max:10)`), element counts from 11 to 20, element values from
//!    the two characters the `EntityName` pattern's minimum admits to the 60 of its
//!    maximum, and an order no sort produces.
//! 2. **An enumerated type's permitted value set is stated SORTED**, whatever order the
//!    model declares it in, because the enumeration is what a caller reads to pick a
//!    value. Rows: every enum-typed input member this service's model binds that can be
//!    driven from the wire, over three different enums.
//! 3. **A not-found refusal states the text the service words for THAT operation's
//!    lookup**, built from the request where the service interpolates the id — never one
//!    sentence shared by every lookup. Rows: the application-by-id lookup over several
//!    ids and over both operations that perform it, and the application-version lookup,
//!    whose own sentence is fixed.
//! 4. **Only a LIST has a value a clause states**: a clause about a MAP over its
//!    `@length` keeps the framework's own summary, because no recording of this service
//!    shows AWS stating a map's value in a refusal.
//!
//! Every assertion goes over the same front door a client uses:
//! [`MultiServiceRouter::dispatch`] with a buffered restJson1 request, so the URI
//! bindings, the refusal envelope and the `x-amzn-errortype` header are all exercised.

use serde_json::{json, Value};

// The one wire harness of this crate's suites: the router, the request and the answer.
use super::wire::{call, router, AUTHORIZATION};

/// The error code and status a modelled-constraint refusal carries: `ValidationException`,
/// `@httpError(400)`. Written out here rather than read from the service's own constant so
/// the value itself is under test.
const VALIDATION: &str = "ValidationException";

/// The error code a not-found refusal carries: `ResourceNotFoundException`,
/// `@httpError(404)`.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The `@length` maximum both `EntityNameList` and `IdentifierList` declare. Source:
/// `smithy_query(query="shape",shape="EntityNameList,IdentifierList")`.
const LIST_MAX: usize = 10;

/// `values` as repeated occurrences of the query parameter `name`.
fn repeated_query(name: &str, values: &[String]) -> String {
    let joined: Vec<String> = values
        .iter()
        .map(|value| format!("{name}={value}"))
        .collect();
    format!("?{}", joined.join("&"))
}

/// `count` names of the recorded probe form (`probe-name-0`, `probe-name-1`, …), all
/// within the `EntityName` pattern.
fn probe_names(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("probe-name-{index}")).collect()
}

/// The clause a single-violation refusal states, as the envelope wraps it.
fn one_clause(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// One request a list rule is checked on.
struct ListCase {
    /// What makes this row different from the others.
    case: &'static str,
    method: &'static str,
    /// The path the list is bound on, without its query string.
    path: &'static str,
    /// The `@httpQuery` parameter the list binds to, which is also the member path a
    /// clause names it by.
    member: &'static str,
    /// The values the request carries, in the order it carries them.
    values: Vec<String>,
}

/// Rule 1: a clause about a list member over its `@length` maximum states the list the
/// caller sent — every element, in order, bracketed — whatever operation bound it, however
/// many elements it carries and whatever those elements are.
#[tokio::test]
async fn a_clause_about_a_list_states_the_list_the_caller_sent() {
    // The element forms: the recorded probe names, the shortest name the `EntityName`
    // pattern admits (two characters), the longest (60), and an order no sort produces.
    let mut varied = probe_names(LIST_MAX + 1);
    varied.reverse();
    let long_names: Vec<String> = (0..LIST_MAX + 1)
        .map(|index| format!("N{index}{}", "x".repeat(57)))
        .collect();
    let short_names: Vec<String> = (0..LIST_MAX + 2)
        .map(|index| format!("a{index}"))
        .collect();

    let cases = vec![
        ListCase {
            case: "the recorded ListApplications names",
            method: "GET",
            path: "/applications",
            member: "names",
            values: probe_names(LIST_MAX + 1),
        },
        ListCase {
            case: "the same names on ListEnvironments",
            method: "GET",
            path: "/environments",
            member: "names",
            values: probe_names(LIST_MAX + 1),
        },
        ListCase {
            case: "twice the maximum, on ListApplications",
            method: "GET",
            path: "/applications",
            member: "names",
            values: probe_names(LIST_MAX * 2),
        },
        ListCase {
            case: "names in reverse order",
            method: "GET",
            path: "/applications",
            member: "names",
            values: varied,
        },
        ListCase {
            case: "the longest names the pattern admits",
            method: "GET",
            path: "/environments",
            member: "names",
            values: long_names,
        },
        ListCase {
            case: "the shortest names the pattern admits",
            method: "GET",
            path: "/applications",
            member: "names",
            values: short_names,
        },
        // A member the issue did not cite, on a third operation and a second list shape:
        // `ListBatchJobExecutions$executionIds` -> `IdentifierList`.
        ListCase {
            case: "executionIds on ListBatchJobExecutions",
            method: "GET",
            path: "/applications/a1b2c3/batch-job-executions",
            member: "executionIds",
            values: probe_names(LIST_MAX + 1),
        },
    ];

    let router = router(false);
    for ListCase {
        case,
        method,
        path,
        member,
        values,
    } in cases
    {
        let answer = call(
            &router,
            method,
            &format!("{path}{}", repeated_query(member, &values)),
            None,
            Some(AUTHORIZATION),
        )
        .await;

        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, VALIDATION,
            "case {case:?}: expected x-amzn-errortype {VALIDATION}, got {:?}",
            answer.error_type
        );
        let expected = one_clause(&format!(
            "Value '[{}]' at '{member}' failed to satisfy constraint: Member must have \
             length less than or equal to {LIST_MAX}",
            values.join(", ")
        ));
        assert_eq!(
            answer.message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            answer.message()
        );
    }
}

/// Two broken constraints in one request are two clauses under the plural envelope, and
/// the list is still stated as the caller sent it.
#[tokio::test]
async fn two_broken_constraints_are_two_clauses() {
    let names = probe_names(LIST_MAX + 1);
    let answer = call(
        &router(false),
        "GET",
        &format!(
            "/applications{}&maxResults=0",
            repeated_query("names", &names)
        ),
        None,
        Some(AUTHORIZATION),
    )
    .await;

    assert_eq!(
        answer.status, 400,
        "case names and maxResults: expected HTTP 400, got {} {:?}",
        answer.status, answer.body
    );
    let message = answer.message();
    assert!(
        message.starts_with("2 validation errors detected: "),
        "case names and maxResults: expected the plural envelope, got {message:?}"
    );
    // The `maxResults` bound is the model's own
    // (`smithy_query(query="operation",operation="ListApplications")`: range 1..2000).
    assert!(
        message.contains(
            "Value '0' at 'maxResults' failed to satisfy constraint: Member must have value \
             greater than or equal to 1"
        ),
        "case names and maxResults: expected the maxResults clause, got {message:?}"
    );
    assert!(
        message.contains(&format!("Value '[{}]' at 'names'", names.join(", "))),
        "case names and maxResults: expected the names clause to state the list, got \
         {message:?}"
    );
}

/// One request an enum rule is checked on.
struct EnumCase {
    case: &'static str,
    method: &'static str,
    path: &'static str,
    body: Option<fn() -> Value>,
    /// The member path a clause names.
    member: &'static str,
    /// The value set as the refusal states it: the model's values, sorted.
    expected_set: &'static str,
}

/// Rule 2: a value outside an enumerated type is refused with the permitted set stated in
/// sorted order — on every enum-typed input member, whatever order the model declares the
/// values in.
///
/// The expected sets are written out from the model rather than computed, so the order
/// itself is under test:
/// `smithy_query(query="shape",shape="EngineType,NetworkType,BatchJobExecutionStatus")`
/// declares `EngineType` as `[microfocus, bluage]`, `NetworkType` as `[ipv4, dual]` and
/// `BatchJobExecutionStatus` as `[Submitting, Holding, Dispatching, Running, Cancelling,
/// Cancelled, Succeeded, Failed, Purged, Succeeded With Warning]`.
#[tokio::test]
async fn an_enum_refusal_states_the_permitted_values_sorted() {
    /// The value every row submits: outside every enum this service declares.
    const OUTSIDE: &str = "not-a-real-engine";

    let cases: &[EnumCase] = &[
        EnumCase {
            case: "the recorded ListEngineVersions engineType",
            method: "GET",
            path: "/engine-versions?engineType=not-a-real-engine",
            body: None,
            member: "engineType",
            expected_set: "[bluage, microfocus]",
        },
        EnumCase {
            case: "engineType on ListEnvironments",
            method: "GET",
            path: "/environments?engineType=not-a-real-engine",
            body: None,
            member: "engineType",
            expected_set: "[bluage, microfocus]",
        },
        EnumCase {
            case: "engineType in a CreateApplication body",
            method: "POST",
            path: "/applications",
            body: Some(|| {
                json!({
                    "name": "EnumCase",
                    "engineType": OUTSIDE,
                    "definition": {"content": "{\"template-version\":\"2.0\"}"},
                })
            }),
            member: "engineType",
            expected_set: "[bluage, microfocus]",
        },
        // A second enum, which the issue did not cite: `NetworkType`, declared
        // `[ipv4, dual]`.
        EnumCase {
            case: "networkType in a CreateEnvironment body",
            method: "POST",
            path: "/environments",
            body: Some(|| {
                json!({
                    "name": "EnumCase",
                    "instanceType": "M2.m5.large",
                    "engineType": "bluage",
                    "networkType": OUTSIDE,
                })
            }),
            member: "networkType",
            expected_set: "[dual, ipv4]",
        },
        // A third, whose ten values no two orderings agree on.
        EnumCase {
            case: "status on ListBatchJobExecutions",
            method: "GET",
            path: "/applications/a1b2c3/batch-job-executions?status=not-a-real-engine",
            body: None,
            member: "status",
            expected_set: "[Cancelled, Cancelling, Dispatching, Failed, Holding, Purged, \
                            Running, Submitting, Succeeded, Succeeded With Warning]",
        },
    ];

    let router = router(false);
    for case in cases {
        let answer = call(
            &router,
            case.method,
            case.path,
            case.body.map(|build| build()),
            Some(AUTHORIZATION),
        )
        .await;

        assert_eq!(
            answer.status, 400,
            "case {:?}: expected HTTP 400, got {} {:?}",
            case.case, answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, VALIDATION,
            "case {:?}: expected x-amzn-errortype {VALIDATION}, got {:?}",
            case.case, answer.error_type
        );
        let expected = one_clause(&format!(
            "Value '{OUTSIDE}' at '{}' failed to satisfy constraint: Member must satisfy \
             enum value set: {}",
            case.member, case.expected_set
        ));
        assert_eq!(
            answer.message(),
            expected,
            "case {:?}: expected {expected:?}, got {:?}",
            case.case,
            answer.message()
        );
    }
}

/// The ids rule 3 is checked on: every one of them within the modelled `Identifier`
/// pattern (`^\S{1,80}$`, from
/// `smithy_query(query="operation",operation="GetApplication")`) and naming no application.
fn unknown_ids() -> [(&'static str, &'static str); 4] {
    [
        ("the recorded id", "00000000000000000000000000"),
        ("a minted-length hex id", "0123456789abcdef0123456789abcdef"),
        ("a single character", "x"),
        ("the longest id the pattern admits", "9e07e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4"),
    ]
}

/// Rule 3, first half: the application-by-id lookup states the id the request asked for,
/// in the form the service words it — on every operation that performs that lookup.
#[tokio::test]
async fn the_application_lookup_states_the_id_it_was_asked_for() {
    let router = router(false);
    for (case, id) in unknown_ids() {
        // Both operations that address an application by its id and model a not-found:
        // `GET /applications/{applicationId}` and `PATCH /applications/{applicationId}`
        // (`smithy_query(query="operation",operation="GetApplication,UpdateApplication")`).
        let requests: [(&str, &str, Option<Value>); 2] = [
            ("GetApplication", "GET", None),
            (
                "UpdateApplication",
                "PATCH",
                Some(json!({
                    "currentApplicationVersion": 1,
                    "definition": {"content": "{\"template-version\":\"2.0\"}"},
                })),
            ),
        ];
        for (operation, method, body) in requests {
            let answer = call(
                &router,
                method,
                &format!("/applications/{id}"),
                body,
                Some(AUTHORIZATION),
            )
            .await;

            assert_eq!(
                answer.status, 404,
                "case {case:?} on {operation}: expected HTTP 404, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_type, NOT_FOUND,
                "case {case:?} on {operation}: expected x-amzn-errortype {NOT_FOUND}, got \
                 {:?}",
                answer.error_type
            );
            let expected = format!("app {id} not found");
            assert_eq!(
                answer.message(),
                expected,
                "case {case:?} on {operation}: expected {expected:?}, got {:?}",
                answer.message()
            );
        }
    }
}

/// Rule 3, second half: the application-version lookup states its OWN sentence — the same
/// one however the lookup failed, and never the application lookup's id-bearing form.
#[tokio::test]
async fn the_application_version_lookup_states_its_own_sentence() {
    /// What that lookup answers, recorded (fixture `m2-application-not-found-errors`,
    /// step `task#2`).
    const VERSION_NOT_FOUND: &str = "Application not found.";

    let router = router(false);
    for (case, id) in unknown_ids() {
        let answer = call(
            &router,
            "GET",
            &format!("/applications/{id}/versions/1"),
            None,
            Some(AUTHORIZATION),
        )
        .await;

        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, NOT_FOUND,
            "case {case:?}: expected x-amzn-errortype {NOT_FOUND}, got {:?}",
            answer.error_type
        );
        assert_eq!(
            answer.message(),
            VERSION_NOT_FOUND,
            "case {case:?}: expected {VERSION_NOT_FOUND:?}, got {:?}",
            answer.message()
        );
        assert_ne!(
            answer.message(),
            format!("app {id} not found"),
            "case {case:?}: the two lookups must not share one sentence"
        );
    }

    // A version of an application that IS there: the same lookup, failing on the version
    // rather than on the application, so the same sentence — and the round trip proves
    // the reader answers from the store rather than refusing everything.
    let created = call(
        &router,
        "POST",
        "/applications",
        Some(json!({
            "name": "VersionLookup",
            "engineType": "bluage",
            "definition": {"content": "{\"template-version\":\"2.0\"}"},
        })),
        Some(AUTHORIZATION),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case a created application: expected HTTP 200, got {} {:?}",
        created.status, created.body
    );
    let id = created
        .body
        .get("applicationId")
        .and_then(Value::as_str)
        .expect("CreateApplication answers an applicationId")
        .to_string();

    let recorded = call(
        &router,
        "GET",
        &format!("/applications/{id}/versions/1"),
        None,
        Some(AUTHORIZATION),
    )
    .await;
    assert_eq!(
        recorded.status, 200,
        "case the version the create recorded: expected HTTP 200, got {} {:?}",
        recorded.status, recorded.body
    );

    for (case, version) in [("the next version", 2), ("a far later version", 99)] {
        let missing = call(
            &router,
            "GET",
            &format!("/applications/{id}/versions/{version}"),
            None,
            Some(AUTHORIZATION),
        )
        .await;
        assert_eq!(
            missing.status, 404,
            "case {case:?}: expected HTTP 404, got {} {:?}",
            missing.status, missing.body
        );
        assert_eq!(
            missing.message(),
            VERSION_NOT_FOUND,
            "case {case:?}: expected {VERSION_NOT_FOUND:?}, got {:?}",
            missing.message()
        );
    }

    // And the application itself still reads back, so the id-bearing sentence above is a
    // refusal of an id that names nothing rather than of every id.
    let read = call(
        &router,
        "GET",
        &format!("/applications/{id}"),
        None,
        Some(AUTHORIZATION),
    )
    .await;
    assert_eq!(
        read.status, 200,
        "case the created application: expected HTTP 200, got {} {:?}",
        read.status, read.body
    );
}

/// Rule 4: a LIST is the only collection a clause states the value of. A clause about a
/// MAP over its `@length` keeps the framework's own summary of it: AWS is not recorded
/// stating a map's value in a refusal of this service, so nothing is invented in its
/// place.
#[tokio::test]
async fn a_clause_about_a_map_states_no_rendering() {
    // `CreateApplication$tags` -> `TagMap`, `@length(min:0,max:200)`
    // (`smithy_query(query="operation",operation="CreateApplication")`).
    const MAX_TAGS: usize = 200;
    let tags: serde_json::Map<String, Value> = (0..MAX_TAGS + 1)
        .map(|index| (format!("key-{index}"), json!(format!("value-{index}"))))
        .collect();

    let answer = call(
        &router(false),
        "POST",
        "/applications",
        Some(json!({
            "name": "TagMapCase",
            "engineType": "bluage",
            "definition": {"content": "{\"template-version\":\"2.0\"}"},
            "tags": tags,
        })),
        Some(AUTHORIZATION),
    )
    .await;

    assert_eq!(
        answer.status, 400,
        "case a map over its maximum: expected HTTP 400, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_type, VALIDATION,
        "case a map over its maximum: expected x-amzn-errortype {VALIDATION}, got {:?}",
        answer.error_type
    );
    let message = answer.message();
    assert!(
        message.contains("at 'tags'"),
        "case a map over its maximum: expected the clause to name the member, got \
         {message:?}"
    );
    assert!(
        message.contains(&format!(
            "Member must have length less than or equal to {MAX_TAGS}"
        )),
        "case a map over its maximum: expected the modelled bound, got {message:?}"
    );
    assert!(
        !message.contains("Value '["),
        "case a map over its maximum: only a list has a value a clause states, got \
         {message:?}"
    );
}
