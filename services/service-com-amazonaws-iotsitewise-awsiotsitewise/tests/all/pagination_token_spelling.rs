// SPDX-License-Identifier: CC0-1.0

//! A `nextToken` is honoured only as the TEXT the service issued it as.
//!
//! The rule this suite pins: a paginated operation resumes an enumeration only for a
//! `nextToken` it issued itself, so text that merely decodes to the same resume point
//! — another spelling of the token's hex, or another spelling of a number inside it —
//! is a `nextToken` the operation never issued and is refused with the modelled
//! `InvalidRequestException` (HTTP 400), carrying no page at all. Every token of this
//! service is written one way, so a decoder that read back any other spelling would
//! make the refusal depend on which spelling a caller (or an intermediary rewriting a
//! query string) happened to send.
//!
//! The rows span this service's token FORMS rather than one listing: the resume key a
//! listing pages by, the instant a time-ranged read stops at, and the entry-and-offset
//! cursor a batch read carries — and, for each, the ways a spelling can differ
//! (letter case, where in the token the case differs, a padded number).
//!
//! The harness (`router`, `call`, `create_model`, …) lives in the `asset_lifecycle`
//! suite and the clock anchoring in `support`; this file reuses both rather than
//! keeping a copy. Its own hex is hand-rolled, as an INDEPENDENT oracle: re-using the
//! `hex` crate the handlers encode with would let one encoding bug agree with itself.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::asset_lifecycle::{
    call, create_asset, create_model, error_code, hex_token, property_id, router,
};

/// The epoch this suite's samples are measured from; `super::support` maps it into
/// the window `BatchPutAssetPropertyValue` ingests.
const ROW_EPOCH: i64 = 0;

/// A table second as the absolute `timeInSeconds` the wire carries.
fn anchored(seconds: i64) -> i64 {
    super::support::anchored(ROW_EPOCH, seconds)
}

/// How one paginated read of this service is driven, so the rows below can span the
/// token forms without a copy of the page walk per operation.
enum Paged {
    /// A read whose `nextToken` travels as an `@httpQuery` member: `uri` already
    /// carries every other member of the request.
    Query {
        uri: String,
        items: &'static str,
    },
    /// A read whose `nextToken` travels in the request body.
    Body {
        uri: String,
        body: Value,
        items: &'static str,
    },
}

impl Paged {
    /// The output member the answered page arrives in.
    fn items(&self) -> &'static str {
        match self {
            Self::Query { items, .. } | Self::Body { items, .. } => items,
        }
    }
}

/// Ask for one page of `paged`, resuming at `token` when there is one.
async fn page(router: &MultiServiceRouter, paged: &Paged, token: Option<&str>) -> (u16, Value) {
    match paged {
        Paged::Query { uri, .. } => {
            let uri = match token {
                Some(token) => format!("{uri}&nextToken={token}"),
                None => uri.clone(),
            };
            call(router, "GET", &uri, None).await
        }
        Paged::Body { uri, body, .. } => {
            let mut body = body.clone();
            if let Some(token) = token {
                body["nextToken"] = json!(token);
            }
            call(router, "POST", uri, Some(body)).await
        }
    }
}

/// The text a token spells, decoded by hand so this suite does not share the
/// handlers' codec.
fn token_payload(case: &str, token: &str) -> String {
    assert!(
        token.len().is_multiple_of(2),
        "case {case:?}: an issued token is whole bytes of hex, got {token:?}"
    );
    let bytes: Vec<u8> = token
        .as_bytes()
        .chunks(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair)
                .unwrap_or_else(|_| panic!("case {case:?}: {token:?} is not ASCII hex"));
            u8::from_str_radix(pair, 16)
                .unwrap_or_else(|_| panic!("case {case:?}: {token:?} is not hex"))
        })
        .collect();
    String::from_utf8(bytes)
        .unwrap_or_else(|_| panic!("case {case:?}: {token:?} does not spell text"))
}

/// Spellings of `token` that no encoder of this service writes.
///
/// Hex is written with lowercase digits, so every variant here decodes to the resume
/// point `token` names while being text the service never issued.
fn respellings(case: &str, token: &str) -> Vec<(String, String)> {
    let letters: Vec<usize> = token
        .char_indices()
        .filter(|(_, letter)| letter.is_ascii_alphabetic())
        .map(|(at, _)| at)
        .collect();
    assert!(
        !letters.is_empty(),
        "case {case:?}: the issued token {token:?} carries no hex letter, so this case \
         cannot tell one spelling from another"
    );
    let upper_at = |at: usize| {
        let mut spelled = token.to_string();
        spelled.replace_range(at..at + 1, &token[at..at + 1].to_uppercase());
        spelled
    };
    let mut rows = vec![
        (
            "every hex letter upper-cased".to_string(),
            token.to_uppercase(),
        ),
        (
            "the first hex letter upper-cased".to_string(),
            upper_at(letters[0]),
        ),
        (
            "the last hex letter upper-cased".to_string(),
            upper_at(*letters.last().expect("a hex letter")),
        ),
    ];
    let mut seen = std::collections::BTreeSet::new();
    rows.retain(|(_, spelled)| spelled != token && seen.insert(spelled.clone()));
    assert!(
        !rows.is_empty(),
        "case {case:?}: no other spelling of {token:?} was built"
    );
    rows
}

/// `token` with the first number inside it zero-padded: the same resume point, in
/// text no encoder of this service writes.
fn zero_padded_number(case: &str, token: &str) -> String {
    let payload = token_payload(case, token);
    let at = payload
        .find(|character: char| character.is_ascii_digit())
        .unwrap_or_else(|| panic!("case {case:?}: {payload:?} holds no number to pad"));
    hex_token(&format!("{}0{}", &payload[..at], &payload[at..]))
}

/// Drive `paged` to its second page and assert only the text the operation issued
/// resumes it.
///
/// `numeric_payload` says whether the token's resume point is a number, which adds
/// the zero-padded row: a listing paging by a resource id has no number of its own to
/// re-spell.
async fn only_the_issued_spelling_resumes(
    router: &MultiServiceRouter,
    case: &str,
    paged: &Paged,
    numeric_payload: bool,
) {
    let (status, first) = page(router, paged, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: minting a token: got {status} {first}"
    );
    let token = first["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a nextToken, got {first}"))
        .to_string();
    assert_eq!(
        token,
        token.to_lowercase(),
        "case {case:?}: an issued token is lowercase hex, got {token:?}"
    );

    // The text the service issued resumes the enumeration.
    let (status, resumed) = page(router, paged, Some(&token)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the issued token: got {status} {resumed}"
    );
    assert!(
        resumed[paged.items()]
            .as_array()
            .is_some_and(|page| !page.is_empty()),
        "case {case:?}: the issued token must be answered a page of {}, got {resumed}",
        paged.items()
    );

    let mut spellings = respellings(case, &token);
    if numeric_payload {
        spellings.push((
            "a number inside the token zero-padded".to_string(),
            zero_padded_number(case, &token),
        ));
    }
    for (row, spelled) in spellings {
        assert_ne!(
            spelled, token,
            "case {case:?}, {row}: this row must differ from the issued token"
        );
        let (status, body) = page(router, paged, Some(&spelled)).await;
        assert_eq!(
            status, 400,
            "case {case:?}, {row}: expected 400 for {spelled:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}, {row}: expected InvalidRequestException for {spelled:?}, got {body}"
        );
        assert!(
            body.get(paged.items()).is_none(),
            "case {case:?}, {row}: a refused token must serve no page, got {body}"
        );
    }
}

/// Seed `samples` — table seconds — on one asset property.
async fn write_samples(
    router: &MultiServiceRouter,
    asset_id: &str,
    property_id: &str,
    samples: &[i64],
) {
    let values: Vec<Value> = samples
        .iter()
        .map(|seconds| {
            json!({
                "value": {"doubleValue": *seconds as f64 / 10.0},
                "timestamp": {"timeInSeconds": anchored(*seconds), "offsetInNanos": 0},
            })
        })
        .collect();
    let (status, body) = call(
        router,
        "POST",
        "/properties",
        Some(json!({"entries": [{
            "entryId": "seed",
            "assetId": asset_id,
            "propertyId": property_id,
            "propertyValues": values,
        }]})),
    )
    .await;
    assert_eq!(status, 200, "writing samples: got {status} {body}");
    assert_eq!(
        body["errorEntries"].as_array().map(Vec::len),
        Some(0),
        "writing samples: every entry must be accepted, got {body}"
    );
}

#[tokio::test]
async fn a_paginated_read_resumes_only_for_the_token_text_it_issued() {
    let router = router();
    let model_id = create_model(
        &router,
        "SpellingFan",
        json!([{"name": "Rpm", "dataType": "DOUBLE", "unit": "rpm", "type": {"measurement": {}}}]),
    )
    .await;
    // A second model, so the model listing has more than one page too.
    let _ = create_model(&router, "SpellingValve", json!([])).await;
    let mut assets = Vec::new();
    for name in ["SpellingFan1", "SpellingFan2", "SpellingFan3"] {
        assets.push(create_asset(&router, name, &model_id).await);
    }

    let asset_id = assets.first().expect("a created asset").clone();
    let (status, asset) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset: got {status} {asset}");
    let rpm = property_id(&asset["assetProperties"], "Rpm");
    write_samples(&router, &asset_id, &rpm, &[100, 200, 300]).await;

    // One row per token FORM this service mints.
    let rows: Vec<(&str, Paged, bool)> = vec![
        (
            // The resume key of a listing: paged through the one shared page walk every
            // listing of this service uses.
            "ListAssets",
            Paged::Query {
                uri: format!("/assets?assetModelId={model_id}&maxResults=1"),
                items: "assetSummaries",
            },
            false,
        ),
        (
            "ListAssetModels",
            Paged::Query {
                uri: "/asset-models?maxResults=1".to_string(),
                items: "assetModelSummaries",
            },
            false,
        ),
        (
            // The instant a time-ranged read stopped at, beside the enumeration it was
            // counted in.
            "GetAssetPropertyValueHistory",
            Paged::Query {
                uri: format!("/properties/history?assetId={asset_id}&propertyId={rpm}&maxResults=1"),
                items: "assetPropertyValueHistory",
            },
            true,
        ),
        (
            // The entry-and-offset cursor a batch read pages across its entries with.
            "BatchGetAssetPropertyValueHistory",
            Paged::Body {
                uri: "/properties/batch/history".to_string(),
                body: json!({
                    "entries": [{"entryId": "e0", "assetId": asset_id, "propertyId": rpm}],
                    "maxResults": 1,
                }),
                items: "successEntries",
            },
            true,
        ),
    ];
    for (case, paged, numeric_payload) in rows {
        only_the_issued_spelling_resumes(&router, case, &paged, numeric_payload).await;
    }
}
