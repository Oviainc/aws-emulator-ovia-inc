// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS the refusal of a page cursor it never issued, over HTTP.
//!
//! THE RULE: a `NextToken` that is not a position in the listing being read is refused
//! with HTTP 400, the input error that listing models, and AWS's one fixed sentence for
//! the bad-cursor class — `"Invalid next token."` — which carries no part of the request
//! and in particular never echoes the rejected token. One sentence for every listing of
//! this service and for every form a bad cursor can take, because it is one condition.
//!
//! The table spans what the rule generalises over:
//!
//! * the LISTING — one reached by `@httpQuery` on a `GET` and one reached through a
//!   `POST` body, one that models `BadRequestException` for the refusal and one that
//!   models `InvalidNextTokenException`, so the sentence is shown to belong to the
//!   condition rather than to an operation or to an error shape;
//! * the FORM of the bad cursor — base64 of text no listing ever minted, a cursor
//!   ANOTHER listing of this service did mint, text that is not base64 at all, a bare
//!   resource name, and the empty string.
//!
//! Every row is answered over the real wire path (`MultiServiceRouter::dispatch` with
//! this service registered), and each listing's positive control — the same call with no
//! cursor at all — is asserted beside the refusals, so a listing that refused everything
//! could not pass.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    encode_tagged_page_token, CONTACT_LISTS_PAGE_TOKEN_TAG,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call in this suite is signed for.
const HOME: &str = "us-east-1";

/// AWS's fixed sentence for a cursor this service never issued.
const INVALID_NEXT_TOKEN: &str = "Invalid next token.";

/// One answered request.
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// The modelled error code the response names, or an empty string on success.
    fn error_code(&self) -> String {
        let raw = self
            .error_type
            .as_deref()
            .or_else(|| self.body.get("__type").and_then(Value::as_str))
            .unwrap_or_default();
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// The `message` member of the error body, or an empty string when there is none.
    fn message(&self) -> String {
        self.body
            .get("message")
            .or_else(|| self.body.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulated service behind its router.
struct Ses {
    router: MultiServiceRouter,
}

impl Ses {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = match &body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://email.{HOME}.amazonaws.com{path}"))
            .header("host", format!("email.{HOME}.amazonaws.com"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/{HOME}/ses/\
                     aws4_request, SignedHeaders=host, Signature=00"
                ),
            )
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }
}

/// Where a listing carries its `NextToken`: the two wire forms this service's listings
/// take.
#[derive(Debug, Clone, Copy)]
enum Cursor {
    /// `NextToken` is bound to `@httpQuery` on a `GET`.
    Query,
    /// `NextToken` is a member of the `POST` body.
    Body,
}

/// One listing under test: how it is addressed, where its cursor goes, and the error
/// shape it models for a cursor that is not a position in it.
struct Listing {
    case: &'static str,
    method: &'static str,
    path: &'static str,
    cursor: Cursor,
    error_code: &'static str,
}

/// The listings the table spans, each with the error code its own model declares for the
/// refusal (`smithy_query(query="operation",operation="…")`).
fn listings() -> Vec<Listing> {
    vec![
        Listing {
            case: "ListDedicatedIpPools",
            method: "GET",
            path: "/v2/email/dedicated-ip-pools",
            cursor: Cursor::Query,
            error_code: "BadRequestException",
        },
        Listing {
            case: "GetDedicatedIps",
            method: "GET",
            path: "/v2/email/dedicated-ips",
            cursor: Cursor::Query,
            error_code: "BadRequestException",
        },
        Listing {
            case: "ListConfigurationSets",
            method: "POST",
            path: "/v2/email/list-configuration-sets",
            cursor: Cursor::Body,
            error_code: "BadRequestException",
        },
        Listing {
            case: "ListExportJobs",
            method: "POST",
            path: "/v2/email/list-export-jobs",
            cursor: Cursor::Body,
            error_code: "BadRequestException",
        },
        Listing {
            case: "ListSuppressedDestinations",
            method: "GET",
            path: "/v2/email/suppression/addresses",
            cursor: Cursor::Query,
            error_code: "InvalidNextTokenException",
        },
    ]
}

/// The forms a cursor this service never issued can arrive in: base64 of text no listing
/// mints, a cursor another listing of this service DID mint, text that is not base64,
/// a bare resource name, and nothing at all.
fn forged_cursors() -> Vec<(&'static str, String)> {
    vec![
        (
            "base64 of text no listing ever minted",
            "c2ltMnJlYWwtZm9yZ2VkLW5leHQtdG9rZW4=".to_string(),
        ),
        (
            "a cursor another listing of this service minted",
            encode_tagged_page_token(CONTACT_LISTS_PAGE_TOKEN_TAG, "a-list"),
        ),
        ("text that is not base64 at all", "!!not-a-token!!".to_string()),
        (
            "a bare resource name",
            ::aws_smithy_types::base64::encode("a-pool"),
        ),
        ("the empty string", String::new()),
    ]
}

#[tokio::test]
async fn a_cursor_this_service_never_issued_is_refused_with_aws_sentence() {
    let ses = Ses::new();
    for listing in listings() {
        // The positive control first: the same listing, with no cursor at all, is served.
        let served = match listing.cursor {
            Cursor::Query => ses.call(listing.method, listing.path, None).await,
            Cursor::Body => {
                ses.call(listing.method, listing.path, Some(json!({})))
                    .await
            }
        };
        assert_eq!(
            served.status,
            200,
            "case {:?} with no cursor: expected the listing to be served, got {} {:?}",
            listing.case,
            served.status,
            served.body
        );

        for (form, token) in forged_cursors() {
            let case = format!("{} with {form}", listing.case);
            let refused = match listing.cursor {
                Cursor::Query => {
                    let path = format!(
                        "{}?NextToken={}",
                        listing.path,
                        query_component(&token)
                    );
                    ses.call(listing.method, &path, None).await
                }
                Cursor::Body => {
                    ses.call(listing.method, listing.path, Some(json!({"NextToken": token})))
                        .await
                }
            };
            assert_eq!(
                (refused.status, refused.error_code().as_str()),
                (400, listing.error_code),
                "case {case:?}: expected 400 {}, got {} {} {:?}",
                listing.error_code,
                refused.status,
                refused.error_code(),
                refused.body
            );
            assert_eq!(
                refused.message(),
                INVALID_NEXT_TOKEN,
                "case {case:?}: expected the refusal to read {INVALID_NEXT_TOKEN:?}, got {:?}",
                refused.message()
            );
            assert!(
                token.is_empty() || !refused.message().contains(&token),
                "case {case:?}: the refusal must not echo the rejected cursor, got {:?}",
                refused.message()
            );
        }
    }
}

/// Percent-encode one query-string component, leaving only RFC 3986's unreserved set, so
/// a forged cursor's `+`, `/`, `=` and `!` reach the handler as sent.
fn query_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}
