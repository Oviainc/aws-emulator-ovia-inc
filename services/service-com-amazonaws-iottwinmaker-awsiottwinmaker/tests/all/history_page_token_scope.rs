// SPDX-License-Identifier: CC0-1.0

//! What a `GetPropertyValueHistory` page's `nextToken` resumes, end to end over
//! HTTP.
//!
//! RULE under test: a page that resumes at an ORDINAL is only resumed by a
//! request whose own members decide the same sequence that ordinal was counted
//! in — so a history token is resumed by a request carrying the scope, the
//! selection, the window, the order, the `propertyFilters` and the
//! `interpolation` it was minted under, and refused with a 400
//! `ValidationException` by one that decides another sequence of values, rather
//! than having the ordinal applied to values it was never counted in.
//!
//! The rows of the tables below differ along what the rule generalises over:
//! WHICH member the resuming request changes (a filter's property, its operator,
//! its comparison value, how many filters there are, whether there are any
//! filters at all, whether the series is resampled, how wide the resampling
//! interval is, the time direction, a window bound, the selection) and, on the
//! other side of the rule, which SPELLING of the same query still resumes (the
//! whole request repeated, a default spelled out, the filters listed in another
//! order, a filter repeated, a selection listed in another order, another page
//! size) — because a rule that refused every change of spelling would break the
//! paginator that resumes by repeating the whole request.

use serde_json::{json, Value};

use crate::harness::{data_writer_functions, Emulator, Resp};

/// The workspace every case below reads from.
const WORKSPACE: &str = "historytoken";

/// The component type whose time series properties carry the values paged over.
const SENSOR: &str = "com.example.sensorHistory";

/// Instants one minute apart, as the ISO 8601 `Time` a write carries. Written
/// out rather than computed so a failing assertion names the instant it is
/// about.
const T0: &str = "2026-03-01T00:00:00Z";
/// See [`T0`].
const T1: &str = "2026-03-01T00:01:00Z";
/// See [`T0`].
const T2: &str = "2026-03-01T00:02:00Z";
/// See [`T0`].
const T3: &str = "2026-03-01T00:03:00Z";

/// The workspace, component type and entity every case below pages over: four
/// values of one time series property, each a different number, so a page that
/// repeats or skips one is visible in the values it reports.
async fn seed(emulator: &Emulator) {
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );

    let created = emulator
        .create_component_type(
            WORKSPACE,
            SENSOR,
            json!({
                "propertyDefinitions": {
                    "Temp": {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true},
                    // A second time series property, so a filter can name another
                    // property of the same scope.
                    "Count": {"dataType": {"type": "INTEGER"}, "isTimeSeries": true},
                },
                // The write connector the seeded values are set THROUGH.
                "functions": data_writer_functions(),
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateComponentType {SENSOR:?}: expected 200, got {created:?}"
    );

    let created = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": "unit-1",
                "entityName": "unit-1",
                "components": {"Sensor": {"componentTypeId": SENSOR}},
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateEntity unit-1: expected 200, got {created:?}"
    );

    let written = emulator
        .batch_put_property_values(
            WORKSPACE,
            json!([{
                "entityPropertyReference": {
                    "entityId": "unit-1",
                    "componentName": "Sensor",
                    "propertyName": "Temp",
                },
                "propertyValues": [
                    {"time": T0, "value": {"doubleValue": 10.0}},
                    {"time": T1, "value": {"doubleValue": 20.0}},
                    {"time": T2, "value": {"doubleValue": 30.0}},
                    {"time": T3, "value": {"doubleValue": 40.0}},
                ],
            }]),
        )
        .await;
    assert_eq!(
        written.status, 200,
        "BatchPutPropertyValues seeding the series: expected 200, got {written:?}"
    );
}

/// Every `(time, value)` pair a history answer reports, series by series, in the
/// order it reports them.
fn reported(response: &Resp) -> Vec<(String, Value)> {
    response
        .member("propertyValues")
        .as_array()
        .map(|series| {
            series
                .iter()
                .flat_map(|one| one["values"].as_array().cloned().unwrap_or_default())
                .map(|value| {
                    (
                        value["time"].as_str().unwrap_or_default().to_string(),
                        value["value"].clone(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A history request over the seeded component, carrying the members of `query`
/// and resuming at `next_token` where one is given.
fn history_request(query: &Value, max_results: Option<u32>, next_token: Option<&str>) -> Value {
    let mut body = json!({"entityId": "unit-1", "componentName": "Sensor"});
    if let Some(members) = query.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    if let Some(max_results) = max_results {
        body["maxResults"] = json!(max_results);
    }
    if let Some(next_token) = next_token {
        body["nextToken"] = json!(next_token);
    }
    body
}

/// The filters the raw-series token below is minted under: two every seeded
/// value passes, so the sequence is the whole series and a shorter one is the
/// resuming request's doing.
fn minting_filters() -> Value {
    json!([
        {"propertyName": "Temp", "operator": ">=", "value": {"doubleValue": 10.0}},
        {"propertyName": "Temp", "operator": "<=", "value": {"doubleValue": 40.0}},
    ])
}

/// The query the raw-series token below is minted by: the whole window, the one
/// selected property, and the filters every seeded value passes.
fn minting_query() -> Value {
    json!({
        "selectedProperties": ["Temp"],
        "startTime": T0,
        "endTime": T3,
        "propertyFilters": minting_filters(),
    })
}

/// The query the interpolated token below is minted by: the same window
/// resampled once a minute, which is a point per seeded value.
fn interpolated_query() -> Value {
    json!({
        "selectedProperties": ["Temp"],
        "startTime": T0,
        "endTime": T3,
        "interpolation": {"interpolationType": "LINEAR", "intervalInSeconds": 60},
    })
}

/// One resume of a history page: the query the resuming request carries, and
/// whether it is the one the token's ordinal was counted under.
#[derive(Debug)]
struct Resume {
    /// What this row is about, quoted by every assertion it makes.
    what: &'static str,
    query: Value,
    /// `true` where this query is the minting one — however it is spelled — so
    /// the page resumes at the value after the first page's last; `false` where
    /// it decides another sequence, so the ordinal would land in values it was
    /// not counted in and the token is refused instead.
    resumes: bool,
}

/// Every spelling of the minting query that still names the same sequence of
/// values, and every change that names another one.
fn resumes() -> Vec<Resume> {
    let lower = json!({"propertyName": "Temp", "operator": ">=", "value": {"doubleValue": 10.0}});
    let upper = json!({"propertyName": "Temp", "operator": "<=", "value": {"doubleValue": 40.0}});
    vec![
        Resume {
            what: "the whole request repeated, which is what an SDK paginator does",
            query: minting_query(),
            resumes: true,
        },
        Resume {
            what: "the ASCENDING default spelled out, which is the direction it was read in",
            query: json!({
                "selectedProperties": ["Temp"],
                "startTime": T0,
                "endTime": T3,
                "orderByTime": "ASCENDING",
                "propertyFilters": minting_filters(),
            }),
            resumes: true,
        },
        Resume {
            what: "the two filters listed in the other order, which is the same conjunction",
            query: json!({
                "selectedProperties": ["Temp"],
                "startTime": T0,
                "endTime": T3,
                "propertyFilters": [upper, lower],
            }),
            resumes: true,
        },
        Resume {
            what: "a filter repeated, which leaves the conjunction the values pass unchanged",
            query: json!({
                "selectedProperties": ["Temp"],
                "startTime": T0,
                "endTime": T3,
                "propertyFilters": [lower, upper, lower],
            }),
            resumes: true,
        },
        Resume {
            what: "the one selected property repeated, which selects the same series",
            query: json!({
                "selectedProperties": ["Temp", "Temp"],
                "startTime": T0,
                "endTime": T3,
                "propertyFilters": minting_filters(),
            }),
            resumes: true,
        },
        Resume {
            what: "the filters dropped altogether, which widens the sequence",
            query: json!({
                "selectedProperties": ["Temp"],
                "startTime": T0,
                "endTime": T3,
            }),
            resumes: false,
        },
        Resume {
            what: "one filter dropped",
            query: json!({
                "selectedProperties": ["Temp"],
                "startTime": T0,
                "endTime": T3,
                "propertyFilters": [lower],
            }),
            resumes: false,
        },
        Resume {
            what: "a filter's comparison value changed, which shortens the sequence past the \
                   ordinal",
            query: json!({
                "selectedProperties": ["Temp"],
                "startTime": T0,
                "endTime": T3,
                "propertyFilters": [
                    {"propertyName": "Temp", "operator": ">=", "value": {"doubleValue": 35.0}},
                    upper,
                ],
            }),
            resumes: false,
        },
        Resume {
            what: "a filter's operator changed, which drops the value at the bound",
            query: json!({
                "selectedProperties": ["Temp"],
                "startTime": T0,
                "endTime": T3,
                "propertyFilters": [
                    {"propertyName": "Temp", "operator": ">", "value": {"doubleValue": 10.0}},
                    upper,
                ],
            }),
            resumes: false,
        },
        Resume {
            what: "a filter on another property, which is another conjunction even where it \
                   happens to keep the same values",
            query: json!({
                "selectedProperties": ["Temp"],
                "startTime": T0,
                "endTime": T3,
                "propertyFilters": [
                    lower,
                    upper,
                    {"propertyName": "Count", "operator": "!=", "value": {"integerValue": 7}},
                ],
            }),
            resumes: false,
        },
        Resume {
            what: "a filter added that leaves the sequence shorter than the ordinal",
            query: json!({
                "selectedProperties": ["Temp"],
                "startTime": T0,
                "endTime": T3,
                "propertyFilters": [
                    lower,
                    upper,
                    {"propertyName": "Temp", "operator": ">", "value": {"doubleValue": 35.0}},
                ],
            }),
            resumes: false,
        },
        Resume {
            what: "an interpolation added, which replaces the values with resampled points",
            query: json!({
                "selectedProperties": ["Temp"],
                "startTime": T0,
                "endTime": T3,
                "propertyFilters": minting_filters(),
                "interpolation": {"interpolationType": "LINEAR", "intervalInSeconds": 30},
            }),
            resumes: false,
        },
        Resume {
            what: "the time direction flipped, which reverses the sequence",
            query: json!({
                "selectedProperties": ["Temp"],
                "startTime": T0,
                "endTime": T3,
                "orderByTime": "DESCENDING",
                "propertyFilters": minting_filters(),
            }),
            resumes: false,
        },
        Resume {
            what: "the window's start moved, which drops the values before it",
            query: json!({
                "selectedProperties": ["Temp"],
                "startTime": T1,
                "endTime": T3,
                "propertyFilters": minting_filters(),
            }),
            resumes: false,
        },
        Resume {
            what: "another property selected, which is another series altogether",
            query: json!({
                "selectedProperties": ["Count"],
                "startTime": T0,
                "endTime": T3,
                "propertyFilters": minting_filters(),
            }),
            resumes: false,
        },
    ]
}

/// RULE: a history `nextToken` carries an ordinal in ONE sequence of values, so
/// it is resumed only by the query that decides that sequence and refused by
/// every query that decides another one.
#[tokio::test]
async fn a_history_next_token_is_bound_to_the_query_that_produced_it() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // The whole sequence the minting query decides, read in ONE page, so the
    // values a resumed page has to carry are that sequence's own rather than a
    // literal written down here.
    let whole = emulator
        .get_property_value_history(WORKSPACE, history_request(&minting_query(), None, None))
        .await;
    assert_eq!(
        whole.status, 200,
        "the whole sequence: expected 200, got {whole:?}"
    );
    let sequence = reported(&whole);
    assert_eq!(
        sequence.len(),
        4,
        "the whole sequence: expected a value per seeded instant, got {whole:?}"
    );

    let minted = emulator
        .get_property_value_history(WORKSPACE, history_request(&minting_query(), Some(2), None))
        .await;
    assert_eq!(
        minted.status, 200,
        "the minting page: expected 200, got {minted:?}"
    );
    assert_eq!(
        reported(&minted),
        sequence[..2].to_vec(),
        "the minting page: expected the sequence's first two values, got {minted:?}"
    );
    let token = minted
        .member("nextToken")
        .as_str()
        .unwrap_or_else(|| {
            panic!("the minting page left values over, so it must mint a token: {minted:?}")
        })
        .to_string();

    for Resume {
        what,
        query,
        resumes,
    } in resumes()
    {
        let page = emulator
            .get_property_value_history(WORKSPACE, history_request(&query, Some(2), Some(&token)))
            .await;
        if resumes {
            assert_eq!(page.status, 200, "case {what:?}: expected 200, got {page:?}");
            assert_eq!(
                reported(&page),
                sequence[2..].to_vec(),
                "case {what:?}: expected the values the minting page left over, got {page:?}"
            );
            assert!(
                page.member("nextToken").is_null(),
                "case {what:?}: expected the walk to end with those values, got {page:?}"
            );
        } else {
            assert_eq!(page.status, 400, "case {what:?}: expected 400, got {page:?}");
            assert_eq!(
                page.error_type.as_deref(),
                Some("ValidationException"),
                "case {what:?}: expected a ValidationException, got {page:?}"
            );
            assert!(
                reported(&page).is_empty(),
                "case {what:?}: expected no values at all, got {page:?}"
            );
        }
    }
}

/// One resume of an INTERPOLATED history page, the same rule from the other
/// starting point: a token minted over resampled points.
#[derive(Debug)]
struct InterpolatedResume {
    what: &'static str,
    query: Value,
    resumes: bool,
}

/// RULE: the resampling decides which points the page is of, so an interpolated
/// token is resumed only by the same resampling and refused by a request that
/// re-spaces or drops it.
#[tokio::test]
async fn an_interpolated_history_next_token_is_bound_to_its_resampling() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let whole = emulator
        .get_property_value_history(WORKSPACE, history_request(&interpolated_query(), None, None))
        .await;
    assert_eq!(
        whole.status, 200,
        "the whole resampled sequence: expected 200, got {whole:?}"
    );
    let sequence = reported(&whole);
    assert_eq!(
        sequence.len(),
        4,
        "the whole resampled sequence: expected a point per minute of the window, got {whole:?}"
    );

    let minted = emulator
        .get_property_value_history(
            WORKSPACE,
            history_request(&interpolated_query(), Some(2), None),
        )
        .await;
    assert_eq!(
        minted.status, 200,
        "the minting page: expected 200, got {minted:?}"
    );
    let token = minted
        .member("nextToken")
        .as_str()
        .unwrap_or_else(|| {
            panic!("the minting page left points over, so it must mint a token: {minted:?}")
        })
        .to_string();

    let rows: [InterpolatedResume; 4] = [
        InterpolatedResume {
            what: "the whole interpolated request repeated",
            query: interpolated_query(),
            resumes: true,
        },
        InterpolatedResume {
            what: "a narrower interval, which resamples the window at other points",
            query: json!({
                "selectedProperties": ["Temp"],
                "startTime": T0,
                "endTime": T3,
                "interpolation": {"interpolationType": "LINEAR", "intervalInSeconds": 30},
            }),
            resumes: false,
        },
        InterpolatedResume {
            what: "a wider interval, which resamples the window at fewer points",
            query: json!({
                "selectedProperties": ["Temp"],
                "startTime": T0,
                "endTime": T3,
                "interpolation": {"interpolationType": "LINEAR", "intervalInSeconds": 120},
            }),
            resumes: false,
        },
        InterpolatedResume {
            what: "the interpolation dropped, which reports the values as written",
            query: json!({
                "selectedProperties": ["Temp"],
                "startTime": T0,
                "endTime": T3,
            }),
            resumes: false,
        },
    ];

    for InterpolatedResume {
        what,
        query,
        resumes,
    } in rows
    {
        let page = emulator
            .get_property_value_history(WORKSPACE, history_request(&query, Some(2), Some(&token)))
            .await;
        if resumes {
            assert_eq!(page.status, 200, "case {what:?}: expected 200, got {page:?}");
            assert_eq!(
                reported(&page),
                sequence[2..].to_vec(),
                "case {what:?}: expected the points the minting page left over, got {page:?}"
            );
        } else {
            assert_eq!(page.status, 400, "case {what:?}: expected 400, got {page:?}");
            assert_eq!(
                page.error_type.as_deref(),
                Some("ValidationException"),
                "case {what:?}: expected a ValidationException, got {page:?}"
            );
            assert!(
                reported(&page).is_empty(),
                "case {what:?}: expected no points at all, got {page:?}"
            );
        }
    }
}

/// RULE, the other side of it: the members that do NOT decide the sequence leave
/// the token resumable — a token is refused for naming another sequence, not for
/// every difference between two requests.
#[tokio::test]
async fn a_history_next_token_outlives_a_change_that_decides_no_value() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let minted = emulator
        .get_property_value_history(WORKSPACE, history_request(&minting_query(), Some(2), None))
        .await;
    assert_eq!(
        minted.status, 200,
        "the minting page: expected 200, got {minted:?}"
    );
    let token = minted
        .member("nextToken")
        .as_str()
        .unwrap_or_else(|| {
            panic!("the minting page left values over, so it must mint a token: {minted:?}")
        })
        .to_string();

    // The page SIZE decides how many values a page carries, not which values the
    // sequence holds or what order they come in: the token resumes at the same
    // ordinal under another one.
    let resumed = emulator
        .get_property_value_history(
            WORKSPACE,
            history_request(&minting_query(), Some(1), Some(&token)),
        )
        .await;
    assert_eq!(
        resumed.status, 200,
        "another maxResults: expected 200, got {resumed:?}"
    );
    assert_eq!(
        reported(&resumed),
        vec![(T2.to_string(), json!({"doubleValue": 30.0}))],
        "another maxResults: expected the one value asked for, the sequence's third, got \
         {resumed:?}"
    );
    assert!(
        !resumed.member("nextToken").is_null(),
        "another maxResults: expected a token for the value still left over, got {resumed:?}"
    );
}
