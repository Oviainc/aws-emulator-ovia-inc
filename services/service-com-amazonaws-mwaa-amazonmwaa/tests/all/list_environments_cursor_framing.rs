// SPDX-License-Identifier: CC0-1.0

//! How a `ListEnvironments` page cursor FRAMES the two strings it carries — the scope the
//! page was computed over and the position it ended at.
//!
//! The rule every case here asserts: a page cursor round-trips, so decoding a token a
//! listing minted gives back exactly the scope and the position it was minted with, for
//! EVERY scope value and EVERY position value — the scope being whatever the request's
//! SigV4 credential scope carries, which nothing constrains to a Region code's alphabet.
//! A listing therefore always resumes from the token it issued itself, and a token is
//! still refused by a listing of any other scope, including one whose value is a leading
//! segment of the issuer's.
//!
//! The rows vary what the rule generalises over: the characters the scope carries (a plain
//! Region code, one and several colons, a value that is a leading segment of another row's,
//! the cursor's own prefix text, JSON punctuation, a quote and a backslash), how many
//! environments the scope holds, and the page size the listing is walked at. The second
//! test sweeps the scope/position pair exhaustively over a small adversarial alphabet,
//! through the cursor codec itself.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mwaa_amazonmwaa::state;

/// The canonical emulator account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The names every scope of the table holds, in no particular order: the listing sorts
/// them. All three are inside `EnvironmentName`'s own `^[a-zA-Z][0-9a-zA-Z-_]*$` and
/// `@length(1, 80)` (source: smithy_query(query="shape",shape="EnvironmentName")). Every
/// scope holds the SAME names, so a token leaking into another scope's listing would land
/// on a position that scope really has.
const NAMES: [&str; 3] = ["Zeta", "Alpha", "Mango"];

/// The scope values the table runs over, as `(case, the Region the credential scope names)`.
///
/// `crate::state::request_region` takes this value verbatim out of the request's SigV4
/// credential scope, which admits any text with no `/` or `,` — so the rows differ along
/// the characters the scope carries, not along geography. `"us"` is a leading segment of
/// `"us:east"`, the pair a cursor split at its first separator confuses.
const SCOPES: [(&str, &str); 8] = [
    ("a plain Region code", "us-east-1"),
    ("a scope carrying one colon", "us:east"),
    ("a scope that is a leading segment of another", "us"),
    ("a scope carrying several colons", "ap:southeast:2"),
    (
        "a scope quoting the cursor's own prefix",
        "mwaa-after-environment:eu-west-1",
    ),
    ("a scope carrying JSON punctuation", "eu{west}1"),
    ("a scope carrying a quote", "sa\"east\"1"),
    ("a scope carrying a backslash", "ca\\central\\1"),
];

/// One HTTP answer, reduced to what these tests assert on.
struct Answer {
    status: u16,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The modelled error code, which restJson1 carries in `__type`.
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string()
    }

    fn message(&self) -> String {
        self.json()
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One page of a listing.
struct PageAnswer {
    names: Vec<String>,
    next_token: Option<String>,
}

/// The emulator, assembled the way the router assembles it: one shared engine behind one
/// mounted service, asked under whichever credential scope a case names.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_mwaa_amazonmwaa::EmulatedService::new(Arc::clone(&engine)),
        );
        Self { router }
    }

    async fn send(&self, region: &str, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body
            .map(|value| serde_json::to_vec(&value).expect("a serialisable body"))
            .unwrap_or_default();
        // A signed-looking request whose SigV4 credential scope names `region`, the way an
        // SDK configured for that Region signs. Signature verification is off in this
        // harness, so the signature value is never checked. The host names this service's
        // control-plane endpoint and carries no Region, so the scope under test reaches the
        // handler through the credential scope alone.
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/airflow/aws4_request, \
             SignedHeaders=host, Signature=0000"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "api.airflow.amazonaws.com")
            .header("authorization", authorization)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        Answer {
            status: response.status().as_u16(),
            body: response.into_body().to_vec(),
        }
    }

    async fn create(&self, region: &str, name: &str) -> Answer {
        self.send(
            region,
            "PUT",
            &format!("/environments/{name}"),
            Some(create_body()),
        )
        .await
    }

    async fn list(&self, region: &str, query: &str) -> Answer {
        self.send(region, "GET", &format!("/environments{query}"), None)
            .await
    }

    /// One page of `region`'s listing, which must succeed.
    async fn page(&self, case: &str, region: &str, query: &str) -> PageAnswer {
        let answer = self.list(region, query).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?} ({region:?}{query}): expected ListEnvironments 200, got {} {}",
            answer.status,
            answer.text()
        );
        let body = answer.json();
        let names = body
            .get("Environments")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?} ({region:?}{query}): Environments is required, got {}",
                    answer.text()
                )
            })
            .iter()
            .map(|entry| entry.as_str().unwrap_or_default().to_string())
            .collect();
        PageAnswer {
            names,
            next_token: body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string),
        }
    }

    /// Walk `region`'s listing to exhaustion at `page_size`, following only the cursors the
    /// listing itself handed out, and return every name seen.
    async fn walk(&self, case: &str, region: &str, page_size: usize) -> Vec<String> {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(value) => format!("?MaxResults={page_size}&NextToken={value}"),
                None => format!("?MaxResults={page_size}"),
            };
            let page = self.page(case, region, &query).await;
            assert!(
                page.names.len() <= page_size,
                "case {case:?} at page size {page_size}: a page holds at most {page_size} \
                 names, got {:?}",
                page.names
            );
            seen.extend(page.names);
            pages += 1;
            assert!(
                pages <= NAMES.len() + 2,
                "case {case:?} at page size {page_size}: paging must terminate, got {pages} pages"
            );
            token = page.next_token;
            if token.is_none() {
                break;
            }
        }
        seen
    }
}

/// A create body every member of which satisfies its modelled constraints; nothing in it
/// varies per case, because the rule here is about the cursor, not the stored record.
fn create_body() -> Value {
    json!({
        "ExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/mwaa-execution"),
        "SourceBucketArn": "arn:aws:s3:::my-airflow-bucket",
        "DagS3Path": "dags",
        "MaxWorkers": 3,
        "NetworkConfiguration": {
            "SubnetIds": ["subnet-00000000000000001", "subnet-00000000000000002"],
            "SecurityGroupIds": ["sg-00000000000000001"],
        },
    })
}

/// [`NAMES`] in the order a listing answers in.
fn expected_names() -> Vec<String> {
    let mut names: Vec<String> = NAMES.iter().map(|name| (*name).to_string()).collect();
    names.sort();
    names
}

/// Every scope of the table populated with [`NAMES`].
async fn populated() -> Harness {
    let harness = Harness::new();
    for (case, region) in SCOPES {
        for name in NAMES {
            let answer = harness.create(region, name).await;
            assert_eq!(
                answer.status,
                200,
                "case {case:?} ({region:?}/{name}): the fixture create must succeed, got {} {}",
                answer.status,
                answer.text()
            );
        }
    }
    harness
}

#[tokio::test]
async fn a_listing_resumes_from_its_own_cursor_whatever_the_scope_carries() {
    let harness = populated().await;
    let expected = expected_names();

    for (case, region) in SCOPES {
        // Walked at every page size from one up past the whole listing: each page's cursor
        // is the one the previous page handed out, so a cursor the listing cannot read back
        // shows up here as a refused page.
        for page_size in 1..=(NAMES.len() + 1) {
            let seen = harness.walk(case, region, page_size).await;
            assert_eq!(
                seen, expected,
                "case {case:?} ({region:?}) at page size {page_size}: paging must visit every \
                 name of this scope exactly once, in order, got {seen:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_cursor_is_refused_by_every_other_scopes_listing() {
    let harness = populated().await;
    let expected = expected_names();

    for (issuer_case, issuer) in SCOPES {
        let token = harness
            .page(issuer_case, issuer, "?MaxResults=1")
            .await
            .next_token
            .unwrap_or_else(|| {
                panic!("case {issuer_case:?} ({issuer:?}): a page of one of three names must \
                        carry a cursor")
            });

        for (other_case, other) in SCOPES.iter().filter(|(_, other)| *other != issuer) {
            let case = format!("a cursor of {issuer_case} ({issuer:?}) replayed in {other_case}");
            let answer = harness.list(other, &format!("?NextToken={token}")).await;
            assert_eq!(
                answer.status,
                400,
                "case {case:?}: a cursor of another scope's listing must be refused, got {} {}",
                answer.status,
                answer.text()
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected the modelled wire code, got {:?}",
                answer.error_code()
            );
            assert_eq!(
                answer.message(),
                state::INVALID_NEXT_TOKEN_MESSAGE,
                "case {case:?}: expected the refusal's own wording, got {:?}",
                answer.message()
            );

            // And the refused cursor dropped nothing: that scope's own listing still names
            // every environment it holds, including the ones sorting at or before the
            // cursor's position.
            let own = harness.page(other_case, other, "").await.names;
            assert_eq!(
                own, expected,
                "case {case:?}: the refused cursor must leave {other:?}'s own listing whole, \
                 got {own:?}"
            );
        }
    }
}

/// The characters a scope segment is built from in the sweep below: a letter plus the
/// punctuation that breaks a framing which relies on one of them being absent — the
/// separator a cursor was split at, the quote and the backslash a quoted form escapes, and
/// an object brace.
const SCOPE_ALPHABET: [char; 5] = ['a', ':', '"', '\\', '{'];

/// Every string of 1 and 2 characters over [`SCOPE_ALPHABET`], plus the table's own scope
/// values: the scopes the sweep round-trips.
fn swept_scopes() -> Vec<String> {
    let mut scopes: Vec<String> = SCOPE_ALPHABET.iter().map(char::to_string).collect();
    for first in SCOPE_ALPHABET {
        for second in SCOPE_ALPHABET {
            scopes.push(format!("{first}{second}"));
        }
    }
    scopes.extend(SCOPES.iter().map(|(_, region)| (*region).to_string()));
    scopes
}

/// The positions the sweep round-trips: the shortest name, a two-character one, and one
/// carrying both punctuation marks `EnvironmentName`'s own pattern allows
/// (`^[a-zA-Z][0-9a-zA-Z-_]*$`, source:
/// smithy_query(query="shape",shape="EnvironmentName")).
const SWEPT_POSITIONS: [&str; 3] = ["A", "Zz", "A-b_9"];

#[test]
fn every_scope_and_position_pair_round_trips_through_the_cursor() {
    let scopes = swept_scopes();
    for scope in &scopes {
        for position in SWEPT_POSITIONS {
            let token = state::encode_cursor(scope, position);
            let read_back = state::decode_cursor(scope, Some(&token)).ok().flatten();
            assert_eq!(
                read_back.as_deref(),
                Some(position),
                "case scope {scope:?} position {position:?}: the listing that minted a cursor \
                 must read back the position it named, got {read_back:?}"
            );

            // The same token names no position in any OTHER scope's listing, however that
            // scope's value relates to this one (a leading segment of it, an extension of
            // it, or unrelated).
            for other in scopes.iter().filter(|other| *other != scope) {
                let refused = state::decode_cursor(other, Some(&token));
                assert!(
                    refused.is_err(),
                    "case scope {scope:?} position {position:?} replayed in {other:?}: a cursor \
                     of another scope must be refused, got {:?}",
                    refused.ok().flatten()
                );
            }
        }
    }
}
