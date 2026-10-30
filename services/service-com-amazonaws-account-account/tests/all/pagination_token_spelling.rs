// SPDX-License-Identifier: CC0-1.0

//! `ListRegions` pagination-token fidelity, end to end over HTTP.
//!
//! The rule under test: a `NextToken` resumes a walk only when it is byte-for-byte
//! the token the operation itself minted for that resume point; every other
//! spelling — including one whose bytes decode to a Region the catalog serves — is
//! a token the operation never issued and is refused with the modelled
//! `ValidationException` (HTTP 400), never served as a resume point.
//!
//! The tables below differ along what the rule generalises over: the resume point
//! (tokens minted at several positions of the list, by several page sizes) and the
//! spelling handed back (as issued, case-shifted whole, case-shifted at one digit,
//! padded, prefixed, truncated). The case-flip table is driven exhaustively over
//! every digit of a minted token, so no position is privileged.
//!
//! Every case goes through the router, so it exercises routing, deserialization,
//! the boundary constraint walk, the handler and serialization: the same path a
//! client takes.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_account_account::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request to
/// the `account` service (its sigv4 signing name).
const SIGNED_FOR_US_EAST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/account/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// `ValidationException`'s `reason` for an input member that failed validation,
/// from `smithy_query(query="shape", shape="ValidationExceptionReason")`.
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The refusal message, or the empty string when the body carries none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    /// The `RegionName`s the response lists, in the order it carries them.
    fn region_names(&self) -> Vec<String> {
        self.body["Regions"]
            .as_array()
            .unwrap_or_else(|| panic!("a ListRegions success carries Regions, got {:?}", self.body))
            .iter()
            .map(|region| {
                region["RegionName"]
                    .as_str()
                    .unwrap_or_else(|| panic!("every Region carries a RegionName: {region:?}"))
                    .to_string()
            })
            .collect()
    }

    /// The `NextToken` the response carries, if any.
    fn next_token(&self) -> Option<String> {
        self.body["NextToken"].as_str().map(str::to_string)
    }
}

async fn list_regions(router: &MultiServiceRouter, request: Value) -> Answer {
    let payload = request.to_string();
    let http_request = ::http::Request::builder()
        .method("POST")
        .uri("/listRegions")
        .header("host", "account.us-east-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_US_EAST_1)
        .header("content-type", "application/json")
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "POST /listRegions answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// Every Region the caller's own account reports, in one page.
async fn whole_list(router: &MultiServiceRouter) -> Vec<String> {
    let answer = list_regions(router, json!({})).await;
    assert_eq!(
        answer.status, 200,
        "case \"the whole list\": expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.next_token().is_none(),
        "case \"the whole list\": the unpaged list is not truncated, got {:?}",
        answer.body
    );
    answer.region_names()
}

/// The token a page of `page_size` Regions ends with, and the Regions it carried.
async fn minted_token(router: &MultiServiceRouter, page_size: usize) -> (String, Vec<String>) {
    let answer = list_regions(router, json!({"MaxResults": page_size})).await;
    assert_eq!(
        answer.status, 200,
        "case \"MaxResults={page_size}\": expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    let token = answer.next_token().unwrap_or_else(|| {
        panic!(
            "case \"MaxResults={page_size}\": a page shorter than the catalog is truncated, \
             got {:?}",
            answer.body
        )
    });
    (token, answer.region_names())
}

/// Assert that `token` is refused as a token this operation never issued, with the
/// modelled payload.
fn assert_refused(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: a NextToken the operation never issued is refused, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {:?}",
        answer.body
    );
    assert!(
        !answer.message().is_empty(),
        "case {case:?}: the refusal carries a message, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.body["reason"].as_str().unwrap_or_default(),
        FIELD_VALIDATION_FAILED,
        "case {case:?}: expected reason {FIELD_VALIDATION_FAILED}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.body["fieldList"][0]["name"].as_str().unwrap_or_default(),
        "NextToken",
        "case {case:?}: fieldList must name NextToken, got {:?}",
        answer.body
    );
}

/// The page sizes the tables below mint tokens with: resume points near the start,
/// inside and near the end of the list, so no one position is privileged.
fn sampled_page_sizes(list_len: usize) -> Vec<usize> {
    let mut sizes = vec![1, 2, 3, list_len / 2, list_len.saturating_sub(1)];
    sizes.retain(|size| *size >= 1 && *size < list_len);
    sizes.sort_unstable();
    sizes.dedup();
    sizes
}

/// The token the operation mints resumes the walk exactly after the page it was
/// returned with, and it is the only spelling of itself that does: the same bytes
/// spelled with any other case are refused.
#[tokio::test]
async fn only_the_spelling_the_operation_minted_resumes_a_walk() {
    let router = emulator();
    let everything = whole_list(&router).await;
    assert!(
        everything.len() >= 4,
        "the catalog must be long enough to page, got {:?}",
        everything
    );

    for page_size in sampled_page_sizes(everything.len()) {
        let (token, first_page) = minted_token(&router, page_size).await;
        assert_eq!(
            first_page,
            everything[..page_size].to_vec(),
            "case \"MaxResults={page_size}\": expected the first {page_size} Regions, got {:?}",
            first_page
        );

        // The spelling the operation mints: lower-case hex, so a token handed back
        // unchanged is the only one that can round-trip.
        assert_eq!(
            token,
            token.to_lowercase(),
            "case \"MaxResults={page_size}\": the minted token is lower case, got {token:?}"
        );

        // As issued: it resumes after the page it came with.
        let resumed = list_regions(&router, json!({"NextToken": token})).await;
        assert_eq!(
            resumed.status, 200,
            "case \"MaxResults={page_size}, token as issued\": expected 200, got {} with {:?}",
            resumed.status, resumed.body
        );
        assert_eq!(
            resumed.region_names(),
            everything[page_size..].to_vec(),
            "case \"MaxResults={page_size}, token as issued\": expected the Regions after the \
             page, got {:?}",
            resumed.region_names()
        );

        // Every other spelling of the SAME resume point, one case flip at a time.
        for (index, digit) in token.char_indices() {
            let flipped = digit.to_ascii_uppercase();
            if flipped == digit {
                continue;
            }
            let mut respelled = token.clone();
            respelled.replace_range(index..index + digit.len_utf8(), &flipped.to_string());
            let case = format!("MaxResults={page_size}, digit {index} upper-cased");
            let answer = list_regions(&router, json!({"NextToken": respelled})).await;
            assert_refused(&case, &answer);
        }
    }
}

/// A token a client assembled or mangled by hand is refused whatever form it takes:
/// a case-shifted whole token, a padded or prefixed one, a truncated one, a Region
/// code in the clear, an encoding of a Region nobody serves, and the empty token.
#[tokio::test]
async fn a_next_token_the_operation_never_issued_is_refused() {
    let router = emulator();
    let everything = whole_list(&router).await;
    let (token, _) = minted_token(&router, 2).await;

    // The hex of a Region code the catalog does not serve, built the way a client
    // fabricating a token would: `hex::encode` is the spelling the operation mints,
    // so this case differs only in the Region it names.
    let nowhere = ::hex::encode("zz-nowhere-1");
    // The resume point the minted token names, in the clear.
    let in_the_clear = everything[1].clone();
    // Every other alphabetic hex digit upper-cased: a mixed-case spelling of the
    // same bytes. Only the letters can change case, so the alternation runs over
    // them rather than over every position.
    let mut letters_seen = 0usize;
    let alternating: String = token
        .chars()
        .map(|digit| match digit.is_ascii_alphabetic() {
            true => {
                letters_seen += 1;
                match letters_seen % 2 {
                    1 => digit.to_ascii_uppercase(),
                    _ => digit,
                }
            }
            false => digit,
        })
        .collect();

    let cases: Vec<(String, String)> = vec![
        ("every digit upper-cased".to_string(), token.to_uppercase()),
        ("every other letter upper-cased".to_string(), alternating),
        ("a trailing newline".to_string(), format!("{token}\n")),
        ("a leading space".to_string(), format!(" {token}")),
        ("a 0x prefix".to_string(), format!("0x{token}")),
        (
            "the last digit dropped".to_string(),
            token[..token.len() - 1].to_string(),
        ),
        ("a Region code in the clear".to_string(), in_the_clear),
        ("a Region nobody serves".to_string(), nowhere),
        ("a word".to_string(), "next-page".to_string()),
        ("the empty token".to_string(), String::new()),
    ];

    for (case, candidate) in cases {
        assert_ne!(
            candidate, token,
            "case {case:?}: the case must differ from the token the operation minted"
        );
        let answer = list_regions(&router, json!({"NextToken": candidate})).await;
        assert_refused(&case, &answer);
    }

    // The minted token still works after all of that: the refusals changed nothing.
    let resumed = list_regions(&router, json!({"NextToken": token})).await;
    assert_eq!(
        resumed.status, 200,
        "case \"the issued token after the refusals\": expected 200, got {} with {:?}",
        resumed.status, resumed.body
    );
    assert_eq!(
        resumed.region_names(),
        everything[2..].to_vec(),
        "case \"the issued token after the refusals\": expected the Regions after the page, \
         got {:?}",
        resumed.region_names()
    );
}
