// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the EXPANDED size a function's configuration reports for each
//! layer it holds.
//!
//! The rule under test: every `FunctionConfiguration.Layers` entry carries, beside the
//! stored size of that layer version's archive (`CodeSize`), the size of its CONTENTS once
//! unzipped (`UncompressedCodeSize`) — the quantity Lambda measures a function against its
//! unzipped-deployment-package limit, so a client can sum its layers and see how much of
//! that limit is left. The number is measured off the archive this emulator stores: an
//! archive whose contents expand to far more than it stores reports the larger number, and
//! an archive this emulator holds no bytes for, or holds bytes it cannot read as an
//! archive, reports 0 — exactly as its stored size does.
//!
//! Every row publishes a real layer version, attaches it through `CreateFunction` and reads
//! the function back through `GetFunctionConfiguration` and `GetFunction`, so no assertion
//! trusts the create's own answer, and the published version of the function is read too —
//! a published version freezes the configuration it was published from, layers included.

use std::io::Write as _;
use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::function_lifecycle::{
    create_function, json_call, make_activation_due, router_on, Answer, ACCOUNT, REGION, ROLE,
};

/// Where `PublishLayerVersion` is bound
/// (`smithy_query(query="operation", operation="PublishLayerVersion")`:
/// `POST /2018-10-31/layers/{LayerName}/versions`).
const LAYERS_URI: &str = "/2018-10-31/layers";

/// A layer Lambda itself provides, in the second form the model's `LayerVersionArn`
/// pattern admits (`arn:<partition>:lambda:::awslayer:<name>`): it belongs to no account,
/// so this emulator holds no archive for it.
const AWS_MANAGED_LAYER: &str = "arn:aws:lambda:::awslayer:AWSLambdaPowertoolsPython";

/// The bytes of a .zip archive holding `entries`, written in memory. Each entry's content
/// is a run of one byte, so an archive's STORED size and the size its contents expand to
/// are deliberately far apart for a large entry.
fn archive(entries: &[(&str, usize)]) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        for (name, length) in entries {
            writer.start_file(*name, options).expect("a new entry");
            writer
                .write_all(&vec![b'x'; *length])
                .expect("the entry's bytes");
        }
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// A `ZipFile` member: restJson1 carries a blob base64-encoded in a JSON body.
fn zip_file(bytes: &[u8]) -> Value {
    Value::String(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// `PublishLayerVersion` of one layer, with the given `Content`.
async fn publish_layer(router: &MultiServiceRouter, name: &str, content: &Value) -> Answer {
    json_call(
        router,
        "POST",
        &format!("{LAYERS_URI}/{name}/versions"),
        &json!({ "Content": content }),
    )
    .await
}

/// `GetFunctionConfiguration` on a function name.
async fn get_configuration(router: &MultiServiceRouter, name: &str) -> Answer {
    json_call(
        router,
        "GET",
        &format!("/2015-03-31/functions/{name}/configuration"),
        &Value::Null,
    )
    .await
}

/// `GetFunctionConfiguration` on a function name at one qualifier.
async fn get_configuration_at(
    router: &MultiServiceRouter,
    name: &str,
    qualifier: &str,
) -> Answer {
    json_call(
        router,
        "GET",
        &format!("/2015-03-31/functions/{name}/configuration?Qualifier={qualifier}"),
        &Value::Null,
    )
    .await
}

/// `GetFunction` on a function name, whose `Configuration` carries the same member set.
async fn get_function(router: &MultiServiceRouter, name: &str) -> Answer {
    json_call(
        router,
        "GET",
        &format!("/2015-03-31/functions/{name}"),
        &Value::Null,
    )
    .await
}

/// The `Layers` list of a configuration body, which a function holding layers must carry.
fn layers(case: &str, reader: &str, body: &Value) -> Vec<Value> {
    body.get("Layers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?} ({reader}): expected a Layers member, got {body}")
        })
}

/// One `Layers` entry's `Arn`, `CodeSize` and `UncompressedCodeSize`, each of which must be
/// present rather than left for the client to default.
fn reported_layer(case: &str, reader: &str, entry: &Value) -> (String, i64, i64) {
    let read = |member: &str| -> i64 {
        entry
            .get(member)
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("case {case:?} ({reader}): expected a {member} number, got {entry}"))
    };
    let arn = entry
        .get("Arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?} ({reader}): expected an Arn, got {entry}"))
        .to_string();
    (arn, read("CodeSize"), read("UncompressedCodeSize"))
}

/// One row: how the layer version was published, and what the function's configuration
/// must therefore report for it.
struct LayerCase {
    /// What a failing assertion calls the row, which is also the layer and function name.
    case: &'static str,
    /// The `Content` the layer version is published with, or `None` for a layer Lambda
    /// provides, which is attached by ARN and published by nobody.
    content: Option<Value>,
    /// The size of the stored archive, which is what `CodeSize` reports.
    code_size: i64,
    /// The size the archive's contents expand to, which is what `UncompressedCodeSize`
    /// reports.
    uncompressed_code_size: i64,
}

/// The rows span what the rule generalises over: how many entries the archive holds, how
/// large they are, how far the stored size and the expanded size are apart, and the three
/// ways there is nothing to measure (no entry, bytes that are no archive, and an archive
/// whose bytes this emulator does not hold).
fn layer_cases() -> Vec<(LayerCase, Vec<u8>)> {
    let one = archive(&[("python/helper.py", 1_024)]);
    let several = archive(&[
        ("python/helper.py", 1_024),
        ("python/vendor/dep.py", 2_048),
        ("python/data.json", 16),
    ]);
    // An entry whose content is one repeated byte: it compresses to a fraction of itself,
    // so the stored size is nowhere near the expanded one and a reader that reported the
    // stored size twice would fail this row.
    let compressible = archive(&[("python/big.bin", 1_048_576)]);
    let empty = archive(&[]);
    let not_an_archive = vec![0xffu8; 128];
    vec![
        (
            LayerCase {
                case: "one-entry",
                content: Some(json!({ "ZipFile": zip_file(&one) })),
                code_size: one.len() as i64,
                uncompressed_code_size: 1_024,
            },
            one.clone(),
        ),
        (
            LayerCase {
                case: "several-entries",
                content: Some(json!({ "ZipFile": zip_file(&several) })),
                code_size: several.len() as i64,
                uncompressed_code_size: 1_024 + 2_048 + 16,
            },
            several.clone(),
        ),
        (
            LayerCase {
                case: "highly-compressible",
                content: Some(json!({ "ZipFile": zip_file(&compressible) })),
                code_size: compressible.len() as i64,
                uncompressed_code_size: 1_048_576,
            },
            compressible.clone(),
        ),
        (
            LayerCase {
                case: "no-entries",
                content: Some(json!({ "ZipFile": zip_file(&empty) })),
                code_size: empty.len() as i64,
                uncompressed_code_size: 0,
            },
            empty.clone(),
        ),
        (
            LayerCase {
                case: "not-an-archive",
                content: Some(json!({ "ZipFile": zip_file(&not_an_archive) })),
                code_size: not_an_archive.len() as i64,
                uncompressed_code_size: 0,
            },
            not_an_archive.clone(),
        ),
        (
            LayerCase {
                case: "from-s3",
                content: Some(json!({ "S3Bucket": "layer-archives", "S3Key": "helpers.zip" })),
                code_size: 0,
                uncompressed_code_size: 0,
            },
            Vec::new(),
        ),
        (
            LayerCase {
                case: "aws-managed",
                content: None,
                code_size: 0,
                uncompressed_code_size: 0,
            },
            Vec::new(),
        ),
    ]
}

#[tokio::test]
async fn every_layer_a_function_holds_reports_its_expanded_size() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for (case, _) in layer_cases() {
        // The layer version the function will attach: published here, so the ARN the
        // function names is one the store holds a record for.
        let layer_arn = match &case.content {
            Some(content) => {
                let published = publish_layer(&router, case.case, content).await;
                assert_eq!(
                    published.status,
                    201,
                    "case {:?}: expected 201 from PublishLayerVersion, got {} {}",
                    case.case,
                    published.status,
                    String::from_utf8_lossy(&published.body)
                );
                published
                    .string("LayerVersionArn")
                    .unwrap_or_else(|| panic!("case {:?}: expected a LayerVersionArn", case.case))
            }
            None => AWS_MANAGED_LAYER.to_string(),
        };

        let created = create_function(
            &router,
            &json!({
                "FunctionName": case.case,
                "Role": ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": { "ZipFile": zip_file(&archive(&[("index.py", 32)])) },
                "Layers": [layer_arn],
            }),
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {:?}: expected 201 from CreateFunction, got {} {}",
            case.case,
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        make_activation_due(&engine);

        // The create's answer and both readers report the same entry: each is projected
        // from the stored resolution of the layer rather than from the request.
        let configuration = get_configuration(&router, case.case).await;
        let function = get_function(&router, case.case).await;
        let bodies = [
            ("CreateFunction", created.json()),
            ("GetFunctionConfiguration", configuration.json()),
            ("GetFunction", function.json()["Configuration"].clone()),
        ];
        for (reader, body) in bodies {
            let entries = layers(case.case, reader, &body);
            assert_eq!(
                entries.len(),
                1,
                "case {:?} ({reader}): expected exactly the one attached layer, got {entries:?}",
                case.case
            );
            let (arn, code_size, uncompressed) = reported_layer(case.case, reader, &entries[0]);
            assert_eq!(
                arn, layer_arn,
                "case {:?} ({reader}): expected the attached layer version ARN, got {arn:?}",
                case.case
            );
            assert_eq!(
                code_size, case.code_size,
                "case {:?} ({reader}): expected CodeSize {}, got {code_size}",
                case.case, case.code_size
            );
            assert_eq!(
                uncompressed, case.uncompressed_code_size,
                "case {:?} ({reader}): expected UncompressedCodeSize {}, got {uncompressed}",
                case.case, case.uncompressed_code_size
            );
        }
    }
}

#[tokio::test]
async fn a_published_version_keeps_the_expanded_size_of_the_layers_it_froze() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    // Two layer versions on one function, so the entries are reported per layer and in the
    // order the caller named them — and the function is published, which freezes them.
    let first = archive(&[("python/first.py", 4_096)]);
    let second = archive(&[("python/second.py", 8), ("python/more.py", 1_000)]);
    let mut attached: Vec<(String, i64, i64)> = Vec::new();
    for (name, bytes, expanded) in [
        ("frozen-first", &first, 4_096_i64),
        ("frozen-second", &second, 1_008),
    ] {
        let published = publish_layer(&router, name, &json!({ "ZipFile": zip_file(bytes) })).await;
        assert_eq!(
            published.status,
            201,
            "case {name}: expected 201 from PublishLayerVersion, got {} {}",
            published.status,
            String::from_utf8_lossy(&published.body)
        );
        let arn = published
            .string("LayerVersionArn")
            .unwrap_or_else(|| panic!("case {name}: expected a LayerVersionArn"));
        assert_eq!(
            arn,
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:layer:{name}:1"),
            "case {name}: expected the minted layer version ARN, got {arn:?}"
        );
        attached.push((arn, bytes.len() as i64, expanded));
    }

    let created = create_function(
        &router,
        &json!({
            "FunctionName": "frozen-layers",
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": zip_file(&archive(&[("index.py", 32)])) },
            "Layers": attached.iter().map(|(arn, _, _)| arn.clone()).collect::<Vec<_>>(),
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateFunction must answer 201, got {} {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    make_activation_due(&engine);

    let version = json_call(
        &router,
        "POST",
        "/2015-03-31/functions/frozen-layers/versions",
        &json!({ "Description": "with layers" }),
    )
    .await;
    assert_eq!(
        version.status,
        201,
        "PublishVersion must answer 201, got {} {}",
        version.status,
        String::from_utf8_lossy(&version.body)
    );

    // The published version's own read reports the layers it froze, in the order they were
    // named, each with both sizes.
    let frozen = get_configuration_at(&router, "frozen-layers", "1").await;
    assert_eq!(
        frozen.status,
        200,
        "the published version's configuration must be readable, got {} {}",
        frozen.status,
        String::from_utf8_lossy(&frozen.body)
    );
    for (reader, body) in [
        ("PublishVersion", version.json()),
        ("GetFunctionConfiguration at the version", frozen.json()),
    ] {
        let entries = layers("frozen-layers", reader, &body);
        let reported: Vec<(String, i64, i64)> = entries
            .iter()
            .map(|entry| reported_layer("frozen-layers", reader, entry))
            .collect();
        assert_eq!(
            reported, attached,
            "case {reader}: expected the frozen layers with both sizes, got {reported:?}"
        );
    }
}
