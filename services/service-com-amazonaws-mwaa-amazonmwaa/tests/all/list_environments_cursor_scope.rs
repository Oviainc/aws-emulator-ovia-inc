// SPDX-License-Identifier: CC0-1.0

//! The scope of a `ListEnvironments` page cursor, end to end over HTTP through the router
//! this service mounts in production.
//!
//! The rule every case here asserts: a page cursor names a position in the listing that
//! issued it, so a `NextToken` presented to a listing of a different scope — this listing
//! is per-Region, so a different Region — names no position and is refused as an invalid
//! pagination token, never reinterpreted against the other scope's own ordering; and
//! inside the listing that issued it, paging still visits every name of that Region
//! exactly once, in order, whatever the page size.
//!
//! Each row of the table varies what the rule generalises over: the Region (geography
//! prefix, direction, ordinal), how many environments the Region holds (none, one, two,
//! three), and where its names sort relative to another Region's cursor name (before it,
//! equal to it, after it) — the last being what a cursor accepted across Regions would
//! silently drop. Page size is varied from one up past the largest Region.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mwaa_amazonmwaa::state;

/// The canonical emulator account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The Regions the table runs over, with the environments each one holds. They differ in
/// geography prefix (`us`/`eu`/`ap`/`sa`), direction and ordinal — the shape
/// `EnvironmentArn`'s own Region segment (`[a-z0-9\-]+`) allows — and in how many
/// environments they hold, including one that holds none at all. The names differ in
/// where they sort against another Region's names (`Alpha` sorts before every name of
/// `us-east-1`, `Zeta` is shared with it, `Zulu` sorts after everything in `ap-southeast-2`),
/// all inside `EnvironmentName`'s own `^[a-zA-Z][0-9a-zA-Z-_]*$`.
const FIXTURE: [(&str, &[&str]); 4] = [
    ("us-east-1", &["Zeta", "Zulu", "Mango"]),
    ("eu-west-1", &["Alpha"]),
    ("ap-southeast-2", &["Beta", "Zeta"]),
    ("sa-east-1", &[]),
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

/// The emulator, assembled exactly as the router assembles it: one shared engine behind
/// one mounted service, reached through several regional endpoints.
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
        // harness, so the signature value is never checked.
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/airflow/aws4_request, \
             SignedHeaders=host, Signature=0000"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("api.airflow.{region}.amazonaws.com"))
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
    async fn page(&self, region: &str, query: &str) -> PageAnswer {
        let answer = self.list(region, query).await;
        assert_eq!(
            answer.status,
            200,
            "case {region}{query}: expected ListEnvironments 200, got {} {}",
            answer.status,
            answer.text()
        );
        let body = answer.json();
        let names = body
            .get("Environments")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!("case {region}{query}: Environments is required, got {}", answer.text())
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

    /// Walk `region`'s listing to exhaustion at `page_size`, returning every name seen.
    async fn walk(&self, region: &str, page_size: usize) -> Vec<String> {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(value) => format!("?MaxResults={page_size}&NextToken={value}"),
                None => format!("?MaxResults={page_size}"),
            };
            let page = self.page(region, &query).await;
            assert!(
                page.names.len() <= page_size,
                "case {region} at page size {page_size}: a page holds at most {page_size} names, \
                 got {:?}",
                page.names
            );
            seen.extend(page.names);
            pages += 1;
            assert!(
                pages <= expected_names(region).len() + 2,
                "case {region} at page size {page_size}: paging must terminate, got {pages} pages"
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

/// The names `region` holds, sorted — the order its listing answers in.
fn expected_names(region: &str) -> Vec<String> {
    let mut names: Vec<String> = FIXTURE
        .iter()
        .find(|(candidate, _)| *candidate == region)
        .map(|(_, names)| names.iter().map(|name| name.to_string()).collect())
        .unwrap_or_else(|| panic!("{region} is not one of the fixture Regions"));
    names.sort();
    names
}

/// Every fixture Region populated with its own environments.
async fn populated() -> Harness {
    let harness = Harness::new();
    for (region, names) in FIXTURE {
        for name in names {
            let answer = harness.create(region, name).await;
            assert_eq!(
                answer.status,
                200,
                "case {region}/{name}: the fixture create must succeed, got {} {}",
                answer.status,
                answer.text()
            );
        }
    }
    harness
}

/// The cursor `region`'s own listing issues after its first name, with the names that
/// still follow it there.
async fn first_page_cursor(harness: &Harness, region: &str) -> Option<(String, Vec<String>)> {
    let page = harness.page(region, "?MaxResults=1").await;
    let expected = expected_names(region);
    assert_eq!(
        page.names,
        expected.iter().take(1).cloned().collect::<Vec<String>>(),
        "case {region}: the first page of one names this Region's first environment, got {:?}",
        page.names
    );
    page.next_token
        .map(|token| (token, expected.into_iter().skip(1).collect()))
}

#[tokio::test]
async fn a_cursor_resumes_only_in_the_region_whose_listing_issued_it() {
    let harness = populated().await;

    for (issuer, _) in FIXTURE {
        let Some((token, rest)) = first_page_cursor(&harness, issuer).await else {
            // A Region whose listing fits in one page issues no cursor; it is still a
            // TARGET below.
            continue;
        };

        // In the Region that issued it, the cursor resumes exactly after the page it ended.
        let resumed = harness
            .page(issuer, &format!("?NextToken={token}"))
            .await
            .names;
        assert_eq!(
            resumed, rest,
            "case {issuer}: its own cursor must resume after the first page, got {resumed:?}"
        );

        // In every other Region it names no position: it is refused, not read against that
        // Region's ordering.
        for (other, _) in FIXTURE.iter().filter(|(other, _)| *other != issuer) {
            let answer = harness.list(other, &format!("?NextToken={token}")).await;
            let case = format!("a cursor from {issuer} replayed in {other}");
            assert_eq!(
                answer.status,
                400,
                "case {case:?}: a cursor of another Region's listing must be refused, got {} {}",
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

            // And nothing of that Region was dropped: its own listing still names every
            // environment it holds, including the ones sorting at or before the cursor's name.
            let own = harness.page(other, "").await.names;
            assert_eq!(
                own,
                expected_names(other),
                "case {case:?}: the refused cursor must leave {other}'s own listing whole, got {own:?}"
            );
        }
    }
}

#[tokio::test]
async fn paging_a_region_visits_its_own_names_exactly_once_at_every_page_size() {
    let harness = populated().await;
    let widest = FIXTURE
        .iter()
        .map(|(_, names)| names.len())
        .max()
        .unwrap_or_default();

    for (region, _) in FIXTURE {
        for page_size in 1..=(widest + 1) {
            let seen = harness.walk(region, page_size).await;
            assert_eq!(
                seen,
                expected_names(region),
                "case {region} at page size {page_size}: paging must visit every name of this \
                 Region exactly once, in order, got {seen:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_token_that_names_no_issuing_region_is_refused_everywhere() {
    let harness = populated().await;

    // `(label, the NextToken value)`. The first row is the cursor text WITHOUT an issuing
    // Region — the form a scope-less cursor would carry — and the rest are tokens no
    // listing of this service mints at all: a plain name, an unknown prefix, a Region
    // segment with no position, and a value that is not even hex.
    let forged: [(&str, String); 5] = [
        ("no Region segment", ::hex::encode("mwaa-after-environment:Zeta")),
        ("a bare name", ::hex::encode("Zeta")),
        (
            "another service's prefix",
            ::hex::encode("after-environment:us-east-1:Zeta"),
        ),
        (
            "a Region and no position",
            ::hex::encode("mwaa-after-environment:us-east-1"),
        ),
        ("not hex at all", "not-a-token".to_string()),
    ];

    for (region, _) in FIXTURE {
        for (label, token) in &forged {
            let case = format!("{label} presented to {region}");
            let answer = harness.list(region, &format!("?NextToken={token}")).await;
            assert_eq!(
                answer.status,
                400,
                "case {case:?}: a token no listing of this Region issued must be refused, got {} {}",
                answer.status,
                answer.text()
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected the modelled wire code, got {:?}",
                answer.error_code()
            );
        }

        // `NextToken` is `@length(0, 2048)`, so the empty string is a legal value: it names
        // no position, which is the start of this Region's own listing.
        let from_start = harness.page(region, "?NextToken=").await.names;
        assert_eq!(
            from_start,
            expected_names(region),
            "case {region}: an empty NextToken starts at the beginning, got {from_start:?}"
        );
    }
}
