// SPDX-License-Identifier: CC0-1.0

//! A `NextToken` resumes a page only in the ONE spelling this service handed out.
//!
//! The rule under test: a page cursor is opaque, so a position is named by the single token
//! the service minted for it and by nothing else — a value that merely decodes to the same
//! cursor bytes, while being a string no response of this service ever carried, is a token
//! this service did not issue and is refused with the modelled `ValidationException` (HTTP
//! 400) rather than answered with the page that follows. Hex is a case-INSENSITIVE encoding
//! read back but a lower-case one written out, so re-spelling an issued token's letter digits
//! in upper case is exactly such a value: same bytes, a spelling the minter cannot write.
//!
//! The rule belongs to the shared cursor codec rather than to one read, so the rows are the
//! reads of this service that resolve a cursor, and they differ along what the rule
//! generalises over: what the cursor MEANS (an offset into a derived projection, a key of the
//! store, an ordinal of a stored set, a key within one scope), where the model binds
//! `NextToken` (the query string or the JSON body), the HTTP method and the route — and,
//! per read, how many of the issued token's letter digits are re-spelt and where (all of
//! them, the first, the last, every other one), rather than one re-spelling picked by hand.
//!
//! Each row also asserts the positive control: the token AS ISSUED still resumes, and
//! resumes after the entry the first page served, so the refusals above are earned by the
//! spelling alone.
//!
//! The harness is `slo_lifecycle`'s, and the exclusion-window request helpers are
//! `exclusion_windows`', so every suite here speaks to one mounted router.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::exclusion_windows::{batch, window, DURATION_UNITS};
use crate::slo_lifecycle::{call, create, name_path_segment, router, sli_config};

/// The ONE message a read of this service answers a `NextToken` it did not issue with.
const UNISSUED: &str = "NextToken is not a token this service issued";

/// The services the seeded SLOs monitor: more than one, so every paged read below holds a set
/// a cursor can divide and a page of one carries a token this service really did issue.
const SEEDED_SERVICES: [&str; 3] = ["spelling-alpha", "spelling-beta", "spelling-gamma"];

/// The environment every seeded entity is hosted in, as `slo_lifecycle::sli_config` names it.
const ENVIRONMENT: &str = "prod";

/// The instrumentation scope the seeded configurations belong to. `InstrumentationType` is a
/// modelled enum value (`smithy_query(shape="InstrumentationType")` -> `["BREAKPOINT",
/// "PROBE"]`).
const INSTRUMENTATION_TYPE: &str = "PROBE";
const INSTRUMENTED_SERVICE: &str = "spelling-alpha";

/// The only `DynamicInstrumentationSignalType` the model declares
/// (`smithy_query(shape="DynamicInstrumentationSignalType")` -> `["SNAPSHOT"]`).
const SIGNAL_TYPE: &str = "SNAPSHOT";

/// How many exclusion windows and instrumentation configurations the seed stores: more than
/// one, so a page of one of each is followed by another page.
const SEEDED_PER_SET: usize = 3;

/// One HTTP request as this suite's harness takes it: method, URI and optional JSON body.
type Request = (&'static str, String, Option<Value>);

/// Builds the request that reads one entry, resuming from the given cursor when there is one.
type RequestBuilder = Box<dyn Fn(Option<&str>) -> Request>;

/// One read of this service whose `NextToken` this service resolves.
struct Read {
    /// The operation under test, named in every assertion.
    operation: &'static str,
    /// The output member the page is reported in.
    items: &'static str,
    /// The message this read answers a token it did not issue with: the service's one
    /// wording, narrowed by the reads whose cursor is only a cursor within one scope.
    refusal: String,
    /// The request reading ONE entry, resuming from `token` when it is `Some`, with every
    /// member in the place this operation's model binds it to.
    request: RequestBuilder,
}

/// Seconds since the epoch, for a window around the instant the seeded SLOs are written at.
fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64
}

/// The SLO name the SLO monitoring `service` is stored under.
fn slo_name(service: &str) -> String {
    format!("{service}-latency")
}

/// One instrumentation configuration in the seeded scope, distinct in the line it instruments.
fn instrumentation_body(line: usize) -> Value {
    json!({
        "InstrumentationType": INSTRUMENTATION_TYPE,
        "Service": INSTRUMENTED_SERVICE,
        "Environment": ENVIRONMENT,
        "SignalType": SIGNAL_TYPE,
        "Location": {
            "CodeLocation": {
                "Language": "Java",
                "CodeUnit": "com.amazon.spelling",
                "ClassName": "CursorSpelling",
                "MethodName": "resume",
                "FilePath": "src/spelling/CursorSpelling.java",
                "LineNumber": line
            }
        },
        "CaptureConfiguration": { "CodeCapture": { "CaptureLimits": {} } }
    })
}

/// Store the sets every read below pages over: one SLO per entry of [`SEEDED_SERVICES`],
/// several exclusion windows on the first of them, and several instrumentation
/// configurations in one scope.
async fn seed(router: &MultiServiceRouter) {
    for service in SEEDED_SERVICES {
        let (status, created) = create(
            router,
            json!({
                "Name": slo_name(service),
                "SliConfig": sli_config(service, "GET /cart", 100.0)
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {service:?}: expected the setup create to answer 200, got {status} with {created}"
        );
    }

    // `Window.Duration` targets `ExclusionDuration`, `range 1..`
    // (`smithy_query(shape="Window")`), so every duration here is at least 1.
    let windows: Vec<Value> = (0..SEEDED_PER_SET)
        .map(|index| {
            window(
                DURATION_UNITS[index % DURATION_UNITS.len()],
                (index + 1) as i64,
                Some(1_700_000_000 + index as i64),
                None,
                Some(&format!("spelling window {index}")),
            )
        })
        .collect();
    let (status, updated) = batch(router, &[slo_name(SEEDED_SERVICES[0])], &windows, &[]).await;
    assert_eq!(
        status, 200,
        "case exclusion windows: expected the setup add to answer 200, got {status} with {updated}"
    );

    for index in 0..SEEDED_PER_SET {
        let (status, created) = call(
            router,
            "POST",
            "/create-instrumentation-configuration",
            Some(instrumentation_body(index + 1)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case instrumentation configuration {index}: expected the setup create to answer \
             200, got {status} with {created}"
        );
    }
}

/// The query-string tail carrying a cursor, appended after the other query members, empty
/// when there is no cursor.
fn cursor_query(token: Option<&str>) -> String {
    token.map_or_else(String::new, |token| format!("&NextToken={token}"))
}

/// A body member carrying a cursor, added only when there is one.
fn with_cursor(mut body: Value, token: Option<&str>) -> Value {
    if let Some(token) = token {
        body["NextToken"] = json!(token);
    }
    body
}

/// Every read under test, each pointed at a one-entry page of the seeded sets.
fn reads(now: i64) -> Vec<Read> {
    let window = format!("StartTime={}&EndTime={}", now - 3_600, now + 3_600);
    let services_window = window.clone();
    vec![
        // A cursor that is an OFFSET into a derived projection, with `NextToken` bound to
        // the query string of a GET.
        Read {
            operation: "ListServices",
            items: "ServiceSummaries",
            refusal: UNISSUED.to_string(),
            request: Box::new(move |token| {
                (
                    "GET",
                    format!(
                        "/services?{services_window}&MaxResults=1{}",
                        cursor_query(token)
                    ),
                    None,
                )
            }),
        },
        // The same kind of cursor, with every input member — `NextToken` included — bound to
        // the JSON body.
        Read {
            operation: "ListServiceStates",
            items: "ServiceStates",
            refusal: UNISSUED.to_string(),
            request: Box::new(move |token| {
                (
                    "POST",
                    "/service/states".to_string(),
                    Some(with_cursor(
                        json!({
                            "StartTime": now - 3_600,
                            "EndTime": now + 3_600,
                            "MaxResults": 1
                        }),
                        token,
                    )),
                )
            }),
        },
        // A cursor that is a KEY of the store.
        Read {
            operation: "ListServiceLevelObjectives",
            items: "SloSummaries",
            refusal: UNISSUED.to_string(),
            request: Box::new(|token| {
                (
                    "POST",
                    format!("/slos?MaxResults=1{}", cursor_query(token)),
                    Some(json!({})),
                )
            }),
        },
        // A cursor that is an ORDINAL of a stored set, on a read whose subject is a path
        // label.
        Read {
            operation: "ListServiceLevelObjectiveExclusionWindows",
            items: "ExclusionWindows",
            refusal: UNISSUED.to_string(),
            request: Box::new(|token| {
                (
                    "GET",
                    format!(
                        "/slo/{}/exclusion-windows?MaxResults=1{}",
                        name_path_segment(&slo_name(SEEDED_SERVICES[0])),
                        cursor_query(token)
                    ),
                    None,
                )
            }),
        },
        // A cursor that is a key within the ONE scope that issued it, which is why this read
        // narrows the refusal to that scope.
        Read {
            operation: "ListInstrumentationConfigurations",
            items: "LatestConfigurations",
            refusal: format!("{UNISSUED} for this service, environment and instrumentation type"),
            request: Box::new(|token| {
                (
                    "POST",
                    "/list-instrumentation-configurations".to_string(),
                    Some(with_cursor(
                        json!({
                            "InstrumentationType": INSTRUMENTATION_TYPE,
                            "Service": INSTRUMENTED_SERVICE,
                            "Environment": ENVIRONMENT,
                            "MaxResults": 1
                        }),
                        token,
                    )),
                )
            }),
        },
    ]
}

/// Re-spell `token` by upper-casing the hex digits whose letter positions `select` accepts,
/// leaving every other character exactly as issued.
fn respell(token: &str, select: impl Fn(usize) -> bool) -> String {
    let mut letters = 0usize;
    token
        .chars()
        .map(|digit| {
            if digit.is_ascii_alphabetic() {
                let position = letters;
                letters += 1;
                if select(position) {
                    return digit.to_ascii_uppercase();
                }
            }
            digit
        })
        .collect()
}

/// Every re-spelling of `token` under test: what the case is, and the token as re-spelt.
fn respellings(case: &str, token: &str) -> Vec<(String, String)> {
    let letters = token.chars().filter(char::is_ascii_alphabetic).count();
    assert!(
        letters >= 2,
        "case {case:?}: an issued token must carry letter digits to re-spell, got {letters} \
         in {token:?}"
    );
    vec![
        (
            "every letter digit re-spelt".to_string(),
            token.to_ascii_uppercase(),
        ),
        (
            "the first letter digit re-spelt".to_string(),
            respell(token, |position| position == 0),
        ),
        (
            "the last letter digit re-spelt".to_string(),
            respell(token, |position| position == letters - 1),
        ),
        (
            "every other letter digit re-spelt".to_string(),
            respell(token, |position| position % 2 == 0),
        ),
    ]
}

/// The first one-entry page of `read`: the cursor it issued, and the entries it served.
async fn first_page(router: &MultiServiceRouter, read: &Read) -> (String, Value) {
    let (method, uri, body) = (read.request)(None);
    let (status, page) = call(router, method, &uri, body).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected the first page to answer 200, got {status} with {page}",
        read.operation
    );
    let token = page["NextToken"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: the seeded set holds more than one entry, so a page of one must \
                 carry a NextToken, got {page}",
                read.operation
            )
        })
        .to_string();
    let items = page[read.items].clone();
    (token, items)
}

/// A re-spelt token names the SAME cursor bytes as the issued one — so nothing but how its
/// digits are written separates it from the token this service handed out.
#[tokio::test]
async fn a_respelt_token_carries_the_issued_cursor_bytes_in_a_spelling_the_service_never_mints() {
    let router = router();
    seed(&router).await;
    let now = now_seconds();

    for read in reads(now) {
        let (issued, _items) = first_page(&router, &read).await;
        assert_eq!(
            issued,
            issued.to_ascii_lowercase(),
            "case {:?}: the minted token is lower-case hex, got {issued:?}",
            read.operation
        );
        let issued_bytes = ::hex::decode(&issued).unwrap_or_else(|error| {
            panic!(
                "case {:?}: an issued token is hex, got {issued:?} ({error})",
                read.operation
            )
        });
        for (why, respelt) in respellings(read.operation, &issued) {
            let case = format!("{} with {why}", read.operation);
            assert_ne!(
                respelt, issued,
                "case {case:?}: the re-spelling must differ from the issued token"
            );
            let respelt_bytes = ::hex::decode(&respelt).unwrap_or_else(|error| {
                panic!("case {case:?}: a re-spelling is still hex, got {respelt:?} ({error})")
            });
            assert_eq!(
                respelt_bytes, issued_bytes,
                "case {case:?}: expected the issued cursor bytes, got {respelt_bytes:?}"
            );
        }
    }
}

/// Every cursor-resolving read refuses a re-spelt token with the modelled
/// `ValidationException` and its own unissued-token wording, carrying no page, no cursor and
/// no echo of the rejected token — while the token as issued still resumes the page after
/// the entry the first page served.
#[tokio::test]
async fn every_cursor_resolving_read_refuses_a_respelt_token_and_resumes_the_issued_one() {
    let router = router();
    seed(&router).await;
    let now = now_seconds();

    for read in reads(now) {
        let (issued, first_items) = first_page(&router, &read).await;

        for (why, respelt) in respellings(read.operation, &issued) {
            let case = format!("{} with {why}", read.operation);
            let (method, uri, body) = (read.request)(Some(&respelt));
            let (status, answer) = call(&router, method, &uri, body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for a token this service never minted, got \
                 {status} with {answer}"
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
                message, read.refusal,
                "case {case:?}: expected the message {:?}, got {message:?}",
                read.refusal
            );
            assert!(
                !message.contains(&respelt),
                "case {case:?}: the message must not echo the rejected token, got {message:?}"
            );
            assert!(
                answer.get(read.items).is_none() && answer.get("NextToken").is_none(),
                "case {case:?}: a refused cursor carries no page and no cursor, got {answer}"
            );
        }

        // The positive control: the token as this service spelt it still resumes, and resumes
        // AFTER the entry the first page served.
        let (method, uri, body) = (read.request)(Some(&issued));
        let (status, resumed) = call(&router, method, &uri, body).await;
        assert_eq!(
            status, 200,
            "case {:?} resuming from the issued token: expected 200, got {status} with {resumed}",
            read.operation
        );
        assert_ne!(
            resumed[read.items], first_items,
            "case {:?} resuming from the issued token: expected the page after the first, got \
             {resumed}",
            read.operation
        );
    }
}
