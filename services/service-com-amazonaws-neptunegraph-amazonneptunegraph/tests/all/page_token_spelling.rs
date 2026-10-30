// SPDX-License-Identifier: CC0-1.0

//! Which SPELLINGS of a `nextToken` every paginated listing of this service accepts,
//! driven over the wire (`restJson1`) through the same router the emulator serves from.
//!
//! The rule every assertion here is about: the set of pagination tokens a listing
//! accepts is exactly the set it mints, so a value is a token of this service only when
//! it is — byte for byte — the token the service would hand out for the position it
//! names; any other spelling of the same position is a token the service never issued
//! and is refused with the modelled `ValidationException` rather than served as a page.
//!
//! The table spans what the rule generalises over: every listing the service serves
//! (`ListGraphs`, `ListGraphSnapshots`, `ListImportTasks`, `ListExportTasks`,
//! `ListPrivateGraphEndpoints`), two positions per listing in its own id space, and,
//! per position, the FORM of the non-canonical spelling — every token letter recased,
//! only the first, only the last, and every other one — each of which carries exactly
//! the bytes of the issued token and differs from it only in spelling. The canonical
//! spelling of each of those positions is asked for too, so the refusals are about the
//! spelling and not about the position.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// The smallest `provisionedMemory` the model allows (`range 16..24576`), which is all
/// this suite needs from a graph.
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// The modelled `reason` a token the caller sent that this service cannot resolve to a
/// position is classified by, from `ValidationExceptionReason`'s values
/// (`smithy_query(query="shape",shape="ValidationExceptionReason")`): the request named
/// an argument this operation does not accept.
const ILLEGAL_ARGUMENT: &str = "ILLEGAL_ARGUMENT";

/// An emulator holding only this service, on its own engine, so each test starts from an
/// empty store.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one `restJson1` request and return its status and parsed JSON body.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |value| {
        serde_json::to_vec(&value).expect("a JSON request body serializes")
    });
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/neptune-graph/\
                 aws4_request, SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code of a failure body (`__type`), without its namespace.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Create one graph and return its id, so the endpoint listing has a graph to be a
/// listing OF and the graph listing has a row.
async fn graph_id(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({"graphName": name, "provisionedMemory": MIN_PROVISIONED_MEMORY});
    let (status, created) = call(router, "POST", "/graphs", Some(body.clone())).await;
    assert_eq!(
        status, 201,
        "case {body}: expected 201 from CreateGraph, got {status}: {created}"
    );
    created["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {body}: no id in {created}"))
        .to_string()
}

/// One paginated listing: the operation, where it is served, the member carrying its
/// entries, and two positions in the id space its tokens name a position in.
struct Listing {
    operation: &'static str,
    path: String,
    entries_member: &'static str,
    positions: [&'static str; 2],
}

/// The byte offsets of the letters of `token` — the only digits of the hex grammar that
/// have more than one spelling.
fn letter_offsets(token: &str) -> Vec<usize> {
    token
        .char_indices()
        .filter(|(_, digit)| digit.is_ascii_alphabetic())
        .map(|(offset, _)| offset)
        .collect()
}

/// `token` with the letters at `offsets` recased to upper case.
fn recased(token: &str, offsets: &[usize]) -> String {
    token
        .char_indices()
        .map(|(offset, digit)| {
            if offsets.contains(&offset) {
                digit.to_ascii_uppercase()
            } else {
                digit
            }
        })
        .collect()
}

/// Every spelling of `token` this suite asks with: the issued one recased in each of the
/// ways a hex spelling can differ while carrying the same bytes.
fn other_spellings(token: &str) -> Vec<(String, String)> {
    let letters = letter_offsets(token);
    assert!(
        !letters.is_empty(),
        "case {token:?}: the probe token has no letters, so it has only one spelling"
    );
    let last = letters[letters.len() - 1];
    let alternating: Vec<usize> = letters.iter().copied().step_by(2).collect();
    vec![
        ("every letter recased".to_string(), recased(token, &letters)),
        (
            "the first letter recased".to_string(),
            recased(token, &letters[..1]),
        ),
        (
            "the last letter recased".to_string(),
            recased(token, &[last]),
        ),
        (
            "every other letter recased".to_string(),
            recased(token, &alternating),
        ),
    ]
}

/// Ask `listing` with `token` and assert it is refused as a token this service never
/// issued: the modelled 400, naming the value the caller sent.
async fn assert_refused(router: &MultiServiceRouter, listing: &Listing, case: &str, token: &str) {
    let path = format!("{}?nextToken={token}", listing.path);
    let (status, answer) = call(router, "GET", &path, None).await;
    assert_eq!(
        status, 400,
        "case {case}: expected 400, got {status}: {answer}"
    );
    assert_eq!(
        error_code(&answer),
        "ValidationException",
        "case {case}: expected ValidationException, got {answer}"
    );
    let expected = format!("nextToken '{token}' is not a pagination token this service issued");
    let actual = answer["message"].as_str().unwrap_or_default();
    assert_eq!(
        actual, expected,
        "case {case}: expected {expected:?}, got {actual:?}"
    );
    let reason = answer["reason"].as_str();
    assert_eq!(
        reason,
        Some(ILLEGAL_ARGUMENT),
        "case {case}: expected reason {ILLEGAL_ARGUMENT:?}, got {reason:?} in {answer}"
    );
}

/// The five listings of this service, each with two positions in its own id space,
/// written from the modelled patterns `^g-[a-z0-9]{10}$` (`GraphId`),
/// `^gs-[a-z0-9]{10}$` (`SnapshotId`), `^t-[a-z0-9]{10}$` (`TaskId` / `ExportTaskId`)
/// and `^vpc-[a-z0-9]+$` (`VpcId`)
/// (`smithy_query(query="shape",shape="GraphId,SnapshotId,TaskId,ExportTaskId,VpcId")`).
fn listings(graph: &str) -> Vec<Listing> {
    vec![
        Listing {
            operation: "ListGraphs",
            path: "/graphs".to_string(),
            entries_member: "graphs",
            positions: ["g-0000000000", "g-abcdefghij"],
        },
        Listing {
            operation: "ListGraphSnapshots",
            path: "/snapshots".to_string(),
            entries_member: "graphSnapshots",
            positions: ["gs-0000000000", "gs-abcdefghij"],
        },
        Listing {
            operation: "ListImportTasks",
            path: "/importtasks".to_string(),
            entries_member: "tasks",
            positions: ["t-0000000000", "t-abcdefghij"],
        },
        Listing {
            operation: "ListExportTasks",
            path: "/exporttasks".to_string(),
            entries_member: "tasks",
            positions: ["t-0000000000", "t-abcdefghij"],
        },
        Listing {
            operation: "ListPrivateGraphEndpoints",
            path: format!("/graphs/{graph}/endpoints/"),
            entries_member: "privateGraphEndpoints",
            positions: ["vpc-0a1b2c3d4e5f00000", "vpc-abcdef"],
        },
    ]
}

#[tokio::test]
async fn every_listing_accepts_only_the_token_spelling_it_mints() {
    let router = emulator();
    let graph = graph_id(&router, "token-spelling-graph").await;

    for listing in &listings(&graph) {
        let operation = listing.operation;
        for position in listing.positions {
            // The one spelling the service mints for this position resumes the listing.
            let issued = hex::encode(position);
            let case = format!("{operation} with the issued token {issued:?}");
            let path = format!("{}?nextToken={issued}", listing.path);
            let (status, answer) = call(&router, "GET", &path, None).await;
            assert_eq!(
                status, 200,
                "case {case}: expected 200, got {status}: {answer}"
            );
            assert!(
                answer[listing.entries_member].is_array(),
                "case {case}: expected {} to be reported, got {answer}",
                listing.entries_member
            );

            // Every other spelling of the same bytes is a token this service never
            // handed out, so it names no page.
            for (form, spelling) in other_spellings(&issued) {
                let case = format!("{operation} with {form}: {spelling:?} for {position:?}");
                assert_ne!(
                    spelling, issued,
                    "case {case}: the probe is the issued spelling, got {spelling:?}"
                );
                assert_eq!(
                    hex::decode(&spelling).ok(),
                    hex::decode(&issued).ok(),
                    "case {case}: the probe does not carry the bytes of {issued:?}"
                );
                assert_refused(&router, listing, &case, &spelling).await;
            }
        }
    }
}

#[tokio::test]
async fn a_token_a_listing_issued_is_the_only_spelling_it_takes_back() {
    let router = emulator();
    // Two graphs, so a page of one entry leaves an entry behind and a token is minted.
    for i in 0..2 {
        graph_id(&router, &format!("token-spelling-issued-{i}")).await;
    }

    let (status, page) = call(&router, "GET", "/graphs?maxResults=1", None).await;
    assert_eq!(
        status, 200,
        "case first page: expected 200, got {status}: {page}"
    );
    let issued = page["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case first page: expected a nextToken, got {page}"))
        .to_string();

    let listing = Listing {
        operation: "ListGraphs",
        path: "/graphs".to_string(),
        entries_member: "graphs",
        positions: ["g-0000000000", "g-abcdefghij"],
    };

    // The token as issued resumes the listing...
    let path = format!("/graphs?maxResults=1&nextToken={issued}");
    let (status, resumed) = call(&router, "GET", &path, None).await;
    assert_eq!(
        status, 200,
        "case resume with {issued:?}: expected 200, got {status}: {resumed}"
    );
    assert_eq!(
        resumed["graphs"].as_array().map(Vec::len),
        Some(1),
        "case resume with {issued:?}: expected the remaining entry, got {resumed}"
    );

    // ...and no other spelling of it does.
    for (form, spelling) in other_spellings(&issued) {
        let case = format!("resume with {form}: {spelling:?} for the issued {issued:?}");
        assert_refused(&router, &listing, &case, &spelling).await;
    }
}
