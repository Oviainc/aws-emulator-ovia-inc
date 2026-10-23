// SPDX-License-Identifier: CC0-1.0

//! A Lightsail bucket's OBJECT data plane, driven over the wire.
//!
//! The rule under test: a Lightsail bucket IS an S3-compatible object store
//! (`AccessKey` — "Access keys grant full programmatic access to the specified bucket
//! and its objects"; `Bucket$url` — "The URL of the bucket"), so a bucket this service
//! creates is served by the one object store this emulator holds — Amazon S3's own.
//! An S3 request addressed to the bucket's name therefore writes, reads, lists and
//! removes its objects; the address `Bucket$url` announces is where that data plane
//! answers; and what the control plane says about the bucket's contents — whether a
//! `DeleteBucket` without `forceDelete` is refused — is read from that same store.
//!
//! Both services are mounted on ONE shared engine, exactly as the router mounts them,
//! so what is exercised is the hand-off between the two rather than either store
//! directly. Every request goes through the same front door a client uses: the
//! Lightsail calls as `POST /` with `X-Amz-Target`, the object calls as the path-style
//! S3 requests a client signed with a bucket access key sends (`PUT /<bucket>/<key>`,
//! `GET /<bucket>/<key>`, `GET /<bucket>?list-type=2`, `DELETE /<bucket>/<key>`).
//!
//! The harness turns request authentication OFF, as the emulator itself runs unless
//! started with it on, so the access key minted here is not what admits the object
//! requests — the credential scope's service field is what routes them. What the
//! access-key half of the rule is asserted on instead is the control plane: the key is
//! minted `Active` with a secret, and a bucket that still holds one cannot be deleted
//! without a force delete.

use std::sync::Arc;

use quick_xml::events::Event;
use quick_xml::Reader;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::instance_lifecycle::error_code;

/// The `X-Amz-Target` prefix the Lightsail service routes on.
const TARGET_PREFIX: &str = "Lightsail_20161128";

/// The endpoint every request here reaches the emulator on (its `Host`), and so the
/// endpoint a bucket's announced address is built from.
const ENDPOINT_HOST: &str = "127.0.0.1:9297";

/// The Region every call in this suite is made in: one client, one Region, the way a
/// client that creates a bucket and then stores objects in it is configured.
const REGION: &str = "us-east-1";

/// The bundle every bucket here is created on — the cheapest published plan
/// (`crate::state::BUCKET_BUNDLE_SPECS`).
const BUNDLE_ID: &str = "small_1_0";

/// One answer from the object data plane.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

impl Answer {
    /// The wire error code: the `<Code>` element of S3's restXml error document (the
    /// model sets `noErrorWrapping`, so it is the document's own child), or the whole body
    /// when it carries no such element.
    ///
    /// Read with [`elements`], the same XML reader the listings go through, so an escaped
    /// or nested code is read as a client reads it rather than sliced out of the bytes.
    fn error_code(&self) -> String {
        elements(&self.body)
            .into_iter()
            .find(|(element, _)| element == "Code")
            .map_or_else(
                || String::from_utf8_lossy(&self.body).to_string(),
                |(_, code)| code,
            )
    }
}

/// Lightsail and Amazon S3 mounted on ONE shared engine, the way the router mounts
/// every service: the object store the Lightsail control plane provisions a bucket in
/// is the store the S3 data plane serves.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_lightsail_lightsail_20161128::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            engine,
        ));
        Self { router }
    }

    /// Call a Lightsail `operation` with `body`, answering its HTTP status and parsed
    /// body.
    async fn lightsail(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", ENDPOINT_HOST)
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("authorization", authorization("lightsail"))
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call a Lightsail `operation` and require success, answering the response body.
    async fn lightsail_ok(&self, operation: &str, body: Value) -> Value {
        let (status, parsed) = self.lightsail(operation, body).await;
        assert_eq!(status, 200, "{operation} must succeed, got {parsed:?}");
        parsed
    }

    /// One object-data-plane request, addressed at `ENDPOINT_HOST`.
    async fn object(&self, method: &str, path: &str, body: &[u8]) -> Answer {
        self.object_at(ENDPOINT_HOST, method, path, body).await
    }

    /// One object-data-plane request, addressed at `host` — what a client that followed
    /// the bucket's announced `url` sends.
    async fn object_at(&self, host: &str, method: &str, path: &str, body: &[u8]) -> Answer {
        let request = ::http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("authorization", authorization("s3"))
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let (parts, body) = response.into_parts();
        Answer {
            status: parts.status.as_u16(),
            body,
        }
    }

    /// `PUT /<bucket>/<key>` — S3's `PutObject`.
    async fn put_object(&self, bucket: &str, key: &str, body: &[u8]) -> Answer {
        self.object("PUT", &format!("/{bucket}/{key}"), body).await
    }

    /// `GET /<bucket>/<key>` — S3's `GetObject`.
    async fn get_object(&self, bucket: &str, key: &str) -> Answer {
        self.object("GET", &format!("/{bucket}/{key}"), b"").await
    }

    /// `DELETE /<bucket>/<key>` — S3's `DeleteObject`.
    async fn delete_object(&self, bucket: &str, key: &str) -> Answer {
        self.object("DELETE", &format!("/{bucket}/{key}"), b"").await
    }

    /// `GET /<bucket>?list-type=2` — S3's `ListObjectsV2`.
    async fn list_objects_v2(&self, bucket: &str) -> Answer {
        self.object("GET", &format!("/{bucket}?list-type=2"), b"")
            .await
    }

    /// Create a Lightsail bucket named `name`, answering the bucket it reported.
    async fn create_bucket(&self, name: &str) -> Value {
        self.lightsail_ok(
            "CreateBucket",
            json!({"bucketName": name, "bundleId": BUNDLE_ID}),
        )
        .await["bucket"]
            .clone()
    }

    /// Mint an access key for the bucket `name`, answering the `accessKey` it reported.
    async fn create_access_key(&self, name: &str) -> Value {
        self.lightsail_ok("CreateBucketAccessKey", json!({"bucketName": name}))
            .await["accessKey"]
            .clone()
    }

    /// Whether `GetBuckets` still reports a bucket named `name`.
    async fn bucket_exists(&self, name: &str) -> bool {
        let (status, body) = self.lightsail("GetBuckets", json!({"bucketName": name})).await;
        match status {
            200 => body["buckets"]
                .as_array()
                .is_some_and(|buckets| !buckets.is_empty()),
            404 => false,
            other => panic!("GetBuckets for {name:?} answered {other}: {body:?}"),
        }
    }
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is the
/// routing key the router claims a request by, and [`REGION`], which is the Region a
/// Lightsail call is made in. Neither the key id nor the signature is a credential:
/// this harness turns request authentication off, so nothing here is verified.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261022/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// Every element of an XML document as `(element name, text)` pairs in document order,
/// read with an XML reader rather than by slicing the body.
///
/// The reader breaks text at every entity reference, so an element's fragments are
/// accumulated: an ETag travels as `&quot;…&quot;`.
fn elements(body: &[u8]) -> Vec<(String, String)> {
    let document = String::from_utf8_lossy(body).into_owned();
    let mut reader = Reader::from_str(&document);
    let mut read = Vec::new();
    let mut element = String::new();
    let mut current = String::new();
    loop {
        match reader
            .read_event()
            .unwrap_or_else(|error| panic!("the body must be well-formed XML: {error}"))
        {
            Event::Start(start) => {
                element = String::from_utf8_lossy(start.name().as_ref()).into_owned();
                current.clear();
            }
            Event::Text(text) => current.push_str(&text.xml10_content().unwrap_or_default()),
            Event::GeneralRef(reference) => {
                current.push_str(&reference.decode().unwrap_or_default());
            }
            Event::End(end) => {
                let closing = String::from_utf8_lossy(end.name().as_ref()).into_owned();
                if closing == element {
                    read.push((closing, current.clone()));
                    current.clear();
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    read
}

/// The `Name` a listing reports and every `Key` it lists.
fn listing(answer: &Answer) -> (String, Vec<String>) {
    let read = elements(&answer.body);
    let mut name = String::new();
    let mut keys = Vec::new();
    for (element, text) in read {
        match element.as_str() {
            "Name" => name = text,
            "Key" => keys.push(text),
            _ => {}
        }
    }
    (name, keys)
}

/// One object round-trip case: what is written, and under which key.
struct ObjectRow {
    case: &'static str,
    key: &'static str,
    body: Vec<u8>,
}

/// The object cases. The rows differ along what the rule generalises over: the FORM of
/// the key (a flat name, a nested path, a name carrying dots and dashes, a single
/// character) and the SIZE and kind of the payload (empty, short text, a kibibyte of
/// binary bytes).
fn object_rows() -> Vec<ObjectRow> {
    vec![
        ObjectRow {
            case: "a flat key holding short text",
            key: "roundtrip-test.txt",
            body: b"lightsail bucket payload".to_vec(),
        },
        ObjectRow {
            case: "a nested key",
            key: "logs/2026/10/22/app.log",
            body: b"GET / 200\nGET /health 200\n".to_vec(),
        },
        ObjectRow {
            case: "an empty object",
            key: "empty-object",
            body: Vec::new(),
        },
        ObjectRow {
            case: "a kibibyte of binary bytes",
            key: "archive.tar.gz-001",
            body: (0..1024u32).map(|byte| (byte % 251) as u8).collect(),
        },
        ObjectRow {
            case: "a single-character key",
            key: "a",
            body: b"x".to_vec(),
        },
    ]
}

/// An object written to a Lightsail bucket's data plane reads back byte for byte, is
/// listed under the bucket's own name, and is gone — `NoSuchKey` — once deleted.
#[tokio::test]
async fn an_object_written_to_a_lightsail_bucket_reads_back_and_then_deletes() {
    let emulator = Emulator::new();
    let bucket = "object-roundtrip-store";
    emulator.create_bucket(bucket).await;

    // The key a client signs its object requests with is minted Active, carrying the
    // secret that is "returned only in response to the CreateBucketAccessKey action".
    let key = emulator.create_access_key(bucket).await;
    assert_eq!(
        key["status"].as_str(),
        Some("Active"),
        "the minted access key must be Active, got {key:?}"
    );
    for member in ["accessKeyId", "secretAccessKey"] {
        assert!(
            key[member].as_str().is_some_and(|value| !value.is_empty()),
            "the minted access key must carry a {member}, got {key:?}"
        );
    }

    for row in object_rows() {
        let case = row.case;
        let written = emulator.put_object(bucket, row.key, &row.body).await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: PutObject must succeed, got {} with {:?}",
            written.status,
            String::from_utf8_lossy(&written.body)
        );

        let read = emulator.get_object(bucket, row.key).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetObject must succeed, got {} with {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            read.body.as_ref(),
            row.body.as_slice(),
            "case {case:?}: GetObject must answer the bytes PutObject stored, got {} bytes",
            read.body.len()
        );

        let listed = emulator.list_objects_v2(bucket).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: ListObjectsV2 must succeed, got {} with {:?}",
            listed.status,
            String::from_utf8_lossy(&listed.body)
        );
        let (name, keys) = listing(&listed);
        assert_eq!(
            name, bucket,
            "case {case:?}: the listing's Name must be the bucket, got {name:?}"
        );
        assert_eq!(
            keys,
            vec![row.key.to_string()],
            "case {case:?}: the listing must report exactly the written key, got {keys:?}"
        );

        let removed = emulator.delete_object(bucket, row.key).await;
        assert_eq!(
            removed.status, 204,
            "case {case:?}: DeleteObject must answer 204, got {} with {:?}",
            removed.status,
            String::from_utf8_lossy(&removed.body)
        );
        let gone = emulator.get_object(bucket, row.key).await;
        assert_eq!(
            (gone.status, gone.error_code()),
            (404, "NoSuchKey".to_string()),
            "case {case:?}: a deleted key must answer NoSuchKey, got {} with {:?}",
            gone.status,
            String::from_utf8_lossy(&gone.body)
        );
        let (_, keys) = listing(&emulator.list_objects_v2(bucket).await);
        assert!(
            keys.is_empty(),
            "case {case:?}: the delete must leave the bucket empty, got {keys:?}"
        );
    }
}

/// The data plane serves the buckets this control plane created and NOTHING else: a
/// name no Lightsail bucket was created under answers `NoSuchBucket`, whether it never
/// named one or no longer does.
#[tokio::test]
async fn the_data_plane_serves_only_the_buckets_the_control_plane_created() {
    let emulator = Emulator::new();

    // (case, the bucket name a request addresses, whether a bucket is created first)
    let rows: &[(&str, &str, bool)] = &[
        ("a name that never named a bucket", "never-created-store", false),
        ("a name a bucket was created under", "created-store", true),
    ];

    for (case, name, created) in rows {
        if *created {
            emulator.create_bucket(name).await;
        }
        let written = emulator.put_object(name, "probe.txt", b"probe").await;
        let expected = if *created {
            (200, String::new())
        } else {
            (404, "NoSuchBucket".to_string())
        };
        let actual = if *created {
            (written.status, String::new())
        } else {
            (written.status, written.error_code())
        };
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            String::from_utf8_lossy(&written.body)
        );
    }

    // And a bucket the control plane deleted stops being served, so the data plane
    // answers for exactly the buckets that exist.
    emulator
        .lightsail_ok(
            "DeleteBucket",
            json!({"bucketName": "created-store", "forceDelete": true}),
        )
        .await;
    let after = emulator.put_object("created-store", "probe.txt", b"probe").await;
    assert_eq!(
        (after.status, after.error_code()),
        (404, "NoSuchBucket".to_string()),
        "case a deleted bucket: expected NoSuchBucket, got {} with {:?}",
        after.status,
        String::from_utf8_lossy(&after.body)
    );
}

/// `Bucket$objectVersioning` says what the bucket's OBJECT store does with the versions
/// of a key, so what the control plane reports is what the data plane's own versioning
/// state says: a bucket created with versioning keeps versions, one created without it
/// does not.
///
/// The rows differ along the three forms the `enableObjectVersioning` flag takes.
#[tokio::test]
async fn object_versioning_is_what_the_data_plane_was_provisioned_with() {
    // (case, the flag the create carries, the reported objectVersioning, the versioning
    // Status the data plane reports — empty for a bucket with no versioning state)
    let rows: &[(&str, Option<bool>, &str, &str)] = &[
        ("no flag at all", None, "NeverEnabled", ""),
        ("versioning declined", Some(false), "NeverEnabled", ""),
        ("versioning asked for", Some(true), "Enabled", "Enabled"),
    ];

    for (index, (case, flag, reported, status)) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        let bucket = format!("versioned-store-{index}");
        let mut request = json!({"bucketName": &bucket, "bundleId": BUNDLE_ID});
        if let Some(flag) = flag {
            request["enableObjectVersioning"] = json!(flag);
        }
        let created = emulator.lightsail_ok("CreateBucket", request).await["bucket"].clone();
        assert_eq!(
            created["objectVersioning"].as_str(),
            Some(*reported),
            "case {case:?}: expected objectVersioning {reported}, got {created:?}"
        );

        // `GET /<bucket>?versioning` — S3's `GetBucketVersioning`, the data plane's own
        // report of what it does with versions.
        let answer = emulator
            .object("GET", &format!("/{bucket}?versioning"), b"")
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: GetBucketVersioning must succeed, got {} with {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let reported_status = elements(&answer.body)
            .into_iter()
            .find(|(element, _)| element == "Status")
            .map(|(_, text)| text)
            .unwrap_or_default();
        assert_eq!(
            reported_status, *status,
            "case {case:?}: expected the data plane's versioning status {status:?}, got \
             {reported_status:?}"
        );
    }
}

/// The object API serves a Lightsail bucket's objects but does not own the bucket: an
/// S3 `DeleteBucket` addressed to it is refused, and the bucket keeps both its
/// control-plane record and its data plane.
#[tokio::test]
async fn the_object_api_does_not_delete_a_bucket_the_control_plane_owns() {
    let emulator = Emulator::new();
    let bucket = "lightsail-owned-store";
    emulator.create_bucket(bucket).await;

    let refused = emulator.object("DELETE", &format!("/{bucket}"), b"").await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (403, "AccessDenied".to_string()),
        "an S3 DeleteBucket of a Lightsail bucket must be refused, got {} with {:?}",
        refused.status,
        String::from_utf8_lossy(&refused.body)
    );
    assert!(
        emulator.bucket_exists(bucket).await,
        "the refused delete must leave the bucket's control-plane record"
    );
    let written = emulator.put_object(bucket, "still-served.txt", b"served").await;
    assert_eq!(
        written.status, 200,
        "the refused delete must leave the bucket's data plane, got {} with {:?}",
        written.status,
        String::from_utf8_lossy(&written.body)
    );
}

/// `Bucket$url` is "The URL of the bucket", so the address it announces is the address
/// this emulator actually answers object requests for the bucket on: a request to that
/// URL's own host and path is served.
///
/// The rows differ along the endpoint the create arrived at, which is what the address
/// is built from.
#[tokio::test]
async fn the_announced_url_is_where_the_data_plane_answers() {
    // (case, the Host the create arrives at, the bucket's name)
    let rows: &[(&str, &str, &str)] = &[
        ("the default data port", ENDPOINT_HOST, "announced-store-one"),
        (
            "another host and port",
            "lightsail.test.internal:8443",
            "announced-store-two",
        ),
    ];

    for (case, host, name) in rows {
        let emulator = Emulator::new();
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", *host)
            .header("x-amz-target", format!("{TARGET_PREFIX}.CreateBucket"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("authorization", authorization("lightsail"))
            .body(bytes::Bytes::from(
                json!({"bucketName": name, "bundleId": BUNDLE_ID}).to_string(),
            ))
            .expect("the request is well formed");
        let response = emulator.router.dispatch(request).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "case {case:?}: CreateBucket must succeed, got {:?}",
            String::from_utf8_lossy(response.body())
        );
        let body: Value =
            serde_json::from_slice(response.body()).expect("the answer is a JSON document");
        let url = body["bucket"]["url"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an announced url, got {body:?}"))
            .to_string();

        // The announced address, taken apart the way a client follows it: its host is
        // the endpoint the create arrived at, and its path addresses the bucket.
        let authority_and_path = url
            .strip_prefix("https://")
            .unwrap_or_else(|| panic!("case {case:?}: expected an https address, got {url:?}"));
        let (authority, path) = authority_and_path
            .split_once('/')
            .unwrap_or_else(|| panic!("case {case:?}: expected a path in {url:?}"));
        assert_eq!(
            authority, *host,
            "case {case:?}: the address must name the endpoint the request arrived at, got {url:?}"
        );

        // Following it reaches this bucket's objects: a listing of the announced address
        // answers for this bucket.
        let listed = emulator
            .object_at(authority, "GET", &format!("/{path}?list-type=2"), b"")
            .await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: a listing of the announced address must succeed, got {} with {:?}",
            listed.status,
            String::from_utf8_lossy(&listed.body)
        );
        let (listed_name, _) = listing(&listed);
        assert_eq!(
            listed_name, *name,
            "case {case:?}: the announced address must address this bucket, got {listed_name:?}"
        );
    }
}

/// One delete case: what the bucket holds when the delete is made, whether the request
/// forces it, and what must happen.
struct DeleteRow {
    case: &'static str,
    /// The object keys the bucket holds when the delete is made.
    objects: &'static [&'static str],
    /// How many access keys the bucket holds then.
    access_keys: usize,
    /// The `forceDelete` the request carries, absent when `None`.
    force_delete: Option<bool>,
    /// The HTTP status the delete must answer.
    status: u16,
    /// The error code it must carry, or `None` for a delete that succeeds.
    error_code: Option<&'static str>,
}

/// The delete cases. The rows differ along each documented condition a force delete is
/// required for (objects, access keys), how many of them hold at once, and the three
/// forms `forceDelete` takes (absent, false, true).
///
/// The third condition this store can hold — instances granted access through
/// `SetResourceAccessForBucket` — has no row: that operation is still a 501 scaffold
/// under its own issue, so no bucket in this emulator can have one yet.
const DELETE_ROWS: &[DeleteRow] = &[
    DeleteRow {
        case: "an empty bucket with no access keys, no flag",
        objects: &[],
        access_keys: 0,
        force_delete: None,
        status: 200,
        error_code: None,
    },
    DeleteRow {
        case: "an empty bucket with no access keys, forceDelete false",
        objects: &[],
        access_keys: 0,
        force_delete: Some(false),
        status: 200,
        error_code: None,
    },
    DeleteRow {
        case: "a bucket holding one object, no flag",
        objects: &["blocking-object.txt"],
        access_keys: 0,
        force_delete: None,
        status: 400,
        error_code: Some("InvalidInputException"),
    },
    DeleteRow {
        case: "a bucket holding three objects, forceDelete false",
        objects: &["one.txt", "two.txt", "nested/three.txt"],
        access_keys: 0,
        force_delete: Some(false),
        status: 400,
        error_code: Some("InvalidInputException"),
    },
    DeleteRow {
        case: "a bucket holding one object, forceDelete true",
        objects: &["blocking-object.txt"],
        access_keys: 0,
        force_delete: Some(true),
        status: 200,
        error_code: None,
    },
    DeleteRow {
        case: "an empty bucket holding one access key, no flag",
        objects: &[],
        access_keys: 1,
        force_delete: None,
        status: 400,
        error_code: Some("InvalidInputException"),
    },
    DeleteRow {
        case: "a bucket holding objects and two access keys, forceDelete true",
        objects: &["kept.txt"],
        access_keys: 2,
        force_delete: Some(true),
        status: 200,
        error_code: None,
    },
];

/// A `DeleteBucket` without `forceDelete` is refused while the bucket still holds
/// anything the model says a force delete is required for, and the refusal leaves the
/// bucket and its objects exactly as they were; a delete that goes through releases
/// both the record and the object store, so the data plane stops answering for it and
/// the name is free again.
#[tokio::test]
async fn delete_bucket_refuses_what_a_force_delete_is_required_for() {
    for row in DELETE_ROWS {
        let case = row.case;
        let emulator = Emulator::new();
        let bucket = "delete-gated-store";
        emulator.create_bucket(bucket).await;
        for index in 0..row.access_keys {
            let key = emulator.create_access_key(bucket).await;
            assert!(
                key["accessKeyId"].as_str().is_some(),
                "case {case:?}: access key {index} must be minted, got {key:?}"
            );
        }
        for key in row.objects {
            let written = emulator.put_object(bucket, key, b"blocking").await;
            assert_eq!(
                written.status, 200,
                "case {case:?}: writing {key:?} must succeed, got {} with {:?}",
                written.status,
                String::from_utf8_lossy(&written.body)
            );
        }

        let mut request = json!({"bucketName": bucket});
        if let Some(force_delete) = row.force_delete {
            request["forceDelete"] = json!(force_delete);
        }
        let (status, body) = emulator.lightsail("DeleteBucket", request).await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected HTTP {}, got {status} with {body:?}",
            row.status
        );
        match row.error_code {
            Some(expected) => {
                assert_eq!(
                    error_code(&body),
                    expected,
                    "case {case:?}: expected {expected}, got {body:?}"
                );
                // The refusal names the bucket and the condition that blocks the
                // delete, which is what a caller needs to know what to clear.
                let message = body["message"].as_str().unwrap_or_default();
                let condition = if row.objects.is_empty() {
                    "in use"
                } else {
                    "not empty"
                };
                assert!(
                    message.contains(bucket) && message.contains(condition),
                    "case {case:?}: expected a refusal naming {bucket:?} and {condition:?}, \
                     got {message:?}"
                );
                // The refusal changed nothing: the bucket is still there, and so is
                // everything it held.
                assert!(
                    emulator.bucket_exists(bucket).await,
                    "case {case:?}: the refused delete must leave the bucket"
                );
                let (_, keys) = listing(&emulator.list_objects_v2(bucket).await);
                let mut expected_keys: Vec<String> =
                    row.objects.iter().map(|key| (*key).to_string()).collect();
                expected_keys.sort();
                assert_eq!(
                    keys, expected_keys,
                    "case {case:?}: the refused delete must leave every object, got {keys:?}"
                );
            }
            None => {
                let operations = body["operations"].as_array().unwrap_or_else(|| {
                    panic!("case {case:?}: a delete must answer its operations, got {body:?}")
                });
                assert_eq!(
                    operations.len(),
                    1,
                    "case {case:?}: expected one operation, got {operations:?}"
                );
                assert_eq!(
                    operations[0]["operationType"].as_str(),
                    Some("DeleteBucket"),
                    "case {case:?}: expected a DeleteBucket operation, got {:?}",
                    operations[0]
                );
                assert_eq!(
                    operations[0]["resourceName"].as_str(),
                    Some(bucket),
                    "case {case:?}: the operation must name the bucket, got {:?}",
                    operations[0]
                );
                // The record is gone, the object store with it, and the name is free.
                assert!(
                    !emulator.bucket_exists(bucket).await,
                    "case {case:?}: the deleted bucket must be gone from every read"
                );
                let served = emulator.list_objects_v2(bucket).await;
                assert_eq!(
                    (served.status, served.error_code()),
                    (404, "NoSuchBucket".to_string()),
                    "case {case:?}: the data plane must stop answering for a deleted bucket, \
                     got {} with {:?}",
                    served.status,
                    String::from_utf8_lossy(&served.body)
                );
                let recreated = emulator
                    .lightsail(
                        "CreateBucket",
                        json!({"bucketName": bucket, "bundleId": BUNDLE_ID}),
                    )
                    .await;
                assert_eq!(
                    recreated.0, 200,
                    "case {case:?}: the released name must be free again, got {:?}",
                    recreated.1
                );
            }
        }
    }
}

/// A delete that names no bucket of this account is the modelled 404, and it deletes
/// nothing — whatever `forceDelete` says.
#[tokio::test]
async fn deleting_a_bucket_that_does_not_exist_is_a_not_found() {
    let emulator = Emulator::new();
    emulator.create_bucket("surviving-store").await;

    // (case, the name the delete addresses, the forceDelete it carries)
    let rows: &[(&str, &str, Option<bool>)] = &[
        ("a name that never named a bucket", "no-such-store", None),
        (
            "a name that never named a bucket, forced",
            "no-such-store",
            Some(true),
        ),
    ];

    for (case, name, force_delete) in rows {
        let mut request = json!({"bucketName": name});
        if let Some(force_delete) = force_delete {
            request["forceDelete"] = json!(force_delete);
        }
        let (status, body) = emulator.lightsail("DeleteBucket", request).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body:?}"
        );
        assert!(
            emulator.bucket_exists("surviving-store").await,
            "case {case:?}: the refused delete must leave every other bucket"
        );
    }
}
