// SPDX-License-Identifier: CC0-1.0

//! The repository catalog record, driven end to end over the wire.
//!
//! Every case goes through the front door a client uses — `POST /` with
//! `X-Amz-Target: SpencerFrontendService.<Operation>` and an `awsJson1_1` body
//! (`OPERATIONS.md`, "Wire protocol") — reusing the harness the sibling
//! `repository_image_lifecycle` suite drives the same router with. Nothing reaches into
//! the state struct, so a `catalogData` that is not computed from persisted state cannot
//! pass.
//!
//! The rule under test: `PutRepositoryCatalogData` replaces the whole catalog record of
//! an existing repository with the document the request supplied — text and list members
//! verbatim, `logoUrl` present only while a logo payload is, and `marketplaceCertified`
//! absent, since nothing a caller of this API does makes a certification statement — and
//! `GetRepositoryCatalogData` reports back exactly that record. The tables below span the
//! document forms the rule generalises over (every member set, single members, empty
//! strings, empty lists, unicode, list and text sizes at each published bound, logo
//! payloads of several sizes) rather than one recorded request.

use serde_json::{json, Value};

use crate::repository_image_lifecycle::{call, logo_url, ok, refused, service, REGISTRY_ID};

/// The repository every document in the main table is written to, in turn.
const REPO: &str = "team/catalog";

/// A second repository, never named by a put in the main table: it pins that putting one
/// repository's catalog data leaves another's alone.
const OTHER_REPO: &str = "team/untouched";

/// A `logoImageBlob` member as awsJson1_1 carries a blob: base64 of the raw bytes,
/// through the SDK's own standard-alphabet codec (`aws_smithy_types::base64`) — the one
/// the wire serializer uses for every Smithy blob.
fn blob_member(blob: &[u8]) -> Value {
    Value::String(aws_smithy_types::base64::encode(blob))
}

/// One row of the main table: a catalog document to put, and the record both
/// `PutRepositoryCatalogData` and `GetRepositoryCatalogData` must then report.
struct Row {
    case: &'static str,
    input: Value,
    expected: Value,
}

/// Create `name` with no catalog data of its own.
async fn create(router: &smithy_server_runtime::MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateRepository",
        json!({ "repositoryName": name }),
        name,
    )
    .await;
}

/// Every catalog document form the rule spans, written to ONE repository in sequence —
/// so each row also proves the previous row's members are gone, which is what "the
/// request's document replaces the record" means.
fn table() -> Vec<Row> {
    // The bytes 0x00 0x01 0x02, and a one-byte payload: two distinct logo payloads whose
    // derived URLs must differ.
    let logo = vec![0u8, 1, 2];
    let other_logo = vec![0xffu8];
    // 50 entries is `ArchitectureList` / `OperatingSystemList`'s published maximum.
    let fifty_architectures: Vec<String> = (0..50).map(|i| format!("arch-{i}")).collect();
    let fifty_operating_systems: Vec<String> = (0..50).map(|i| format!("os-{i}")).collect();

    vec![
        Row {
            case: "every member set",
            input: json!({
                "description": "a short description",
                "architectures": ["ARM 64", "x86-64"],
                "operatingSystems": ["Linux", "Windows"],
                "aboutText": "# about",
                "usageText": "docker pull",
                "logoImageBlob": blob_member(&logo),
            }),
            expected: json!({
                "description": "a short description",
                "architectures": ["ARM 64", "x86-64"],
                "operatingSystems": ["Linux", "Windows"],
                "aboutText": "# about",
                "usageText": "docker pull",
                "logoUrl": logo_url(REPO, &logo),
            }),
        },
        Row {
            case: "only a description: every other member of the previous document is gone",
            input: json!({ "description": "just a description" }),
            expected: json!({
                "description": "just a description",
            }),
        },
        Row {
            case: "an empty document clears the record",
            input: json!({}),
            expected: json!({}),
        },
        Row {
            case: "empty strings and empty lists are stored, not dropped",
            input: json!({
                "description": "",
                "aboutText": "",
                "usageText": "",
                "architectures": [],
                "operatingSystems": [],
            }),
            expected: json!({
                "description": "",
                "aboutText": "",
                "usageText": "",
                "architectures": [],
                "operatingSystems": [],
            }),
        },
        Row {
            case: "unicode text round-trips byte for byte",
            input: json!({
                "description": "Umgebung für Container — 容器 🚀",
                "aboutText": "## О проекте\nстрока с emoji 😀 и ümläuts",
                "usageText": "docker pull — ✅ طبقة",
                "architectures": ["ARM 64 — 中文"],
                "operatingSystems": ["Linux ✅"],
            }),
            expected: json!({
                "description": "Umgebung für Container — 容器 🚀",
                "aboutText": "## О проекте\nстрока с emoji 😀 и ümläuts",
                "usageText": "docker pull — ✅ طبقة",
                "architectures": ["ARM 64 — 中文"],
                "operatingSystems": ["Linux ✅"],
            }),
        },
        Row {
            // `Architecture` / `OperatingSystem` are plain 1..50 strings in the model —
            // no enum, no value list — so an unheard-of architecture is stored verbatim.
            case: "freeform architecture and operating-system values",
            input: json!({
                "architectures": ["riscv128", "Nonexistent Arch 9000", "s390x"],
                "operatingSystems": ["Plan 9", "TempleOS"],
            }),
            expected: json!({
                "architectures": ["riscv128", "Nonexistent Arch 9000", "s390x"],
                "operatingSystems": ["Plan 9", "TempleOS"],
            }),
        },
        Row {
            case: "50 architectures and 50 operating systems: each list's published maximum",
            input: json!({
                "architectures": fifty_architectures,
                "operatingSystems": fifty_operating_systems,
            }),
            expected: json!({
                "architectures": fifty_architectures,
                "operatingSystems": fifty_operating_systems,
            }),
        },
        Row {
            case: "texts at their published maximum lengths",
            input: json!({
                "description": "d".repeat(1024),
                "aboutText": "a".repeat(25600),
                "usageText": "u".repeat(25600),
            }),
            expected: json!({
                "description": "d".repeat(1024),
                "aboutText": "a".repeat(25600),
                "usageText": "u".repeat(25600),
            }),
        },
        Row {
            case: "a logo payload on its own",
            input: json!({ "logoImageBlob": blob_member(&other_logo) }),
            expected: json!({
                "logoUrl": logo_url(REPO, &other_logo),
            }),
        },
        Row {
            case: "an empty logo payload is still a payload",
            input: json!({ "logoImageBlob": blob_member(&[]) }),
            expected: json!({
                "logoUrl": logo_url(REPO, &[]),
            }),
        },
        Row {
            case: "the largest logo payload the model admits (512000 bytes)",
            input: json!({ "logoImageBlob": blob_member(&vec![7u8; 512_000]) }),
            expected: json!({
                "logoUrl": logo_url(REPO, &vec![7u8; 512_000]),
            }),
        },
    ]
}

#[tokio::test]
async fn put_replaces_the_catalog_record_and_get_reports_it_back() {
    let router = service();
    create(&router, REPO).await;
    create(&router, OTHER_REPO).await;

    // The second repository's own record, set once and never touched again.
    let other_expected = json!({ "description": "the other repository" });
    ok(
        &router,
        "PutRepositoryCatalogData",
        json!({
            "repositoryName": OTHER_REPO,
            "catalogData": { "description": "the other repository" },
        }),
        "seed the untouched repository",
    )
    .await;

    for Row {
        case,
        input,
        expected,
    } in table()
    {
        let put = ok(
            &router,
            "PutRepositoryCatalogData",
            json!({ "repositoryName": REPO, "catalogData": input }),
            case,
        )
        .await;
        assert_eq!(
            put["catalogData"], expected,
            "case {case:?}: expected PutRepositoryCatalogData to report {expected}, got {}",
            put["catalogData"]
        );
        // `logoImageBlob` is an input member only: `RepositoryCatalogData` models
        // `logoUrl` instead, so no response may echo the payload back.
        assert!(
            put["catalogData"].get("logoImageBlob").is_none(),
            "case {case:?}: expected no logoImageBlob in the reported record, got {}",
            put["catalogData"]
        );

        let got = ok(
            &router,
            "GetRepositoryCatalogData",
            json!({ "repositoryName": REPO }),
            case,
        )
        .await;
        assert_eq!(
            got["catalogData"], expected,
            "case {case:?}: expected GetRepositoryCatalogData to report the stored \
             record {expected}, got {}",
            got["catalogData"]
        );

        // A put names ONE repository, so the other repository's record is still its own.
        let other = ok(
            &router,
            "GetRepositoryCatalogData",
            json!({ "repositoryName": OTHER_REPO }),
            case,
        )
        .await;
        assert_eq!(
            other["catalogData"], other_expected,
            "case {case:?}: expected putting {REPO:?} to leave {OTHER_REPO:?} alone, got {}",
            other["catalogData"]
        );
    }
}

#[tokio::test]
async fn the_logo_url_is_derived_from_the_payload_bytes() {
    let router = service();
    create(&router, REPO).await;

    // Distinct payloads of several sizes and shapes, each put twice: the derived URL is
    // stable for the same bytes and distinct for different bytes.
    let payloads: Vec<Vec<u8>> = vec![
        vec![],
        vec![0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a],
        vec![0xff, 0xd8, 0xff, 0xe0],
        (0..=255u8).collect(),
        vec![42u8; 100_000],
    ];

    let mut seen: Vec<String> = Vec::new();
    for payload in &payloads {
        let case = format!("{}-byte payload", payload.len());
        let mut urls = Vec::new();
        for attempt in 0..2 {
            let put = ok(
                &router,
                "PutRepositoryCatalogData",
                json!({
                    "repositoryName": REPO,
                    "catalogData": { "logoImageBlob": blob_member(payload) },
                }),
                &case,
            )
            .await;
            let url = put["catalogData"]["logoUrl"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                url,
                logo_url(REPO, payload),
                "case {case:?} (attempt {attempt}): expected the logo URL to name the \
                 payload's own digest, got {url:?}"
            );
            urls.push(url);
        }
        assert_eq!(
            urls[0], urls[1],
            "case {case:?}: expected the same payload to yield the same logo URL, got {urls:?}"
        );
        assert!(
            !seen.contains(&urls[0]),
            "case {case:?}: expected a distinct logo URL per payload, but {:?} was already \
             served for another payload",
            urls[0]
        );
        seen.push(urls[0].clone());
    }

    // Dropping the payload drops the URL: the document the request supplies is the whole
    // record.
    let cleared = ok(
        &router,
        "PutRepositoryCatalogData",
        json!({ "repositoryName": REPO, "catalogData": { "description": "no logo" } }),
        "no payload",
    )
    .await;
    assert_eq!(
        cleared["catalogData"]["logoUrl"],
        Value::Null,
        "case \"no payload\": expected no logo URL once the payload is no longer supplied, \
         got {}",
        cleared["catalogData"]
    );
}

#[tokio::test]
async fn a_put_needs_an_existing_repository_in_this_registry() {
    let router = service();
    create(&router, REPO).await;

    // Every name form the modelled `repositoryName` pattern admits, none of them created.
    for missing in ["ab", "missing-repo", "team/missing", "a.b_c-d/deep/nested"] {
        let case = format!("unknown repository {missing:?}");
        refused(
            &router,
            "PutRepositoryCatalogData",
            json!({
                "repositoryName": missing,
                "catalogData": { "description": "catalog data for nothing" },
            }),
            "RepositoryNotFoundException",
            &case,
        )
        .await;
        // The refusal created nothing: the reader still reports the repository missing.
        refused(
            &router,
            "GetRepositoryCatalogData",
            json!({ "repositoryName": missing }),
            "RepositoryNotFoundException",
            &case,
        )
        .await;
    }

    // `registryId` naming this registry is accepted; naming another account's public
    // registry is refused, because this emulator serves exactly one.
    ok(
        &router,
        "PutRepositoryCatalogData",
        json!({
            "repositoryName": REPO,
            "registryId": REGISTRY_ID,
            "catalogData": { "description": "explicit registry" },
        }),
        "this registry by id",
    )
    .await;
    for foreign in ["999999999999", "123456789012"] {
        let case = format!("foreign registry {foreign:?}");
        refused(
            &router,
            "PutRepositoryCatalogData",
            json!({
                "repositoryName": REPO,
                "registryId": foreign,
                "catalogData": { "description": "another account's registry" },
            }),
            "InvalidParameterException",
            &case,
        )
        .await;
        refused(
            &router,
            "GetRepositoryCatalogData",
            json!({ "repositoryName": REPO, "registryId": foreign }),
            "InvalidParameterException",
            &case,
        )
        .await;
    }

    // The refused calls changed nothing: the record is still what the accepted put left.
    let got = ok(
        &router,
        "GetRepositoryCatalogData",
        json!({ "repositoryName": REPO, "registryId": REGISTRY_ID }),
        "record after the refusals",
    )
    .await;
    assert_eq!(
        got["catalogData"],
        json!({ "description": "explicit registry" }),
        "expected the refused calls to leave the stored record alone, got {}",
        got["catalogData"]
    );
}

#[tokio::test]
async fn the_published_catalog_limits_are_enforced_on_each_bound() {
    let router = service();
    create(&router, REPO).await;

    // Every length bound `RepositoryCatalogDataInput`'s members publish: the maximum
    // itself is accepted and one past it is refused. A `logoImageBlob` is sent as raw
    // bytes, so its bound is counted in bytes.
    let text_limits: [(&str, usize); 3] = [
        ("description", 1024),
        ("aboutText", 25600),
        ("usageText", 25600),
    ];
    for (member, max) in text_limits {
        for (len, accepted) in [(0usize, true), (max, true), (max + 1, false)] {
            let case = format!("{member} of {len} characters");
            let body = json!({
                "repositoryName": REPO,
                "catalogData": { member: "x".repeat(len) },
            });
            if accepted {
                let put = ok(&router, "PutRepositoryCatalogData", body, &case).await;
                assert_eq!(
                    put["catalogData"][member],
                    Value::String("x".repeat(len)),
                    "case {case:?}: expected the text stored verbatim, got {}",
                    put["catalogData"]
                );
            } else {
                refused(
                    &router,
                    "PutRepositoryCatalogData",
                    body,
                    "InvalidParameterException",
                    &case,
                )
                .await;
            }
        }
    }

    // `ArchitectureList` and `OperatingSystemList` are 0..50 lists of 1..50 strings.
    for member in ["architectures", "operatingSystems"] {
        for (count, accepted) in [(0usize, true), (50, true), (51, false)] {
            let case = format!("{member} with {count} entries");
            let entries: Vec<String> = (0..count).map(|i| format!("entry-{i}")).collect();
            let body = json!({
                "repositoryName": REPO,
                "catalogData": { member: entries.clone() },
            });
            if accepted {
                let put = ok(&router, "PutRepositoryCatalogData", body, &case).await;
                assert_eq!(
                    put["catalogData"][member],
                    json!(entries),
                    "case {case:?}: expected the list stored verbatim, got {}",
                    put["catalogData"]
                );
            } else {
                refused(
                    &router,
                    "PutRepositoryCatalogData",
                    body,
                    "InvalidParameterException",
                    &case,
                )
                .await;
            }
        }
        for (entry_len, accepted) in [(0usize, false), (1, true), (50, true), (51, false)] {
            let case = format!("{member} entry of {entry_len} characters");
            let body = json!({
                "repositoryName": REPO,
                "catalogData": { member: [ "y".repeat(entry_len) ] },
            });
            if accepted {
                ok(&router, "PutRepositoryCatalogData", body, &case).await;
            } else {
                refused(
                    &router,
                    "PutRepositoryCatalogData",
                    body,
                    "InvalidParameterException",
                    &case,
                )
                .await;
            }
        }
    }

    // `LogoImageBlob` is 0..512000 bytes.
    for (len, accepted) in [(0usize, true), (512_000, true), (512_001, false)] {
        let case = format!("logoImageBlob of {len} bytes");
        let payload = vec![3u8; len];
        let body = json!({
            "repositoryName": REPO,
            "catalogData": { "logoImageBlob": blob_member(&payload) },
        });
        if accepted {
            let put = ok(&router, "PutRepositoryCatalogData", body, &case).await;
            assert_eq!(
                put["catalogData"]["logoUrl"],
                Value::String(logo_url(REPO, &payload)),
                "case {case:?}: expected the payload's digest in the logo URL, got {}",
                put["catalogData"]
            );
        } else {
            refused(
                &router,
                "PutRepositoryCatalogData",
                body,
                "InvalidParameterException",
                &case,
            )
            .await;
        }
    }
}

#[tokio::test]
async fn the_last_write_wins_and_a_recreated_repository_starts_clean() {
    let router = service();
    create(&router, REPO).await;

    // A sequence of writers, each supplying a whole document: the record a reader sees
    // is the one the last writer supplied, whatever the earlier ones held.
    let writers = [
        json!({ "description": "first", "architectures": ["x86-64"] }),
        json!({ "aboutText": "second", "operatingSystems": ["Linux"] }),
        json!({ "description": "third", "usageText": "docker pull" }),
    ];
    let mut last = Value::Null;
    for (index, document) in writers.iter().enumerate() {
        last = ok(
            &router,
            "PutRepositoryCatalogData",
            json!({ "repositoryName": REPO, "catalogData": document }),
            &format!("writer {index}"),
        )
        .await["catalogData"]
            .clone();
    }
    assert_eq!(
        last,
        json!({
            "description": "third",
            "usageText": "docker pull",
        }),
        "expected the last writer's document to be the record, got {last}"
    );
    let got = ok(
        &router,
        "GetRepositoryCatalogData",
        json!({ "repositoryName": REPO }),
        "after the last writer",
    )
    .await;
    assert_eq!(
        got["catalogData"], last,
        "expected the reader to report the last writer's record, got {}",
        got["catalogData"]
    );

    // Deleting the repository takes its catalog record with it, so a repository
    // recreated under the same name starts with an empty record rather than the old one.
    ok(
        &router,
        "DeleteRepository",
        json!({ "repositoryName": REPO }),
        "delete before recreate",
    )
    .await;
    refused(
        &router,
        "GetRepositoryCatalogData",
        json!({ "repositoryName": REPO }),
        "RepositoryNotFoundException",
        "deleted repository",
    )
    .await;
    create(&router, REPO).await;
    let recreated = ok(
        &router,
        "GetRepositoryCatalogData",
        json!({ "repositoryName": REPO }),
        "recreated repository",
    )
    .await;
    assert_eq!(
        recreated["catalogData"],
        json!({}),
        "expected a recreated repository to start with an empty catalog record, got {}",
        recreated["catalogData"]
    );
}

#[tokio::test]
async fn a_put_without_catalog_data_is_refused_before_the_handler() {
    let router = service();
    create(&router, REPO).await;

    // `catalogData` is `@required`, so a request that omits it never reaches the
    // handler: the boundary refuses it as this service's one client-input error.
    refused(
        &router,
        "PutRepositoryCatalogData",
        json!({ "repositoryName": REPO }),
        "InvalidParameterException",
        "no catalogData member",
    )
    .await;

    // And the record is untouched by that refusal.
    let (status, body) = call(
        &router,
        "GetRepositoryCatalogData",
        json!({ "repositoryName": REPO }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"no catalogData member\": expected the repository to still be readable, \
         got {status} with {body}"
    );
    assert_eq!(
        body["catalogData"],
        json!({}),
        "case \"no catalogData member\": expected the untouched empty record, got {}",
        body["catalogData"]
    );
}
