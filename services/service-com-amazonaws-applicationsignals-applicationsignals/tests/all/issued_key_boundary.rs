// SPDX-License-Identifier: CC0-1.0

//! The KEY a page cursor carries is honoured only when it is one this service issued.
//!
//! The rule under test: a `NextToken` is opaque but NOT authenticated — it is the hex of a tag
//! and a payload — so the payload inside the tag is a value a caller may have written, and a
//! read whose cursor is a stored KEY resolves it only when it is a boundary its own paging
//! could have handed out over the set being read: spelled the way that collection's keys are
//! spelled, and strictly before the last key the set now holds. Anything else is refused with
//! the modelled `ValidationException` (HTTP 400) and that read's wording for a token this
//! service did not issue, rather than honoured as a page boundary — honouring it would serve
//! the first page again under a cursor, or report the set as exhausted to a caller whose
//! cursor the service never gave it.
//!
//! A key cursor keys on a stored record, so a key INSIDE that range is honoured whether or not
//! the set still holds that very record: that is the property a key cursor is chosen for, and
//! the last test here pins it — a boundary whose record is deleted between two pages still
//! resumes, which is why the low end of the range is the collection's key SPACE rather than
//! the first key it currently holds.
//!
//! The rows span what the rule generalises over: WHICH read resolves the cursor (both of this
//! service's whose cursor is a stored key — the objective listing, keyed by SLO name, and the
//! scoped instrumentation listing, keyed by signal type and location hash — differing in route,
//! method, in where their model binds the page members and in what their key is SPELT like),
//! and WHY the payload is none of that collection's keys (it carries no key at all, it is
//! misspelt in each way the key's own model forbids, it names the last key, it sorts past every
//! key the set holds).
//!
//! `issued_page_position` is the same rule for the reads whose payload is a decimal position,
//! and `unissued_next_token` the half of it that is about the TAG; this suite is the payload
//! half for the reads whose payload is a KEY.
//!
//! The harness is `slo_lifecycle`'s, so this suite speaks to the same mounted router as every
//! other.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The hex plaintext of a token is read through the helper the sibling payload suite already
// owns: both suites compile into the one `all` test binary, so the decode lives in one place.
use crate::issued_page_position::plaintext;
use crate::slo_lifecycle::{call, create, name_path_segment, router, sli_config};

/// The ONE message a read of this service answers a `NextToken` it did not issue with.
const UNISSUED: &str = "NextToken is not a token this service issued";

/// The services the seeded objectives monitor: three, so every set below holds a key a cursor
/// can sit before, on and after.
const SEEDED_SERVICES: [&str; 3] = ["boundary-alpha", "boundary-beta", "boundary-gamma"];

/// The environment every seeded entity is hosted in, as `slo_lifecycle::sli_config` names it.
const ENVIRONMENT: &str = "prod";

/// The instrumentation scope the seeded configurations belong to. `InstrumentationType` is a
/// modelled enum value (`smithy_query(shape="InstrumentationType")` -> `["BREAKPOINT",
/// "PROBE"]`).
const INSTRUMENTATION_TYPE: &str = "PROBE";

/// The only `DynamicInstrumentationSignalType` the model declares
/// (`smithy_query(shape="DynamicInstrumentationSignalType")` -> `["SNAPSHOT"]`), which is the
/// first half of a configuration's key within its scope.
const SIGNAL_TYPE: &str = "SNAPSHOT";

/// How many configurations the seed stores in that scope.
const SEEDED_PER_SET: usize = 3;

/// A page bound that holds every seeded set whole, inside the smallest maximum these reads'
/// `MaxResults` members declare (`ListServiceLevelObjectivesMaxResults`, `range 1..50`).
const WHOLE_SET_BOUND: usize = 50;

/// One HTTP request as this suite's harness takes it: method, URI and optional JSON body.
type Request = (&'static str, String, Option<Value>);

/// Builds the request reading one page of `max_results`, resuming from `token` when given.
type RequestBuilder = Box<dyn Fn(usize, Option<&str>) -> Request>;

/// The payloads that are no key of a collection at all, given `key`, one key it really holds —
/// so each row differs from a key of the set in its SPELLING alone.
type Misspellings = fn(&str) -> Vec<(&'static str, String)>;

/// One read of this service whose cursor is a stored key.
struct Read {
    /// The operation under test, named in every assertion.
    operation: &'static str,
    /// The output member carrying the page.
    items: &'static str,
    /// The message this read answers a token it did not issue with: the service's one wording,
    /// narrowed by the reads whose cursor is only a cursor within one scope.
    refusal: String,
    /// The key the set holds one reported entry under.
    key_of: fn(&Value) -> String,
    /// The payloads this collection's keys are never spelled as.
    misspellings: Misspellings,
    /// A well-spelled key of this collection sorting strictly after `last`.
    beyond: fn(&str) -> String,
    request: RequestBuilder,
}

/// The SLO name the objective monitoring `service` is stored under.
fn slo_name(service: &str) -> String {
    format!("{service}-latency")
}

/// One instrumentation configuration in the seeded scope, distinct in the line it instruments.
fn instrumentation_body(line: usize) -> Value {
    json!({
        "InstrumentationType": INSTRUMENTATION_TYPE,
        "Service": SEEDED_SERVICES[0],
        "Environment": ENVIRONMENT,
        "SignalType": SIGNAL_TYPE,
        "Location": {
            "CodeLocation": {
                "Language": "Java",
                "CodeUnit": "com.amazon.boundary",
                "ClassName": "KeyBoundary",
                "MethodName": "resume",
                "FilePath": "src/boundary/KeyBoundary.java",
                "LineNumber": line
            }
        },
        "CaptureConfiguration": { "CodeCapture": { "CaptureLimits": {} } }
    })
}

/// Store the sets the reads below page over: one objective per entry of [`SEEDED_SERVICES`],
/// and [`SEEDED_PER_SET`] instrumentation configurations in one scope.
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

/// The SLO name an objective summary carries, which is the key the store holds it under.
fn slo_key(item: &Value) -> String {
    item["Name"].as_str().unwrap_or_default().to_string()
}

/// The key a scope holds one configuration under: its signal type and its location hash, the
/// two parts `state::instrumentation_key` spells.
fn instrumentation_key(item: &Value) -> String {
    format!(
        "{}/{}",
        item["SignalType"].as_str().unwrap_or_default(),
        item["LocationHash"].as_str().unwrap_or_default()
    )
}

/// The payloads a `ServiceLevelObjectiveName` is never spelled as, from the member's own
/// modelled pattern (`smithy_query(shape="ServiceLevelObjectiveName")`:
/// `^[0-9A-Za-z][-._0-9A-Za-z ]{0,126}[0-9A-Za-z]$`) — each of its three parts broken in turn,
/// and the length it bounds overrun by one.
fn slo_misspellings(key: &str) -> Vec<(&'static str, String)> {
    vec![
        (
            "a single character, which a pattern anchoring two admits no name of",
            "a".to_string(),
        ),
        ("a leading separator", format!("-{key}")),
        ("a trailing separator", format!("{key}.")),
        (
            "a character the name pattern admits in no position",
            format!("{key}:1"),
        ),
        (
            "one character past the longest name the pattern admits",
            "a".repeat(129),
        ),
    ]
}

/// The payloads an instrumentation key is never spelled as: a signal type outside the modelled
/// `DynamicInstrumentationSignalType` values, a missing separator, and a `LocationHash` that is
/// not the 16 lowercase hex characters its `length 16..16` and the
/// `InstrumentationConfigurationArn` pattern's hash segment fix.
fn instrumentation_misspellings(key: &str) -> Vec<(&'static str, String)> {
    let hash = key
        .split_once('/')
        .map(|(_, hash)| hash.to_string())
        .unwrap_or_default();
    vec![
        ("no separator at all", SIGNAL_TYPE.to_string()),
        (
            "a signal type the model declares no such value for",
            format!("PROBE/{hash}"),
        ),
        (
            "a hash one character short",
            format!("{SIGNAL_TYPE}/{}", &hash[..hash.len() - 1]),
        ),
        (
            "a hash one character long",
            format!("{SIGNAL_TYPE}/{hash}0"),
        ),
        (
            "a hash in upper case, which the hash this service writes is never in",
            format!("{SIGNAL_TYPE}/{}", hash.to_ascii_uppercase()),
        ),
        (
            "a hash carrying a character hex does not spell",
            format!("{SIGNAL_TYPE}/-{}", &hash[1..]),
        ),
    ]
}

/// A well-formed SLO name sorting after `last`: `last` with a further segment, which the name
/// pattern admits and which sorts after every name `last` is a prefix of.
fn slo_beyond(last: &str) -> String {
    format!("{last}-z")
}

/// A well-spelled instrumentation key sorting after every key a scope can hold: the largest
/// `LocationHash` the 16 lowercase hex characters admit.
fn instrumentation_beyond(_last: &str) -> String {
    format!("{SIGNAL_TYPE}/{}", "f".repeat(16))
}

/// Both reads whose cursor is a stored key, each pointed at a seeded set of its own.
fn reads() -> Vec<Read> {
    vec![
        Read {
            operation: "ListServiceLevelObjectives",
            items: "SloSummaries",
            refusal: UNISSUED.to_string(),
            key_of: slo_key,
            misspellings: slo_misspellings,
            beyond: slo_beyond,
            request: Box::new(|max_results, token| {
                let mut query = format!("MaxResults={max_results}");
                if let Some(token) = token {
                    query.push_str(&format!("&NextToken={token}"));
                }
                ("POST", format!("/slos?{query}"), Some(json!({})))
            }),
        },
        Read {
            operation: "ListInstrumentationConfigurations",
            items: "LatestConfigurations",
            // This read's cursor is only a cursor within the scope that issued it, so its
            // refusal names that scope after the same sentence.
            refusal: format!("{UNISSUED} for this service, environment and instrumentation type"),
            key_of: instrumentation_key,
            misspellings: instrumentation_misspellings,
            beyond: instrumentation_beyond,
            request: Box::new(|max_results, token| {
                let mut body = json!({
                    "InstrumentationType": INSTRUMENTATION_TYPE,
                    "Service": SEEDED_SERVICES[0],
                    "Environment": ENVIRONMENT,
                    "MaxResults": max_results
                });
                if let Some(token) = token {
                    body["NextToken"] = json!(token);
                }
                (
                    "POST",
                    "/list-instrumentation-configurations".to_string(),
                    Some(body),
                )
            }),
        },
    ]
}

/// A token carrying `payload` under the tag of the token this service really issued for `key` —
/// what a caller does to forge one: keep the tag it was given and write its own payload inside
/// it.
fn repayloaded(token: &str, key: &str, payload: &str) -> String {
    let plain = plaintext(token);
    let tag = plain
        .strip_suffix(key)
        .expect("an issued key cursor ends with the key it bounds the page at");
    ::hex::encode(format!("{tag}{payload}"))
}

/// One page of `read`, bounded by `max_results` and resuming from `token` when given.
async fn page(
    router: &MultiServiceRouter,
    read: &Read,
    max_results: usize,
    token: Option<&str>,
) -> (u16, Value) {
    let (method, uri, body) = (read.request)(max_results, token);
    call(router, method, &uri, body).await
}

/// The keys a page reports, in order.
fn keys(read: &Read, answer: &Value) -> Vec<String> {
    answer[read.items]
        .as_array()
        .map(|items| items.iter().map(read.key_of).collect())
        .unwrap_or_default()
}

/// Assert that `answer` is this read's refusal of a token it did not issue, carrying no page,
/// no cursor and no echo of the rejected token.
fn assert_refused(case: &str, read: &Read, status: u16, answer: &Value, token: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 for a key this service never issued, got {status} with {answer}"
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
        !message.contains(token),
        "case {case:?}: the message must not echo the rejected token, got {message:?}"
    );
    assert!(
        answer.get(read.items).is_none() && answer.get("NextToken").is_none(),
        "case {case:?}: a refused cursor carries no page and no cursor, got {answer}"
    );
}

/// Every read whose cursor is a stored key refuses a payload that is no boundary its paging
/// could have minted over the set being read, and keeps honouring the boundaries it could.
#[tokio::test]
async fn a_key_cursor_honours_only_a_boundary_this_service_could_have_issued() {
    let router = router();
    seed(&router).await;

    for read in reads() {
        let operation = read.operation;

        // The WHOLE set this read pages, which is what the payloads below are judged against.
        let (status, whole) = page(&router, &read, WHOLE_SET_BOUND, None).await;
        assert_eq!(
            status, 200,
            "case {operation:?}: expected the unbounded read to be served, got {status} with {whole}"
        );
        assert!(
            whole["NextToken"].is_null(),
            "case {operation:?}: expected the whole set on one page of {WHOLE_SET_BOUND}, got {whole}"
        );
        let set = keys(&read, &whole);
        assert_eq!(
            set.len(),
            SEEDED_PER_SET,
            "case {operation:?}: expected the seeding to leave {SEEDED_PER_SET} entries to page, got {set:?}"
        );
        let last = set
            .last()
            .unwrap_or_else(|| panic!("case {operation:?}: expected a last key, got {set:?}"))
            .clone();

        // A page of one of several issues a token, and the payload inside it is the key of the
        // entry it just served.
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
        assert!(
            plaintext(&issued).ends_with(&set[0]),
            "case {operation:?}: expected the token of a page of one to carry the key it served, got {:?}",
            plaintext(&issued)
        );

        // The boundaries this read's paging CAN mint over this set are still honoured, and
        // serve the entries after them.
        for position in 0..set.len() - 1 {
            let case = format!("{operation} resuming at the key {:?}", set[position]);
            let token = repayloaded(&issued, &set[0], &set[position]);
            let (status, resumed) = page(&router, &read, 1, Some(&token)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected a boundary this paging can mint to be served, got {status} with {resumed}"
            );
            assert_eq!(
                keys(&read, &resumed),
                set[position + 1..position + 2].to_vec(),
                "case {case:?}: expected the entry after that key, got {resumed}"
            );
        }

        // No payload that is not such a boundary is one this service issued: one carrying no
        // key at all, one misspelt in each way the key's own model forbids, the last key (which
        // bounds no page) and one sorting past every key the set holds.
        let mut unissued: Vec<(String, String)> = vec![(
            "the bare tag, which carries no key at all".to_string(),
            String::new(),
        )];
        for (why, payload) in (read.misspellings)(&set[0]) {
            assert!(
                payload.as_str() < last.as_str(),
                "case {operation:?}: a misspelt payload must sort before the last key {last:?} so \
                 the refusal is earned by its spelling, got {payload:?}"
            );
            unissued.push((why.to_string(), payload));
        }
        unissued.push((
            "the last key the set holds, which bounds no page".to_string(),
            last.clone(),
        ));
        let beyond = (read.beyond)(&last);
        assert!(
            beyond.as_str() > last.as_str(),
            "case {operation:?}: expected a key sorting after the last key {last:?}, got {beyond:?}"
        );
        unissued.push((
            "a key sorting after every key the set holds".to_string(),
            beyond,
        ));

        for (why, payload) in unissued {
            let case = format!("{operation} with {why}");
            let token = repayloaded(&issued, &set[0], &payload);
            let (status, answer) = page(&router, &read, 1, Some(&token)).await;
            assert_refused(&case, &read, status, &answer, &token);
        }

        // The refusals changed nothing: the whole set reads back as it did before them.
        let (status, after) = page(&router, &read, WHOLE_SET_BOUND, None).await;
        assert_eq!(
            status, 200,
            "case {operation:?}: expected the read after the refusals to be served, got {status} with {after}"
        );
        assert_eq!(
            keys(&read, &after),
            set,
            "case {operation:?}: expected the refused cursors to leave the set as it was, got {after}"
        );
    }
}

/// A token this service DID issue still resumes after the record its key names is removed: the
/// boundary is a position in the stored order, so a write between two pages leaves the rest of
/// the listing reachable rather than earning the caller a refusal.
#[tokio::test]
async fn an_issued_boundary_still_resumes_once_its_own_record_is_gone() {
    let router = router();
    seed(&router).await;
    let read = reads()
        .into_iter()
        .next()
        .expect("the objective listing is the first read");

    let (status, first) = page(&router, &read, 1, None).await;
    assert_eq!(
        status, 200,
        "expected the first page to be served, got {status} with {first}"
    );
    let token = first["NextToken"]
        .as_str()
        .expect("a page of one of several objectives carries a cursor")
        .to_string();
    let boundary = keys(&read, &first)
        .first()
        .expect("the first page reports one objective")
        .clone();

    let (status, deleted) = call(
        &router,
        "DELETE",
        &format!("/slo/{}", name_path_segment(&boundary)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the objective the cursor keys on to be deleted, got {status} with {deleted}"
    );

    let (status, resumed) = page(&router, &read, WHOLE_SET_BOUND, Some(&token)).await;
    assert_eq!(
        status, 200,
        "expected the issued boundary to resume after its own record was removed, got {status} \
         with {resumed}"
    );
    let expected: Vec<String> = SEEDED_SERVICES[1..]
        .iter()
        .map(|service| slo_name(service))
        .collect();
    assert_eq!(
        keys(&read, &resumed),
        expected,
        "expected the rest of the listing, got {resumed}"
    );
    assert!(
        resumed["NextToken"].is_null(),
        "expected no cursor once the objectives run out, got {resumed}"
    );
}

/// A boundary whose set has lost every record AFTER it is refused rather than answered with an
/// empty last page: the remaining set cannot bound a page at that key, so it is no boundary
/// over the set being read — and the refusal loses nothing, because every survivor sorts before
/// the boundary and a cursorless read still reports them all.
///
/// The high end of the range is the set as it stands now for that reason, which is also the
/// answer the ordinal cursor of the exclusion-window read gives
/// (`exclusion_window_page_stability::a_boundary_the_remaining_set_cannot_bound_a_page_at_is_refused`)
/// and the one the offset cursors give a position past the end of what they report.
#[tokio::test]
async fn a_boundary_the_remaining_set_cannot_bound_a_page_at_is_refused() {
    let router = router();
    seed(&router).await;
    let read = reads()
        .into_iter()
        .next()
        .expect("the objective listing is the first read");

    let (status, first) = page(&router, &read, 1, None).await;
    assert_eq!(
        status, 200,
        "expected the first page to be served, got {status} with {first}"
    );
    let token = first["NextToken"]
        .as_str()
        .expect("a page of one of several objectives carries a cursor")
        .to_string();

    // Every objective AFTER the boundary is removed, so the boundary bounds no page over what
    // is left.
    for service in &SEEDED_SERVICES[1..] {
        let name = slo_name(service);
        let (status, deleted) = call(
            &router,
            "DELETE",
            &format!("/slo/{}", name_path_segment(&name)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected the objective after the boundary to be deleted, got {status} with {deleted}"
        );
    }

    let (status, answer) = page(&router, &read, WHOLE_SET_BOUND, Some(&token)).await;
    assert_refused(
        "the boundary of a listing whose later objectives are gone",
        &read,
        status,
        &answer,
        &token,
    );

    // The survivor still reads back whole from the start, which is why the refusal costs the
    // caller no record.
    let (status, whole) = page(&router, &read, WHOLE_SET_BOUND, None).await;
    assert_eq!(
        status, 200,
        "expected the cursorless read to be served after the refusal, got {status} with {whole}"
    );
    assert_eq!(
        keys(&read, &whole),
        vec![slo_name(SEEDED_SERVICES[0])],
        "expected the surviving objective to read back from the start, got {whole}"
    );
}
