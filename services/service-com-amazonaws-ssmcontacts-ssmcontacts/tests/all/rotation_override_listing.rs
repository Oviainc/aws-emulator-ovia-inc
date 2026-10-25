// SPDX-License-Identifier: CC0-1.0

//! `ListRotationOverrides` end to end over the wire.
//!
//! Every case drives the crate's ONE integration-test harness (`contact_lifecycle`'s
//! `Emulator`) with a buffered awsJson1_1 request (`POST /`,
//! `X-Amz-Target: SSMContacts.ListRotationOverrides`), so what is asserted is what a signing
//! AWS client would see — routing, deserialization, constraint validation, the handler and
//! serialization included. The overrides under test are created through
//! `CreateRotationOverride`; nothing is placed in the store by hand except the account's
//! replication set, which no modelled operation of this service can mint.
//!
//! The rules under test, each as a TABLE rather than a single call:
//!
//! 1. THE RANGE FILTER. The listing reports exactly the stored overrides of the named
//!    rotation whose own window intersects the requested `[StartTime, EndTime]` range —
//!    "a list of rotation overrides in the specified time range" — in the order the store
//!    minted them, each carrying the members the create persisted. The rows differ along
//!    what that generalises over: where an override's window sits relative to the range
//!    (before, touching either bound, straddling either bound, inside, enclosing, after),
//!    the length of both the window and the range (down to an instant and to sub-second
//!    precision) and their position (including before the epoch).
//! 2. THE SCOPE. An override is reached only through the rotation it overrides, so a
//!    listing of one rotation never reports another rotation's overrides.
//! 3. THE PAGINATION. `MaxResults` bounds a page and the opaque `NextToken` resumes after
//!    it, so walking every page yields each matching override exactly once in the same
//!    order one unpaginated call reports, and the last page carries no token. A token this
//!    list did not issue is refused.
//! 4. THE REFUSALS. A rotation the store does not hold answers 404, a range whose end
//!    precedes its start answers 400, and an account with no ACTIVE replication set is
//!    refused 400 before the rotation is resolved.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::contact_lifecycle::{error_code, Emulator, ACCOUNT, REGION};
use crate::contact_updates::set_replication_set;
use crate::rotation_overrides::{create_override, create_rotation, ensure_contacts};

/// A rotation ARN this service never minted, for the "no such rotation" rows.
///
/// `pub(crate)` because the sibling override suites need the same ARN: one spelling of "a
/// rotation the store does not hold" rather than one per suite.
pub(crate) fn absent_rotation_arn() -> String {
    format!(
        "arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:rotation/\
         00000000-0000-0000-0000-000000000000"
    )
}

/// One override to create on the rotation under test: its window, in epoch seconds as the
/// wire carries a `DateTime`.
///
/// `team` is how many contacts replace the rotation's own team, so the listing is also
/// asserted over a replacement list at the modelled minimum (`RotationOverride$NewContactIds`
/// is `length 0..25`, `smithy_query(query="shape",shape="RotationOverride")`).
struct Window {
    case: &'static str,
    start: f64,
    end: f64,
    team: usize,
}

/// The overrides, created in THIS order — which is the order the listing must report them
/// in. Their windows are placed around the `[1000, 2000]` range the first query row names,
/// so every position a window can hold relative to a range is a row.
const WINDOWS: &[Window] = &[
    Window {
        case: "entirely before the range",
        start: 100.0,
        end: 200.0,
        team: 1,
    },
    Window {
        case: "ending exactly at the range start",
        start: 500.0,
        end: 1_000.0,
        team: 2,
    },
    Window {
        case: "straddling the range start",
        start: 900.0,
        end: 1_500.0,
        team: 1,
    },
    Window {
        case: "entirely inside the range",
        start: 1_200.0,
        end: 1_300.0,
        team: 0,
    },
    Window {
        case: "straddling the range end",
        start: 1_800.0,
        end: 2_500.0,
        team: 3,
    },
    Window {
        case: "starting exactly at the range end",
        start: 2_000.0,
        end: 3_000.0,
        team: 1,
    },
    Window {
        case: "entirely after the range",
        start: 5_000.0,
        end: 6_000.0,
        team: 1,
    },
    Window {
        case: "enclosing the whole range",
        start: 0.0,
        end: 9_999.0,
        team: 2,
    },
    Window {
        case: "a sub-second window inside the range",
        start: 1_400.25,
        end: 1_400.75,
        team: 1,
    },
    Window {
        case: "a window that predates the epoch",
        start: -86_400.0,
        end: -3_600.0,
        team: 1,
    },
];

/// One listing row: the range asked for, and the override cases it must report, in creation
/// order.
struct Query {
    case: &'static str,
    from: f64,
    to: f64,
    expected: &'static [&'static str],
}

const QUERIES: &[Query] = &[
    Query {
        case: "the range the windows are placed around",
        from: 1_000.0,
        to: 2_000.0,
        expected: &[
            "ending exactly at the range start",
            "straddling the range start",
            "entirely inside the range",
            "straddling the range end",
            "starting exactly at the range end",
            "enclosing the whole range",
            "a sub-second window inside the range",
        ],
    },
    Query {
        case: "a range ending where the earliest window ends",
        from: 0.0,
        to: 100.0,
        expected: &["entirely before the range", "enclosing the whole range"],
    },
    Query {
        case: "a range past the range the windows surround",
        from: 3_000.0,
        to: 4_000.0,
        expected: &[
            "starting exactly at the range end",
            "enclosing the whole range",
        ],
    },
    Query {
        case: "an instant rather than a span",
        from: 1_200.0,
        to: 1_200.0,
        expected: &[
            "straddling the range start",
            "entirely inside the range",
            "enclosing the whole range",
        ],
    },
    Query {
        case: "a sub-second instant",
        from: 1_400.5,
        to: 1_400.5,
        expected: &[
            "straddling the range start",
            "enclosing the whole range",
            "a sub-second window inside the range",
        ],
    },
    Query {
        case: "a range before the epoch",
        from: -90_000.0,
        to: -4_000.0,
        expected: &["a window that predates the epoch"],
    },
    Query {
        case: "a range no window reaches",
        from: 100_000.0,
        to: 200_000.0,
        expected: &[],
    },
    Query {
        case: "a range spanning every window",
        from: -100_000.0,
        to: 100_000.0,
        expected: &[
            "entirely before the range",
            "ending exactly at the range start",
            "straddling the range start",
            "entirely inside the range",
            "straddling the range end",
            "starting exactly at the range end",
            "entirely after the range",
            "enclosing the whole range",
            "a sub-second window inside the range",
            "a window that predates the epoch",
        ],
    },
];

/// Create every [`WINDOWS`] row on `rotation`, answering `case -> RotationOverrideId` and
/// the request each create carried.
async fn create_windows(
    emulator: &Emulator,
    rotation: &str,
) -> BTreeMap<&'static str, (String, Value)> {
    let mut created = BTreeMap::new();
    for window in WINDOWS {
        let team = ensure_contacts(emulator, window.team, window.case).await;
        let request = json!({
            "RotationId": rotation,
            "NewContactIds": team,
            "StartTime": window.start,
            "EndTime": window.end,
        });
        let id = create_override(emulator, request.clone(), window.case).await;
        created.insert(window.case, (id, request));
    }
    created
}

/// Call `ListRotationOverrides`, asserting it answered 200, and answer the reported list.
async fn list(emulator: &Emulator, request: Value, case: &str) -> Vec<Value> {
    let (status, body) = emulator
        .call("ListRotationOverrides", request.clone())
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListRotationOverrides {request} to answer 200, got \
         {status} {body:?}"
    );
    body["RotationOverrides"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a RotationOverrides list, got {body:?}")
        })
        .clone()
}

/// The `RotationOverrideId`s a listing reported, in the order it reported them.
fn ids_of(page: &[Value], case: &str) -> Vec<String> {
    page.iter()
        .map(|entry| {
            entry["RotationOverrideId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a RotationOverrideId on {entry:?}")
                })
                .to_string()
        })
        .collect()
}

/// THE RANGE FILTER: every window case, read back through every range case.
#[tokio::test]
async fn a_listing_reports_the_overrides_whose_window_meets_the_requested_range() {
    let emulator = Emulator::new();
    let rotation = create_rotation(&emulator, "override-listing", "the range filter").await;
    let created = create_windows(&emulator, &rotation).await;

    for query in QUERIES {
        let case = query.case;
        let page = list(
            &emulator,
            json!({
                "RotationId": rotation,
                "StartTime": query.from,
                "EndTime": query.to,
                "MaxResults": WINDOWS.len() + 1,
            }),
            case,
        )
        .await;

        let expected_ids: Vec<String> = query
            .expected
            .iter()
            .map(|window_case| created[window_case].0.clone())
            .collect();
        assert_eq!(
            ids_of(&page, case),
            expected_ids,
            "case {case:?}: expected the overrides {:?} in creation order, got {page:?}",
            query.expected
        );

        // Each reported entry carries what the create persisted — the window, the
        // replacement team in the created order, and a `CreateTime` the store already held.
        for (entry, window_case) in page.iter().zip(query.expected) {
            let (_, request) = &created[window_case];
            assert_eq!(
                entry["StartTime"].as_f64(),
                request["StartTime"].as_f64(),
                "case {case:?}/{window_case:?}: expected the created StartTime, got {entry:?}"
            );
            assert_eq!(
                entry["EndTime"].as_f64(),
                request["EndTime"].as_f64(),
                "case {case:?}/{window_case:?}: expected the created EndTime, got {entry:?}"
            );
            assert_eq!(
                entry["NewContactIds"], request["NewContactIds"],
                "case {case:?}/{window_case:?}: expected the created NewContactIds in the \
                 created order, got {entry:?}"
            );

            // The listing and the single-resource read agree: both report the stored record.
            let id = entry["RotationOverrideId"].as_str().unwrap_or_default();
            let (status, read) = emulator
                .call(
                    "GetRotationOverride",
                    json!({"RotationId": rotation, "RotationOverrideId": id}),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}/{window_case:?}: expected GetRotationOverride to answer 200, \
                 got {status} {read:?}"
            );
            assert_eq!(
                entry["CreateTime"].as_f64(),
                read["CreateTime"].as_f64(),
                "case {case:?}/{window_case:?}: expected the listed CreateTime to be the one \
                 the read reports, got {entry:?} and {read:?}"
            );
        }
    }
}

/// THE SCOPE: a listing reports only the overrides of the rotation it names, and a rotation
/// with no overrides at all reports an empty list rather than another rotation's.
#[tokio::test]
async fn a_listing_reports_only_the_named_rotations_overrides() {
    let emulator = Emulator::new();
    let first = create_rotation(&emulator, "override-scope-first", "the scope").await;
    let second = create_rotation(&emulator, "override-scope-second", "the scope").await;
    let third = create_rotation(&emulator, "override-scope-third", "the scope").await;

    let team = ensure_contacts(&emulator, 1, "the scope").await;
    let mut expected: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for (rotation, label, count) in [(&first, "first", 2usize), (&second, "second", 1)] {
        for index in 0..count {
            let start = 1_000.0 + (index as f64) * 100.0;
            let id = create_override(
                &emulator,
                json!({
                    "RotationId": rotation,
                    "NewContactIds": team,
                    "StartTime": start,
                    "EndTime": start + 50.0,
                }),
                label,
            )
            .await;
            expected.entry(label).or_default().push(id);
        }
    }

    for (rotation, label) in [
        (&first, "first"),
        (&second, "second"),
        (&third, "third — no overrides of its own"),
    ] {
        let page = list(
            &emulator,
            json!({"RotationId": rotation, "StartTime": 0, "EndTime": 100_000}),
            label,
        )
        .await;
        let wanted = expected.get(label).cloned().unwrap_or_default();
        assert_eq!(
            ids_of(&page, label),
            wanted,
            "case {label:?}: expected only this rotation's own overrides {wanted:?}, got \
             {page:?}"
        );
    }
}

/// THE PAGINATION: every page size walks the same list in the same order, exactly once.
#[tokio::test]
async fn the_listing_paginates_over_every_page_size() {
    let emulator = Emulator::new();
    let rotation = create_rotation(&emulator, "override-pagination", "the pagination").await;
    create_windows(&emulator, &rotation).await;

    // The widest range, so every override created above is a candidate.
    let range = json!({"RotationId": rotation, "StartTime": -100_000, "EndTime": 100_000});
    let unpaginated = ids_of(&list(&emulator, range.clone(), "unpaginated").await, "unpaginated");
    assert_eq!(
        unpaginated.len(),
        WINDOWS.len(),
        "case \"unpaginated\": expected every created override, got {unpaginated:?}"
    );

    // 0 is the modelled minimum of `MaxResults` (range 0..1024) and names no page size, so
    // it serves the default page exactly as an absent member does.
    for page_size in [0usize, 1, 2, 3, 4, WINDOWS.len(), WINDOWS.len() + 7, 1024] {
        let case = format!("MaxResults {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;

        loop {
            let mut request = range.clone();
            request["MaxResults"] = json!(page_size);
            if let Some(token) = token.as_deref() {
                request["NextToken"] = json!(token);
            }
            let (status, body) = emulator.call("ListRotationOverrides", request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected page {pages} to answer 200, got {status} {body:?}"
            );
            let page = body["RotationOverrides"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected a list, got {body:?}"))
                .clone();
            if page_size > 0 {
                assert!(
                    page.len() <= page_size,
                    "case {case:?}: expected at most {page_size} items on page {pages}, got {}",
                    page.len()
                );
            }
            walked.extend(ids_of(&page, &case));
            pages += 1;
            assert!(
                pages <= WINDOWS.len() + 2,
                "case {case:?}: expected the walk to end, got {pages} pages"
            );
            match body["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }

        assert_eq!(
            walked, unpaginated,
            "case {case:?}: expected the walk to report the unpaginated list exactly once \
             each, got {walked:?}"
        );
    }
}

/// A `NextToken` this list did not issue is refused 400 rather than served.
#[tokio::test]
async fn a_next_token_this_list_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    let rotation = create_rotation(&emulator, "override-tokens", "the tokens").await;
    create_windows(&emulator, &rotation).await;

    // A token another list of this service issued, which is the strongest "not this list's
    // token" case the service can produce.
    let (status, body) = emulator
        .call("ListRotations", json!({"MaxResults": 1}))
        .await;
    assert_eq!(
        status, 200,
        "case \"a real ListRotations token\": expected 200, got {status} {body:?}"
    );
    let foreign = body["NextToken"]
        .as_str()
        .map(ToString::to_string)
        .unwrap_or_default();

    for (case, token) in [
        ("a token from another list of this service", foreign),
        ("a value that is merely printable", "not-a-token".to_string()),
        (
            "hex that decodes to a payload this list never minted",
            "6e6f70650a".to_string(),
        ),
    ] {
        if token.is_empty() {
            continue;
        }
        let (status, body) = emulator
            .call(
                "ListRotationOverrides",
                json!({
                    "RotationId": rotation,
                    "StartTime": 0,
                    "EndTime": 100_000,
                    "NextToken": token,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
    }
}

/// THE REFUSALS: an unresolvable rotation, an inverted range, and the account gate.
#[tokio::test]
async fn the_listing_refuses_what_it_cannot_serve() {
    let emulator = Emulator::new();
    let rotation = create_rotation(&emulator, "override-refusals", "the refusals").await;
    create_windows(&emulator, &rotation).await;

    // A rotation this service never minted has no overrides to report.
    let absent = absent_rotation_arn();
    let (status, body) = emulator
        .call(
            "ListRotationOverrides",
            json!({"RotationId": absent, "StartTime": 0, "EndTime": 100_000}),
        )
        .await;
    assert_eq!(
        status, 404,
        "case \"a rotation the store does not hold\": expected HTTP 404, got {status} {body:?}"
    );
    assert!(
        error_code(&body).contains("ResourceNotFoundException"),
        "case \"a rotation the store does not hold\": expected a ResourceNotFoundException, \
         got {body:?}"
    );

    // A range whose end precedes its start names no range at all.
    for (case, from, to) in [
        ("an end one second before the start", 2_000.0, 1_999.0),
        ("an end a sub-second before the start", 1_000.5, 1_000.25),
        ("an end long before the start", 100_000.0, -100_000.0),
    ] {
        let (status, body) = emulator
            .call(
                "ListRotationOverrides",
                json!({"RotationId": rotation, "StartTime": from, "EndTime": to}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
    }

    // THE ACCOUNT GATE: without an ACTIVE replication set the listing is refused before the
    // rotation is resolved, so a stored rotation and an absent one are refused alike.
    set_replication_set(&emulator, None);
    for (case, rotation_id) in [
        ("a stored rotation on a non-onboarded account", rotation),
        ("an absent rotation on a non-onboarded account", absent),
    ] {
        let (status, body) = emulator
            .call(
                "ListRotationOverrides",
                json!({"RotationId": rotation_id, "StartTime": 0, "EndTime": 100_000}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
    }
}
