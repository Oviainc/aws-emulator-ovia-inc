// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListCrls`.
//!
//! Every request goes through the real `MultiServiceRouter` — the same dispatch the
//! emulator binary uses — so routing, restJson1 (de)serialization, boundary constraint
//! validation and the handler all run exactly as they do in production.
//!
//! The rule under test: `ListCrls` reports every certificate revocation list the account
//! holds, whichever trust anchor each was imported against, each entry identical to what
//! the per-resource read answers, in a page no larger than the requested `pageSize` and
//! resuming exactly where the previous page ended. So the tests span a TABLE of store
//! sizes, page sizes and trust-anchor spreads rather than one recorded call.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A placeholder PEM bundle for a trust anchor's certificate source.
/// `SourceData$x509CertificateData` is `@length(1, 204800)` with no `@pattern`, so any
/// non-empty string is a legal bundle on the wire.
const CERTIFICATE_BUNDLE_PEM: &str =
    "-----BEGIN CERTIFICATE-----\nZW11bGF0ZWQtdGVzdC1jZXJ0\n-----END CERTIFICATE-----\n";

/// A placeholder CRL in PEM form. `ImportCrlRequest$crlData` is a `Blob`
/// `@length(1, 300000)` with no `@pattern`
/// (`smithy_query(query="operation", operation="ImportCrl")`), so the bytes are opaque
/// to the service.
const CRL_PEM: &str = "-----BEGIN X509 CRL-----\nZW11bGF0ZWQtdGVzdC1jcmw=\n-----END X509 CRL-----\n";

/// The Region the tests sign for.
const REGION: &str = "us-west-2";

/// The server's own page ceiling (`state::MAX_PAGE_SIZE`): `ListRequest$pageSize` carries
/// no `@range` in the model (`smithy_query(query="operation", operation="ListCrls")`), so
/// this bound is the emulator's own.
const MAX_PAGE_SIZE: i32 = 100;

/// A client over one emulator: one engine, one mounted service, so every request in a
/// test observes the state the previous one wrote.
struct Client {
    router: MultiServiceRouter,
}

/// One HTTP answer: the status, the parsed JSON body (`Value::Null` for an empty body)
/// and the wire error code when the answer is a failure.
#[derive(Debug)]
struct Answer {
    status: u16,
    body: Value,
    error_code: String,
}

impl Client {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_rolesanywhere_rolesanywhere::EmulatedService::new(engine),
        );
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body
            .map(|value| bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            // The SigV4 credential scope is how the router resolves the service and how
            // the handler learns the request's Region.
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/rolesanywhere/aws4_request, \
                     SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(payload)
            .expect("build request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        let error_code = if header_code.is_empty() {
            body.get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .rsplit(['#', '/'])
                .next()
                .unwrap_or_default()
                .to_string()
        } else {
            header_code
        };
        Answer {
            status,
            body,
            error_code,
        }
    }

    async fn create_trust_anchor(&self, name: &str) -> Answer {
        self.send(
            "POST",
            "/trustanchors",
            Some(json!({
                "name": name,
                "source": {
                    "sourceType": "CERTIFICATE_BUNDLE",
                    "sourceData": {"x509CertificateData": CERTIFICATE_BUNDLE_PEM},
                },
            })),
        )
        .await
    }

    async fn import_crl(&self, body: Value) -> Answer {
        self.send("POST", "/crls", Some(body)).await
    }

    async fn get_crl(&self, id: &str) -> Answer {
        self.send("GET", &format!("/crl/{id}"), None).await
    }

    async fn list_crls(&self, query: &str) -> Answer {
        self.send("GET", &format!("/crls{query}"), None).await
    }
}

/// The `crlData` wire form: restJson1 carries a `Blob` as standard-alphabet base64, the
/// codec the wire serializer itself uses (`aws_smithy_types::base64`).
fn encode_blob(bytes: &[u8]) -> Value {
    json!(aws_smithy_types::base64::encode(bytes))
}

/// Create a trust anchor and answer its ARN.
async fn trust_anchor_arn(client: &Client, name: &str) -> String {
    let created = client.create_trust_anchor(name).await;
    assert_eq!(
        created.status, 201,
        "trust anchor {name:?}: expected CreateTrustAnchor 201, got {created:?}"
    );
    created
        .body
        .pointer("/trustAnchor/trustAnchorArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("trust anchor {name:?}: no trustAnchorArn in {created:?}"))
        .to_string()
}

/// Import one CRL against `anchor` and answer `(crlId, the CrlDetail the import returned)`.
async fn import(case: &str, client: &Client, name: &str, anchor: &str, data: &[u8]) -> (String, Value) {
    let answer = client
        .import_crl(json!({
            "name": name,
            "crlData": encode_blob(data),
            "trustAnchorArn": anchor,
        }))
        .await;
    assert_eq!(
        answer.status, 201,
        "case {case:?}: expected ImportCrl 201 for {name:?}, got {answer:?}"
    );
    let crl = answer.body.get("crl").cloned().unwrap_or(Value::Null);
    let id = crl
        .get("crlId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no crlId in {crl:?}"))
        .to_string();
    (id, crl)
}

/// Walk every page of `ListCrls`, asserting the page bound, and answer the entries in the
/// order they were served.
async fn drain_pages(case: &str, client: &Client, page_size: Option<i32>) -> Vec<Value> {
    let mut entries: Vec<Value> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut query = Vec::new();
        if let Some(size) = page_size {
            query.push(format!("pageSize={size}"));
        }
        if let Some(token) = &next_token {
            query.push(format!("nextToken={token}"));
        }
        let suffix = if query.is_empty() {
            String::new()
        } else {
            format!("?{}", query.join("&"))
        };
        let answer = client.list_crls(&suffix).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected ListCrls 200, got {answer:?}"
        );
        let page = answer
            .body
            .get("crls")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected a crls list, got {answer:?}"));
        if let Some(size) = page_size {
            assert!(
                i32::try_from(page.len()).is_ok_and(|len| len <= size),
                "case {case:?}: a page must not exceed pageSize {size}, got {} entries",
                page.len()
            );
        }
        entries.extend(page);
        next_token = answer
            .body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        pages += 1;
        assert!(
            pages <= entries.len() + 2,
            "case {case:?}: paging must terminate, but it produced {pages} pages"
        );
        if next_token.is_none() {
            break;
        }
    }
    entries
}

/// `ListCrls` reports exactly what is stored — across trust anchors — once per CRL,
/// honouring `pageSize` as a hard bound and resuming where the previous page ended.
///
/// The table varies the store size, the page size (absent, 1, smaller than the store,
/// larger than the store) and how the CRLs are spread over trust anchors (all on one
/// anchor, one per anchor, several anchors with uneven membership).
#[tokio::test]
async fn list_crls_reports_every_stored_crl_across_trust_anchors() {
    // (case name, CRLs per trust anchor, pageSize)
    let cases: [(&str, &[usize], Option<i32>); 8] = [
        ("empty-store-no-page-size", &[], None),
        ("empty-store-with-page-size", &[], Some(2)),
        ("one-anchor-one-crl", &[1], Some(1)),
        ("one-anchor-many-crls", &[5], Some(2)),
        ("two-anchors-one-crl-each", &[1, 1], None),
        ("two-anchors-uneven", &[3, 1], Some(2)),
        ("three-anchors-page-larger-than-store", &[2, 2, 1], Some(50)),
        ("three-anchors-single-entry-pages", &[1, 2, 3], Some(1)),
    ];

    for (case, per_anchor, page_size) in cases {
        let client = Client::new();
        // crlId -> (the CrlDetail the import returned, the anchor it was imported against)
        let mut imported: BTreeMap<String, (Value, String)> = BTreeMap::new();
        for (anchor_index, count) in per_anchor.iter().enumerate() {
            let anchor = trust_anchor_arn(&client, &format!("{case}-anchor-{anchor_index}")).await;
            for crl_index in 0..*count {
                let name = format!("crl-{anchor_index}-{crl_index}");
                let data = format!("{CRL_PEM}{name}");
                let (id, crl) = import(case, &client, &name, &anchor, data.as_bytes()).await;
                assert!(
                    imported
                        .insert(id.clone(), (crl, anchor.clone()))
                        .is_none(),
                    "case {case:?}: every import must mint a distinct crlId, saw {id:?} twice"
                );
            }
        }

        let listed = drain_pages(case, &client, page_size).await;
        assert_eq!(
            listed.len(),
            imported.len(),
            "case {case:?}: paging must report every stored CRL exactly once, expected {} \
             entries, got {}",
            imported.len(),
            listed.len()
        );

        let mut seen: BTreeSet<String> = BTreeSet::new();
        for entry in &listed {
            let id = entry
                .get("crlId")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: a listed entry has no crlId: {entry:?}"))
                .to_string();
            assert!(
                seen.insert(id.clone()),
                "case {case:?}: {id:?} was listed twice across pages"
            );
            let (expected_detail, expected_anchor) = imported
                .get(&id)
                .unwrap_or_else(|| panic!("case {case:?}: {id:?} was never imported"));
            assert_eq!(
                entry.get("trustAnchorArn").and_then(Value::as_str),
                Some(expected_anchor.as_str()),
                "case {case:?}: {id:?} must report the trust anchor it was imported against, \
                 got {entry:?}"
            );
            assert_eq!(
                entry, expected_detail,
                "case {case:?}: the listed entry for {id:?} must equal the stored record"
            );
            // The list is built from the same record the per-resource read answers from.
            let fetched = client.get_crl(&id).await;
            assert_eq!(
                fetched.status, 200,
                "case {case:?}: expected GetCrl 200 for {id:?}, got {fetched:?}"
            );
            assert_eq!(
                fetched.body.get("crl"),
                Some(entry),
                "case {case:?}: GetCrl and ListCrls must report the same record for {id:?}"
            );
        }
        assert_eq!(
            seen,
            imported.keys().cloned().collect::<BTreeSet<String>>(),
            "case {case:?}: the pages must hold exactly the imported CRLs"
        );

        if imported.is_empty() {
            let answer = client.list_crls("").await;
            assert_eq!(
                answer.body.get("crls"),
                Some(&json!([])),
                "case {case:?}: an empty store returns an empty crls list, got {answer:?}"
            );
            assert!(
                answer.body.get("nextToken").is_none(),
                "case {case:?}: an empty store returns no nextToken, got {answer:?}"
            );
        }
    }
}

/// A read never changes what a later caller sees: listing repeatedly, with any paging,
/// answers the same set, and a CRL imported afterwards joins it.
#[tokio::test]
async fn listing_crls_does_not_disturb_the_store() {
    let client = Client::new();
    let anchor = trust_anchor_arn(&client, "stable-anchor").await;
    let mut expected: BTreeSet<String> = BTreeSet::new();
    for index in 0..4 {
        let (id, _) = import(
            "stable",
            &client,
            &format!("stable-crl-{index}"),
            &anchor,
            CRL_PEM.as_bytes(),
        )
        .await;
        expected.insert(id);
    }

    let ids = |entries: &[Value]| -> BTreeSet<String> {
        entries
            .iter()
            .filter_map(|entry| entry.get("crlId").and_then(Value::as_str))
            .map(ToString::to_string)
            .collect()
    };

    for page_size in [None, Some(1), Some(3), Some(MAX_PAGE_SIZE)] {
        let case = format!("repeat pageSize={page_size:?}");
        let entries = drain_pages(&case, &client, page_size).await;
        assert_eq!(
            ids(&entries),
            expected,
            "case {case:?}: every repeat of the list must report the same CRLs"
        );
    }

    // A later import is reflected; nothing earlier is lost.
    let (fresh, _) = import("stable", &client, "added-later", &anchor, CRL_PEM.as_bytes()).await;
    expected.insert(fresh);
    let entries = drain_pages("after-import", &client, Some(2)).await;
    assert_eq!(
        ids(&entries),
        expected,
        "case \"after-import\": a CRL imported after a list must appear in the next one"
    );
}

/// Pages do not overlap or skip: with a page size of 1 over a store of several CRLs,
/// each page carries exactly one entry and a `nextToken` comes back only while entries
/// remain.
#[tokio::test]
async fn crl_pages_are_contiguous_and_the_last_page_ends_the_walk() {
    let client = Client::new();
    let first = trust_anchor_arn(&client, "contiguous-anchor-a").await;
    let second = trust_anchor_arn(&client, "contiguous-anchor-b").await;
    let mut expected: Vec<String> = Vec::new();
    for index in 0..6 {
        let anchor = if index % 2 == 0 { &first } else { &second };
        let (id, _) = import(
            "contiguous",
            &client,
            &format!("contiguous-crl-{index}"),
            anchor,
            CRL_PEM.as_bytes(),
        )
        .await;
        expected.push(id);
    }
    expected.sort();

    let mut walked: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    for step in 0..expected.len() {
        let query = match &next_token {
            Some(token) => format!("?pageSize=1&nextToken={token}"),
            None => "?pageSize=1".to_string(),
        };
        let answer = client.list_crls(&query).await;
        assert_eq!(
            answer.status, 200,
            "case step={step}: expected ListCrls 200, got {answer:?}"
        );
        let page = answer
            .body
            .get("crls")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            page.len(),
            1,
            "case step={step}: a pageSize=1 walk over a non-exhausted store serves one entry, \
             got {}",
            page.len()
        );
        walked.push(
            page[0]
                .get("crlId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        );
        next_token = answer
            .body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let last = step + 1 == expected.len();
        assert_eq!(
            next_token.is_none(),
            last,
            "case step={step}: a nextToken must come back exactly while entries remain, got \
             {next_token:?}"
        );
    }

    assert_eq!(
        walked, expected,
        "the single-entry walk must enumerate every stored CRL once, in the store's stable order"
    );
}

/// `pageSize` bounds ONE response: a non-positive page is no page at all and is refused
/// with the modelled `ValidationException` (400), while a page above the server's own
/// ceiling is served clamped rather than refused.
#[tokio::test]
async fn list_crls_refuses_a_non_positive_page_size() {
    let client = Client::new();
    let anchor = trust_anchor_arn(&client, "page-size-anchor").await;
    for index in 0..3 {
        import(
            "page-size",
            &client,
            &format!("page-size-crl-{index}"),
            &anchor,
            CRL_PEM.as_bytes(),
        )
        .await;
    }

    for requested in [0, -1, -100] {
        let answer = client.list_crls(&format!("?pageSize={requested}")).await;
        assert_eq!(
            answer.status, 400,
            "case pageSize={requested}: a non-positive page must be refused, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case pageSize={requested}: expected ValidationException, got {answer:?}"
        );
    }

    for query in [
        format!("?pageSize={}", MAX_PAGE_SIZE),
        format!("?pageSize={}", MAX_PAGE_SIZE + 1),
        "?pageSize=5000".to_string(),
        String::new(),
    ] {
        let answer = client.list_crls(&query).await;
        assert_eq!(
            answer.status, 200,
            "case {query:?}: a page at or above the server ceiling is served, not refused, got \
             {answer:?}"
        );
        assert_eq!(
            answer
                .body
                .get("crls")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(3),
            "case {query:?}: a page wider than the store holds the whole store, got {answer:?}"
        );
    }
}

/// A `nextToken` this service never issued is a bad request, not an empty page: the
/// cursor is opaque, so a caller cannot forge one.
#[tokio::test]
async fn list_crls_refuses_a_token_it_did_not_issue() {
    let client = Client::new();
    let anchor = trust_anchor_arn(&client, "token-anchor").await;
    import("token", &client, "token-crl", &anchor, CRL_PEM.as_bytes()).await;

    for (case, token) in [
        ("not-hex", "zzzz"),
        ("odd-length-hex", "abc"),
        ("plain-text", "next-page-please"),
        ("non-utf8-hex", "ff"),
    ] {
        let answer = client.list_crls(&format!("?nextToken={token}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a forged nextToken must be refused, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
    }

    // A token the service DID issue is accepted and resumes the walk.
    let first = client.list_crls("?pageSize=1").await;
    assert_eq!(
        first.status, 200,
        "expected the first page to be served, got {first:?}"
    );
    assert!(
        first.body.get("nextToken").is_none(),
        "a one-entry store is exhausted by a one-entry page, got {first:?}"
    );
}
