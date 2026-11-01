// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `id` a MicroVM image carries: the identity the service
//! mints with the record and answers for it from then on.
//!
//! RULE: every response that reports a stored MicroVM IMAGE record carries that
//! record's own `id` — a lowercase hyphenated 8-4-4-4-12 version-4 UUID, minted once
//! when the image is created, unique across the images an account holds and
//! byte-identical in every later answer about that image (the read, every listing row,
//! an update that mints a further version, and an idempotent repeat of the create).
//!
//! The member is RECORDED rather than modelled: `CreateMicrovmImageResponse`,
//! `GetMicrovmImageOutput`, `UpdateMicrovmImageResponse` and `MicrovmImageSummary`
//! declare no `id` (`smithy_query(query="shape",
//! shape="CreateMicrovmImageResponse,GetMicrovmImageOutput,MicrovmImageSummary",
//! model_path="services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json")`),
//! so it is written beside the modelled payload.
//!
//! Each test states that rule and exercises it over a SET of inputs; the values any one
//! row happens to carry are examples, not the specification. The wire harness
//! (`emulator`, `call`, `image_body`, …) is the one `microvm_lifecycle` owns.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::{
    managed_base_image_arn, MANAGED_BASE_IMAGE_IDS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::microvm_image_and_instance_paths::get_image;
use crate::microvm_lifecycle::{
    as_uri_value, call, emulator, expected_image_arn, image_body, PAST_THE_SPAN, REGION,
};

/// Whether `value` is the id form the service mints: 36 characters in the lowercase
/// hyphenated `8-4-4-4-12` groups of a version-4 UUID — hexadecimal throughout, the
/// version nibble `4` and the variant nibble one of `8`, `9`, `a`, `b` (RFC 4122, which
/// is the layout `uuid::Builder::from_random_bytes` writes).
fn is_minted_id(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    let lengths: Vec<usize> = groups.iter().map(|group| group.len()).collect();
    if lengths != vec![8, 4, 4, 4, 12] {
        return false;
    }
    let hexadecimal = groups
        .iter()
        .all(|group| group.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
    let version = groups[2].starts_with('4');
    let variant = groups[3].starts_with(['8', '9', 'a', 'b']);
    hexadecimal && version && variant
}

/// The `id` of a response body, failing the case when it is absent or not a string.
fn id_of(body: &Value, case: &str) -> String {
    let id = body["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: the answer carries an id, got {body}"));
    assert!(
        is_minted_id(id),
        "case {case}: expected a lowercase 8-4-4-4-12 UUID, got {id:?}"
    );
    id.to_string()
}

/// `GET /2025-09-09/microvm-images`, with whatever `query` adds to the query string.
async fn list_images(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    let uri = match query.is_empty() {
        true => "/2025-09-09/microvm-images".to_string(),
        false => format!("/2025-09-09/microvm-images?{query}"),
    };
    call(router, "GET", &uri, None).await
}

/// The `id` the listing reports for the row named `name`, selected by name exactly as a
/// reconciliation sweep would.
async fn listed_id(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let (status, body) = list_images(router, "").await;
    assert_eq!(status, 200, "case {case}: listing got {status} {body}");
    let row = body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: items is a list, got {body}"))
        .iter()
        .find(|item| item["name"].as_str() == Some(name))
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: the listing carries a row for {name}, got {body}"));
    id_of(&row, case)
}

/// `PUT /2025-09-09/microvm-images/{imageIdentifier}` with a configuration the update
/// accepts, which mints a further version of the image. The base image is the one the
/// AWS-managed catalogue publishes (`state::MANAGED_BASE_IMAGE_IDS`), in the region the
/// harness signs for.
async fn update_image(router: &MultiServiceRouter, identifier: &str) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/2025-09-09/microvm-images/{}", as_uri_value(identifier)),
        Some(json!({
            "baseImageArn": managed_base_image_arn(REGION, MANAGED_BASE_IMAGE_IDS[0]),
            "buildRoleArn": "arn:aws:iam::123456789012:role/microvm-build",
            "codeArtifact": {"uri": "s3://artifacts/app-2.zip"},
        })),
    )
    .await
}

/// RULE: the create answers the image's own id, and the READ, the LISTING ROW and an
/// UPDATE of that image all answer the SAME id — so a client that keyed its records on
/// the create can validate the identity and join a listing back to what it created.
#[tokio::test]
async fn one_image_reports_one_id_through_every_projection() {
    let router = emulator();

    // Rows span what the create can differ in — the name's published bounds and
    // punctuation (`ImageName` is len 1..64, pattern ^[a-zA-Z0-9-_]+$), which optional
    // members the request carries, and whether it is tagged — because none of them is
    // what the id is derived from.
    let longest = format!("ident-{}", "x".repeat(58));
    let rows: Vec<(&str, String, Value)> = vec![
        ("required members only", "a".to_string(), json!({})),
        ("name at the 64-character maximum", longest, json!({})),
        (
            "underscores, hyphens and digits",
            "id_0-1_2".to_string(),
            json!({"description": "a described image"}),
        ),
        (
            "every optional member",
            "fully-identified".to_string(),
            json!({
                "baseImageVersion": "7",
                "description": "fully specified",
                "logging": {"cloudWatch": {"logGroup": "/aws/microvm/demo"}},
                "egressNetworkConnectors": ["connector-egress"],
                "cpuConfigurations": [{"architecture": "ARM_64"}],
                "resources": [{"minimumMemoryInMiB": 2048}],
                "additionalOsCapabilities": ["ALL"],
                "hooks": {"port": 8080},
                "environmentVariables": {"STAGE": "prod"},
                "tags": {"team": "platform"},
            }),
        ),
    ];

    let mut created: Vec<(String, String)> = Vec::new();
    for (case, name, extra) in rows {
        let (status, body) = call(
            &router,
            "POST",
            "/2025-09-09/microvm-images",
            Some(image_body(&name, extra)),
        )
        .await;
        assert_eq!(status, 201, "case {case}: create got {status} {body}");
        created.push((name, id_of(&body, case)));
    }

    // Every id is distinct: the id is the image's identity, not a value derived from
    // anything two images can share.
    let distinct: BTreeSet<&String> = created.iter().map(|(_, id)| id).collect();
    assert_eq!(
        distinct.len(),
        created.len(),
        "each image gets its own id, got {created:?}"
    );

    // One asynchronous build span covers every row, so it is waited out once: the read
    // below therefore also crosses the clock-owed CREATING -> CREATED transition, which
    // rewrites the record and must not rewrite its identity.
    tokio::time::sleep(PAST_THE_SPAN).await;

    for (name, minted) in &created {
        // Both forms `MicrovmImageIdentifier` accepts resolve to the one record, so
        // both answer the one id.
        for identifier in [name.clone(), expected_image_arn(name)] {
            let case = format!("read {identifier:?}");
            let (status, body) = get_image(&router, &identifier).await;
            assert_eq!(status, 200, "case {case}: got {status} {body}");
            assert_eq!(
                &id_of(&body, &case),
                minted,
                "case {case}: expected the id the create answered ({minted}), got {body}"
            );
        }

        let case = format!("listing row for {name:?}");
        assert_eq!(
            &listed_id(&router, name, &case).await,
            minted,
            "case {case}: expected the id the create answered ({minted})"
        );

        let case = format!("update {name:?}");
        let (status, body) = update_image(&router, name).await;
        assert_eq!(status, 200, "case {case}: got {status} {body}");
        assert_eq!(
            &id_of(&body, &case),
            minted,
            "case {case}: an update mints a version, not an identity, got {body}"
        );
    }

    // And the identity outlives the version the update is building: the read after it
    // still answers the id the create minted.
    tokio::time::sleep(PAST_THE_SPAN).await;
    for (name, minted) in &created {
        let case = format!("read after the update {name:?}");
        let (status, body) = get_image(&router, name).await;
        assert_eq!(status, 200, "case {case}: got {status} {body}");
        assert_eq!(
            &id_of(&body, &case),
            minted,
            "case {case}: expected the id the create answered ({minted}), got {body}"
        );
    }
}

/// RULE: the id belongs to the stored record, so an `@idempotencyToken` repeat of a
/// create — which answers the record that token already minted rather than minting a
/// second one — answers that record's id, and a read is a pure report: repeating it
/// changes nothing a later caller sees.
#[tokio::test]
async fn a_replayed_create_and_a_repeated_read_answer_the_same_id() {
    let router = emulator();

    // Rows differ in the client token's form, which is an opaque string the service
    // only compares: a plain word, a UUID-shaped token, and one carrying punctuation.
    let rows: Vec<(&str, &str, &str)> = vec![
        ("a plain token", "replayed-plain", "token-1"),
        (
            "a uuid-shaped token",
            "replayed-uuid",
            "6f1a0e1e-0000-4000-8000-00000000abcd",
        ),
        ("a token with punctuation", "replayed-punct", "a.b_c-d:1/2"),
    ];

    for (case, name, token) in rows {
        let body = image_body(name, json!({"clientToken": token}));
        let (status, first) = call(&router, "POST", "/2025-09-09/microvm-images", Some(body.clone()))
            .await;
        assert_eq!(status, 201, "case {case}: create got {status} {first}");
        let minted = id_of(&first, case);

        let (status, replayed) =
            call(&router, "POST", "/2025-09-09/microvm-images", Some(body)).await;
        assert_eq!(status, 201, "case {case}: replay got {status} {replayed}");
        assert_eq!(
            id_of(&replayed, case),
            minted,
            "case {case}: a replay answers the record the token minted, got {replayed}"
        );

        let (status, read) = get_image(&router, name).await;
        assert_eq!(status, 200, "case {case}: read got {status} {read}");
        let (status, again) = get_image(&router, name).await;
        assert_eq!(status, 200, "case {case}: re-read got {status} {again}");
        assert_eq!(
            read, again,
            "case {case}: a read reports and changes nothing, got {again}"
        );
        assert_eq!(
            id_of(&again, case),
            minted,
            "case {case}: expected the id the create answered ({minted}), got {again}"
        );
    }
}

/// RULE: the id is reported for every row the listing serves, however the pages are
/// cut — a reconciliation sweep that walks the listing can match every row to the
/// resource it created.
#[tokio::test]
async fn every_listing_row_of_every_page_carries_its_images_id() {
    let router = emulator();

    // Five images, so the walk below cuts the listing at sizes that leave one, several
    // and no further pages.
    let mut minted: Vec<(String, String)> = Vec::new();
    for index in 0..5 {
        let name = format!("page-row-{index}");
        let (status, body) = call(
            &router,
            "POST",
            "/2025-09-09/microvm-images",
            Some(image_body(&name, json!({}))),
        )
        .await;
        assert_eq!(status, 201, "create {name}: got {status} {body}");
        minted.push((name, id_of(&body, "create")));
    }

    // Rows differ in the page size, which is what the walk generalises over.
    for size in [None, Some(1), Some(2), Some(5), Some(50)] {
        let case = format!("maxResults={size:?}");
        let mut seen: Vec<(String, String)> = Vec::new();
        let mut token: Option<String> = None;

        for _ in 0..=minted.len() {
            let mut query: Vec<String> = Vec::new();
            if let Some(size) = size {
                query.push(format!("maxResults={size}"));
            }
            if let Some(token) = &token {
                query.push(format!("nextToken={token}"));
            }
            let (status, body) = list_images(&router, &query.join("&")).await;
            assert_eq!(status, 200, "case {case}: got {status} {body}");
            for row in body["items"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: items is a list, got {body}"))
            {
                let name = row["name"].as_str().unwrap_or_default().to_string();
                seen.push((name, id_of(row, &case)));
            }
            token = body["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }

        let mut expected = minted.clone();
        expected.sort();
        seen.sort();
        assert_eq!(
            seen, expected,
            "case {case}: every row carries the id its image was created with"
        );
    }
}
