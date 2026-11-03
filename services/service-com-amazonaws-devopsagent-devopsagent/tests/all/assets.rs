// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the asset plane: `CreateAsset` and the `GetAsset`
//! read-back it is proved through.
//!
//! Every case drives the service through the same router the emulator serves, so
//! the routes, status codes, protocol envelope and state are all exercised.

use std::io::Write;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_devopsagent_devopsagent::ops::types::AssetFileBody;
use service_com_amazonaws_devopsagent_devopsagent::state::{
    ASSET_TYPE_RULES, MAX_ASSETS_PER_AGENT_SPACE, MAX_ASSET_CONTENT_BYTES, MAX_ASSET_FILES,
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
    router_on(Arc::new(engine::InMemoryEngine::new()))
}

/// The same emulator, built on an engine the caller keeps a handle to, so a test
/// can read the records a request persisted.
fn router_on(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_devopsagent_devopsagent::EmulatedService::new(engine),
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

fn get_asset_uri(space: &str, asset: &str) -> String {
    format!("/asset/agent-space/{space}/assets/{asset}")
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

/// RULE: a create persists the asset and a read answers exactly what was
/// persisted — for every shape of `content` and `metadata` the model allows.
#[tokio::test]
async fn created_asset_is_returned_unchanged_by_get() {
    // The rows differ along what the rule generalises over: the content variant
    // (single file as text, single file as bytes, a zip bundle of one and of
    // several entries, a source URL), the asset type, and the size and shape of
    // the metadata document.
    let cases: Vec<(&str, Value)> = vec![
        (
            "text file, skill",
            json!({
                "assetType": "skill",
                "metadata": {"name": "deploy-helper"},
                "content": {"file": {"path": "SKILL.md", "body": {"text": "# how to deploy"}}},
            }),
        ),
        (
            "binary file, artifact",
            json!({
                "assetType": "artifact",
                "metadata": {"title": "release notes", "description": "v2"},
                "content": {
                    "file": {
                        "path": "notes/release (1).bin",
                        "body": {"bytes": "AAECAw=="},
                    }
                },
            }),
        ),
        (
            "per-file metadata",
            json!({
                "assetType": "attachment",
                "metadata": {"filename": "diagram.txt"},
                "content": {
                    "file": {
                        "path": "diagram.txt",
                        "body": {"text": "a -> b"},
                        "metadata": {"contentType": "text/plain"},
                    }
                },
            }),
        ),
        (
            "metadata absent on a type with no required field",
            json!({
                "assetType": "runbook",
                "content": {"file": {"path": "run.md", "body": {"text": "step 1"}}},
            }),
        ),
        (
            "nested metadata document",
            json!({
                "assetType": "skill",
                "metadata": {
                    "name": "nested",
                    "tags": ["a", "b"],
                    "limits": {"retries": 3, "enabled": true},
                },
                "content": {"file": {"path": "a/b/c.md", "body": {"text": ""}}},
            }),
        ),
        (
            "zip bundle, one entry",
            json!({
                "assetType": "artifact",
                "metadata": {"title": "one entry"},
                "content": {
                    "zip": {
                        "zipFile": aws_smithy_types::base64::encode(
                            zip_bundle(&[("only.md", b"only")]),
                        ),
                    }
                },
            }),
        ),
        (
            "zip bundle, several entries and a directory",
            json!({
                "assetType": "artifact",
                "metadata": {"title": "many entries"},
                "content": {
                    "zip": {
                        "zipFile": aws_smithy_types::base64::encode(zip_bundle(&[
                            ("dir/", b""),
                            ("dir/one.md", b"one"),
                            ("two (copy).md", b"two"),
                        ])),
                    }
                },
            }),
        ),
        (
            "source url import",
            json!({
                "assetType": "artifact",
                "metadata": {"title": "imported"},
                "content": {"sourceUrl": {"url": "https://example.com/bundle.zip"}},
            }),
        ),
    ];

    let router = new_router();
    let space = create_space(&router, "asset-space").await;
    let mut seen: Vec<String> = Vec::new();

    for (case, input) in &cases {
        let (status, created) =
            call(&router, "POST", &create_asset_uri(&space), Some(input.clone())).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateAsset, got {status}: {created}"
        );
        let asset = &created["asset"];
        let asset_id = asset["assetId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no assetId in {created}"))
            .to_string();

        // A minted id satisfies the modelled `ResourceId` pattern
        // `^[a-zA-Z0-9_.-]+$` (1..128) and is never handed out twice.
        assert!(
            (1..=128).contains(&asset_id.chars().count())
                && asset_id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')),
            "case {case:?}: minted id {asset_id:?} violates ^[a-zA-Z0-9_.-]+$"
        );
        assert!(
            !seen.contains(&asset_id),
            "case {case:?}: minted id {asset_id:?} was handed out twice"
        );
        seen.push(asset_id.clone());

        assert_eq!(
            asset["assetType"], input["assetType"],
            "case {case:?}: assetType came back as {:?}",
            asset["assetType"]
        );
        // `Asset$metadata` is required: the members the request described, or none
        // at all, completed with the lifecycle state the SERVICE maintains in that
        // document — see the `status` rule in `asset_metadata_and_deletion`.
        let mut expected_metadata = input.get("metadata").cloned().unwrap_or_else(|| json!({}));
        expected_metadata["status"] = json!(
            service_com_amazonaws_devopsagent_devopsagent::state::ASSET_STATUS_ACTIVE
        );
        assert_eq!(
            asset["metadata"], expected_metadata,
            "case {case:?}: metadata came back as {:?}",
            asset["metadata"]
        );
        assert_eq!(
            asset["version"], json!(1),
            "case {case:?}: first stored revision is {:?}, not 1",
            asset["version"]
        );
        assert!(
            !asset["createdAt"].is_null() && !asset["updatedAt"].is_null(),
            "case {case:?}: createdAt/updatedAt missing from {created}"
        );

        // READ BACK: GetAsset answers exactly what CreateAsset persisted, and a
        // second read is identical, so reading changes nothing.
        let (status, fetched) = call(&router, "GET", &get_asset_uri(&space, &asset_id), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetAsset, got {status}: {fetched}"
        );
        assert_eq!(
            fetched["asset"], created["asset"],
            "case {case:?}: GetAsset returned {:?}, not what CreateAsset persisted ({:?})",
            fetched["asset"], created["asset"]
        );
        let (_, again) = call(&router, "GET", &get_asset_uri(&space, &asset_id), None).await;
        assert_eq!(
            again, fetched,
            "case {case:?}: two reads of one asset disagree: {again:?}"
        );

        // The version the asset carries is readable by name; any other version
        // names no revision of it.
        let (status, by_version) = call(
            &router,
            "GET",
            &format!("{}?assetVersion=1", get_asset_uri(&space, &asset_id)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 reading version 1, got {status}: {by_version}"
        );
        assert_eq!(
            by_version["asset"], created["asset"],
            "case {case:?}: version 1 read back as {:?}",
            by_version["asset"]
        );
        let (status, missing) = call(
            &router,
            "GET",
            &format!("{}?assetVersion=2", get_asset_uri(&space, &asset_id)),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for a version the asset never had, got {status}: {missing}"
        );
        assert_eq!(
            error_code(&missing),
            "ResourceNotFoundException",
            "case {case:?}: version refusal answered {:?}",
            error_code(&missing)
        );
    }
}

/// RULE: the metadata an asset must carry is determined by its asset type — a
/// create that omits the field its type requires is refused with HTTP 400
/// `ValidationException` naming that field and the type that requires it, and a
/// create that carries it succeeds.
#[tokio::test]
async fn required_metadata_fields_are_type_specific() {
    let router = new_router();
    let space = create_space(&router, "metadata-space").await;

    // The per-type field and the wording of the refusal, written out: these rows
    // pin what the service answers rather than reading it back off the catalog the
    // handler enforces.
    let pinned: &[(&str, &str)] = &[
        ("skill", "name is required for Skill knowledge items"),
        ("artifact", "title is required for Artifact knowledge items"),
        (
            "attachment",
            "filename is required for Attachment knowledge items",
        ),
    ];
    for (asset_type, expected) in pinned {
        let (status, refused) = call(
            &router,
            "POST",
            &create_asset_uri(&space),
            Some(json!({
                "assetType": asset_type,
                "content": {"file": {"path": "f.md", "body": {"text": "x"}}},
            })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {asset_type:?}: expected 400 with no metadata, got {status}: {refused}"
        );
        assert_eq!(
            error_message(&refused),
            *expected,
            "case {asset_type:?}: message was {:?}",
            error_message(&refused)
        );
    }

    // The catalog's own rows drive the table, so every type this service enforces
    // is covered, including the ones this issue did not cite.
    for rule in ASSET_TYPE_RULES {
        for field in rule.required_metadata {
            let case = format!("{} missing {field}", rule.asset_type);

            // Omitted entirely, explicitly null, and present but empty are all
            // "not provided".
            for (how, metadata) in [
                ("absent", json!({})),
                ("null", json!({ *field: Value::Null })),
                ("empty string", json!({ *field: "" })),
                ("another type's field", json!({"someOtherField": "x"})),
            ] {
                let (status, refused) = call(
                    &router,
                    "POST",
                    &create_asset_uri(&space),
                    Some(json!({
                        "assetType": rule.asset_type,
                        "metadata": metadata,
                        "content": {"file": {"path": "f.md", "body": {"text": "x"}}},
                    })),
                )
                .await;
                assert_eq!(
                    status, 400,
                    "case {case:?} ({how}): expected 400, got {status}: {refused}"
                );
                assert_eq!(
                    error_code(&refused),
                    "ValidationException",
                    "case {case:?} ({how}): answered {:?}",
                    error_code(&refused)
                );
                assert_eq!(
                    error_message(&refused),
                    format!("{field} is required for {} items", rule.display_name),
                    "case {case:?} ({how}): message was {:?}",
                    error_message(&refused)
                );
            }

            // The same create with the field provided is accepted.
            let (status, created) = call(
                &router,
                "POST",
                &create_asset_uri(&space),
                Some(json!({
                    "assetType": rule.asset_type,
                    "metadata": json!({ *field: "provided" }),
                    "content": {"file": {"path": "f.md", "body": {"text": "x"}}},
                })),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: expected 201 once {field} is provided, got {status}: {created}"
            );
        }
    }

    // A type this service enforces no rule for is not refused for a field no rule
    // names.
    let (status, created) = call(
        &router,
        "POST",
        &create_asset_uri(&space),
        Some(json!({
            "assetType": "unknown-type",
            "content": {"file": {"path": "f.md", "body": {"text": "x"}}},
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case \"type with no rule\": expected 201, got {status}: {created}"
    );

    // A metadata document that is not a set of members describes no asset.
    for (case, metadata) in [
        ("list", json!(["name"])),
        ("string", json!("name")),
        ("number", json!(7)),
        ("boolean", json!(true)),
    ] {
        let (status, refused) = call(
            &router,
            "POST",
            &create_asset_uri(&space),
            Some(json!({
                "assetType": "runbook",
                "metadata": metadata,
                "content": {"file": {"path": "f.md", "body": {"text": "x"}}},
            })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for metadata that is not a set of members, got {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: answered {:?}",
            error_code(&refused)
        );
    }
}

/// RULE: the asset plane authorises the caller against the named agent space
/// first, so a space it cannot reach fails closed with HTTP 403 `AccessDenied`,
/// while a missing asset inside a reachable space is HTTP 404.
#[tokio::test]
async fn asset_plane_fails_closed_on_an_unreachable_agent_space() {
    let router = new_router();
    let space = create_space(&router, "refusal-space").await;
    let (_, created) = call(
        &router,
        "POST",
        &create_asset_uri(&space),
        Some(json!({
            "assetType": "artifact",
            "metadata": {"title": "t"},
            "content": {"file": {"path": "f.md", "body": {"text": "x"}}},
        })),
    )
    .await;
    let asset_id = created["asset"]["assetId"]
        .as_str()
        .unwrap_or_else(|| panic!("no assetId in {created}"))
        .to_string();

    // Rows differ along the shape of the identifier that names no space.
    for case in ["never-created", "as-0000000000009999", "x"] {
        let (status, refused) = call(&router, "GET", &get_asset_uri(case, &asset_id), None).await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected 403 reading an asset of an unreachable space, got {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "AccessDeniedException",
            "case {case:?}: answered {:?}",
            error_code(&refused)
        );
        assert_eq!(
            error_message(&refused),
            "Access denied",
            "case {case:?}: message was {:?}",
            error_message(&refused)
        );

        let (status, refused) = call(
            &router,
            "POST",
            &create_asset_uri(case),
            Some(json!({
                "assetType": "artifact",
                "metadata": {"title": "t"},
                "content": {"file": {"path": "f.md", "body": {"text": "x"}}},
            })),
        )
        .await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected 403 creating in an unreachable space, got {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "AccessDeniedException",
            "case {case:?}: create answered {:?}",
            error_code(&refused)
        );
    }

    // Inside a space the caller reaches, an asset that does not exist is reported
    // as missing, naming the identifier the request carried.
    for case in ["missing-asset", "asset-0000000000009999"] {
        let (status, refused) = call(&router, "GET", &get_asset_uri(&space, case), None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for a missing asset, got {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case:?}: answered {:?}",
            error_code(&refused)
        );
        assert_eq!(
            error_message(&refused),
            format!("Asset not found: {case}"),
            "case {case:?}: message was {:?}",
            error_message(&refused)
        );
    }

    // Deleting the space takes its assets with it: what is left is unreachable,
    // not missing.
    let (status, body) = call(&router, "DELETE", &format!("/v1/agentspaces/{space}"), None).await;
    assert_eq!(status, 204, "expected 204 from DeleteAgentSpace, got {status}: {body}");
    let (status, refused) = call(&router, "GET", &get_asset_uri(&space, &asset_id), None).await;
    assert_eq!(
        status, 403,
        "case \"deleted space\": expected 403, got {status}: {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "AccessDeniedException",
        "case \"deleted space\": answered {:?}",
        error_code(&refused)
    );
}

/// RULE: a repeat of one `@idempotencyToken` create returns the asset the first
/// call created instead of minting a second one; a different token mints another.
#[tokio::test]
async fn one_client_token_creates_one_asset() {
    let router = new_router();
    let space = create_space(&router, "token-space").await;
    let request = |token: &str| {
        json!({
            "assetType": "skill",
            "metadata": {"name": "idempotent"},
            "content": {"file": {"path": "SKILL.md", "body": {"text": "body"}}},
            "clientToken": token,
        })
    };

    let (status, first) =
        call(&router, "POST", &create_asset_uri(&space), Some(request("token-a"))).await;
    assert_eq!(status, 201, "expected 201 from the first create, got {status}: {first}");
    let (status, replayed) =
        call(&router, "POST", &create_asset_uri(&space), Some(request("token-a"))).await;
    assert_eq!(status, 201, "expected 201 from the repeat, got {status}: {replayed}");
    assert_eq!(
        replayed["asset"], first["asset"],
        "case \"same token\": the repeat answered {:?}, not the asset the first call created ({:?})",
        replayed["asset"], first["asset"]
    );

    let (status, other) =
        call(&router, "POST", &create_asset_uri(&space), Some(request("token-b"))).await;
    assert_eq!(status, 201, "expected 201 for a second token, got {status}: {other}");
    assert_ne!(
        other["asset"]["assetId"], first["asset"]["assetId"],
        "case \"different token\": a second token replayed asset {:?}",
        other["asset"]["assetId"]
    );

    // An asset lives inside its agent space, so the same token in another space
    // names another asset — and the first space's token still replays its own.
    let second_space = create_space(&router, "token-space-2").await;
    let (status, elsewhere) = call(
        &router,
        "POST",
        &create_asset_uri(&second_space),
        Some(request("token-a")),
    )
    .await;
    assert_eq!(
        status, 201,
        "case \"same token, another space\": expected 201, got {status}: {elsewhere}"
    );
    assert_ne!(
        elsewhere["asset"]["assetId"], first["asset"]["assetId"],
        "case \"same token, another space\": answered the other space's asset {:?}",
        elsewhere["asset"]["assetId"]
    );
    let (_, replayed_again) =
        call(&router, "POST", &create_asset_uri(&space), Some(request("token-a"))).await;
    assert_eq!(
        replayed_again["asset"], first["asset"],
        "case \"same token, first space again\": answered {:?}, not the asset the first call created",
        replayed_again["asset"]
    );
}

/// RULE: content a create cannot store is refused by its own error class — a zip
/// bundle that is not a readable archive, and a path that names no file of the
/// asset (whichever content variant it arrived in), are 400s; the per-space asset
/// quota is a 402.
#[tokio::test]
async fn unstorable_content_and_the_asset_quota_are_refused() {
    let router = new_router();
    let space = create_space(&router, "content-space").await;

    let cases: Vec<(&str, Value)> = vec![
        ("not an archive", json!({"zip": {"zipFile": "bm90IGEgemlw"}})),
        ("empty bytes", json!({"zip": {"zipFile": ""}})),
        (
            "entry escaping the asset",
            json!({
                "zip": {
                    "zipFile": aws_smithy_types::base64::encode(
                        zip_bundle(&[("../escape.md", b"x")]),
                    )
                }
            }),
        ),
        (
            "entry starting at a root",
            json!({
                "zip": {
                    "zipFile": aws_smithy_types::base64::encode(
                        zip_bundle(&[("/rooted.md", b"x")]),
                    )
                }
            }),
        ),
        (
            // The SAME path rule governs the single-file variant: a path the
            // modelled pattern admits but that leaves the asset names no file of
            // it, however it arrived.
            "single file escaping the asset",
            json!({"file": {"path": "../escape.md", "body": {"text": "x"}}}),
        ),
        (
            "single file starting at a root",
            json!({"file": {"path": "/rooted.md", "body": {"text": "x"}}}),
        ),
        (
            "single file whose parent walk is nested",
            json!({"file": {"path": "docs/../../escape.md", "body": {"text": "x"}}}),
        ),
        (
            "entry naming no asset file path",
            json!({
                "zip": {
                    "zipFile": aws_smithy_types::base64::encode(
                        zip_bundle(&[("not*a*path.md", b"x")]),
                    )
                }
            }),
        ),
    ];
    for (case, content) in &cases {
        let (status, refused) = call(
            &router,
            "POST",
            &create_asset_uri(&space),
            Some(json!({
                "assetType": "artifact",
                "metadata": {"title": "t"},
                "content": content,
            })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: answered {:?}",
            error_code(&refused)
        );
    }

    // The archive bounds: more entries than one asset holds answers the quota
    // error, and content that DECOMPRESSES past what one asset carries answers the
    // size error — a small, highly compressible archive is exactly the case the
    // per-entry budget exists for.
    let mut too_many: Vec<(String, Vec<u8>)> = Vec::new();
    for index in 0..=MAX_ASSET_FILES {
        too_many.push((format!("f{index}.md"), b"x".to_vec()));
    }
    let too_many: Vec<(&str, &[u8])> = too_many
        .iter()
        .map(|(path, content)| (path.as_str(), content.as_slice()))
        .collect();
    let (status, refused) = call(
        &router,
        "POST",
        &create_asset_uri(&space),
        Some(json!({
            "assetType": "artifact",
            "metadata": {"title": "t"},
            "content": {"zip": {"zipFile": aws_smithy_types::base64::encode(zip_bundle(&too_many))}},
        })),
    )
    .await;
    assert_eq!(
        status, 402,
        "case \"more entries than one asset holds\": expected 402, got {status}: {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ServiceQuotaExceededException",
        "case \"more entries than one asset holds\": answered {:?}",
        error_code(&refused)
    );

    let oversized = vec![0u8; MAX_ASSET_CONTENT_BYTES + 1];
    let (status, refused) = call(
        &router,
        "POST",
        &create_asset_uri(&space),
        Some(json!({
            "assetType": "artifact",
            "metadata": {"title": "t"},
            "content": {
                "zip": {
                    "zipFile": aws_smithy_types::base64::encode(
                        zip_bundle(&[("big.bin", &oversized)]),
                    )
                }
            },
        })),
    )
    .await;
    assert_eq!(
        status, 413,
        "case \"content decompressing past the asset's size\": expected 413, got {status}: {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ContentSizeExceededException",
        "case \"content decompressing past the asset's size\": answered {:?}",
        error_code(&refused)
    );

    // The quota bounds how many assets one space holds; past it the create answers
    // the service's quota error rather than growing the space without end.
    let quota_space = create_space(&router, "quota-space").await;
    let engine_request = json!({
        "assetType": "artifact",
        "metadata": {"title": "t"},
        "content": {"file": {"path": "f.md", "body": {"text": "x"}}},
    });
    for index in 0..MAX_ASSETS_PER_AGENT_SPACE {
        let (status, created) = call(
            &router,
            "POST",
            &create_asset_uri(&quota_space),
            Some(engine_request.clone()),
        )
        .await;
        assert_eq!(
            status, 201,
            "case \"asset {index}\": expected 201 below the quota, got {status}: {created}"
        );
    }
    let (status, refused) = call(
        &router,
        "POST",
        &create_asset_uri(&quota_space),
        Some(engine_request),
    )
    .await;
    assert_eq!(
        status, 402,
        "case \"past the quota\": expected 402, got {status}: {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ServiceQuotaExceededException",
        "case \"past the quota\": answered {:?}",
        error_code(&refused)
    );
}

/// One row of the content-materialisation table: the case name, the `content`
/// member to send, and the path -> bytes the asset must end up holding.
type ContentCase<'a> = (&'a str, Value, Vec<(&'a str, &'a [u8])>);

/// RULE: the files an asset holds are the ones its content carried — a single file
/// at the path the request named, or the entries the archive carries, each stored
/// with the bytes it holds.
///
/// `Asset` carries no files member and the file reads are still their own issues'
/// stubs, so this asserts the stored records directly, the way
/// `tests/all/state_roundtrip.rs` reaches this service's state.
#[tokio::test]
async fn content_is_materialised_into_the_assets_files() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(Arc::clone(&engine));
    let space = create_space(&router, "content-state-space").await;

    // Rows differ along the content variant and, for an archive, along the number
    // of entries, the presence of a directory entry and the bytes each entry
    // holds. The expectation is the path -> content map the asset must end up
    // holding.
    let cases: Vec<ContentCase<'_>> = vec![
        (
            "single text file",
            json!({"file": {"path": "docs/SKILL.md", "body": {"text": "# body"}}}),
            vec![("docs/SKILL.md", b"# body")],
        ),
        (
            "single binary file",
            json!({"file": {"path": "blob.bin", "body": {"bytes": "AAECAw=="}}}),
            vec![("blob.bin", &[0u8, 1, 2, 3])],
        ),
        (
            "zip bundle of several entries past a directory entry",
            json!({
                "zip": {
                    "zipFile": aws_smithy_types::base64::encode(zip_bundle(&[
                        ("dir/", b""),
                        ("dir/one.md", b"one"),
                        ("two (copy).md", b"two"),
                    ]))
                }
            }),
            vec![("dir/one.md", b"one"), ("two (copy).md", b"two")],
        ),
        (
            "zip bundle of one entry",
            json!({
                "zip": {
                    "zipFile": aws_smithy_types::base64::encode(
                        zip_bundle(&[("only.md", b"only bytes")]),
                    )
                }
            }),
            vec![("only.md", b"only bytes")],
        ),
        (
            // Nothing is fetched for a source URL, so no file is invented for it.
            "source url import",
            json!({"sourceUrl": {"url": "https://example.com/bundle.zip"}}),
            vec![],
        ),
    ];

    for (case, content, expected) in &cases {
        let (status, created) = call(
            &router,
            "POST",
            &create_asset_uri(&space),
            Some(json!({
                "assetType": "artifact",
                "metadata": {"title": "t"},
                "content": content,
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201, got {status}: {created}"
        );
        let asset_id = created["asset"]["assetId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no assetId in {created}"))
            .to_string();

        let store = engine
            .state::<service_com_amazonaws_devopsagent_devopsagent::ComAmazonawsDevopsagentDevopsagentState>();
        let store = store.lock();
        let asset = store
            .agent_spaces
            .get(&space)
            .and_then(|space| space.assets.get(&asset_id))
            .unwrap_or_else(|| panic!("case {case:?}: asset {asset_id} was not persisted"));

        let stored: Vec<(String, Vec<u8>)> = asset
            .files
            .values()
            .map(|file| {
                (
                    file.path.clone(),
                    match &file.body {
                        AssetFileBody::Bytes(bytes) => bytes.clone(),
                        AssetFileBody::Text(text) => text.as_bytes().to_vec(),
                    },
                )
            })
            .collect();
        let want: Vec<(String, Vec<u8>)> = expected
            .iter()
            .map(|(path, content)| ((*path).to_string(), content.to_vec()))
            .collect();
        assert_eq!(
            stored, want,
            "case {case:?}: the asset holds {stored:?}, not the content the request carried ({want:?})"
        );
        for file in asset.files.values() {
            assert_eq!(
                file.version, 1,
                "case {case:?}: file {:?} was stored at revision {}, not 1",
                file.path, file.version
            );
        }
        assert_eq!(
            asset.source_url.as_deref(),
            content.get("sourceUrl").map(|_| "https://example.com/bundle.zip"),
            "case {case:?}: the stored source url is {:?}",
            asset.source_url
        );
    }
}
