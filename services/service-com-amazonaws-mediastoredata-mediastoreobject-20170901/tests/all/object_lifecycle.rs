// SPDX-License-Identifier: CC0-1.0

//! The MediaStore object lifecycle, end to end over HTTP.
//!
//! Every test drives the real wire surface — a buffered `http::Request` through the
//! router that mounts this service over one shared engine — so what is asserted is the
//! same bytes an AWS SDK client would see: the restJson1 route table, the HTTP binding
//! of `@httpLabel` / `@httpHeader` / `@httpPayload` / `@httpResponseCode`, and the
//! modelled error envelopes.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one case:
//!
//! * a `PutObject` persists the body bytes and its metadata under the normalized path,
//!   and reports the content's own SHA-256 digest; a later `DescribeObject`,
//!   `GetObject` and `ListItems` report exactly what was persisted;
//! * a `GetObject` carrying a satisfiable `Range` answers that byte span with HTTP 206,
//!   and one no byte satisfies answers `RequestedRangeNotSatisfiableException` (416);
//! * `ListItems` enumerates the immediate children of a folder in lexicographic order,
//!   never returns more than `MaxResults` of them, and pages through all of them with
//!   no repeat and no omission;
//! * an operation naming a path that holds no object answers
//!   `ObjectNotFoundException` (404), and `DeleteObject` makes a path answer that.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use sha2::{Digest, Sha256};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mediastoredata_mediastoreobject_20170901::EmulatedService;

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name, which is how the router picks the service that serves the request. Request
/// authentication is switched off on the test router, so the signature is not read.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/mediastore/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// One MediaStore Data endpoint: this service mounted on a router over one engine.
struct DataPlane {
    router: MultiServiceRouter,
}

impl DataPlane {
    fn new() -> Self {
        // The one shared engine the production router builds and hands to every
        // handler of this service.
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn send(
        &self,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> http::Response<Bytes> {
        let mut builder = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "movies.data.mediastore.us-east-1.amazonaws.com")
            .header("authorization", AUTHORIZATION);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(Bytes::copy_from_slice(body))
            .expect("the test request must be well formed");
        self.router.dispatch(request).await
    }

    async fn put(&self, path: &str, body: &[u8], headers: &[(&str, &str)]) -> http::Response<Bytes> {
        self.send("PUT", &format!("/{path}"), headers, body).await
    }

    async fn get(&self, path: &str, headers: &[(&str, &str)]) -> http::Response<Bytes> {
        self.send("GET", &format!("/{path}"), headers, b"").await
    }

    async fn head(&self, path: &str) -> http::Response<Bytes> {
        self.send("HEAD", &format!("/{path}"), &[], b"").await
    }

    async fn delete(&self, path: &str) -> http::Response<Bytes> {
        self.send("DELETE", &format!("/{path}"), &[], b"").await
    }

    async fn list(&self, query: &str) -> http::Response<Bytes> {
        self.send("GET", &format!("/{query}"), &[], b"").await
    }
}

/// The value of one response header, as a string.
fn header(resp: &http::Response<Bytes>, name: &str) -> Option<String> {
    resp.headers()
        .get(name)
        .map(|v| String::from_utf8_lossy(v.as_bytes()).to_string())
}

/// The response body parsed as JSON.
fn json(resp: &http::Response<Bytes>) -> serde_json::Value {
    serde_json::from_slice(resp.body())
        .unwrap_or_else(|e| panic!("response body must be JSON ({e}): {:?}", resp.body()))
}

/// The restJson1 error code of an error response (its `__type` discriminator).
fn error_code(resp: &http::Response<Bytes>) -> String {
    json(resp)
        .get("__type")
        .and_then(|t| t.as_str())
        .unwrap_or_default()
        .rsplit(['#', '.'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The lowercase hex SHA-256 of `bytes`, computed independently of the service.
fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// One `PutObject` the round-trip table performs.
#[derive(Debug)]
struct PutCase {
    path: &'static str,
    body: Vec<u8>,
    content_type: Option<&'static str>,
    cache_control: Option<&'static str>,
    storage_class: Option<&'static str>,
    upload_availability: Option<&'static str>,
}

/// A PUT persists the body and its metadata, and every later read of that path reports
/// exactly what was persisted.
///
/// The rows differ along what the rule generalises over: body size (empty, one byte,
/// text, every byte value, 70 KiB), path depth (one segment up to the modelled maximum
/// of ten folder segments plus a name), and which optional metadata headers the request
/// carries.
#[tokio::test]
async fn a_put_is_read_back_byte_identical_with_its_stored_metadata() {
    let cases = vec![
        PutCase {
            path: "folder/file.txt",
            body: b"hello mediastore".to_vec(),
            content_type: Some("text/plain"),
            cache_control: Some("max-age=300"),
            storage_class: Some("TEMPORAL"),
            upload_availability: Some("STANDARD"),
        },
        PutCase {
            path: "single.ts",
            body: b"x".to_vec(),
            content_type: None,
            cache_control: None,
            storage_class: None,
            upload_availability: None,
        },
        PutCase {
            path: "empty.bin",
            body: Vec::new(),
            content_type: Some("application/octet-stream"),
            cache_control: None,
            storage_class: None,
            upload_availability: Some("STREAMING"),
        },
        PutCase {
            path: "binary/all-bytes.bin",
            body: (0u8..=255).collect(),
            content_type: Some("video/MP2T"),
            cache_control: Some("no-cache"),
            storage_class: None,
            upload_availability: None,
        },
        PutCase {
            // Ten folder segments plus a name: the deepest path the modelled
            // `^(?:[A-Za-z0-9_\.\-\~]+/){0,10}[A-Za-z0-9_\.\-\~]+$` admits.
            path: "a/b/c/d/e/f/g/h/i/j/~deep-1.0_file.m3u8",
            body: vec![b'z'; 70_000],
            content_type: Some("application/x-mpegURL"),
            cache_control: Some("max-age=1"),
            storage_class: Some("TEMPORAL"),
            upload_availability: None,
        },
    ];

    let endpoint = DataPlane::new();
    for case in &cases {
        let mut headers: Vec<(&str, &str)> = Vec::new();
        if let Some(value) = case.content_type {
            headers.push(("content-type", value));
        }
        if let Some(value) = case.cache_control {
            headers.push(("cache-control", value));
        }
        if let Some(value) = case.storage_class {
            headers.push(("x-amz-storage-class", value));
        }
        if let Some(value) = case.upload_availability {
            headers.push(("x-amz-upload-availability", value));
        }

        let put = endpoint.put(case.path, &case.body, &headers).await;
        assert_eq!(
            put.status().as_u16(),
            200,
            "case {case:?}: expected PutObject 200, got {} body {:?}",
            put.status(),
            String::from_utf8_lossy(put.body())
        );
        let put_body = json(&put);
        let digest = sha256_hex(&case.body);
        let reported_sha = put_body
            .get("ContentSHA256")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            reported_sha, digest,
            "case {case:?}: expected ContentSHA256 {digest}, got {reported_sha:?}"
        );
        let etag = put_body
            .get("ETag")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        assert!(
            (1..=64).contains(&etag.len()) && etag.chars().all(|c| c.is_ascii_hexdigit()),
            "case {case:?}: ETag must be 1..64 hex characters, got {etag:?}"
        );
        let storage_class = put_body
            .get("StorageClass")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            storage_class, "TEMPORAL",
            "case {case:?}: expected the effective StorageClass TEMPORAL, got {storage_class:?}"
        );

        // DescribeObject reports the stored length, digest and metadata.
        let described = endpoint.head(case.path).await;
        assert_eq!(
            described.status().as_u16(),
            200,
            "case {case:?}: expected DescribeObject 200, got {}",
            described.status()
        );
        assert_eq!(
            header(&described, "etag"),
            Some(etag.clone()),
            "case {case:?}: DescribeObject must report the ETag PutObject returned"
        );
        assert_eq!(
            header(&described, "content-length"),
            Some(case.body.len().to_string()),
            "case {case:?}: expected Content-Length {}, got {:?}",
            case.body.len(),
            header(&described, "content-length")
        );
        assert_eq!(
            header(&described, "cache-control"),
            case.cache_control.map(str::to_string),
            "case {case:?}: Cache-Control must be the stored one"
        );
        if let Some(expected) = case.content_type {
            assert_eq!(
                header(&described, "content-type"),
                Some(expected.to_string()),
                "case {case:?}: Content-Type must be the stored one"
            );
        }
        let last_modified = header(&described, "last-modified").unwrap_or_default();
        assert!(
            last_modified.ends_with(" GMT"),
            "case {case:?}: Last-Modified must be an HTTP-date, got {last_modified:?}"
        );

        // GetObject returns the exact bytes, with the same digest and metadata.
        let fetched = endpoint.get(case.path, &[]).await;
        assert_eq!(
            fetched.status().as_u16(),
            200,
            "case {case:?}: expected GetObject 200, got {}",
            fetched.status()
        );
        assert_eq!(
            fetched.body().as_ref(),
            case.body.as_slice(),
            "case {case:?}: GetObject must return the bytes PutObject stored"
        );
        assert_eq!(
            sha256_hex(fetched.body()),
            digest,
            "case {case:?}: the fetched bytes must have the reported ContentSHA256"
        );
        assert_eq!(
            header(&fetched, "etag"),
            Some(etag.clone()),
            "case {case:?}: GetObject must report the ETag PutObject returned"
        );
        assert_eq!(
            header(&fetched, "content-length"),
            Some(case.body.len().to_string()),
            "case {case:?}: GetObject must report the stored byte length"
        );
        assert_eq!(
            header(&fetched, "content-range"),
            None,
            "case {case:?}: a whole-object GetObject carries no Content-Range"
        );

        // A leading `/` names the same object, as does the path without one.
        let slashed = endpoint.get(&format!("/{}", case.path), &[]).await;
        assert_eq!(
            slashed.body().as_ref(),
            case.body.as_slice(),
            "case {case:?}: a leading `/` must name the same object"
        );
    }
}

/// Re-putting a path replaces the object: the digests follow the new content and the
/// old bytes are gone.
#[tokio::test]
async fn a_second_put_of_a_path_replaces_the_object() {
    let endpoint = DataPlane::new();
    let revisions: &[&[u8]] = &[b"first revision", b"second, longer revision", b"3"];
    let mut seen_digests = BTreeSet::new();

    for body in revisions {
        let put = endpoint
            .put("series/clip.ts", body, &[("content-type", "video/MP2T")])
            .await;
        assert_eq!(
            put.status().as_u16(),
            200,
            "case {body:?}: expected PutObject 200, got {}",
            put.status()
        );
        let reported = json(&put);
        let digest = sha256_hex(body);
        assert_eq!(
            reported.get("ContentSHA256").and_then(|v| v.as_str()),
            Some(digest.as_str()),
            "case {body:?}: ContentSHA256 must follow the new content, got {reported:?}"
        );
        assert_eq!(
            reported.get("ETag").and_then(|v| v.as_str()),
            Some(digest.as_str()),
            "case {body:?}: the ETag must follow the new content, got {reported:?}"
        );
        seen_digests.insert(digest);

        let fetched = endpoint.get("series/clip.ts", &[]).await;
        assert_eq!(
            fetched.body().as_ref(),
            *body,
            "case {body:?}: GetObject must return the newest bytes"
        );
        assert_eq!(
            header(&fetched, "content-length"),
            Some(body.len().to_string()),
            "case {body:?}: Content-Length must follow the new content"
        );
    }
    assert_eq!(
        seen_digests.len(),
        revisions.len(),
        "each distinct revision must report its own digest, got {seen_digests:?}"
    );

    // Exactly one object lives at that path, whatever the number of writes.
    let listed = endpoint.list("?Path=series").await;
    let items = json(&listed);
    let names: Vec<String> = items["Items"]
        .as_array()
        .expect("Items must be a list")
        .iter()
        .map(|item| item["Name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        names,
        vec!["clip.ts".to_string()],
        "a re-put path stays one item, got {names:?}"
    );
}

/// One row of the `Range` table: the header, the expected HTTP status, the expected
/// bytes, and the expected `Content-Range`.
#[derive(Debug)]
struct RangeCase {
    range: &'static str,
    status: u16,
    expected_body: &'static [u8],
    content_range: Option<&'static str>,
}

/// A satisfiable `Range` narrows the answer to that byte span with HTTP 206; a range no
/// byte of the object satisfies is `RequestedRangeNotSatisfiableException` (416).
///
/// The rows span the three forms the modelled `RangePattern` admits (`first-last`,
/// `first-`, `-suffix`), each at its edges: the first byte, the last byte, a span past
/// the end, and a start past the end.
#[tokio::test]
async fn a_range_request_returns_that_span_and_refuses_an_unsatisfiable_one() {
    // 16 bytes: "hello mediastore".
    let body: &[u8] = b"hello mediastore";
    let cases = &[
        RangeCase {
            range: "bytes=0-4",
            status: 206,
            expected_body: b"hello",
            content_range: Some("bytes=0-4/16"),
        },
        RangeCase {
            range: "bytes=0-0",
            status: 206,
            expected_body: b"h",
            content_range: Some("bytes=0-0/16"),
        },
        RangeCase {
            range: "bytes=15-15",
            status: 206,
            expected_body: b"e",
            content_range: Some("bytes=15-15/16"),
        },
        RangeCase {
            range: "bytes=6-",
            status: 206,
            expected_body: b"mediastore",
            content_range: Some("bytes=6-15/16"),
        },
        RangeCase {
            range: "bytes=0-99",
            status: 206,
            expected_body: b"hello mediastore",
            content_range: Some("bytes=0-15/16"),
        },
        RangeCase {
            range: "bytes=-5",
            status: 206,
            expected_body: b"store",
            content_range: Some("bytes=11-15/16"),
        },
        RangeCase {
            range: "bytes=-99",
            status: 206,
            expected_body: b"hello mediastore",
            content_range: Some("bytes=0-15/16"),
        },
        RangeCase {
            range: "bytes=16-",
            status: 416,
            expected_body: b"",
            content_range: None,
        },
        RangeCase {
            range: "bytes=100-200",
            status: 416,
            expected_body: b"",
            content_range: None,
        },
        RangeCase {
            range: "bytes=-0",
            status: 416,
            expected_body: b"",
            content_range: None,
        },
    ];

    let endpoint = DataPlane::new();
    let put = endpoint
        .put("folder/file.txt", body, &[("content-type", "text/plain")])
        .await;
    assert_eq!(put.status().as_u16(), 200, "the fixture object must store");

    for case in cases {
        let fetched = endpoint
            .get("folder/file.txt", &[("range", case.range)])
            .await;
        assert_eq!(
            fetched.status().as_u16(),
            case.status,
            "case {case:?}: expected HTTP {}, got {} body {:?}",
            case.status,
            fetched.status(),
            String::from_utf8_lossy(fetched.body())
        );
        if case.status == 416 {
            let code = error_code(&fetched);
            assert_eq!(
                code, "RequestedRangeNotSatisfiableException",
                "case {case:?}: expected RequestedRangeNotSatisfiableException, got {code:?}"
            );
            continue;
        }
        assert_eq!(
            fetched.body().as_ref(),
            case.expected_body,
            "case {case:?}: expected the span {:?}, got {:?}",
            String::from_utf8_lossy(case.expected_body),
            String::from_utf8_lossy(fetched.body())
        );
        assert_eq!(
            header(&fetched, "content-length"),
            Some(case.expected_body.len().to_string()),
            "case {case:?}: Content-Length must be the span's own length"
        );
        assert_eq!(
            header(&fetched, "content-range").as_deref(),
            case.content_range,
            "case {case:?}: unexpected Content-Range"
        );
    }

    // A range of an object that holds no bytes satisfies nothing.
    assert_eq!(
        endpoint.put("folder/empty", b"", &[]).await.status().as_u16(),
        200,
        "the empty fixture object must store"
    );
    let empty = endpoint.get("folder/empty", &[("range", "bytes=0-")]).await;
    assert_eq!(
        empty.status().as_u16(),
        416,
        "case bytes=0- of an empty object: expected 416, got {}",
        empty.status()
    );
}

/// `ListItems` reports one entry per immediate child of the folder — an OBJECT with the
/// stored metadata, a FOLDER for each sub-folder — and pages through all of them under
/// any `MaxResults` with no repeat and no omission.
#[tokio::test]
async fn list_items_enumerates_a_folder_and_pages_through_it() {
    let endpoint = DataPlane::new();
    let stored: &[(&str, &[u8], &str)] = &[
        ("folder/b.txt", b"bee", "text/plain"),
        ("folder/a.txt", b"ay", "text/plain"),
        ("folder/c.txt", b"cee!", "text/plain"),
        ("folder/nested/deep.txt", b"deep", "text/plain"),
        ("folder/nested/deeper/deepest.txt", b"deepest", "text/plain"),
        ("root.txt", b"root", "text/plain"),
    ];
    for (path, body, content_type) in stored {
        let put = endpoint
            .put(path, body, &[("content-type", content_type)])
            .await;
        assert_eq!(
            put.status().as_u16(),
            200,
            "case {path:?}: the fixture object must store, got {}",
            put.status()
        );
    }

    // The children of `folder/`, in the lexicographic order of their names.
    let expected: Vec<(&str, &str)> = vec![
        ("a.txt", "OBJECT"),
        ("b.txt", "OBJECT"),
        ("c.txt", "OBJECT"),
        ("nested", "FOLDER"),
    ];

    // The whole folder in one page: every member of an OBJECT entry is the stored one,
    // and a FOLDER entry carries no object metadata.
    let listed = endpoint.list("?Path=folder").await;
    assert_eq!(
        listed.status().as_u16(),
        200,
        "expected ListItems 200, got {} body {:?}",
        listed.status(),
        String::from_utf8_lossy(listed.body())
    );
    let page = json(&listed);
    assert!(
        page.get("NextToken").is_none(),
        "a single-page listing carries no NextToken, got {page:?}"
    );
    let items = page["Items"].as_array().expect("Items must be a list");
    assert_eq!(
        items.len(),
        expected.len(),
        "expected {} entries, got {items:?}",
        expected.len()
    );
    for (item, (name, kind)) in items.iter().zip(&expected) {
        assert_eq!(
            item["Name"].as_str(),
            Some(*name),
            "case {name:?}: unexpected entry name in {item:?}"
        );
        assert_eq!(
            item["Type"].as_str(),
            Some(*kind),
            "case {name:?}: expected Type {kind}, got {item:?}"
        );
        if *kind == "FOLDER" {
            assert!(
                item.get("ETag").is_none() && item.get("ContentLength").is_none(),
                "case {name:?}: a FOLDER entry carries no object metadata, got {item:?}"
            );
            continue;
        }
        let (_, body, content_type) = stored
            .iter()
            .find(|(path, _, _)| *path == format!("folder/{name}"))
            .expect("every OBJECT entry is one of the stored objects");
        assert_eq!(
            item["ETag"].as_str(),
            Some(sha256_hex(body).as_str()),
            "case {name:?}: the listed ETag must be the stored one, got {item:?}"
        );
        assert_eq!(
            item["ContentLength"].as_u64(),
            Some(body.len() as u64),
            "case {name:?}: the listed ContentLength must be the stored one, got {item:?}"
        );
        assert_eq!(
            item["ContentType"].as_str(),
            Some(*content_type),
            "case {name:?}: the listed ContentType must be the stored one, got {item:?}"
        );
        assert!(
            item["LastModified"].as_f64().is_some_and(|t| t > 0.0),
            "case {name:?}: the listed LastModified must be set, got {item:?}"
        );
    }

    // Paging: every page size reaches exactly the same sequence of entries, never more
    // than `MaxResults` at a time, and the final page carries no token.
    for page_size in [1usize, 2, 3, 4, 5] {
        let mut collected: Vec<(String, String)> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                None => format!("?Path=folder&MaxResults={page_size}"),
                Some(t) => format!("?Path=folder&MaxResults={page_size}&NextToken={t}"),
            };
            let resp = endpoint.list(&query).await;
            assert_eq!(
                resp.status().as_u16(),
                200,
                "case MaxResults={page_size}: expected 200, got {} body {:?}",
                resp.status(),
                String::from_utf8_lossy(resp.body())
            );
            let body = json(&resp);
            let items = body["Items"].as_array().expect("Items must be a list");
            assert!(
                items.len() <= page_size,
                "case MaxResults={page_size}: a page must not exceed the bound, got {} entries",
                items.len()
            );
            for item in items {
                collected.push((
                    item["Name"].as_str().unwrap_or_default().to_string(),
                    item["Type"].as_str().unwrap_or_default().to_string(),
                ));
            }
            token = body
                .get("NextToken")
                .and_then(|t| t.as_str())
                .map(str::to_string);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case MaxResults={page_size}: paging must terminate, collected {collected:?}"
            );
            if token.is_none() {
                break;
            }
        }
        let expected_pairs: Vec<(String, String)> = expected
            .iter()
            .map(|(name, kind)| ((*name).to_string(), (*kind).to_string()))
            .collect();
        assert_eq!(
            collected, expected_pairs,
            "case MaxResults={page_size}: paging must yield every entry once, in order"
        );
    }

    // The container root lists the root object and the top-level folder, whether the
    // request names no path at all or the root explicitly.
    for query in ["", "?Path=", "?Path=/"] {
        let resp = endpoint.list(query).await;
        assert_eq!(
            resp.status().as_u16(),
            200,
            "case {query:?}: expected ListItems 200, got {}",
            resp.status()
        );
        let names: Vec<(String, String)> = json(&resp)["Items"]
            .as_array()
            .expect("Items must be a list")
            .iter()
            .map(|item| {
                (
                    item["Name"].as_str().unwrap_or_default().to_string(),
                    item["Type"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        assert_eq!(
            names,
            vec![
                ("folder".to_string(), "FOLDER".to_string()),
                ("root.txt".to_string(), "OBJECT".to_string()),
            ],
            "case {query:?}: unexpected root listing"
        );
    }

    // A folder path is the same folder with or without its slashes.
    for query in ["?Path=folder", "?Path=/folder", "?Path=folder/", "?Path=/folder/"] {
        let resp = endpoint.list(query).await;
        let count = json(&resp)["Items"]
            .as_array()
            .expect("Items must be a list")
            .len();
        assert_eq!(
            count,
            expected.len(),
            "case {query:?}: expected {} entries, got {count}",
            expected.len()
        );
    }

    // A folder that holds nothing lists nothing — an empty list, not an error.
    let empty = endpoint.list("?Path=no-such-folder").await;
    assert_eq!(
        empty.status().as_u16(),
        200,
        "an empty folder lists successfully, got {}",
        empty.status()
    );
    let items = json(&empty)["Items"].as_array().cloned().unwrap_or_default();
    assert!(
        items.is_empty(),
        "an empty folder lists no entries, got {items:?}"
    );
}

/// A delete removes the object: reads of that path then answer
/// `ObjectNotFoundException`, the listing drops it, and a second delete is that same
/// error rather than a success.
#[tokio::test]
async fn a_delete_removes_the_object_from_every_later_read() {
    let endpoint = DataPlane::new();
    for path in ["folder/file.txt", "folder/keep.txt"] {
        assert_eq!(
            endpoint
                .put(path, b"payload", &[("content-type", "text/plain")])
                .await
                .status()
                .as_u16(),
            200,
            "case {path:?}: the fixture object must store"
        );
    }

    let deleted = endpoint.delete("folder/file.txt").await;
    assert_eq!(
        deleted.status().as_u16(),
        200,
        "expected DeleteObject 200, got {} body {:?}",
        deleted.status(),
        String::from_utf8_lossy(deleted.body())
    );

    for (method, resp) in [
        ("DescribeObject", endpoint.head("folder/file.txt").await),
        ("GetObject", endpoint.get("folder/file.txt", &[]).await),
        ("DeleteObject", endpoint.delete("folder/file.txt").await),
    ] {
        assert_eq!(
            resp.status().as_u16(),
            404,
            "case {method}: expected 404 after the delete, got {}",
            resp.status()
        );
        if method != "DescribeObject" {
            let code = error_code(&resp);
            assert_eq!(
                code, "ObjectNotFoundException",
                "case {method}: expected ObjectNotFoundException, got {code:?}"
            );
        }
    }

    let names: Vec<String> = json(&endpoint.list("?Path=folder").await)["Items"]
        .as_array()
        .expect("Items must be a list")
        .iter()
        .map(|item| item["Name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        names,
        vec!["keep.txt".to_string()],
        "the deleted object must be gone from the listing, got {names:?}"
    );

    // The object that was not deleted is untouched.
    let kept = endpoint.get("folder/keep.txt", &[]).await;
    assert_eq!(
        kept.body().as_ref(),
        b"payload",
        "a delete must not disturb another object"
    );
}

/// Every operation that names a path holding no object answers
/// `ObjectNotFoundException` (404) — not a 501, and not a hollow 200.
#[tokio::test]
async fn an_absent_path_is_object_not_found_for_every_operation() {
    let endpoint = DataPlane::new();
    // One stored object, so the store is not merely empty.
    assert_eq!(
        endpoint
            .put("present.txt", b"here", &[])
            .await
            .status()
            .as_u16(),
        200,
        "the fixture object must store"
    );

    let paths = [
        "never/written.txt",
        "present.txt.other",
        "a/b/c/d/e/absent.ts",
        "present",
    ];
    for path in paths {
        for (method, resp) in [
            ("GetObject", endpoint.get(path, &[]).await),
            ("DeleteObject", endpoint.delete(path).await),
        ] {
            assert_eq!(
                resp.status().as_u16(),
                404,
                "case ({method}, {path:?}): expected 404, got {} body {:?}",
                resp.status(),
                String::from_utf8_lossy(resp.body())
            );
            let code = error_code(&resp);
            assert_eq!(
                code, "ObjectNotFoundException",
                "case ({method}, {path:?}): expected ObjectNotFoundException, got {code:?}"
            );
        }
        let described = endpoint.head(path).await;
        assert_eq!(
            described.status().as_u16(),
            404,
            "case (DescribeObject, {path:?}): expected 404, got {}",
            described.status()
        );
    }
}

/// A request outside a modelled constraint or enum value set is refused with HTTP 400,
/// and a value inside it is accepted.
#[tokio::test]
async fn a_request_outside_the_modelled_shape_is_refused() {
    let endpoint = DataPlane::new();

    // The `StorageClass` / `UploadAvailability` enum value sets, from the model.
    let accepted: &[(&str, &str)] = &[
        ("x-amz-storage-class", "TEMPORAL"),
        ("x-amz-upload-availability", "STANDARD"),
        ("x-amz-upload-availability", "STREAMING"),
    ];
    for (name, value) in accepted {
        let resp = endpoint
            .put("enum/ok.txt", b"body", &[(name, value)])
            .await;
        assert_eq!(
            resp.status().as_u16(),
            200,
            "case ({name}, {value}): a modelled enum value must be accepted, got {} body {:?}",
            resp.status(),
            String::from_utf8_lossy(resp.body())
        );
    }

    let refused: &[(&str, &str)] = &[
        ("x-amz-storage-class", "STANDARD"),
        ("x-amz-storage-class", "temporal"),
        ("x-amz-upload-availability", "standard"),
        ("x-amz-upload-availability", "FAST"),
    ];
    for (name, value) in refused {
        let resp = endpoint
            .put("enum/bad.txt", b"body", &[(name, value)])
            .await;
        assert_eq!(
            resp.status().as_u16(),
            400,
            "case ({name}, {value}): a value outside the enum must be refused, got {} body {:?}",
            resp.status(),
            String::from_utf8_lossy(resp.body())
        );
        // Nothing was stored by a refused write.
        let fetched = endpoint.get("enum/bad.txt", &[]).await;
        assert_eq!(
            fetched.status().as_u16(),
            404,
            "case ({name}, {value}): a refused PutObject must store nothing"
        );
    }

    // `Path` outside its modelled length / pattern, and `MaxResults` outside its range.
    let long_path = format!("{}.txt", "p".repeat(900));
    let bad_requests: &[(&str, String)] = &[
        ("a path with an illegal character", "bad!name.txt".to_string()),
        ("a path with a space", "bad%20name.txt".to_string()),
        ("a path over 900 characters", long_path.clone()),
        (
            "a path deeper than ten folders",
            "a/b/c/d/e/f/g/h/i/j/k/l.txt".to_string(),
        ),
    ];
    for (what, path) in bad_requests {
        let resp = endpoint.get(path, &[]).await;
        assert_eq!(
            resp.status().as_u16(),
            400,
            "case {what}: expected 400, got {} body {:?}",
            resp.status(),
            String::from_utf8_lossy(resp.body())
        );
    }
    for max_results in ["0", "1001", "-1"] {
        let resp = endpoint
            .list(&format!("?Path=folder&MaxResults={max_results}"))
            .await;
        assert_eq!(
            resp.status().as_u16(),
            400,
            "case MaxResults={max_results}: outside the modelled 1..1000, expected 400, got {}",
            resp.status()
        );
    }

    // A `NextToken` this service never issued is refused rather than silently ignored.
    for token in ["not-a-token", "00", "6162636465"] {
        let resp = endpoint
            .list(&format!("?Path=folder&NextToken={token}"))
            .await;
        assert_eq!(
            resp.status().as_u16(),
            400,
            "case NextToken={token:?}: expected 400, got {} body {:?}",
            resp.status(),
            String::from_utf8_lossy(resp.body())
        );
    }
}

/// The store's ceilings are enforced at the write: an object larger than the documented
/// size limit for its upload availability is refused, as is a new object past the
/// emulator's object-count ceiling — while a replacement of an existing path is not.
#[tokio::test]
async fn a_write_past_the_store_bounds_is_refused() {
    use service_com_amazonaws_mediastoredata_mediastoreobject_20170901::state::{
        MAX_OBJECTS, MAX_OBJECT_BYTES_STANDARD, MAX_OBJECT_BYTES_STREAMING,
    };

    let endpoint = DataPlane::new();

    // "Object sizes are limited to 25 MB for standard upload availability and 10 MB for
    // streaming upload availability" — PutObject's own documentation.
    let size_cases: &[(&str, Option<&str>, usize, u16)] = &[
        ("at the streaming limit", Some("STREAMING"), MAX_OBJECT_BYTES_STREAMING, 200),
        ("past the streaming limit", Some("STREAMING"), MAX_OBJECT_BYTES_STREAMING + 1, 400),
        (
            "past the streaming limit but within the standard one",
            Some("STANDARD"),
            MAX_OBJECT_BYTES_STREAMING + 1,
            200,
        ),
        ("past the standard limit", None, MAX_OBJECT_BYTES_STANDARD + 1, 400),
    ];
    for (what, availability, len, expected) in size_cases {
        let headers: Vec<(&str, &str)> = availability
            .map(|value| vec![("x-amz-upload-availability", value)])
            .unwrap_or_default();
        let body = vec![b'q'; *len];
        let resp = endpoint.put("bounds/object.bin", &body, &headers).await;
        assert_eq!(
            resp.status().as_u16(),
            *expected,
            "case {what} ({len} bytes): expected HTTP {expected}, got {} body {:?}",
            resp.status(),
            String::from_utf8_lossy(resp.body())
        );
    }

    // The object-count ceiling applies to a NEW path; replacing a stored one does not
    // grow the store.
    let counted = DataPlane::new();
    for index in 0..MAX_OBJECTS {
        let resp = counted
            .put(&format!("many/object-{index}.bin"), b"x", &[])
            .await;
        assert_eq!(
            resp.status().as_u16(),
            200,
            "case object-{index}: a store holding {index} objects must accept another, got {}",
            resp.status()
        );
    }
    let one_too_many = counted.put("many/one-too-many.bin", b"x", &[]).await;
    assert_eq!(
        one_too_many.status().as_u16(),
        400,
        "case one-too-many: a store at {MAX_OBJECTS} objects must refuse a new path, got {} body {:?}",
        one_too_many.status(),
        String::from_utf8_lossy(one_too_many.body())
    );
    let replacement = counted.put("many/object-0.bin", b"yy", &[]).await;
    assert_eq!(
        replacement.status().as_u16(),
        200,
        "case object-0: replacing a stored object adds no path and must be accepted, got {}",
        replacement.status()
    );
    assert_eq!(
        counted.get("many/object-0.bin", &[]).await.body().as_ref(),
        b"yy",
        "the replacement must be what a later read returns"
    );
    // A delete frees the room the refused write needed.
    assert_eq!(
        counted
            .delete("many/object-1.bin")
            .await
            .status()
            .as_u16(),
        200,
        "the fixture object must delete"
    );
    let after_delete = counted.put("many/one-too-many.bin", b"x", &[]).await;
    assert_eq!(
        after_delete.status().as_u16(),
        200,
        "case one-too-many after a delete: expected 200, got {} body {:?}",
        after_delete.status(),
        String::from_utf8_lossy(after_delete.body())
    );
}

/// State is per engine, not per request: one endpoint's objects are reachable from
/// every later request, and a second endpoint starts empty.
#[tokio::test]
async fn objects_live_in_the_shared_engine_state() {
    let first = DataPlane::new();
    assert_eq!(
        first
            .put("folder/file.txt", b"stateful", &[])
            .await
            .status()
            .as_u16(),
        200,
        "the fixture object must store"
    );
    assert_eq!(
        first.get("folder/file.txt", &[]).await.body().as_ref(),
        b"stateful",
        "a later request must see the stored object"
    );

    let second = DataPlane::new();
    assert_eq!(
        second.get("folder/file.txt", &[]).await.status().as_u16(),
        404,
        "a separate engine holds no objects"
    );
}
