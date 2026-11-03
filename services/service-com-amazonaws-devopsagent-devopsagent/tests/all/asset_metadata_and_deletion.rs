// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what an asset's `metadata` document carries, how an
//! asset of one file refuses a second one, and what `DeleteAsset` removes.
//!
//! Every case drives the service through the same router the emulator serves, so
//! the routes, status codes, protocol envelope and state are all exercised.

use std::io::Write;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_devopsagent_devopsagent::state::{ASSET_STATUS_ACTIVE, ASSET_TYPE_RULES};

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

fn create_asset_uri(space: &str) -> String {
    format!("/asset/agent-space/{space}/assets")
}

fn asset_uri(space: &str, asset: &str) -> String {
    format!("/asset/agent-space/{space}/assets/{asset}")
}

fn asset_file_uri(space: &str, asset: &str, path: &str) -> String {
    format!("/asset/agent-space/{space}/assets/{asset}/files/{path}")
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

/// The document `described` with `field` set to `value` where it names none: the
/// expectation the rule under test states, written out independently of the
/// handler's own helper.
fn completed_with(described: Option<&Value>, field: &str, value: &str) -> Value {
    let mut members: Map<String, Value> = match described {
        Some(Value::Object(members)) => members.clone(),
        Some(other) => return other.clone(),
        None => Map::new(),
    };
    members
        .entry(field.to_string())
        .or_insert_with(|| json!(value));
    Value::Object(members)
}

/// RULE: the `metadata` document this service serves for an asset carries the
/// members the caller described it with, completed with the member the SERVICE
/// maintains — the asset's lifecycle state, under `status` — and a member the
/// caller described is never replaced.
#[tokio::test]
async fn served_asset_metadata_carries_the_lifecycle_state() {
    // The rows differ along what the rule generalises over: whether a metadata
    // document was described at all, how many members it carries, the types of
    // their values (string, number, boolean, list, nested document) and whether
    // the caller described the service's own member themselves.
    let cases: Vec<(&str, &str, Option<Value>)> = vec![
        ("no metadata described", "runbook", None),
        ("an empty set of members", "runbook", Some(json!({}))),
        (
            "one described member",
            "skill",
            Some(json!({"name": "deploy-helper"})),
        ),
        (
            "several members of several types",
            "artifact",
            Some(json!({
                "title": "release notes",
                "revision": 7,
                "draft": false,
                "tags": ["a", "b"],
                "limits": {"retries": 3},
            })),
        ),
        (
            "the caller describes the service's own member",
            "artifact",
            Some(json!({"title": "owned", "status": "DRAFT"})),
        ),
    ];

    let router = new_router();
    let space = create_space(&router, "asset-metadata-space").await;

    for (case, asset_type, described) in &cases {
        let mut request = json!({
            "assetType": asset_type,
            "content": {"file": {"path": "doc.md", "body": {"text": "# doc"}}},
        });
        if let Some(described) = described {
            request["metadata"] = described.clone();
        }

        let (status, created) =
            call(&router, "POST", &create_asset_uri(&space), Some(request)).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateAsset, got {status}: {created}"
        );
        let expected = completed_with(described.as_ref(), "status", ASSET_STATUS_ACTIVE);
        assert_eq!(
            created["asset"]["metadata"], expected,
            "case {case:?}: CreateAsset served metadata {:?}, expected {expected:?}",
            created["asset"]["metadata"]
        );

        // READ BACK: the same document comes off a read of the stored asset, so
        // the create and the read cannot disagree about it.
        let asset_id = created["asset"]["assetId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no assetId in {created}"));
        let (status, fetched) = call(&router, "GET", &asset_uri(&space, asset_id), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetAsset, got {status}: {fetched}"
        );
        assert_eq!(
            fetched["asset"]["metadata"], expected,
            "case {case:?}: GetAsset served metadata {:?}, expected {expected:?}",
            fetched["asset"]["metadata"]
        );
    }
}

/// RULE: an asset whose content IS one file holds exactly that file, so creating
/// another file in it is refused with HTTP 400 `ValidationException` reporting how
/// many files an asset of that TYPE supports and the operation that replaces the
/// one it holds.
#[tokio::test]
async fn one_file_asset_refuses_another_file_naming_its_asset_type() {
    let router = new_router();
    let space = create_space(&router, "one-file-message-space").await;

    // The rows differ along the asset type the refusal is parameterised on —
    // including every type this service enforces a metadata rule for, and one it
    // knows no rule for — and along the path and body of the file the asset holds
    // and of the one the create names.
    let described: Vec<(&str, Value)> = vec![
        ("skill", json!({"name": "a skill"})),
        ("artifact", json!({"title": "an artifact"})),
        ("attachment", json!({"filename": "an attachment"})),
        ("runbook", json!({})),
    ];
    // Every type this service enforces a metadata rule for is covered above.
    for rule in ASSET_TYPE_RULES {
        assert!(
            described.iter().any(|(name, _)| *name == rule.asset_type),
            "asset type {:?} has a rule but no case",
            rule.asset_type
        );
    }

    for (asset_type, metadata) in &described {
        for (existing, body) in [
            ("only.md", json!({"text": "only"})),
            ("dir/only.bin", json!({"bytes": "AAECAw=="})),
        ] {
            let (status, created) = call(
                &router,
                "POST",
                &create_asset_uri(&space),
                Some(json!({
                    "assetType": asset_type,
                    "metadata": metadata,
                    "content": {"file": {"path": existing, "body": body}},
                })),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {asset_type:?}/{existing:?}: expected 201 from CreateAsset, got {status}: {created}"
            );
            let asset = created["asset"]["assetId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {asset_type:?}: no assetId in {created}"))
                .to_string();

            for added in ["added.md", "dir/added.bin"] {
                let (status, refused) = call(
                    &router,
                    "POST",
                    &asset_file_uri(&space, &asset, added),
                    Some(json!({"content": {"text": "added"}})),
                )
                .await;
                assert_eq!(
                    status, 400,
                    "case {asset_type:?} adding {added:?}: expected 400, got {status}: {refused}"
                );
                assert_eq!(
                    error_code(&refused),
                    "ValidationException",
                    "case {asset_type:?} adding {added:?}: refusal answered {:?}",
                    error_code(&refused)
                );
                assert_eq!(
                    error_message(&refused),
                    format!(
                        "Assets of type '{asset_type}' support only one file. \
                         Use UpdateAssetFile to replace the existing file."
                    ),
                    "case {asset_type:?} adding {added:?}: message was {:?}",
                    error_message(&refused)
                );
            }

            // The refusal left the asset as it was: the one file it holds is still
            // readable and the path the create named holds nothing.
            let (status, held) =
                call(&router, "GET", &asset_file_uri(&space, &asset, existing), None).await;
            assert_eq!(
                status, 200,
                "case {asset_type:?}: expected 200 reading {existing:?}, got {status}: {held}"
            );
            let (status, missing) =
                call(&router, "GET", &asset_file_uri(&space, &asset, "added.md"), None).await;
            assert_eq!(
                status, 404,
                "case {asset_type:?}: a refused create persisted a file: {status}: {missing}"
            );
        }
    }
}

/// RULE: `DeleteAsset` removes the asset and all its files, so every later read of
/// the asset or of a file of it reports the asset as missing — HTTP 404
/// `ResourceNotFoundException` naming the requested `assetId` — and a delete of an
/// asset that is not there reports the same.
#[tokio::test]
async fn deleted_asset_and_its_files_are_gone() {
    // The rows differ along the content the asset was created from (a single file,
    // a bundle of several entries, an import that materialises none) and so along
    // how many files the delete takes with it.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "single file",
            json!({"file": {"path": "only.md", "body": {"text": "only"}}}),
            vec!["only.md"],
        ),
        (
            "bundle of several entries",
            json!({
                "zip": {
                    "zipFile": aws_smithy_types::base64::encode(zip_bundle(&[
                        ("one.md", b"one"),
                        ("dir/two.bin", b"two"),
                    ]))
                }
            }),
            vec!["one.md", "dir/two.bin"],
        ),
        (
            "source url import, no files materialised",
            json!({"sourceUrl": {"url": "https://example.com/bundle.zip"}}),
            vec![],
        ),
    ];

    let router = new_router();
    let space = create_space(&router, "asset-deletion-space").await;

    for (case, content, files) in &cases {
        let (status, created) = call(
            &router,
            "POST",
            &create_asset_uri(&space),
            Some(json!({
                "assetType": "artifact",
                "metadata": {"title": case},
                "content": content,
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateAsset, got {status}: {created}"
        );
        let asset = created["asset"]["assetId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no assetId in {created}"))
            .to_string();

        // The asset and each of its files are readable before the delete, so what
        // the delete changes is observable.
        let (status, before) = call(&router, "GET", &asset_uri(&space, &asset), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetAsset before the delete, got {status}: {before}"
        );
        for path in files {
            let (status, file) =
                call(&router, "GET", &asset_file_uri(&space, &asset, path), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 reading {path:?} before the delete, got {status}: {file}"
            );
        }

        let (status, deleted) = call(&router, "DELETE", &asset_uri(&space, &asset), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from DeleteAsset, got {status}: {deleted}"
        );

        // The asset is gone: the read that served it answers that it is missing,
        // naming the id the request carried.
        let (status, missing) = call(&router, "GET", &asset_uri(&space, &asset), None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from GetAsset after the delete, got {status}: {missing}"
        );
        assert_eq!(
            error_code(&missing),
            "ResourceNotFoundException",
            "case {case:?}: read refusal answered {:?}",
            error_code(&missing)
        );
        assert_eq!(
            error_message(&missing),
            format!("Asset not found: {asset}"),
            "case {case:?}: read refusal said {:?}",
            error_message(&missing)
        );

        // Its files went with it, so a read of one reports the asset as missing
        // rather than the file.
        for path in files {
            let (status, gone) =
                call(&router, "GET", &asset_file_uri(&space, &asset, path), None).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404 reading {path:?} after the delete, got {status}: {gone}"
            );
            assert_eq!(
                error_message(&gone),
                format!("Asset not found: {asset}"),
                "case {case:?}: reading {path:?} said {:?}",
                error_message(&gone)
            );
        }

        // A second delete of the same asset finds nothing to delete, and so does a
        // delete of an id that never named one.
        for (what, target) in [
            ("the deleted asset", asset.as_str()),
            ("an unknown id", "asset-ffffffffffffffff"),
        ] {
            let (status, again) = call(&router, "DELETE", &asset_uri(&space, target), None).await;
            assert_eq!(
                status, 404,
                "case {case:?}: deleting {what} answered {status}: {again}"
            );
            assert_eq!(
                error_code(&again),
                "ResourceNotFoundException",
                "case {case:?}: deleting {what} answered {:?}",
                error_code(&again)
            );
            assert_eq!(
                error_message(&again),
                format!("Asset not found: {target}"),
                "case {case:?}: deleting {what} said {:?}",
                error_message(&again)
            );
        }
    }

    // An agent space the caller cannot reach is refused before anything inside it
    // is looked up, so a delete in one reports no asset at all.
    let (status, denied) = call(
        &router,
        "DELETE",
        &asset_uri("agentspace-ffffffffffffffff", "asset-ffffffffffffffff"),
        None,
    )
    .await;
    assert_eq!(
        status, 403,
        "deleting in an unreachable space answered {status}: {denied}"
    );
    assert_eq!(
        error_code(&denied),
        "AccessDeniedException",
        "deleting in an unreachable space answered {:?}",
        error_code(&denied)
    );
}

/// RULE: `DeleteAssetFile` removes one file from an asset's content, which moves
/// the asset to its next revision; an asset whose content IS a single file is that
/// file, so deleting it takes the asset with it and every later read of the asset
/// reports it as missing.
#[tokio::test]
async fn deleting_a_file_removes_it_and_takes_a_one_file_asset_with_it() {
    let router = new_router();
    let space = create_space(&router, "file-deletion-space").await;

    // The rows differ along the content form the asset was created from — the
    // member of `AssetContent` (`smithy_query(query="shape", shape="AssetContent")`:
    // a single `file`, a `zip` bundle) — and so along whether the asset survives
    // the deletion of the named file, and along the path and body of that file.
    let cases: Vec<(&str, Value, &str, Vec<&str>, bool)> = vec![
        (
            "the one file an asset's content IS",
            json!({"file": {"path": "only.md", "body": {"text": "only"}}}),
            "only.md",
            vec![],
            false,
        ),
        (
            "the one binary file an asset's content IS, at a nested path",
            json!({"file": {"path": "dir/only.bin", "body": {"bytes": "AAECAw=="}}}),
            "dir/only.bin",
            vec![],
            false,
        ),
        (
            "one file of a bundle of several",
            json!({
                "zip": {
                    "zipFile": aws_smithy_types::base64::encode(zip_bundle(&[
                        ("one.md", b"one"),
                        ("dir/two.bin", b"two"),
                    ]))
                }
            }),
            "one.md",
            vec!["dir/two.bin"],
            true,
        ),
        (
            "the last file of a bundle of one",
            json!({
                "zip": {
                    "zipFile": aws_smithy_types::base64::encode(
                        zip_bundle(&[("single.md", b"single")]),
                    )
                }
            }),
            "single.md",
            vec![],
            true,
        ),
    ];

    for (case, content, deleted_path, kept, asset_survives) in &cases {
        let (status, created) = call(
            &router,
            "POST",
            &create_asset_uri(&space),
            Some(json!({
                "assetType": "artifact",
                "metadata": {"title": case},
                "content": content,
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateAsset, got {status}: {created}"
        );
        let asset = created["asset"]["assetId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no assetId in {created}"))
            .to_string();

        let (status, deleted) = call(
            &router,
            "DELETE",
            &asset_file_uri(&space, &asset, deleted_path),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from DeleteAssetFile, got {status}: {deleted}"
        );

        let (status, read_asset) = call(&router, "GET", &asset_uri(&space, &asset), None).await;
        if *asset_survives {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from GetAsset after the delete, got {status}: {read_asset}"
            );
            // The asset's content changed, so it is at its next revision.
            assert_eq!(
                read_asset["asset"]["version"],
                json!(2),
                "case {case:?}: the asset is at revision {:?} after the delete",
                read_asset["asset"]["version"]
            );
            // The deleted file is gone from the revision the asset is now at, and
            // every other file of it is untouched.
            let (status, gone) = call(
                &router,
                "GET",
                &asset_file_uri(&space, &asset, deleted_path),
                None,
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404 reading {deleted_path:?}, got {status}: {gone}"
            );
            assert_eq!(
                error_message(&gone),
                format!("Asset file not found: {deleted_path}"),
                "case {case:?}: read refusal said {:?}",
                error_message(&gone)
            );
            for path in kept {
                let (status, held) =
                    call(&router, "GET", &asset_file_uri(&space, &asset, path), None).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 reading {path:?}, got {status}: {held}"
                );
            }
            // A revision is a snapshot, so the content the previous revision held —
            // the deleted file included — is still readable at that version.
            let (status, before) = call(
                &router,
                "GET",
                &format!(
                    "{}?assetVersion=1",
                    asset_file_uri(&space, &asset, deleted_path)
                ),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 reading revision 1 of {deleted_path:?}, got {status}: {before}"
            );
            // A repeat deletes nothing a second time.
            let (status, again) = call(
                &router,
                "DELETE",
                &asset_file_uri(&space, &asset, deleted_path),
                None,
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: deleting {deleted_path:?} again answered {status}: {again}"
            );
        } else {
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404 from GetAsset after the delete, got {status}: {read_asset}"
            );
            assert_eq!(
                error_code(&read_asset),
                "ResourceNotFoundException",
                "case {case:?}: read refusal answered {:?}",
                error_code(&read_asset)
            );
            assert_eq!(
                error_message(&read_asset),
                format!("Asset not found: {asset}"),
                "case {case:?}: read refusal said {:?}",
                error_message(&read_asset)
            );
            // The asset is gone, so a further delete in it reports the asset, not
            // the file.
            let (status, again) = call(
                &router,
                "DELETE",
                &asset_file_uri(&space, &asset, deleted_path),
                None,
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: deleting {deleted_path:?} again answered {status}: {again}"
            );
            assert_eq!(
                error_message(&again),
                format!("Asset not found: {asset}"),
                "case {case:?}: the second delete said {:?}",
                error_message(&again)
            );
        }
    }

    // A path that names no file of the asset is invalid, whatever the asset holds,
    // and an agent space the caller cannot reach is refused before anything inside
    // it is looked up.
    let (status, created) = call(
        &router,
        "POST",
        &create_asset_uri(&space),
        Some(json!({
            "assetType": "artifact",
            "metadata": {"title": "refusals"},
            "content": {
                "zip": {
                    "zipFile": aws_smithy_types::base64::encode(
                        zip_bundle(&[("kept.md", b"kept")]),
                    )
                }
            },
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "refusals: expected 201 from CreateAsset, got {status}: {created}"
    );
    let asset = created["asset"]["assetId"]
        .as_str()
        .expect("no assetId from CreateAsset")
        .to_string();

    for (case, path, expected_status, expected_code) in [
        (
            "a path the asset holds no file at",
            "absent.md",
            404,
            "ResourceNotFoundException",
        ),
        (
            "a path that walks out of the asset",
            "../escape.md",
            400,
            "ValidationException",
        ),
    ] {
        let (status, refused) = call(
            &router,
            "DELETE",
            &asset_file_uri(&space, &asset, path),
            None,
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            expected_code,
            "case {case:?}: refusal answered {:?}",
            error_code(&refused)
        );
    }

    let (status, missing_asset) = call(
        &router,
        "DELETE",
        &asset_file_uri(&space, "asset-ffffffffffffffff", "kept.md"),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "deleting a file of an asset that is not there answered {status}: {missing_asset}"
    );
    assert_eq!(
        error_message(&missing_asset),
        "Asset not found: asset-ffffffffffffffff",
        "deleting a file of an absent asset said {:?}",
        error_message(&missing_asset)
    );

    let (status, denied) = call(
        &router,
        "DELETE",
        &asset_file_uri("agentspace-ffffffffffffffff", &asset, "kept.md"),
        None,
    )
    .await;
    assert_eq!(
        status, 403,
        "deleting a file in an unreachable space answered {status}: {denied}"
    );
    assert_eq!(
        error_code(&denied),
        "AccessDeniedException",
        "deleting a file in an unreachable space answered {:?}",
        error_code(&denied)
    );

    // The file the refusals named is still there, so nothing they refused was
    // persisted.
    let (status, kept) = call(
        &router,
        "GET",
        &asset_file_uri(&space, &asset, "kept.md"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "refusals: expected 200 reading kept.md, got {status}: {kept}"
    );
}
