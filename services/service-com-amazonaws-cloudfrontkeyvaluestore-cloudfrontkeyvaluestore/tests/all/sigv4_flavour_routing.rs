// SPDX-License-Identifier: CC0-1.0

//! Which requests this data plane is DISPATCHED, over every way a client addresses it.
//!
//! The front door names the target service from a five-field `AWS4-HMAC-SHA256` credential
//! scope, then `X-Amz-Target`, then the first `Host` label. A client of THIS API carries
//! none of those in a readable form: it is `restJson1`, so it sends no `X-Amz-Target`; its
//! endpoint rules resolve the `sigv4a` auth scheme, so a client built from the model signs
//! with SigV4A, whose credential scope has no region field at all (one such signature
//! covers the region SET the request names in `X-Amz-Region-Set`) and so is not the
//! five-field form; and a client pinned to an emulator endpoint sends an address as its
//! `Host`, which names no service. Every request to this data plane therefore named no
//! service, was claimed by nobody and was answered the front door's own
//! `404 UnknownService` — so none of these operations was reachable however well its
//! handler worked.
//!
//! The rule under test: a request whose resolved routing key names this service is served
//! by it; a request that names NO service is served by it when it addresses the URI path
//! space this service's own model publishes; and a request whose key names ANOTHER
//! service, or whose path lies outside that space, is not.
//!
//! Each addressing form is driven through the whole surface the data plane answers on —
//! describe, put, read back, describe again, list, and a truncated page — so the claim is
//! pinned per ROUTE rather than on one request.
//!
//! Signatures are the emulator's own concern and no service checks them, so these run
//! unsigned (`set_request_authentication(false)`): what is under test is which service a
//! request is dispatched TO, which is decided before any signature is looked at. The
//! `Signature=` values below are therefore placeholders and are never verified.

use std::sync::Arc;

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudfrontkeyvaluestore_cloudfrontkeyvaluestore::EmulatedService;

use crate::key_value_lifecycle::{arn, segment};

/// The `Host` a client pinned to this emulator's own endpoint sends: an address, which
/// names no service, so the host-style routing signal is absent for every endpoint-pinned
/// form below.
const EMULATOR_ENDPOINT_HOST: &str = "127.0.0.1:9297";

/// This service's SigV4 signing name (`smithy_query(query="service_info")`: `sigv4Name`
/// and `endpointPrefix` are both this).
const SIGNING_NAME: &str = "cloudfront-keyvaluestore";

/// The signing name of the CloudFront CONTROL plane, which creates a key value store but
/// serves none of these routes: a request that names it must not reach this service.
const CONTROL_PLANE_SIGNING_NAME: &str = "cloudfront";

/// A credential scope as plain SigV4 writes it:
/// `<access key id>/<date>/<region>/<signing name>/aws4_request`.
fn sigv4(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
    )
}

/// A credential scope as SigV4A writes it: the `AWS4-ECDSA-P256-SHA256` algorithm and a
/// scope with NO region field, since the signature covers the region set the request
/// names in `X-Amz-Region-Set`.
fn sigv4a(signing_name: &str) -> String {
    format!(
        "AWS4-ECDSA-P256-SHA256 Credential=AKIDEXAMPLE/20260101/{signing_name}/aws4_request, \
         SignedHeaders=host;x-amz-region-set, Signature=0123456789abcdef"
    )
}

/// The presigned query fields a client puts the same SigV4A scope in, with the `/`
/// separators percent-encoded as a query value carries them.
fn presigned_sigv4a(signing_name: &str) -> String {
    format!(
        "X-Amz-Algorithm=AWS4-ECDSA-P256-SHA256&X-Amz-Credential=AKIDEXAMPLE%2F20260101%2F\
         {signing_name}%2Faws4_request&X-Amz-SignedHeaders=host&X-Amz-Signature=0123456789ab"
    )
}

/// The presigned query fields for a plain SigV4 scope, whose five fields the front door
/// DOES read out of a query string.
fn presigned_sigv4(signing_name: &str) -> String {
    format!(
        "X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=AKIDEXAMPLE%2F20260101%2F\
         us-east-1%2F{signing_name}%2Faws4_request&X-Amz-SignedHeaders=host\
         &X-Amz-Signature=0123456789ab"
    )
}

/// One way a client addresses this service.
struct Addressing {
    /// The case name every assertion reports.
    case: &'static str,
    /// The `Host` header the client sends.
    host: &'static str,
    /// The `Authorization` header it sends, when it signs with one.
    authorization: Option<String>,
    /// Extra request headers the flavour carries (`X-Amz-Region-Set` for SigV4A).
    headers: &'static [(&'static str, &'static str)],
    /// The presigned query fields it carries, when its scope travels in the URI.
    presigned_query: Option<String>,
}

impl Addressing {
    /// `path` with this form's presigned query fields appended, so a presigned client's
    /// scope rides on every request it makes — including one that already has a query.
    fn address(&self, path: &str) -> String {
        match &self.presigned_query {
            None => path.to_string(),
            Some(query) => {
                let separator = if path.contains('?') { '&' } else { '?' };
                format!("{path}{separator}{query}")
            }
        }
    }
}

/// The forms that MUST reach this service: the first two name it in the signal the front
/// door reads, and the rest name no service at all and so are claimed by this model's own
/// path space.
fn served_forms() -> Vec<Addressing> {
    vec![
        Addressing {
            case: "host-style endpoint, unsigned",
            host: "cloudfront-keyvaluestore.us-east-1.amazonaws.com",
            authorization: None,
            headers: &[],
            presigned_query: None,
        },
        Addressing {
            case: "plain SigV4 scope, endpoint-pinned client",
            host: EMULATOR_ENDPOINT_HOST,
            authorization: Some(sigv4(SIGNING_NAME)),
            headers: &[],
            presigned_query: None,
        },
        Addressing {
            case: "SigV4A scope, which carries no region field",
            host: EMULATOR_ENDPOINT_HOST,
            authorization: Some(sigv4a(SIGNING_NAME)),
            headers: &[("x-amz-region-set", "us-east-1")],
            presigned_query: None,
        },
        Addressing {
            case: "SigV4A scope signed for a region set",
            host: EMULATOR_ENDPOINT_HOST,
            authorization: Some(sigv4a(SIGNING_NAME)),
            headers: &[("x-amz-region-set", "*")],
            presigned_query: None,
        },
        Addressing {
            case: "SigV4A scope in a presigned query string",
            host: EMULATOR_ENDPOINT_HOST,
            authorization: None,
            headers: &[],
            presigned_query: Some(presigned_sigv4a(SIGNING_NAME)),
        },
        Addressing {
            case: "an authorization scheme the front door does not read",
            host: EMULATOR_ENDPOINT_HOST,
            authorization: Some("Basic QUtJREVYQU1QTEU6c2VjcmV0".to_string()),
            headers: &[],
            presigned_query: None,
        },
        Addressing {
            case: "nothing on the request names a service",
            host: EMULATOR_ENDPOINT_HOST,
            authorization: None,
            headers: &[],
            presigned_query: None,
        },
    ]
}

/// The forms that must NOT reach this service: each names the CloudFront control plane in
/// a signal the front door reads, so the request belongs to that service and not to this
/// one, whatever path it addresses.
fn forms_naming_another_service() -> Vec<Addressing> {
    vec![
        Addressing {
            case: "a plain SigV4 scope naming the control plane",
            host: EMULATOR_ENDPOINT_HOST,
            authorization: Some(sigv4(CONTROL_PLANE_SIGNING_NAME)),
            headers: &[],
            presigned_query: None,
        },
        Addressing {
            case: "a presigned SigV4 scope naming the control plane",
            host: EMULATOR_ENDPOINT_HOST,
            authorization: None,
            headers: &[],
            presigned_query: Some(presigned_sigv4(CONTROL_PLANE_SIGNING_NAME)),
        },
        Addressing {
            case: "the control plane's host-style endpoint, unsigned",
            host: "cloudfront.amazonaws.com",
            authorization: None,
            headers: &[],
            presigned_query: None,
        },
    ]
}

/// One dispatched response.
struct Reply {
    status: u16,
    content_type: String,
    etag: Option<String>,
    body: serde_json::Value,
    raw: String,
}

/// The production dispatch path with this one service mounted.
struct Front {
    router: MultiServiceRouter,
}

impl Front {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are the emulator's concern, not this service's: what is under test
        // is which service the request is dispatched to.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn send(
        &self,
        form: &Addressing,
        method: &str,
        path: &str,
        call_headers: &[(&str, &str)],
        body: &str,
    ) -> Reply {
        let mut builder = http::Request::builder()
            .method(method)
            .uri(form.address(path))
            .header("host", form.host)
            .header("content-type", "application/json");
        if let Some(authorization) = &form.authorization {
            builder = builder.header("authorization", authorization.as_str());
        }
        for (name, value) in form.headers.iter().chain(call_headers) {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(Bytes::from(body.to_string()))
            .expect("build the request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        };
        let content_type = header("content-type").unwrap_or_default();
        let etag = header("etag");
        let raw = String::from_utf8_lossy(response.body()).to_string();
        let body = serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
        Reply {
            status,
            content_type,
            etag,
            body,
            raw,
        }
    }

    async fn describe(&self, form: &Addressing, store: &str) -> Reply {
        self.send(
            form,
            "GET",
            &format!("/key-value-stores/{}", segment(store)),
            &[],
            "",
        )
        .await
    }

    async fn put(
        &self,
        form: &Addressing,
        store: &str,
        key: &str,
        value: &str,
        if_match: &str,
    ) -> Reply {
        self.send(
            form,
            "PUT",
            &format!("/key-value-stores/{}/keys/{}", segment(store), segment(key)),
            &[("if-match", if_match)],
            &serde_json::json!({ "Value": value }).to_string(),
        )
        .await
    }

    async fn get(&self, form: &Addressing, store: &str, key: &str) -> Reply {
        self.send(
            form,
            "GET",
            &format!("/key-value-stores/{}/keys/{}", segment(store), segment(key)),
            &[],
            "",
        )
        .await
    }

    async fn list(
        &self,
        form: &Addressing,
        store: &str,
        page: Option<(i64, Option<&str>)>,
    ) -> Reply {
        let suffix = match page {
            None => String::new(),
            Some((max_results, None)) => format!("?MaxResults={max_results}"),
            Some((max_results, Some(token))) => {
                format!("?MaxResults={max_results}&NextToken={token}")
            }
        };
        self.send(
            form,
            "GET",
            &format!("/key-value-stores/{}/keys{suffix}", segment(store)),
            &[],
            "",
        )
        .await
    }
}

/// The front door's own refusal of a request no mounted service claims — the answer the
/// unclaimed forms must get, and the one every form used to get.
const UNKNOWN_SERVICE: &str = "UnknownService";

/// Assert `reply` came from THIS service rather than from the front door's fallbacks.
fn assert_served_by_this_service(case: &str, call: &str, reply: &Reply, expected_status: u16) {
    assert!(
        !reply.raw.contains(UNKNOWN_SERVICE),
        "case {case:?}: {call} reached no service: {} {}",
        reply.status,
        reply.raw
    );
    assert!(
        !reply.raw.contains("AmbiguousService"),
        "case {case:?}: {call} was claimed by more than one service: {} {}",
        reply.status,
        reply.raw
    );
    assert_eq!(
        reply.status, expected_status,
        "case {case:?}: {call} expected HTTP {expected_status}, got {} {}",
        reply.status, reply.raw
    );
    assert_eq!(
        reply.content_type, "application/json",
        "case {case:?}: {call} is restJson1, so the body is application/json, got {:?}",
        reply.content_type
    );
}

/// Assert the front door refused `reply` because no service claimed it.
fn assert_claimed_by_nobody(case: &str, call: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: {call} must get the front door's refusal, got {} {}",
        reply.status, reply.raw
    );
    assert!(
        reply.raw.contains(UNKNOWN_SERVICE),
        "case {case:?}: {call} must not be claimed by this service, got {} {}",
        reply.status,
        reply.raw
    );
}

/// The pairs each case writes: two, so a page bound smaller than the pair count has
/// something to truncate.
const PAIRS: [(&str, &str); 2] = [("alpha", "alpha-value"), ("beta", "beta-value")];

/// Every addressing form that does not name another service is served the modelled result
/// of every route, computed from that store's own persisted pairs.
#[tokio::test]
async fn every_way_of_addressing_this_endpoint_reaches_the_data_plane() {
    let front = Front::new();

    for form in served_forms() {
        let case = form.case;
        let store = arn(case);

        // Describe an untouched store: the metadata a client polls for readiness, and
        // the ETag every mutating call spends.
        let empty = front.describe(&form, &store).await;
        assert_served_by_this_service(case, "DescribeKeyValueStore", &empty, 200);
        assert_eq!(
            empty.body["ItemCount"].as_i64(),
            Some(0),
            "case {case:?}: an untouched store holds no pairs, got {}",
            empty.raw
        );
        assert_eq!(
            empty.body["TotalSizeInBytes"].as_i64(),
            Some(0),
            "case {case:?}: an untouched store holds no bytes, got {}",
            empty.raw
        );
        assert_eq!(
            empty.body["KvsARN"].as_str(),
            Some(store.as_str()),
            "case {case:?}: the store reports the ARN it was addressed by, got {}",
            empty.raw
        );
        assert_eq!(
            empty.body["Status"].as_str(),
            Some("READY"),
            "case {case:?}: a store this service serves is usable, got {}",
            empty.raw
        );
        assert!(
            empty.body["Created"].as_f64().is_some_and(|secs| secs > 0.0),
            "case {case:?}: Created must be a timestamp, got {}",
            empty.raw
        );
        let mut etag = empty
            .etag
            .clone()
            .unwrap_or_else(|| panic!("case {case:?}: no ETag header on {}", empty.raw));

        // Write both pairs, threading the version the way a client must.
        let mut written = 0usize;
        let mut bytes = 0i64;
        for (key, value) in PAIRS {
            let put = front.put(&form, &store, key, value, &etag).await;
            assert_served_by_this_service(case, "PutKey", &put, 200);
            written += 1;
            bytes += (key.len() + value.len()) as i64;
            assert_eq!(
                put.body["ItemCount"].as_i64(),
                Some(written as i64),
                "case {case:?}: PutKey {key:?} reports the pairs now held, got {}",
                put.raw
            );
            assert_eq!(
                put.body["TotalSizeInBytes"].as_i64(),
                Some(bytes),
                "case {case:?}: PutKey {key:?} reports the bytes now held, got {}",
                put.raw
            );
            let next = put
                .etag
                .clone()
                .unwrap_or_else(|| panic!("case {case:?}: no ETag header on {}", put.raw));
            assert_ne!(
                next, etag,
                "case {case:?}: PutKey {key:?} must advance the version"
            );
            etag = next;
        }

        // Read one pair back: the value the write stored.
        let (key, value) = PAIRS[0];
        let read = front.get(&form, &store, key).await;
        assert_served_by_this_service(case, "GetKey", &read, 200);
        assert_eq!(
            read.body["Key"].as_str(),
            Some(key),
            "case {case:?}: GetKey echoes the key it read, got {}",
            read.raw
        );
        assert_eq!(
            read.body["Value"].as_str(),
            Some(value),
            "case {case:?}: GetKey answers the stored value, got {}",
            read.raw
        );
        assert_eq!(
            read.body["ItemCount"].as_i64(),
            Some(PAIRS.len() as i64),
            "case {case:?}: GetKey reports the pairs held, got {}",
            read.raw
        );
        assert_eq!(
            read.body["TotalSizeInBytes"].as_i64(),
            Some(bytes),
            "case {case:?}: GetKey reports the bytes held, got {}",
            read.raw
        );

        // Describe again: the metadata now reflects the writes, on a new version.
        let after = front.describe(&form, &store).await;
        assert_served_by_this_service(case, "DescribeKeyValueStore after a write", &after, 200);
        assert_eq!(
            after.body["ItemCount"].as_i64(),
            Some(PAIRS.len() as i64),
            "case {case:?}: describe reports the pairs written, got {}",
            after.raw
        );
        assert_eq!(
            after.body["TotalSizeInBytes"].as_i64(),
            Some(bytes),
            "case {case:?}: describe reports the bytes written, got {}",
            after.raw
        );
        assert_eq!(
            after.etag.as_deref(),
            Some(etag.as_str()),
            "case {case:?}: describe reports the current version, got {}",
            after.raw
        );
        assert!(
            after.body["LastModified"]
                .as_f64()
                .is_some_and(|modified| modified
                    >= empty.body["Created"].as_f64().unwrap_or_default()),
            "case {case:?}: LastModified must be stamped at or after the write, got {}",
            after.raw
        );

        // List the whole store, then one truncated page and its continuation.
        let all = front.list(&form, &store, None).await;
        assert_served_by_this_service(case, "ListKeys", &all, 200);
        let items = all.body["Items"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Items must be a list, got {}", all.raw));
        assert_eq!(
            items.len(),
            PAIRS.len(),
            "case {case:?}: ListKeys enumerates every stored pair, got {}",
            all.raw
        );

        let page = front.list(&form, &store, Some((1, None))).await;
        assert_served_by_this_service(case, "ListKeys with a page bound", &page, 200);
        let first_page = page.body["Items"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Items must be a list, got {}", page.raw));
        assert_eq!(
            first_page.len(),
            1,
            "case {case:?}: a page holds at most MaxResults pairs, got {}",
            page.raw
        );
        let token = page.body["NextToken"].as_str().unwrap_or_else(|| {
            panic!(
                "case {case:?}: a truncated page carries a NextToken, got {}",
                page.raw
            )
        });

        let rest = front.list(&form, &store, Some((1, Some(token)))).await;
        assert_served_by_this_service(case, "ListKeys continuation", &rest, 200);
        let second_page = rest.body["Items"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Items must be a list, got {}", rest.raw));
        assert_eq!(
            second_page.len(),
            1,
            "case {case:?}: the continuation holds the remaining pair, got {}",
            rest.raw
        );
        assert_ne!(
            first_page[0]["Key"], second_page[0]["Key"],
            "case {case:?}: the continuation resumes after the page it followed, got {}",
            rest.raw
        );
    }
}

/// A request whose signal names ANOTHER service is left to that service, even on this
/// model's own routes: the front door answers it, which is honest, rather than this
/// service answering for an endpoint it was not addressed at.
#[tokio::test]
async fn a_request_that_names_another_service_is_not_claimed() {
    let front = Front::new();

    for form in forms_naming_another_service() {
        let case = form.case;
        let store = arn(case);
        for (call, reply) in [
            ("DescribeKeyValueStore", front.describe(&form, &store).await),
            ("GetKey", front.get(&form, &store, PAIRS[0].0).await),
            ("ListKeys", front.list(&form, &store, None).await),
        ] {
            assert_claimed_by_nobody(case, call, &reply);
        }
    }
}

/// With nothing on the request naming a service, the path decides: a path inside the space
/// this model publishes reaches this service, and a path outside it reaches nobody.
#[tokio::test]
async fn an_unsignalled_request_is_claimed_only_inside_this_models_path_space() {
    let front = Front::new();
    let form = Addressing {
        case: "nothing names a service",
        host: EMULATOR_ENDPOINT_HOST,
        authorization: None,
        headers: &[],
        presigned_query: None,
    };
    let store = segment(&arn("path space"));

    // (case, method, path, claimed by this service)
    let rows: [(&str, &str, String, bool); 7] = [
        (
            "a store's own route",
            "GET",
            format!("/key-value-stores/{store}"),
            true,
        ),
        (
            "a key's route",
            "GET",
            format!("/key-value-stores/{store}/keys/alpha"),
            true,
        ),
        (
            "the keys collection",
            "GET",
            format!("/key-value-stores/{store}/keys"),
            true,
        ),
        (
            // Inside the path space but naming no modelled route: still this service's
            // request, answered in its own envelope rather than refused by the front door.
            "the path space's root",
            "GET",
            "/key-value-stores".to_string(),
            true,
        ),
        (
            // The CloudFront control plane's own key-value-store route: a different first
            // segment, so a different service's path space.
            "the control plane's dated route",
            "GET",
            "/2020-05-31/key-value-store".to_string(),
            false,
        ),
        (
            "a path this model does not publish",
            "GET",
            format!("/keys/{store}"),
            false,
        ),
        ("the root path", "GET", "/".to_string(), false),
    ];

    for (case, method, path, claimed) in rows {
        let reply = front.send(&form, method, &path, &[], "").await;
        if claimed {
            assert!(
                !reply.raw.contains(UNKNOWN_SERVICE),
                "case {case:?}: {path} is in this model's path space, so this service must \
                 answer it, got {} {}",
                reply.status,
                reply.raw
            );
        } else {
            assert_claimed_by_nobody(case, &path, &reply);
        }
    }
}
