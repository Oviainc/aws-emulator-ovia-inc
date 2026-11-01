// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateAssetFile` and the revisions it mints, read
//! back through `GetAssetFile`.
//!
//! Every case drives the service through the same router the emulator serves, so
//! the routes, status codes, protocol envelope and state are all exercised.

use std::io::Write;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_devopsagent_devopsagent::state::{
    MAX_ASSET_CONTENT_BYTES, MAX_ASSET_FILE_METADATA_FIELDS, MAX_ASSET_VERSIONS_RETAINED,
};

/// A SigV4 credential scope naming this service's signing name (`aidevops`), which
/// is how the router resolves which service claims a request. Signature
/// verification is switched off for these tests, so the signature is a placeholder.
const AUTHORIZATION: &str = concat!(
    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-west-2/aidevops/aws4_request, ",
    "SignedHeaders=host, Signature=0123456789abcdef"
);

/// One emulator: a single shared engine behind the production router.
fn new_router() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_devopsagent_devopsagent::EmulatedService::new(Arc::new(
            engine::InMemoryEngine::new(),
        )),
    );
    router
}

/// Send one request and return its status and parsed body (`Value::Null` for an
/// empty body).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .as_ref()
        .map(|b| Bytes::from(serde_json::to_vec(b).expect("serialize request body")))
        .unwrap_or_default();
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "aidevops.us-west-2.amazonaws.com")
        .header("authorization", AUTHORIZATION);
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "response body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// The wire error code (`__type`) of an error response.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `message` of an error response.
fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// Create an agent space and return its id.
async fn create_space(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(router, "POST", "/v1/agentspaces", Some(json!({"name": name}))).await;
    assert_eq!(
        status, 201,
        "case {name:?}: expected 201 from CreateAgentSpace, got {status}: {body}"
    );
    body["agentSpace"]["agentSpaceId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no agentSpaceId in {body}"))
        .to_string()
}

/// A zip archive carrying `entries`, built with the same crate the handler reads
/// one with.
fn zip_bundle(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        for (path, content) in entries {
            writer
                .start_file(*path, zip::write::SimpleFileOptions::default())
                .expect("start zip entry");
            writer.write_all(content).expect("write zip entry");
        }
        writer.finish().expect("finish zip");
    }
    buffer.into_inner()
}

/// Create an asset whose content is `content` and return its id.
async fn create_asset(router: &MultiServiceRouter, space: &str, case: &str, content: Value) -> String {
    let (status, body) = call(
        router,
        "POST",
        &format!("/asset/agent-space/{space}/assets"),
        Some(json!({
            "assetType": "artifact",
            "metadata": {"title": case},
            "content": content,
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateAsset, got {status}: {body}"
    );
    body["asset"]["assetId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no assetId in {body}"))
        .to_string()
}

/// An asset that holds a SET of files: content that arrived as a bundle, so the
/// file plane may add files to it.
async fn create_bundle_asset(router: &MultiServiceRouter, space: &str, case: &str) -> String {
    let zip = aws_smithy_types::base64::encode(zip_bundle(&[("bundled.md", b"bundled")]));
    create_asset(router, space, case, json!({"zip": {"zipFile": zip}})).await
}

/// The `CreateAssetFile` / `GetAssetFile` / `UpdateAssetFile` URI for one file of
/// one asset. `path` is already in its wire (URI) form.
fn asset_file_uri(space: &str, asset: &str, path: &str) -> String {
    format!("/asset/agent-space/{space}/assets/{asset}/files/{path}")
}

/// The `GetAsset` URI for one asset.
fn asset_uri(space: &str, asset: &str) -> String {
    format!("/asset/agent-space/{space}/assets/{asset}")
}

/// Create a file of an asset and return the created `AssetFile`.
async fn create_file(
    router: &MultiServiceRouter,
    space: &str,
    asset: &str,
    case: &str,
    wire_path: &str,
    input: &Value,
) -> Value {
    let (status, body) = call(
        router,
        "POST",
        &asset_file_uri(space, asset, wire_path),
        Some(input.clone()),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateAssetFile, got {status}: {body}"
    );
    body["file"].clone()
}

/// The revision an asset reports it is at.
async fn asset_version(router: &MultiServiceRouter, space: &str, asset: &str, case: &str) -> i64 {
    let (status, body) = call(router, "GET", &asset_uri(space, asset), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetAsset, got {status}: {body}"
    );
    body["asset"]["version"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: no asset version in {body}"))
}

/// One update case: the file as created, the update applied to it, and what the
/// stored file must hold afterwards.
struct Case {
    name: &'static str,
    /// The path in its wire (URI) form, which is not always the stored path.
    wire_path: &'static str,
    stored_path: &'static str,
    initial: Value,
    update: Value,
    expected_content: Value,
    expected_metadata: Value,
}

/// RULE: an update replaces exactly the parts of a stored file the request names
/// and leaves the rest as they were; because a revision of an asset is a snapshot
/// of its content, a changed file moves the asset to its next revision and the
/// content the previous revision held stays readable at that version.
#[tokio::test]
async fn an_update_mints_a_revision_and_the_previous_one_keeps_its_content() {
    let big_text: String = "line\n".repeat(2_000);
    // The rows differ along what the rule generalises over: which members the
    // request names (content, metadata, both, neither), the body variant the
    // content changes to and from, the size of the new content (empty, small,
    // several KiB), and the shape of the path.
    let cases: Vec<Case> = vec![
        Case {
            name: "text content replaced",
            wire_path: "docs/one.md",
            stored_path: "docs/one.md",
            initial: json!({"content": {"text": "first"}}),
            update: json!({"content": {"text": "second"}}),
            expected_content: json!({"text": "second"}),
            expected_metadata: Value::Null,
        },
        Case {
            name: "text content replaced by bytes",
            wire_path: "mixed.bin",
            stored_path: "mixed.bin",
            initial: json!({"content": {"text": "first"}}),
            update: json!({"content": {"bytes": "AAECAw=="}}),
            expected_content: json!({"bytes": "AAECAw=="}),
            expected_metadata: Value::Null,
        },
        Case {
            name: "bytes content replaced by text",
            wire_path: "other.bin",
            stored_path: "other.bin",
            initial: json!({"content": {"bytes": "AAECAw=="}}),
            update: json!({"content": {"text": "now text"}}),
            expected_content: json!({"text": "now text"}),
            expected_metadata: Value::Null,
        },
        Case {
            name: "content emptied",
            wire_path: "emptied.txt",
            stored_path: "emptied.txt",
            initial: json!({"content": {"text": "not empty"}}),
            update: json!({"content": {"text": ""}}),
            expected_content: json!({"text": ""}),
            expected_metadata: Value::Null,
        },
        Case {
            name: "several KiB of new content",
            wire_path: "big.txt",
            stored_path: "big.txt",
            initial: json!({"content": {"text": "small"}}),
            update: json!({"content": {"text": big_text.clone()}}),
            expected_content: json!({"text": big_text}),
            expected_metadata: Value::Null,
        },
        Case {
            name: "metadata only, content unchanged",
            wire_path: "meta.md",
            stored_path: "meta.md",
            initial: json!({"content": {"text": "kept"}, "metadata": {"reviewed": false}}),
            update: json!({"metadata": {"reviewed": true}}),
            expected_content: json!({"text": "kept"}),
            expected_metadata: json!({"reviewed": true}),
        },
        Case {
            name: "metadata field added, omitted fields retained",
            wire_path: "merge.md",
            stored_path: "merge.md",
            initial: json!({
                "content": {"text": "merge"},
                "metadata": {"contentType": "text/markdown", "reviewed": false},
            }),
            update: json!({"metadata": {"owner": "team"}}),
            expected_content: json!({"text": "merge"}),
            expected_metadata: json!({
                "contentType": "text/markdown",
                "reviewed": false,
                "owner": "team",
            }),
        },
        Case {
            name: "metadata on a file created without any",
            wire_path: "nometa.md",
            stored_path: "nometa.md",
            initial: json!({"content": {"text": "no metadata"}}),
            update: json!({"metadata": {"owner": "team"}}),
            expected_content: json!({"text": "no metadata"}),
            expected_metadata: json!({"owner": "team"}),
        },
        Case {
            name: "content and metadata together at a nested path",
            wire_path: "a/b/c/d/e.md",
            stored_path: "a/b/c/d/e.md",
            initial: json!({"content": {"text": "deep"}, "metadata": {"stage": "draft"}}),
            update: json!({"content": {"text": "deeper"}, "metadata": {"stage": "final"}}),
            expected_content: json!({"text": "deeper"}),
            expected_metadata: json!({"stage": "final"}),
        },
        Case {
            name: "percent-encoded space and parens in the path",
            wire_path: "notes/release%20(1).bin",
            stored_path: "notes/release (1).bin",
            initial: json!({"content": {"bytes": "AAECAw=="}}),
            update: json!({"content": {"bytes": "BAUGBw=="}}),
            expected_content: json!({"bytes": "BAUGBw=="}),
            expected_metadata: Value::Null,
        },
        Case {
            name: "neither content nor metadata",
            wire_path: "untouched.md",
            stored_path: "untouched.md",
            initial: json!({"content": {"text": "untouched"}, "metadata": {"stage": "draft"}}),
            update: json!({}),
            expected_content: json!({"text": "untouched"}),
            expected_metadata: json!({"stage": "draft"}),
        },
    ];

    let router = new_router();
    let space = create_space(&router, "asset-file-update-space").await;

    for case in &cases {
        let name = case.name;
        // One asset per row, so the revision each row observes is its own.
        let asset = create_bundle_asset(&router, &space, name).await;
        let created =
            create_file(&router, &space, &asset, name, case.wire_path, &case.initial).await;

        // A request that names neither content nor metadata changes nothing, so the
        // asset stays at the revision it was at; anything else mints the next one.
        let changes = case.update.get("content").is_some() || case.update.get("metadata").is_some();
        let expected_version = if changes { 2 } else { 1 };

        let uri = asset_file_uri(&space, &asset, case.wire_path);
        let (status, updated) = call(&router, "PATCH", &uri, Some(case.update.clone())).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 from UpdateAssetFile, got {status}: {updated}"
        );
        let file = &updated["file"];
        assert_eq!(
            file["path"],
            json!(case.stored_path),
            "case {name:?}: path came back as {:?}",
            file["path"]
        );
        assert_eq!(
            file["content"], case.expected_content,
            "case {name:?}: content came back as {:?}",
            file["content"]
        );
        assert_eq!(
            file["metadata"], case.expected_metadata,
            "case {name:?}: metadata came back as {:?}",
            file["metadata"]
        );
        assert_eq!(
            file["version"],
            json!(expected_version),
            "case {name:?}: the updated file belongs to revision {:?}",
            file["version"]
        );
        assert_eq!(
            file["createdAt"], created["createdAt"],
            "case {name:?}: createdAt moved to {:?}",
            file["createdAt"]
        );
        assert_eq!(
            asset_version(&router, &space, &asset, name).await,
            expected_version,
            "case {name:?}: the asset is not at revision {expected_version}"
        );

        // READ BACK: the latest read answers exactly what the update persisted, and
        // reading twice changes nothing.
        let (status, fetched) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 from GetAssetFile, got {status}: {fetched}"
        );
        assert_eq!(
            fetched["file"], updated["file"],
            "case {name:?}: GetAssetFile returned {:?}, not what UpdateAssetFile persisted ({:?})",
            fetched["file"], updated["file"]
        );
        let (_, again) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            again, fetched,
            "case {name:?}: two reads of one file disagree: {again:?}"
        );

        // The revision the update minted is readable by name, and so is the one it
        // left behind — with the content and the version it was read with.
        let (status, current_by_version) =
            call(&router, "GET", &format!("{uri}?assetVersion={expected_version}"), None).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 reading revision {expected_version}, got {status}: {current_by_version}"
        );
        assert_eq!(
            current_by_version["file"], updated["file"],
            "case {name:?}: revision {expected_version} read back as {:?}",
            current_by_version["file"]
        );

        let (status, previous) = call(&router, "GET", &format!("{uri}?assetVersion=1"), None).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 reading revision 1, got {status}: {previous}"
        );
        assert_eq!(
            previous["file"], created,
            "case {name:?}: revision 1 read back as {:?}, not the content it held ({created:?})",
            previous["file"]
        );

        // A revision the asset never reached names nothing.
        let (status, missing) = call(
            &router,
            "GET",
            &format!("{uri}?assetVersion={}", expected_version + 1),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {name:?}: expected 404 for a revision the asset never had, got {status}: {missing}"
        );
        assert_eq!(
            error_message(&missing),
            format!("Asset not found: {asset} version {}", expected_version + 1),
            "case {name:?}: the revision refusal said {:?}",
            error_message(&missing)
        );

        // A file the update did not touch belongs to the revision the asset is now
        // at, and still holds the content it was bundled with.
        let (status, sibling) = call(
            &router,
            "GET",
            &asset_file_uri(&space, &asset, "bundled.md"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 reading the untouched file, got {status}: {sibling}"
        );
        assert_eq!(
            sibling["file"]["content"],
            json!({"bytes": aws_smithy_types::base64::encode("bundled")}),
            "case {name:?}: the untouched file holds {:?}",
            sibling["file"]["content"]
        );
        assert_eq!(
            sibling["file"]["version"],
            json!(expected_version),
            "case {name:?}: the untouched file belongs to revision {:?}",
            sibling["file"]["version"]
        );
    }
}

/// RULE: every retained revision of an asset answers the content it held, so a run
/// of updates leaves each one readable at its own version; past what this service
/// retains ([`MAX_ASSET_VERSIONS_RETAINED`]) the oldest revisions are no longer
/// there and name nothing.
#[tokio::test]
async fn each_retained_revision_answers_the_content_it_held() {
    let router = new_router();
    let space = create_space(&router, "revision-history-space").await;
    let asset = create_bundle_asset(&router, &space, "history").await;
    let path = "history.md";
    create_file(
        &router,
        &space,
        &asset,
        "history",
        path,
        &json!({"content": {"text": "revision 1"}}),
    )
    .await;

    // Two updates more than the retention bound, so the rows below span both the
    // revisions that are still there and the ones that are not.
    let updates = MAX_ASSET_VERSIONS_RETAINED + 2;
    let uri = asset_file_uri(&space, &asset, path);
    for update in 1..=updates {
        let version = update + 1;
        let (status, body) = call(
            &router,
            "PATCH",
            &uri,
            Some(json!({"content": {"text": format!("revision {version}")}})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case \"update {update}\": expected 200, got {status}: {body}"
        );
        assert_eq!(
            body["file"]["version"],
            json!(version),
            "case \"update {update}\": the update minted revision {:?}",
            body["file"]["version"]
        );
    }

    let current = updates + 1;
    let oldest_retained = current - MAX_ASSET_VERSIONS_RETAINED;
    for version in 1..=current + 1 {
        let (status, body) = call(&router, "GET", &format!("{uri}?assetVersion={version}"), None).await;
        let case = format!("revision {version} of {current}");
        if version < oldest_retained || version > current {
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404 for a revision that is not there, got {status}: {body}"
            );
            continue;
        }
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {body}"
        );
        assert_eq!(
            body["file"]["content"],
            json!({"text": format!("revision {version}")}),
            "case {case:?}: read back {:?}",
            body["file"]["content"]
        );
        assert_eq!(
            body["file"]["version"],
            json!(version),
            "case {case:?}: the file reports revision {:?}",
            body["file"]["version"]
        );
    }
}

/// RULE: `clientToken` is an idempotency token — a repeat of one update with the
/// same token reports what that update stored and mints no further revision, while
/// a fresh token updates the file again.
#[tokio::test]
async fn repeating_a_client_token_reports_the_first_update() {
    let router = new_router();
    let space = create_space(&router, "update-token-space").await;
    let asset = create_bundle_asset(&router, &space, "tokens").await;
    let path = "token.md";
    create_file(
        &router,
        &space,
        &asset,
        "tokens",
        path,
        &json!({"content": {"text": "created"}}),
    )
    .await;

    let uri = asset_file_uri(&space, &asset, path);
    let first = json!({"content": {"text": "first update"}, "clientToken": "token-a"});
    let (status, updated) = call(&router, "PATCH", &uri, Some(first.clone())).await;
    assert_eq!(
        status, 200,
        "case \"first call\": expected 200, got {status}: {updated}"
    );

    // The rows differ along the token: the same token replayed unchanged, and the
    // same token carrying different members.
    for (case, input) in [
        ("same token, same content", first.clone()),
        (
            "same token, different content",
            json!({"content": {"text": "ignored"}, "clientToken": "token-a"}),
        ),
        (
            "same token, different metadata",
            json!({"metadata": {"owner": "ignored"}, "clientToken": "token-a"}),
        ),
    ] {
        let (status, replayed) = call(&router, "PATCH", &uri, Some(input)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the replay, got {status}: {replayed}"
        );
        assert_eq!(
            replayed["file"], updated["file"],
            "case {case:?}: the replay reported {:?}, not what the first update stored ({:?})",
            replayed["file"], updated["file"]
        );
        assert_eq!(
            asset_version(&router, &space, &asset, case).await,
            2,
            "case {case:?}: the replay minted a further revision"
        );
    }

    // A fresh token is a second update, so it mints the next revision.
    let (status, second) = call(
        &router,
        "PATCH",
        &uri,
        Some(json!({"content": {"text": "second update"}, "clientToken": "token-b"})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"fresh token\": expected 200, got {status}: {second}"
    );
    assert_eq!(
        second["file"]["version"],
        json!(3),
        "case \"fresh token\": the update minted revision {:?}",
        second["file"]["version"]
    );

    // And the replay of the first token still answers the revision it stored, now
    // that the asset has moved on.
    let (status, replayed) = call(&router, "PATCH", &uri, Some(first.clone())).await;
    assert_eq!(
        status, 200,
        "case \"replay after a later update\": expected 200, got {status}: {replayed}"
    );
    assert_eq!(
        replayed["file"], updated["file"],
        "case \"replay after a later update\": the replay reported {:?}",
        replayed["file"]
    );
    assert_eq!(
        asset_version(&router, &space, &asset, "replay after a later update").await,
        3,
        "case \"replay after a later update\": the replay minted a further revision"
    );

    // Past what this service retains, the revision the token named is no longer
    // there — the replay then reports the file as it stands, and still applies
    // nothing a second time.
    let mut version = 3;
    for update in 0..=MAX_ASSET_VERSIONS_RETAINED {
        version += 1;
        let (status, body) = call(
            &router,
            "PATCH",
            &uri,
            Some(json!({"content": {"text": format!("later update {update}")}})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case \"later update {update}\": expected 200, got {status}: {body}"
        );
    }
    let (status, late_replay) = call(&router, "PATCH", &uri, Some(first)).await;
    assert_eq!(
        status, 200,
        "case \"replay past retention\": expected 200, got {status}: {late_replay}"
    );
    assert_eq!(
        late_replay["file"]["version"],
        json!(version),
        "case \"replay past retention\": the replay reported revision {:?}",
        late_replay["file"]["version"]
    );
    assert_eq!(
        asset_version(&router, &space, &asset, "replay past retention").await,
        version,
        "case \"replay past retention\": the replay minted a further revision"
    );
}

/// One metadata field more than one stored file holds.
fn too_many_metadata_fields() -> Value {
    let fields: serde_json::Map<String, Value> = (0..=MAX_ASSET_FILE_METADATA_FIELDS)
        .map(|field| (format!("field-{field}"), json!(field)))
        .collect();
    Value::Object(fields)
}

/// RULE: an update refuses what it cannot persist and persists nothing when it does
/// — a path the asset holds no file at is missing (HTTP 404), an asset that does not
/// exist is missing (HTTP 404), an agent space the caller cannot reach is refused
/// before anything inside it is looked up (HTTP 403), a value that names no file of
/// the asset or metadata that is no set of members is invalid (HTTP 400), and
/// content past the size one asset holds is refused (HTTP 413).
#[tokio::test]
async fn update_asset_file_refuses_what_it_cannot_persist() {
    let router = new_router();
    let space = create_space(&router, "update-refusal-space").await;
    let asset = create_bundle_asset(&router, &space, "refusals").await;
    let path = "stored.md";
    let created = create_file(
        &router,
        &space,
        &asset,
        "refusals",
        path,
        &json!({"content": {"text": "stored"}}),
    )
    .await;

    // Each row: the space, the asset, the wire path, the request body, and the
    // status and error code the request must answer with.
    let cases: Vec<(&str, String, String, String, Value, u16, &str)> = vec![
        (
            "path the asset holds no file at",
            space.clone(),
            asset.clone(),
            "absent.md".to_string(),
            json!({"content": {"text": "x"}}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "asset that does not exist",
            space.clone(),
            "asset-0000000000000000".to_string(),
            path.to_string(),
            json!({"content": {"text": "x"}}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "agent space that does not exist",
            "as-0000000000000000".to_string(),
            asset.clone(),
            path.to_string(),
            json!({"content": {"text": "x"}}),
            403,
            "AccessDeniedException",
        ),
        (
            "path that walks out of the asset",
            space.clone(),
            asset.clone(),
            "dir/../../escape.md".to_string(),
            json!({"content": {"text": "x"}}),
            400,
            "ValidationException",
        ),
        (
            "metadata that is a list, not a set of members",
            space.clone(),
            asset.clone(),
            path.to_string(),
            json!({"metadata": ["owner"]}),
            400,
            "ValidationException",
        ),
        (
            "metadata that is a string, not a set of members",
            space.clone(),
            asset.clone(),
            path.to_string(),
            json!({"metadata": "owner"}),
            400,
            "ValidationException",
        ),
        (
            "content past the size one asset holds",
            space.clone(),
            asset.clone(),
            path.to_string(),
            json!({"content": {"text": "y".repeat(MAX_ASSET_CONTENT_BYTES + 1)}}),
            413,
            "ContentSizeExceededException",
        ),
        (
            "more metadata fields than one file holds",
            space.clone(),
            asset.clone(),
            path.to_string(),
            json!({"metadata": too_many_metadata_fields()}),
            402,
            "ServiceQuotaExceededException",
        ),
    ];

    for (case, case_space, case_asset, case_path, body, expected_status, expected_code) in &cases {
        let (status, answered) = call(
            &router,
            "PATCH",
            &asset_file_uri(case_space, case_asset, case_path),
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected {expected_status}, got {status}: {answered}"
        );
        assert_eq!(
            error_code(&answered),
            *expected_code,
            "case {case:?}: refusal answered {:?}",
            error_code(&answered)
        );
    }

    // The refusals name the resource the caller asked for.
    let (_, missing_file) = call(
        &router,
        "PATCH",
        &asset_file_uri(&space, &asset, "absent.md"),
        Some(json!({"content": {"text": "x"}})),
    )
    .await;
    assert_eq!(
        error_message(&missing_file),
        "Asset file not found: absent.md",
        "the missing-file refusal said {:?}",
        error_message(&missing_file)
    );
    let (_, missing_asset) = call(
        &router,
        "PATCH",
        &asset_file_uri(&space, "asset-0000000000000000", path),
        Some(json!({"content": {"text": "x"}})),
    )
    .await;
    assert_eq!(
        error_message(&missing_asset),
        "Asset not found: asset-0000000000000000",
        "the missing-asset refusal said {:?}",
        error_message(&missing_asset)
    );

    // Nothing above was persisted: the file still holds what it was created with
    // and the asset is still at the revision it was created at.
    let (status, fetched) = call(&router, "GET", &asset_file_uri(&space, &asset, path), None).await;
    assert_eq!(
        status, 200,
        "expected 200 reading the file the refusals named, got {status}: {fetched}"
    );
    assert_eq!(
        fetched["file"], created,
        "a refused update changed the stored file to {:?}",
        fetched["file"]
    );
    assert_eq!(
        asset_version(&router, &space, &asset, "refused updates").await,
        1,
        "a refused update minted a revision"
    );
}
