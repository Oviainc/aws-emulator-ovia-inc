// SPDX-License-Identifier: CC0-1.0

//! The POSITION a page cursor carries is honoured only when it is one this service issued.
//!
//! The rule under test: a `NextToken` is opaque but NOT authenticated — it is the hex of a tag
//! and a payload — so the payload inside the tag is a value a caller may have written, and a
//! read resolves it only when it is a boundary this service's own paging could have handed out
//! over the set being read: spelled exactly as the service spells it, and naming a position
//! that paging can reach. Anything else is refused with the modelled `ValidationException`
//! (HTTP 400) and the service's ONE wording for a token it did not issue, rather than honoured
//! as a page boundary.
//!
//! For the reads whose cursor carries a bare decimal position the service mints one by
//! advancing past the page it just served (at least one entry) and only while the set holds
//! more entries beyond it, so the positions it can issue are the whole numbers from one to the
//! entry count exclusive. A position of zero, a position spelled with a sign or a leading zero,
//! and a position at or past the end of the set are therefore none of its own — honouring them
//! would re-serve the first page under a cursor, serve a page the caller never asked to resume
//! from, or report the set as exhausted to a caller whose cursor the service never gave it.
//!
//! The rows span what the rule generalises over: WHICH read resolves the cursor (all eight whose
//! cursor carries a decimal position — an offset into a derived projection for seven of them,
//! and the stored ordinal of a window for the exclusion-window read, which over a set nothing
//! has changed between pages is the same whole number — differing in route, method, in where
//! their model binds the page members, and in what their set holds), HOW the position is
//! misspelled, and WHERE it sits relative to the set. The chronological cursor of the status
//! read is the same rule on a payload of several fields, and is the last table here.
//!
//! `unissued_next_token` pins the half of the rule that is about the TAG (a token that is not
//! hex, carries no tag of this service, or was issued for another read); this suite is the half
//! about the payload INSIDE a tag the caller really was given.
//!
//! The harness is `slo_lifecycle`'s, so this suite speaks to the same mounted router as every
//! other.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::exclusion_windows::{batch, create_slo, window, DURATION_UNITS};
use crate::instrumentation_configuration_status::{
    create_configuration, create_request, get_status, now_minute, report, report_entry,
    router as status_router, Query, BREAKPOINT,
};
use crate::slo_lifecycle::{call, create, name_path_segment, router};

/// The ONE message a read of this service answers a `NextToken` it did not issue with.
const EXPECTED: &str = "NextToken is not a token this service issued";

/// The services the seeded objectives monitor.
const SERVICES: [&str; 2] = ["position-alpha", "position-beta"];

/// How many objectives monitor each seeded service, which is how many entries each derived
/// projection of one service holds: three, so a page bound of one leaves positions both inside
/// and outside the set.
const OBJECTIVES_PER_SERVICE: usize = 3;

/// The environment every seeded entity is hosted in.
const ENVIRONMENT: &str = "prod";

/// The SLO whose exclusion-window set the windowed read below pages.
const WINDOWED_SLO: &str = "position-windows";

/// How many exclusion windows that SLO holds.
const WINDOWS_STORED: usize = 3;

/// The page bound every read below takes its WHOLE set at: more than any seeded set holds and
/// inside the smallest maximum these reads' `MaxResults` members declare
/// (`ListServiceLevelObjectiveExclusionWindowsMaxResults`, `range 1..10`).
const WHOLE_SET_BOUND: usize = 10;

/// Where a read's model binds `MaxResults` and `NextToken`.
#[derive(Clone, Copy)]
enum PageMembers {
    /// In the query string, beside whatever else the route carries.
    Query,
    /// In the JSON body, as `ListAuditFindings` and `ListServiceStates` bind them.
    Body,
}

/// One read whose cursor carries a decimal position, pointed at one set of its own.
struct Read {
    /// The operation under test, named in every assertion.
    operation: &'static str,
    /// The output member carrying the page.
    items: &'static str,
    method: &'static str,
    /// The route, with every selection member the model binds to it already spelled in.
    path: String,
    page_members: PageMembers,
    /// The body carrying the rest of the selection, if the read takes one.
    body: Option<Value>,
}

/// The key attributes of one Application Signals service entity.
fn service_entity(name: &str) -> Value {
    json!({ "Type": "Service", "Name": name, "Environment": ENVIRONMENT })
}

/// The far end the seeded edges of one service point at — one per service, so the dependents of
/// one service are not the dependents of the other.
fn sink_entity(service: &str) -> Value {
    json!({
        "Type": "RemoteService",
        "Name": format!("{service}-sink"),
        "Environment": ENVIRONMENT
    })
}

/// Seconds since the epoch, for the period every read below covers.
fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64
}

/// Seed the objectives every derived set below comes from: one operation and one edge out of it
/// per objective, so each seeded service has [`OBJECTIVES_PER_SERVICE`] findings, operations,
/// dependencies and change events, and its far end that many dependents.
async fn seed(router: &MultiServiceRouter) {
    for service in SERVICES {
        for index in 0..OBJECTIVES_PER_SERVICE {
            let name = format!("{service}-slo-{index}");
            let (status, created) = create(
                router,
                json!({
                    "Name": name,
                    "SliConfig": {
                        "SliMetricConfig": {
                            "KeyAttributes": service_entity(service),
                            "OperationName": format!("{service}-op-{index}"),
                            "MetricType": "LATENCY",
                            "PeriodSeconds": 60,
                            "DependencyConfig": {
                                "DependencyKeyAttributes": sink_entity(service),
                                "DependencyOperationName": format!("Call-{index}")
                            }
                        },
                        "MetricThreshold": 100.0,
                        "ComparisonOperator": "LessThan"
                    }
                }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {name:?}: expected the setup create to succeed, got {status} with {created}"
            );
        }
    }

    // The exclusion-window read pages a STORED set rather than a derived one, so its set is
    // seeded by adding windows to an SLO of its own.
    create_slo(router, WINDOWED_SLO).await;
    let windows: Vec<Value> = (0..WINDOWS_STORED)
        .map(|index| {
            window(
                DURATION_UNITS[index % DURATION_UNITS.len()],
                (index + 1) as i64,
                Some(1_700_000_000 + index as i64),
                None,
                Some(&format!("position window {index}")),
            )
        })
        .collect();
    let (status, updated) = batch(router, &[WINDOWED_SLO.to_string()], &windows, &[]).await;
    assert_eq!(
        status, 200,
        "expected the setup add of {WINDOWS_STORED} exclusion windows to succeed, got {status} with {updated}"
    );
}

/// Every read whose `NextToken` carries a decimal position, each pointed at a set of
/// at least two entries.
fn reads(now: i64) -> Vec<Read> {
    let window = format!("StartTime={}&EndTime={}", now - 3_600, now + 3_600);

    // The reads that take their subject as a key-attribute map in the body, with their period
    // and page members in the query string.
    let entity_read = |operation: &'static str,
                       items: &'static str,
                       path: &str,
                       subject: Value| Read {
        operation,
        items,
        method: "POST",
        path: format!("{path}?{window}"),
        page_members: PageMembers::Query,
        body: Some(json!({ "KeyAttributes": subject })),
    };

    vec![
        Read {
            operation: "ListAuditFindings",
            items: "AuditFindings",
            method: "POST",
            // `MaxResults` and `NextToken` are BODY members of this read; its period is bound
            // to the query string.
            path: format!("/auditFindings?{window}"),
            page_members: PageMembers::Body,
            body: Some(json!({
                "AuditTargets": [
                    { "Type": "service", "Data": { "Service": service_entity(SERVICES[0]) } }
                ]
            })),
        },
        Read {
            operation: "ListEntityEvents",
            items: "ChangeEvents",
            method: "POST",
            path: "/events".to_string(),
            page_members: PageMembers::Query,
            body: Some(json!({
                "Entity": service_entity(SERVICES[0]),
                "StartTime": now - 3_600,
                "EndTime": now + 3_600
            })),
        },
        Read {
            operation: "ListServices",
            items: "ServiceSummaries",
            method: "GET",
            path: format!("/services?{window}"),
            page_members: PageMembers::Query,
            body: None,
        },
        entity_read(
            "ListServiceOperations",
            "ServiceOperations",
            "/service-operations",
            service_entity(SERVICES[0]),
        ),
        entity_read(
            "ListServiceDependencies",
            "ServiceDependencies",
            "/service-dependencies",
            service_entity(SERVICES[0]),
        ),
        // The dependents of a far end are the edges INTO it, so this read's subject is the
        // entity the seeded edges point at.
        entity_read(
            "ListServiceDependents",
            "ServiceDependents",
            "/service-dependents",
            sink_entity(SERVICES[0]),
        ),
        Read {
            operation: "ListServiceStates",
            items: "ServiceStates",
            method: "POST",
            path: "/service/states".to_string(),
            // Every input member of this read, its page members included, is a body member.
            page_members: PageMembers::Body,
            body: Some(json!({ "StartTime": now - 3_600, "EndTime": now + 3_600 })),
        },
        Read {
            operation: "ListServiceLevelObjectiveExclusionWindows",
            items: "ExclusionWindows",
            method: "GET",
            path: format!("/slo/{}/exclusion-windows", name_path_segment(WINDOWED_SLO)),
            page_members: PageMembers::Query,
            body: None,
        },
    ]
}

/// `path` with `extra` appended as a query member, whichever way the route already ends.
fn with_query(path: &str, extra: &str) -> String {
    let separator = if path.contains('?') { '&' } else { '?' };
    format!("{path}{separator}{extra}")
}

/// One page of `read`, bounded by `max_results` and resuming from `token` when given.
async fn page(
    router: &MultiServiceRouter,
    read: &Read,
    max_results: usize,
    token: Option<&str>,
) -> (u16, Value) {
    let mut path = read.path.clone();
    let mut body = read.body.clone();
    match read.page_members {
        PageMembers::Query => {
            path = with_query(&path, &format!("MaxResults={max_results}"));
            if let Some(token) = token {
                path = with_query(&path, &format!("NextToken={token}"));
            }
        }
        PageMembers::Body => {
            let mut carried = body.unwrap_or_else(|| json!({}));
            carried["MaxResults"] = json!(max_results);
            if let Some(token) = token {
                carried["NextToken"] = json!(token);
            }
            body = Some(carried);
        }
    }
    call(router, read.method, &path, body).await
}

/// The entries a page reports, in order.
fn entries(answer: &Value, items: &str) -> Vec<Value> {
    answer[items].as_array().cloned().unwrap_or_default()
}

/// The plaintext inside a token: a token is the hex of a tag and a payload, with no signature
/// over either, which is why the payload is caller-editable at all.
fn plaintext(token: &str) -> String {
    String::from_utf8(::hex::decode(token).expect("an issued token is hex"))
        .expect("an issued token is hex of text")
}

/// The tag a page token was issued under: everything before the decimal position it ends with.
fn tag_of(token: &str) -> String {
    plaintext(token)
        .trim_end_matches(|character: char| character.is_ascii_digit())
        .to_string()
}

/// The position a page token carries, as the service wrote it.
fn position_of(token: &str) -> String {
    plaintext(token)[tag_of(token).len()..].to_string()
}

/// A token carrying `position` under the tag of the token this service really issued — what a
/// caller does to forge one: keep the tag it was given and write its own payload inside it.
fn repositioned(token: &str, position: &str) -> String {
    ::hex::encode(format!("{}{position}", tag_of(token)))
}

/// Assert that `answer` is this service's refusal of a token it did not issue, carrying no page
/// and no echo of the rejected token.
fn assert_refused(case: &str, status: u16, answer: &Value, items: &str, token: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 for a position this service never issued, got {status} with {answer}"
    );
    assert_eq!(
        answer["__type"]
            .as_str()
            .map(|kind| kind.rsplit('#').next_back().unwrap_or(kind)),
        Some("ValidationException"),
        "case {case:?}: expected a ValidationException, got {answer}"
    );
    let message = answer["message"].as_str().unwrap_or_default();
    assert_eq!(
        message, EXPECTED,
        "case {case:?}: expected the message {EXPECTED:?}, got {message:?}"
    );
    assert!(
        !message.contains(token),
        "case {case:?}: the message must not echo the rejected token, got {message:?}"
    );
    assert!(
        answer.get(items).is_none() && answer.get("NextToken").is_none(),
        "case {case:?}: a refused cursor carries no page and no cursor, got {answer}"
    );
}

/// The positions no page of a projection holding `len` entries could have been issued for, and
/// why each is none of this service's own.
///
/// The spellings are the ones `usize::from_str` accepts but `usize::to_string` never writes (a
/// sign, leading zeros), beside positions outside the range this paging can reach at all.
fn unissued_positions(len: usize) -> Vec<(String, String)> {
    let last = len - 1;
    vec![
        (
            "the start of the projection, which a cursorless read already serves".to_string(),
            "0".to_string(),
        ),
        (
            "the start of the projection spelled with a leading zero".to_string(),
            "00".to_string(),
        ),
        (
            "the first position with a leading plus".to_string(),
            "+1".to_string(),
        ),
        (
            "the first position with a leading zero".to_string(),
            "01".to_string(),
        ),
        (
            "the first position with two leading zeros".to_string(),
            "001".to_string(),
        ),
        (
            "the last issuable position with a leading zero".to_string(),
            format!("0{last}"),
        ),
        (
            "the last issuable position with a leading plus".to_string(),
            format!("+{last}"),
        ),
        (
            "the end of the projection, which bounds no page".to_string(),
            len.to_string(),
        ),
        (
            "one entry past the end of the projection".to_string(),
            (len + 1).to_string(),
        ),
        (
            "far past the end of the projection".to_string(),
            (len + 900).to_string(),
        ),
        (
            "a position larger than any machine integer holds".to_string(),
            "18446744073709551616".to_string(),
        ),
        (
            "a position with a fractional part".to_string(),
            "1.0".to_string(),
        ),
        (
            "a position before the start of the projection".to_string(),
            "-1".to_string(),
        ),
        (
            "a position with a digit separator".to_string(),
            "1_0".to_string(),
        ),
        ("a position with no digits at all".to_string(), String::new()),
    ]
}

/// Every read whose cursor carries a decimal position refuses a position this service never
/// issued, and keeps serving every position it can issue.
#[tokio::test]
async fn an_offset_cursor_honours_only_a_position_this_service_issued() {
    let router = router();
    seed(&router).await;
    let now = now_seconds();

    for read in reads(now) {
        let operation = read.operation;

        // The WHOLE set this read projects, which is what the positions below are judged
        // against.
        let (status, whole) = page(&router, &read, WHOLE_SET_BOUND, None).await;
        assert_eq!(
            status, 200,
            "case {operation:?}: expected the unbounded read to be served, got {status} with {whole}"
        );
        let set = entries(&whole, read.items);
        assert!(
            whole["NextToken"].is_null(),
            "case {operation:?}: expected the whole set on one page of {WHOLE_SET_BOUND}, got {whole}"
        );
        assert!(
            set.len() >= 2,
            "case {operation:?}: expected the seeding to leave at least two entries to page, got {} in {whole}",
            set.len()
        );

        // A page of one of several issues a token, and the position inside it is the bare
        // decimal count of the entries already served.
        let (status, first) = page(&router, &read, 1, None).await;
        assert_eq!(
            status, 200,
            "case {operation:?}: expected the first page to be served, got {status} with {first}"
        );
        let issued = first["NextToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {operation:?}: expected a cursor on a page of one of several, got {first}")
            })
            .to_string();
        assert_eq!(
            position_of(&issued),
            "1",
            "case {operation:?}: expected the token of a page of one to carry position 1, got {:?}",
            plaintext(&issued)
        );

        // Every position this read's paging CAN issue is still honoured, and serves the entry
        // it counts to.
        for position in 1..set.len() {
            let case = format!("{operation} resuming at position {position}");
            let token = repositioned(&issued, &position.to_string());
            let (status, resumed) = page(&router, &read, 1, Some(&token)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected an issuable position to be served, got {status} with {resumed}"
            );
            assert_eq!(
                entries(&resumed, read.items),
                set[position..position + 1].to_vec(),
                "case {case:?}: expected the entry at that position, got {resumed}"
            );
        }

        // No other position is one this service issued.
        for (why, position) in unissued_positions(set.len()) {
            let case = format!("{operation} with {why}");
            let token = repositioned(&issued, &position);
            let (status, answer) = page(&router, &read, 1, Some(&token)).await;
            assert_refused(&case, status, &answer, read.items, &token);
        }

        // The refusals changed nothing: the whole set reads back as it did before them.
        let (status, after) = page(&router, &read, WHOLE_SET_BOUND, None).await;
        assert_eq!(
            status, 200,
            "case {operation:?}: expected the read after the refusals to be served, got {status} with {after}"
        );
        assert_eq!(
            entries(&after, read.items),
            set,
            "case {operation:?}: expected the refused cursors to leave the set as it was, got {after}"
        );
    }
}

/// The tag of a status-event token ends after its third separator, so what follows is the
/// chronological cursor's own payload.
fn status_payload(token: &str) -> String {
    let plain = plaintext(token);
    let boundary = plain
        .match_indices('/')
        .nth(2)
        .map(|(index, _)| index + 1)
        .expect("the tag of a status-event token carries three separators");
    plain[boundary..].to_string()
}

/// A status token carrying `payload` under the tag of one this service issued.
fn status_retagged(token: &str, payload: &str) -> String {
    let plain = plaintext(token);
    let tag_len = plain.len() - status_payload(token).len();
    ::hex::encode(format!("{}{payload}", &plain[..tag_len]))
}

/// The instants a status cursor's payload carries, each as the seconds and the nanoseconds it
/// spells them with.
fn status_instants(payload: &str) -> Vec<(String, String)> {
    payload
        .split('/')
        .map(|instant| {
            let (secs, nanos) = instant
                .split_once('.')
                .expect("every instant of a status cursor carries a subsecond part");
            (secs.to_string(), nanos.to_string())
        })
        .collect()
}

/// How one instant of a status cursor is respelled: the seconds and the nanoseconds this
/// service wrote, rendered a way it never writes them.
type Respelling = fn(&str, &str) -> String;

/// `payload` with the instant at `index` respelled by `respell`, which is what a caller editing
/// one field of a cursor it was issued produces.
fn respelled(payload: &str, index: usize, respell: Respelling) -> String {
    let instants = status_instants(payload);
    instants
        .iter()
        .enumerate()
        .map(|(at, (secs, nanos))| {
            if at == index {
                respell(secs, nanos)
            } else {
                format!("{secs}.{nanos}")
            }
        })
        .collect::<Vec<String>>()
        .join("/")
}

/// The chronological cursor of the status read holds to the same rule on a payload of several
/// fields: an instant respelled a way this service never writes is not a cursor it issued, even
/// though the number each field spells is the one it handed out.
#[tokio::test]
async fn a_chronological_cursor_honours_only_the_spelling_this_service_wrote() {
    let service = "checkout";
    let environment = "ec2:production";
    let now = now_minute();
    let router = status_router();
    let hash = create_configuration(
        &router,
        &create_request(service, environment, BREAKPOINT, "src/Cart.java"),
    )
    .await;

    // Three events inside the default window, so a page of one issues a cursor.
    let instants: Vec<f64> = (1..=3).map(|step| now - 60.0 * step as f64).collect();
    let (status, reported) = report(
        &router,
        &json!({
            "Service": service,
            "Environment": environment,
            "Configurations": instants
                .iter()
                .map(|time| report_entry(BREAKPOINT, &hash, "READY", *time, None))
                .collect::<Vec<Value>>(),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the setup report to succeed, got {status} with {reported}"
    );

    let read = |token: Option<&str>| {
        crate::instrumentation_configuration_status::status_request(
            service,
            environment,
            BREAKPOINT,
            json!({ "LocationHash": hash.clone() }),
            &Query {
                status: Some("READY"),
                max_results: Some(1),
                next_token: token,
                ..Query::default()
            },
        )
    };

    let (status, first) = get_status(&router, &read(None)).await;
    assert_eq!(
        status, 200,
        "expected the first page to be served, got {status} with {first}"
    );
    let issued = first["NextToken"]
        .as_str()
        .expect("a page of one of three events issues a NextToken")
        .to_string();

    // The cursor this service really issued is honoured, and serves the next event.
    let (status, resumed) = get_status(&router, &read(Some(&issued))).await;
    assert_eq!(
        status, 200,
        "expected the issued cursor to be honoured, got {status} with {resumed}"
    );
    assert_eq!(
        resumed["Events"].as_array().map(Vec::len),
        Some(1),
        "expected the resumed page to carry one event, got {resumed}"
    );

    let payload = status_payload(&issued);
    let fields = status_instants(&payload);
    assert_eq!(
        fields.len(),
        3,
        "expected the cursor to carry the window and the boundary, got {payload:?}"
    );

    // Each field of the payload, respelled a way `Display` never writes it: the number is the
    // one the service handed out, the spelling is not.
    let respellings: [(&str, Respelling); 4] = [
        ("a leading plus on the seconds", |secs, nanos| {
            format!("+{secs}.{nanos}")
        }),
        ("a leading zero on the seconds", |secs, nanos| {
            format!("0{secs}.{nanos}")
        }),
        ("a leading zero on the nanoseconds", |secs, nanos| {
            format!("{secs}.0{nanos}")
        }),
        ("a leading plus on the nanoseconds", |secs, nanos| {
            format!("{secs}.+{nanos}")
        }),
    ];

    for (index, _) in fields.iter().enumerate() {
        for (why, respell) in respellings {
            let case = format!("field {index} with {why}");
            let token = status_retagged(&issued, &respelled(&payload, index, respell));
            let (status, answer) = get_status(&router, &read(Some(&token))).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for a cursor spelled a way this service never wrote, got {status} with {answer}"
            );
            assert_eq!(
                answer["__type"]
                    .as_str()
                    .map(|kind| kind.rsplit('#').next_back().unwrap_or(kind)),
                Some("ValidationException"),
                "case {case:?}: expected a ValidationException, got {answer}"
            );
            assert!(
                answer.get("Events").is_none() && answer.get("NextToken").is_none(),
                "case {case:?}: a refused cursor carries no page and no cursor, got {answer}"
            );
        }
    }

    // And the read still serves what it served before the refusals.
    let (status, after) = get_status(&router, &read(None)).await;
    assert_eq!(
        status, 200,
        "expected the read after the refusals to be served, got {status} with {after}"
    );
    assert_eq!(
        after["Events"], first["Events"],
        "expected the refused cursors to leave the chronology as it was, got {after}"
    );
}
