// SPDX-License-Identifier: CC0-1.0

//! How every paginated listing of this service REFUSES a `nextToken` it did not issue,
//! driven over the wire (`restJson1`) through the same router the emulator serves from.
//!
//! The rule every assertion here is about: this service models its own
//! `ValidationException`, which carries a `reason` beside `message`
//! (`smithy_query(query="shape",shape="ValidationException")`), so a refusal of client
//! input is reported through THAT shape — 400, error code `ValidationException`, the
//! member and value named in the message, and the `reason` member filled with the
//! modelled value that classifies the failure — and never through a shape that carries
//! `message` alone.
//!
//! The table spans what the rule generalises over: every listing the service serves
//! (`ListGraphs`, `ListGraphSnapshots`, `ListImportTasks`, `ListExportTasks`,
//! `ListPrivateGraphEndpoints`), and, per listing, the FORM of the unissued token — not
//! hex at all, hex of a position in each other id space of the service, hex of a value
//! carrying this listing's own prefix but outside its pattern, hex of a value in no id
//! space, a bare position in this listing's own id space carrying no listing, and a
//! token three thousand characters long. Each listing is also asked with a token it DID
//! issue, so the refusals are about the token rather than about pagination being refused
//! outright.
//!
//! Which listing a token belongs to is `tests/all/page_token_listing_scope.rs`'s rule;
//! here it is only how a token this listing never issued is REPORTED.

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

/// One position per id space this service orders a listing by, written from the modelled
/// patterns `^g-[a-z0-9]{10}$` (`GraphId`), `^gs-[a-z0-9]{10}$` (`SnapshotId`),
/// `^t-[a-z0-9]{10}$` (`TaskId` / `ExportTaskId`) and `^vpc-[a-z0-9]+$` (`VpcId`)
/// (`smithy_query(query="shape",shape="GraphId,SnapshotId,TaskId,VpcId")`). A listing
/// accepts a token naming a position in its OWN space and refuses every other.
const ID_SPACES: [(&str, &str); 4] = [
    ("graph", "g-0123456789"),
    ("snapshot", "gs-0123456789"),
    ("task", "t-0123456789"),
    ("vpc", "vpc-0a1b2c3d4e5f00000"),
];

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

/// One paginated listing: the operation, where it is served, the member carrying its
/// entries, the id space its tokens name a position in, the scope its tokens belong to,
/// and a value carrying that space's prefix while falling outside its pattern.
struct Listing {
    operation: &'static str,
    path: String,
    entries_member: &'static str,
    space: &'static str,
    /// What a token of this listing carries ahead of the position: the operation's own
    /// name, and — for the listing whose entries are ordered inside one graph's own
    /// collection — that graph too.
    token_scope: String,
    prefixed_but_invalid: &'static str,
}

/// The token this service issues for `position` in the listing `scope` names: the scope
/// the token belongs to and the position it reached, hex-encoded, which is what binds a
/// token to one order.
fn issued_token(scope: &str, position: &str) -> String {
    hex::encode(format!("{scope}{position}"))
}

/// Create one graph and return its id, so the endpoint listing has a graph to be a
/// listing OF — its token gate runs ahead of that lookup, which the refusal rows below
/// do not depend on, but the accepted row does.
async fn graph_id(router: &MultiServiceRouter) -> String {
    let body = json!({"graphName": "token-refusal-graph", "provisionedMemory": MIN_PROVISIONED_MEMORY});
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

#[tokio::test]
async fn every_listing_refuses_an_unissued_token_with_the_modelled_validation_exception() {
    let router = emulator();
    let graph = graph_id(&router).await;

    let listings = [
        Listing {
            operation: "ListGraphs",
            path: "/graphs".to_string(),
            entries_member: "graphs",
            space: "graph",
            token_scope: "ListGraphs:".to_string(),
            prefixed_but_invalid: "g-short",
        },
        Listing {
            operation: "ListGraphSnapshots",
            path: "/snapshots".to_string(),
            entries_member: "graphSnapshots",
            space: "snapshot",
            token_scope: "ListGraphSnapshots:".to_string(),
            prefixed_but_invalid: "gs-short",
        },
        Listing {
            operation: "ListImportTasks",
            path: "/importtasks".to_string(),
            entries_member: "tasks",
            space: "task",
            token_scope: "ListImportTasks:".to_string(),
            prefixed_but_invalid: "t-short",
        },
        Listing {
            operation: "ListExportTasks",
            path: "/exporttasks".to_string(),
            entries_member: "tasks",
            space: "task",
            token_scope: "ListExportTasks:".to_string(),
            prefixed_but_invalid: "t-short",
        },
        Listing {
            operation: "ListPrivateGraphEndpoints",
            path: format!("/graphs/{graph}/endpoints/"),
            entries_member: "privateGraphEndpoints",
            space: "vpc",
            // This listing pages the endpoints held under ONE graph, so the graph is
            // part of what its tokens carry.
            token_scope: format!("ListPrivateGraphEndpoints:{graph}:"),
            // `VpcId`'s pattern requires at least one character after the prefix.
            prefixed_but_invalid: "vpc-",
        },
    ];

    for listing in &listings {
        let operation = listing.operation;

        // Tokens this service never issued for THIS listing's id space, spanning the
        // forms such a value can take.
        let mut tokens: Vec<String> = vec![
            // Not a token this codec could have minted: not hex at all.
            "not-a-token".to_string(),
            // Hex-alphabet but an odd number of digits, so it decodes to nothing.
            "abc".to_string(),
            // Decodes cleanly, but names a position in no id space of the service.
            hex::encode("not-a-position-at-all"),
            // Decodes cleanly to this listing's own prefix with a suffix outside the
            // pattern that space is spelled with.
            hex::encode(listing.prefixed_but_invalid),
            // Three thousand characters, well inside `PaginationToken`'s 1..8192, so
            // the refusal is not about the length of the value.
            hex::encode("z".repeat(1500)),
            // A bare position in this listing's own id space, naming no listing at all:
            // not a value this service ever handed out.
            hex::encode(
                ID_SPACES
                    .iter()
                    .find(|(space, _)| *space == listing.space)
                    .map(|(_, position)| *position)
                    .unwrap_or_default(),
            ),
        ];
        // Every OTHER id space of the service: one listing's token never pages another's.
        tokens.extend(
            ID_SPACES
                .iter()
                .filter(|(space, _)| *space != listing.space)
                .map(|(_, position)| hex::encode(position)),
        );

        for token in &tokens {
            let case = format!("{operation} with token {token:?}");
            let path = format!("{}?nextToken={token}", listing.path);
            let (status, answer) = call(&router, "GET", &path, None).await;
            assert_eq!(
                status, 400,
                "case {case}: expected 400, got {status}: {answer}"
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case}: expected ValidationException, got {answer}"
            );
            let expected =
                format!("nextToken '{token}' is not a pagination token this service issued");
            let actual = answer["message"].as_str().unwrap_or_default();
            assert_eq!(
                actual, expected,
                "case {case}: expected {expected:?}, got {actual:?}"
            );
            // The modelled shape's second member: present, and carrying the value that
            // classifies a bad argument. A body missing it is the synthetic framework
            // shape rather than this service's own.
            let reason = answer["reason"].as_str();
            assert_eq!(
                reason,
                Some(ILLEGAL_ARGUMENT),
                "case {case}: expected reason {ILLEGAL_ARGUMENT:?}, got {reason:?} in {answer}"
            );
        }

        // The same listing, asked with a token that DOES name a position in its own id
        // space, serves a page: the refusals above are about the token, not about this
        // listing refusing every `nextToken`.
        let own = ID_SPACES
            .iter()
            .find(|(space, _)| *space == listing.space)
            .map(|(_, position)| *position)
            .unwrap_or_else(|| panic!("case {operation}: no id space named {}", listing.space));
        let token = issued_token(&listing.token_scope, own);
        let case = format!("{operation} with issued token {token:?}");
        let path = format!("{}?nextToken={token}", listing.path);
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
    }
}
