// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the S3 Files file-system and access-point lifecycle:
//! `CreateFileSystem`, `GetFileSystem`, `ListFileSystems`, `CreateAccessPoint`,
//! `GetAccessPoint` and `DeleteFileSystem`.
//!
//! Every case drives the service the way a client does — one restJson1 HTTP request per
//! call, through the same `MultiServiceRouter` the emulator boots — so what is asserted
//! is the emulator's HTTP behaviour, not a handler called directly.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. A create persists; a read answers from what was persisted, member for member; a
//!    delete makes the resource observably gone.
//! 2. A minted id and ARN satisfy the model's own patterns, and the ARN names the
//!    region the request was signed for and the canonical emulator account.
//! 3. `clientToken` is an `@idempotencyToken`: one token creates one resource, a
//!    different token creates another.
//! 4. An id-valued member accepts either form the model allows — the bare id or the
//!    resource's ARN — and both name the same record.
//! 5. A listing answers every persisted file system matching its filter, in one stable
//!    order, cut into pages of at most `maxResults` with a `nextToken` that resumes
//!    exactly where the previous page stopped.
//! 6. A resource reported in a non-terminal lifecycle state becomes `available` because
//!    the clock made the transition due, not because anybody read it.
//! 7. A file system still in use is not deleted unless the caller asked for its
//!    children to go with it.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A region that is NOT the emulator default, so an ARN built from the request's own
/// SigV4 credential scope is distinguishable from a hardcoded one.
pub(crate) const REGION: &str = "eu-west-2";

/// The canonical emulator account id every ARN carries
/// (`engine::emulator_account_id()`).
pub(crate) const ACCOUNT: &str = "000000000000";

/// The provisioning span the service itself uses, so the transient-status assertion is
/// tied to the implementation's own constant rather than to a copy of it.
const PROVISIONING_SPAN: Duration = service_com_amazonaws_s3files_s3files::state::PROVISIONING_SPAN;

/// Comfortably longer than [`PROVISIONING_SPAN`], so a `creating` -> `available`
/// transition the clock owes has come due.
const PAST_PROVISIONING: Duration = Duration::from_millis(900);

/// An IAM role ARN satisfying `RoleArn`'s pattern
/// `^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`.
pub(crate) const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/S3FilesRole";

/// What one call answered: the HTTP status, the modelled error shape (empty on success)
/// and the parsed body (`Value::Null` for an empty body).
#[derive(Debug)]
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) error_type: String,
    pub(crate) body: Value,
}

impl Answer {
    pub(crate) fn field(&self, name: &str) -> &Value {
        &self.body[name]
    }

    pub(crate) fn text(&self, name: &str) -> String {
        self.body[name]
            .as_str()
            .unwrap_or_else(|| panic!("expected a string `{name}` in {:?}", self.body))
            .to_string()
    }
}

pub(crate) fn new_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the emulator's own concern and is exercised elsewhere;
    // these cases are about this service's behaviour.
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_s3files_s3files::EmulatedService::new(
        engine,
    ));
    router
}

pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let payload = match body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://s3files.{REGION}.amazonaws.com{path}"))
        // The credential scope is how a handler learns the region its ARNs name.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/s3files/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(['/', ':'])
                .next()
                .unwrap_or(value)
                .trim()
                .to_string()
        })
        .unwrap_or_default();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type,
        body,
    }
}

/// `true` when `value` is `<prefix>` followed by 17..=40 lowercase hex characters: the
/// shape the model's `FileSystemId` (`fs-[0-9a-f]{17,40}`) and `AccessPointId`
/// (`fsap-[0-9a-f]{17,40}`) patterns accept (smithy_query: shape FileSystemId /
/// AccessPointId).
pub(crate) fn is_minted_id(prefix: &str, value: &str) -> bool {
    let Some(hex) = value.strip_prefix(prefix) else {
        return false;
    };
    (17..=40).contains(&hex.len()) && hex.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// The `FileSystemArn` prefix every file system this emulator mints must carry, from the
/// model's pattern
/// `^(arn:aws[-a-z]*:s3files:[0-9a-z-:]+:file-system/fs-[0-9a-f]{17,40})$` plus the
/// canonical emulator account id and the region the request was signed for.
pub(crate) fn file_system_arn_prefix() -> String {
    format!("arn:aws:s3files:{REGION}:{ACCOUNT}:file-system/")
}

/// `CreateFileSystem`.
pub(crate) async fn create_file_system(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "PUT", "/file-systems", Some(body)).await
}

/// `CreateFileSystem` on `bucket`, asserting it was accepted, and returning its id.
pub(crate) async fn created_file_system_id(
    router: &MultiServiceRouter,
    bucket: &str,
    client_token: Option<&str>,
) -> String {
    let mut request = json!({ "bucket": bucket, "roleArn": ROLE_ARN });
    if let Some(token) = client_token {
        request["clientToken"] = json!(token);
    }
    let created = create_file_system(router, request).await;
    assert_eq!(
        created.status, 201,
        "case CreateFileSystem({bucket:?}): expected 201, got {} / {:?}",
        created.status, created.body
    );
    created.text("fileSystemId")
}

/// `CreateAccessPoint` under `file_system_id`.
pub(crate) async fn create_access_point(
    router: &MultiServiceRouter,
    file_system_id: &str,
    extra: Value,
) -> Answer {
    let mut request = json!({ "fileSystemId": file_system_id });
    if let Some(members) = extra.as_object() {
        for (key, value) in members {
            request[key] = value.clone();
        }
    }
    call(router, "PUT", "/access-points", Some(request)).await
}

/// The same body with `keys` dropped, so two answers can be compared on everything the
/// clock does not move.
pub(crate) fn without(value: &Value, keys: &[&str]) -> Value {
    let mut copy = value.clone();
    if let Some(members) = copy.as_object_mut() {
        for key in keys {
            members.remove(*key);
        }
    }
    copy
}

/// Every `LifeCycleState` value a freshly created resource may be reported in: the
/// transient one, or the settled one when the provisioning span has already elapsed.
pub(crate) fn assert_lifecycle_state(case: &str, status: &str) {
    assert!(
        status == "creating" || status == "available",
        "case {case:?}: a created resource is `creating` until the clock owes it \
         `available`, got {status:?}"
    );
}

/// The table of `CreateFileSystem` inputs rule 1 and rule 2 are asserted over. The rows
/// differ along every member the create takes: the bucket, the presence and depth of a
/// prefix, the KMS key form, the partition, and how many tags are sent.
fn create_file_system_table() -> Vec<(&'static str, Value)> {
    vec![
        (
            "every member",
            json!({
                "bucket": "arn:aws:s3:::s3files-table-full",
                "roleArn": ROLE_ARN,
                "prefix": "data/",
                "kmsKeyId": "12345678-1234-1234-1234-123456789012",
                "tags": [{ "key": "Name", "value": "full" }, { "key": "env", "value": "test" }],
                "clientToken": "token-full",
                "acceptBucketWarning": true,
            }),
        ),
        (
            "only the required members",
            json!({ "bucket": "arn:aws:s3:::s3files-table-minimal", "roleArn": ROLE_ARN }),
        ),
        (
            "a deep prefix and one tag",
            json!({
                "bucket": "arn:aws:s3:::s3files-table-deep",
                "roleArn": ROLE_ARN,
                "prefix": "a/b/c/d/",
                "tags": [{ "key": "team", "value": "storage" }],
            }),
        ),
        (
            "a KMS alias and an explicitly empty prefix",
            json!({
                "bucket": "arn:aws:s3:::s3files-table-alias",
                "roleArn": ROLE_ARN,
                "prefix": "",
                "kmsKeyId": "alias/s3files-test",
            }),
        ),
        (
            "another partition",
            json!({
                "bucket": "arn:aws-cn:s3:::s3files-table-partition",
                "roleArn": "arn:aws-cn:iam::123456789012:role/path/S3FilesRole",
                "prefix": "cn/",
                "kmsKeyId": "mrk-12345678901234567890123456789012",
            }),
        ),
    ]
}

/// Rule 1 and rule 2: whatever the create was given is persisted, read back member for
/// member by `GetFileSystem`, and carries a minted id and ARN of the modelled shape.
#[tokio::test]
async fn a_created_file_system_is_persisted_and_read_back_member_for_member() {
    let router = new_router();

    for (case, request) in create_file_system_table() {
        let created = create_file_system(&router, request.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201, got {} / {:?}",
            created.status, created.body
        );

        let id = created.text("fileSystemId");
        let arn = created.text("fileSystemArn");
        assert!(
            is_minted_id("fs-", &id),
            "case {case:?}: expected a minted fs-<17..40 hex> id, got {id:?}"
        );
        assert_eq!(
            arn,
            format!("{}{id}", file_system_arn_prefix()),
            "case {case:?}: the ARN must name the signed region, the emulator account and the id"
        );

        // Echoed-back input is echoed from the STORED record, so each member is checked
        // against what the request asked for.
        assert_eq!(
            created.text("bucket"),
            request["bucket"].as_str().unwrap(),
            "case {case:?}: bucket"
        );
        assert_eq!(
            created.text("roleArn"),
            request["roleArn"].as_str().unwrap(),
            "case {case:?}: roleArn"
        );
        // `prefix` carries @default(""): a caller that named none gets the empty prefix,
        // never an absent member.
        assert_eq!(
            created.text("prefix"),
            request["prefix"].as_str().unwrap_or(""),
            "case {case:?}: prefix"
        );
        assert_eq!(
            created.field("kmsKeyId"),
            &request["kmsKeyId"],
            "case {case:?}: kmsKeyId"
        );
        assert_eq!(
            created.field("tags"),
            &request["tags"],
            "case {case:?}: tags"
        );
        assert_eq!(
            created.text("ownerId"),
            ACCOUNT,
            "case {case:?}: ownerId is the account that owns the file system"
        );
        // The model documents `name` as "derived from the Name tag if present".
        let expected_name = request["tags"]
            .as_array()
            .and_then(|tags| {
                tags.iter()
                    .find(|tag| tag["key"] == json!("Name"))
                    .map(|tag| tag["value"].clone())
            })
            .unwrap_or(Value::Null);
        assert_eq!(
            created.field("name"),
            &expected_name,
            "case {case:?}: name comes from the Name tag"
        );
        assert_lifecycle_state(case, &created.text("status"));
        let creation_time = created
            .field("creationTime")
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: expected a creationTime, got {created:?}"));
        assert!(
            creation_time > 1.6e9,
            "case {case:?}: creationTime must be the epoch seconds of the create, got {creation_time}"
        );

        // The read answers from the same stored record: every member but the lifecycle
        // state, which the clock may have moved on since the create answered.
        let read = call(&router, "GET", &format!("/file-systems/{id}"), None).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetFileSystem, got {} / {:?}",
            read.status, read.body
        );
        assert_eq!(
            without(&read.body, &["status"]),
            without(&created.body, &["status"]),
            "case {case:?}: GetFileSystem must answer exactly what CreateFileSystem stored"
        );
        assert_lifecycle_state(case, &read.text("status"));
    }
}

/// Rule 3: one `clientToken` creates one file system, however many times it is sent; a
/// different token creates another; and a create that names no token is a new create
/// each time, because the token it is recorded under is minted per call.
#[tokio::test]
async fn client_tokens_make_creates_idempotent() {
    let router = new_router();
    let bucket = "arn:aws:s3:::s3files-idempotency";
    let long_token = "t".repeat(64);
    let tokens = [
        ("a short token", "t-1"),
        ("another token", "token-two"),
        ("a token at the 64-character bound", long_token.as_str()),
    ];

    let mut minted = BTreeSet::new();
    for (case, token) in tokens {
        let first = created_file_system_id(&router, bucket, Some(token)).await;
        for attempt in 1..=2 {
            let replayed = created_file_system_id(&router, bucket, Some(token)).await;
            assert_eq!(
                replayed, first,
                "case {case:?} (replay {attempt}): the same clientToken must return the \
                 same file system, expected {first}, got {replayed}"
            );
        }
        assert!(
            minted.insert(first.clone()),
            "case {case:?}: a different clientToken must create a different file system, \
             got {first} again"
        );
    }

    // Two creates that name no token are two creates: the model documents the SDK as
    // generating a token per call, so nothing ties them together.
    let untokened_first = created_file_system_id(&router, bucket, None).await;
    let untokened_second = created_file_system_id(&router, bucket, None).await;
    assert_ne!(
        untokened_first, untokened_second,
        "case \"no clientToken\": each call is its own create, got {untokened_first} twice"
    );

    let listed = call(&router, "GET", "/file-systems", None).await;
    let count = listed
        .field("fileSystems")
        .as_array()
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        count,
        tokens.len() + 2,
        "expected one file system per distinct token plus the two untokened creates, \
         got {:?}",
        listed.body
    );
}

/// Rule 4: a `FileSystemId` may be the bare id or the file system's ARN, and both name
/// the same record — on the path label of a read and on the body member of a create.
#[tokio::test]
async fn either_id_form_names_the_same_file_system() {
    let router = new_router();
    let id = created_file_system_id(&router, "arn:aws:s3:::s3files-id-forms", Some("forms")).await;
    let arn = format!("{}{id}", file_system_arn_prefix());
    // A non-greedy `@httpLabel` carries the ARN percent-encoded, which is how an SDK
    // puts a value containing `/` and `:` into a path segment.
    let encoded_arn = arn.replace(':', "%3A").replace('/', "%2F");

    for (case, label) in [("the bare id", id.clone()), ("the ARN", encoded_arn)] {
        let read = call(&router, "GET", &format!("/file-systems/{label}"), None).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetFileSystem, got {} / {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.text("fileSystemId"),
            id,
            "case {case:?}: both forms must name the same record"
        );
    }

    for (case, member) in [("the bare id", id.clone()), ("the ARN", arn.clone())] {
        let created = create_access_point(&router, &member, json!({})).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateAccessPoint, got {} / {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.text("fileSystemId"),
            id,
            "case {case:?}: the access point must be bound to the parent by its id"
        );
    }
}

/// Rule 1's failure half: an id the service never minted is a 404, and a value that is
/// not an id at all is a 400 — over both resources and over the operations that take
/// one.
#[tokio::test]
async fn an_unknown_id_is_not_found_and_a_malformed_one_is_refused() {
    let router = new_router();

    // 17 hex characters, so the value is a well-formed id the store simply does not
    // hold; and two that no `fs-[0-9a-f]{17,40}` / `fsap-[0-9a-f]{17,40}` accepts.
    let cases = [
        ("an unknown file system", "/file-systems/fs-00000000000000000", 404, "ResourceNotFoundException"),
        ("a file system id that is too short", "/file-systems/fs-0a1b2c3d", 400, "ValidationException"),
        ("a file system id that is not hex", "/file-systems/fs-zzzzzzzzzzzzzzzzz", 400, "ValidationException"),
        ("an unknown access point", "/access-points/fsap-00000000000000000", 404, "ResourceNotFoundException"),
        ("an access point id with the wrong prefix", "/access-points/fs-00000000000000000", 400, "ValidationException"),
    ];
    for (case, path, status, error) in cases {
        let read = call(&router, "GET", path, None).await;
        assert_eq!(
            read.status, status,
            "case {case:?}: expected HTTP {status}, got {} / {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.error_type, error,
            "case {case:?}: expected the modelled {error}, got {:?} / {:?}",
            read.error_type, read.body
        );
    }

    // The same two answers from the operations that take an id and change state.
    let deleted = call(&router, "DELETE", "/file-systems/fs-00000000000000000", None).await;
    assert_eq!(
        deleted.status, 404,
        "case \"delete an unknown file system\": expected 404, got {} / {:?}",
        deleted.status, deleted.body
    );
    let orphan = create_access_point(&router, "fs-00000000000000000", json!({})).await;
    assert_eq!(
        orphan.status, 404,
        "case \"an access point under an unknown file system\": expected 404, got {} / {:?}",
        orphan.status, orphan.body
    );
    assert_eq!(
        orphan.error_type, "ResourceNotFoundException",
        "case \"an access point under an unknown file system\": expected the modelled \
         ResourceNotFoundException, got {:?}",
        orphan.error_type
    );
}

/// Rule 5: a listing reports every file system that matches its filter exactly once, in
/// one stable order, in pages of at most `maxResults`, and `nextToken` resumes where the
/// previous page stopped — over a table of page sizes and over the `bucket` filter.
#[tokio::test]
async fn listing_pages_at_most_max_results_and_resumes_where_it_stopped() {
    let router = new_router();
    let buckets = [
        "arn:aws:s3:::s3files-list-a",
        "arn:aws:s3:::s3files-list-b",
    ];
    // Five file systems, unevenly split across two buckets, so a filtered listing and an
    // unfiltered one page differently.
    let mut by_bucket: Vec<(String, String)> = Vec::new();
    for index in 0..5 {
        let bucket = buckets[usize::from(index >= 3)];
        let id = created_file_system_id(&router, bucket, Some(&format!("list-{index}"))).await;
        by_bucket.push((bucket.to_string(), id));
    }
    let everything: BTreeSet<String> = by_bucket.iter().map(|(_, id)| id.clone()).collect();

    // `maxResults` is @range(1, 100) with @default(100): every page size inside the
    // range, each published bound, and the absent case that takes the default.
    for page_size in [Some(1), Some(2), Some(3), Some(5), Some(100), None] {
        for filter in [None, Some(buckets[0]), Some(buckets[1])] {
            let case = format!("maxResults={page_size:?}, bucket={filter:?}");
            let expected: BTreeSet<String> = match filter {
                None => everything.clone(),
                Some(bucket) => by_bucket
                    .iter()
                    .filter(|(on, _)| on == bucket)
                    .map(|(_, id)| id.clone())
                    .collect(),
            };

            let mut seen: Vec<String> = Vec::new();
            let mut next_token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut query: Vec<String> = Vec::new();
                if let Some(size) = page_size {
                    query.push(format!("maxResults={size}"));
                }
                if let Some(bucket) = filter {
                    query.push(format!("bucket={}", bucket.replace(':', "%3A")));
                }
                if let Some(token) = &next_token {
                    query.push(format!("nextToken={token}"));
                }
                let path = if query.is_empty() {
                    "/file-systems".to_string()
                } else {
                    format!("/file-systems?{}", query.join("&"))
                };
                let listed = call(&router, "GET", &path, None).await;
                assert_eq!(
                    listed.status, 200,
                    "case {case:?}: expected 200 from ListFileSystems, got {} / {:?}",
                    listed.status, listed.body
                );
                let page = listed
                    .field("fileSystems")
                    .as_array()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected a fileSystems array, got {:?}", listed.body)
                    })
                    .clone();
                let limit = page_size.unwrap_or(100) as usize;
                assert!(
                    page.len() <= limit,
                    "case {case:?}: a page holds at most {limit} items, got {}",
                    page.len()
                );
                for item in &page {
                    // Every required member of `ListFileSystemsDescription` is answered
                    // from the stored record.
                    for member in [
                        "fileSystemId",
                        "fileSystemArn",
                        "bucket",
                        "status",
                        "roleArn",
                        "ownerId",
                    ] {
                        assert!(
                            item[member].is_string(),
                            "case {case:?}: a listed file system carries {member}, got {item:?}"
                        );
                    }
                    assert!(
                        item["creationTime"].is_number(),
                        "case {case:?}: a listed file system carries creationTime, got {item:?}"
                    );
                    if let Some(bucket) = filter {
                        assert_eq!(
                            item["bucket"], json!(bucket),
                            "case {case:?}: the bucket filter must exclude every other bucket"
                        );
                    }
                    seen.push(item["fileSystemId"].as_str().unwrap().to_string());
                }
                pages += 1;
                assert!(
                    pages <= expected.len() + 1,
                    "case {case:?}: the listing did not terminate after {pages} pages"
                );
                match listed.field("nextToken").as_str() {
                    Some(token) => next_token = Some(token.to_string()),
                    None => break,
                }
            }

            let unique: BTreeSet<String> = seen.iter().cloned().collect();
            assert_eq!(
                unique, expected,
                "case {case:?}: the pages together must report exactly the matching file systems"
            );
            assert_eq!(
                seen.len(),
                unique.len(),
                "case {case:?}: no file system may appear on two pages, got {seen:?}"
            );
        }
    }

    // A `nextToken` the service never issued is refused rather than silently honoured.
    let forged = call(&router, "GET", "/file-systems?nextToken=not-a-token", None).await;
    assert_eq!(
        forged.status, 400,
        "case \"a forged nextToken\": expected 400, got {} / {:?}",
        forged.status, forged.body
    );
    assert_eq!(
        forged.error_type, "ValidationException",
        "case \"a forged nextToken\": expected the modelled ValidationException, got {:?}",
        forged.error_type
    );
}

/// Rule 1 for the child resource: an access point round-trips through `GetAccessPoint`
/// over a table of `posixUser` / `rootDirectory` / tag shapes, and its ARN nests under
/// its parent's.
#[tokio::test]
async fn a_created_access_point_is_persisted_and_read_back_member_for_member() {
    let router = new_router();
    let file_system_id =
        created_file_system_id(&router, "arn:aws:s3:::s3files-access-points", Some("ap")).await;
    let file_system_arn = format!("{}{file_system_id}", file_system_arn_prefix());

    let table = vec![
        ("no optional member", json!({ "clientToken": "ap-plain" })),
        (
            "a posix user",
            json!({
                "clientToken": "ap-posix",
                "posixUser": { "uid": 1000, "gid": 1000 },
            }),
        ),
        (
            "a posix user with secondary gids",
            json!({
                "clientToken": "ap-secondary",
                "posixUser": { "uid": 0, "gid": 0, "secondaryGids": [1, 2, 3] },
            }),
        ),
        (
            "a root directory with creation permissions",
            json!({
                "clientToken": "ap-root",
                "rootDirectory": {
                    "path": "/team/data",
                    "creationPermissions": { "ownerUid": 42, "ownerGid": 43, "permissions": "0755" },
                },
                "tags": [{ "key": "Name", "value": "team-data" }],
            }),
        ),
    ];

    for (case, extra) in table {
        let created = create_access_point(&router, &file_system_id, extra.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200, got {} / {:?}",
            created.status, created.body
        );

        let access_point_id = created.text("accessPointId");
        assert!(
            is_minted_id("fsap-", &access_point_id),
            "case {case:?}: expected a minted fsap-<17..40 hex> id, got {access_point_id:?}"
        );
        assert_eq!(
            created.text("accessPointArn"),
            format!("{file_system_arn}/access-point/{access_point_id}"),
            "case {case:?}: an access point ARN nests under its file system's"
        );
        assert_eq!(
            created.text("fileSystemId"),
            file_system_id,
            "case {case:?}: fileSystemId"
        );
        assert_eq!(
            created.text("ownerId"),
            ACCOUNT,
            "case {case:?}: ownerId"
        );
        assert_eq!(
            created.field("posixUser"),
            &extra["posixUser"],
            "case {case:?}: posixUser"
        );
        assert_eq!(
            created.field("rootDirectory"),
            &extra["rootDirectory"],
            "case {case:?}: rootDirectory"
        );
        assert_eq!(
            created.text("clientToken"),
            extra["clientToken"].as_str().unwrap(),
            "case {case:?}: clientToken"
        );
        assert_lifecycle_state(case, &created.text("status"));

        let read = call(
            &router,
            "GET",
            &format!("/access-points/{access_point_id}"),
            None,
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetAccessPoint, got {} / {:?}",
            read.status, read.body
        );
        assert_eq!(
            without(&read.body, &["status"]),
            without(&created.body, &["status"]),
            "case {case:?}: GetAccessPoint must answer exactly what CreateAccessPoint stored"
        );
    }

    // A create carrying no `clientToken` still answers the required member, with the
    // token the service recorded the creation under.
    let untokened = create_access_point(&router, &file_system_id, json!({})).await;
    assert_eq!(
        untokened.status, 200,
        "case \"no clientToken\": expected 200, got {} / {:?}",
        untokened.status, untokened.body
    );
    assert!(
        !untokened.text("clientToken").is_empty(),
        "case \"no clientToken\": clientToken is @required on the response, got {:?}",
        untokened.body
    );
}

/// Rule 7: a file system with children is refused unless `forceDelete` is true, and a
/// forced delete takes the children with it. Rule 1's delete half: what is deleted is
/// observably gone to the next caller.
#[tokio::test]
async fn delete_refuses_a_file_system_in_use_unless_its_children_may_go_too() {
    let router = new_router();
    let bucket = "arn:aws:s3:::s3files-delete";
    let parent = created_file_system_id(&router, bucket, Some("delete-parent")).await;
    let mut children = Vec::new();
    for index in 0..2 {
        let created = create_access_point(
            &router,
            &parent,
            json!({ "clientToken": format!("child-{index}") }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case \"create child {index}\": expected 200, got {} / {:?}",
            created.status, created.body
        );
        children.push(created.text("accessPointId"));
    }

    // Neither an absent `forceDelete` nor an explicit false may remove a file system in
    // use.
    for (case, query) in [("absent", ""), ("false", "?forceDelete=false")] {
        let refused = call(
            &router,
            "DELETE",
            &format!("/file-systems/{parent}{query}"),
            None,
        )
        .await;
        assert_eq!(
            refused.status, 409,
            "case \"forceDelete {case}\": expected the modelled 409, got {} / {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type, "ConflictException",
            "case \"forceDelete {case}\": expected the modelled ConflictException, got {:?}",
            refused.error_type
        );
        let still_there = call(&router, "GET", &format!("/file-systems/{parent}"), None).await;
        assert_eq!(
            still_there.status, 200,
            "case \"forceDelete {case}\": a refused delete must change nothing, got {} / {:?}",
            still_there.status, still_there.body
        );
    }

    let forced = call(
        &router,
        "DELETE",
        &format!("/file-systems/{parent}?forceDelete=true"),
        None,
    )
    .await;
    assert_eq!(
        forced.status, 204,
        "case \"forceDelete true\": expected 204, got {} / {:?}",
        forced.status, forced.body
    );
    assert_eq!(
        forced.body,
        Value::Null,
        "case \"forceDelete true\": a 204 carries no body, got {:?}",
        forced.body
    );

    let gone = call(&router, "GET", &format!("/file-systems/{parent}"), None).await;
    assert_eq!(
        gone.status, 404,
        "case \"read a deleted file system\": expected 404, got {} / {:?}",
        gone.status, gone.body
    );
    for child in &children {
        let orphan = call(&router, "GET", &format!("/access-points/{child}"), None).await;
        assert_eq!(
            orphan.status, 404,
            "case \"read a child of a deleted file system\": expected 404, got {} / {:?}",
            orphan.status, orphan.body
        );
    }
    let listed = call(&router, "GET", "/file-systems", None).await;
    assert_eq!(
        listed.field("fileSystems").as_array().map(Vec::len),
        Some(0),
        "case \"list after delete\": a deleted file system must not be listed, got {:?}",
        listed.body
    );

    // A file system with no children needs no `forceDelete`.
    let childless = created_file_system_id(&router, bucket, Some("delete-childless")).await;
    let deleted = call(&router, "DELETE", &format!("/file-systems/{childless}"), None).await;
    assert_eq!(
        deleted.status, 204,
        "case \"delete a childless file system\": expected 204, got {} / {:?}",
        deleted.status, deleted.body
    );
}

/// Rule 6: a resource the service reports `creating` becomes `available` because the
/// clock made the transition due — not because anybody read it.
#[tokio::test]
async fn creating_settles_to_available_because_the_clock_made_it_due() {
    let router = new_router();

    // Each transient read is taken immediately after its OWN create, so it has the whole
    // provisioning span to land in; a read that still lands after the span has elapsed
    // (a badly loaded machine) may see the settled status instead of failing on a
    // wall-clock race it does not control.
    let started = Instant::now();
    let file_system_id =
        created_file_system_id(&router, "arn:aws:s3:::s3files-settling", Some("settle")).await;
    let file_system_path = format!("/file-systems/{file_system_id}");
    let read = call(&router, "GET", &file_system_path, None).await;
    assert_transient_status("file system", &read, started.elapsed());

    let started = Instant::now();
    let created = create_access_point(&router, &file_system_id, json!({})).await;
    let access_point_path = format!("/access-points/{}", created.text("accessPointId"));
    let read = call(&router, "GET", &access_point_path, None).await;
    assert_transient_status("access point", &read, started.elapsed());

    tokio::time::sleep(PAST_PROVISIONING).await;

    for (case, path) in [
        ("file system", &file_system_path),
        ("access point", &access_point_path),
    ] {
        let read = call(&router, "GET", path, None).await;
        assert_eq!(
            read.text("status"),
            "available",
            "case {case:?}: the transition the clock owed was never materialised, got {:?}",
            read.field("status")
        );
    }

    // The listing reads the same settled record.
    let listed = call(&router, "GET", "/file-systems", None).await;
    assert_eq!(
        listed.field("fileSystems")[0]["status"],
        json!("available"),
        "case \"list after the transition came due\": expected available, got {:?}",
        listed.body
    );
}

/// A resource read within `elapsed` of its own create must still report `creating`; only
/// a read that took longer than the provisioning span may already see it settled.
fn assert_transient_status(case: &str, read: &Answer, elapsed: Duration) {
    assert_eq!(
        read.status, 200,
        "case {case:?}: expected 200, got {} / {:?}",
        read.status, read.body
    );
    let status = read.text("status");
    if status != "creating" {
        assert!(
            elapsed >= PROVISIONING_SPAN,
            "case {case:?}: a just-created resource is not usable yet — expected `creating` \
             within the {PROVISIONING_SPAN:?} provisioning span, got {status:?} after {elapsed:?}"
        );
    }
}
