// SPDX-License-Identifier: CC0-1.0

//! The OTHER half of the reserved tag-key rule: the tags AWS sets from inside itself.
//!
//! RULE: a tag key in the reserved `aws:` namespace is AWS's own — so a request a CLIENT
//! makes may not write one (`tests/all/tag_key_rules.rs` pins that half), while the service
//! that owns such a tag sets it through no public API at all. In this emulator one service
//! reaches another only over the in-process callback, so a request that arrives on that path
//! may carry a reserved key, and every later read of the pipe reports it beside the client's
//! own tags. That is what lets a stack's `AWS::Pipes::Pipe` carry the three
//! `aws:cloudformation:*` tags CloudFormation records on every resource it provisions.
//!
//! The rows differ along what the rule generalises over: the reserved NAMESPACE the key sits
//! in (the stack provenance a CloudFormation deployment stamps, and another service's own
//! namespace), the case the prefix is spelled in, the bare prefix as the whole key, a key
//! that merely CONTAINS the prefix without being in the namespace, and a client's own key.
//! Every row is driven on both paths, so each one pins the refusal and the admission with
//! one expectation.
//!
//! Every case drives real HTTP through the production front door
//! (`MultiServiceRouter::dispatch`), reusing the harness `pipe_lifecycle` owns; the
//! in-process path is the one the framework's callback transport stamps
//! (`smithy_server_runtime::CallbackRecursionDepth`), which is a request EXTENSION and
//! therefore cannot arrive from a socket.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::{CallbackRecursionDepth, MultiServiceRouter};

use crate::pipe_lifecycle::{describe, minimal_body, router, Reply, REGION};
use crate::tag_key_rules::RESERVED_PREFIX_SENTENCE;

/// The code the tagging layer refuses a broken tag rule with.
const TAG_RULE_CODE: &str = "BadRequestException";

/// The source and target every row's pipe connects. Two Amazon SQS queues, which need no
/// `SourceParameters`: this suite is about the tag keys, not the configuration.
const SOURCE: &str = "arn:aws:sqs:us-west-2:000000000000:tag-origin-source";
const TARGET: &str = "arn:aws:sqs:us-west-2:000000000000:tag-origin-target";

/// Which path a request arrives on, which is what decides whether a reserved tag key is
/// admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Path {
    /// A client's own request, arriving from a socket: it carries no in-process stamp.
    Caller,
    /// A request another emulated service made in-process, which the framework's callback
    /// transport stamps.
    InProcess,
}

/// `CreatePipe` for `name` with `body`, dispatched on `path`.
///
/// The same request either way but for the stamp: an in-process call carries the
/// [`CallbackRecursionDepth`] extension the callback transport puts on it, which is the one
/// thing about a request a client cannot choose for itself.
async fn create_on(
    router: &MultiServiceRouter,
    path: Path,
    name: &str,
    body: &Value,
) -> Reply {
    let host = format!("pipes.{REGION}.amazonaws.com");
    let mut request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/v1/pipes/{name}"))
        .header("host", host)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDPIPESTEST/20260101/{REGION}/pipes/aws4_request, \
                 SignedHeaders=host, Signature=0"
            ),
        );
    if path == Path::InProcess {
        request = request.extension(CallbackRecursionDepth(1));
    }
    let payload = bytes::Bytes::from(serde_json::to_vec(body).expect("serialize the body"));
    let response = router
        .dispatch(request.body(payload).expect("build the request"))
        .await;
    let status = response.status().as_u16();
    let error_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply {
        status,
        error_code,
        body,
    }
}

/// One row: a tag key, and whether it is in the namespace AWS keeps for itself.
struct Row {
    case: &'static str,
    key: &'static str,
    /// Whether the key is in the reserved namespace, so a client's request may not name it
    /// and an in-process one may.
    reserved: bool,
}

/// The rows the rule generalises over.
///
/// The three stack-provenance keys are the ones CloudFormation records on every resource it
/// provisions; the Service Catalog key is another service's own namespace. The last three
/// rows are the boundary: the bare prefix is the namespace, the prefix in another case is
/// still the namespace, and a key that merely contains `aws:` is not in it at all.
const ROWS: &[Row] = &[
    Row {
        case: "the stack a resource belongs to",
        key: "aws:cloudformation:stack-name",
        reserved: true,
    },
    Row {
        case: "that stack's id",
        key: "aws:cloudformation:stack-id",
        reserved: true,
    },
    Row {
        case: "the resource's logical id in the template",
        key: "aws:cloudformation:logical-id",
        reserved: true,
    },
    Row {
        case: "another service's own namespace",
        key: "aws:servicecatalog:provisioningPrincipalArn",
        reserved: true,
    },
    Row {
        case: "the bare reserved prefix as the whole key",
        key: "aws:",
        reserved: true,
    },
    Row {
        case: "the reserved prefix in mixed case",
        key: "Aws:Cloudformation:Stack-Name",
        reserved: true,
    },
    Row {
        case: "a key that merely contains the prefix",
        key: "not-aws:prefixed",
        reserved: false,
    },
    Row {
        case: "a client's own key",
        key: "env",
        reserved: false,
    },
];

/// The value every row writes: the rule is about the KEY, so one value serves every row.
const VALUE: &str = "probe-value";

/// A key a client owns, written beside the row's key so each case also shows the other half
/// of the request travelled.
const CLIENT_KEY: &str = "team";
const CLIENT_VALUE: &str = "platform";

/// The `Tags` a pipe reports, as the map a client reads it as.
fn reported_tags(reply: &Reply) -> BTreeMap<String, String> {
    reply
        .member("Tags")
        .as_object()
        .map(|tags| {
            tags.iter()
                .map(|(key, value)| {
                    (key.clone(), value.as_str().unwrap_or_default().to_string())
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `CreatePipe$Tags`: a reserved key is refused on a client's create — which then creates
/// nothing — and stored on an in-process one, where `DescribePipe` reports it beside the
/// client's own tag.
#[tokio::test]
async fn a_create_takes_a_reserved_tag_key_only_on_the_in_process_path() {
    for row in ROWS {
        let case = row.case;
        for path in [Path::Caller, Path::InProcess] {
            let admitted = !row.reserved || path == Path::InProcess;
            let router = router();
            let name = "tag-origin-pipe";
            let mut body = minimal_body(SOURCE, TARGET);
            body.as_object_mut()
                .expect("the create body is an object")
                .insert(
                    "Tags".to_string(),
                    json!({ row.key: VALUE, CLIENT_KEY: CLIENT_VALUE }),
                );

            let created = create_on(&router, path, name, &body).await;
            if admitted {
                assert_eq!(
                    created.status, 200,
                    "case {case:?} on {path:?}: CreatePipe must store the key, got {} {}",
                    created.status, created.body
                );
                let described = describe(&router, name).await;
                assert_eq!(
                    described.status, 200,
                    "case {case:?} on {path:?}: the pipe must be readable, got {} {}",
                    described.status, described.body
                );
                let expected: BTreeMap<String, String> = [
                    (row.key.to_string(), VALUE.to_string()),
                    (CLIENT_KEY.to_string(), CLIENT_VALUE.to_string()),
                ]
                .into_iter()
                .collect();
                let reported = reported_tags(&described);
                assert_eq!(
                    reported, expected,
                    "case {case:?} on {path:?}: the pipe must carry the whole set, got \
                     {reported:?}"
                );
            } else {
                assert_eq!(
                    (created.status, created.error_code.as_str()),
                    (400, TAG_RULE_CODE),
                    "case {case:?} on {path:?}: CreatePipe must refuse the key, got {} {}",
                    created.status, created.body
                );
                assert_eq!(
                    created.message(),
                    RESERVED_PREFIX_SENTENCE,
                    "case {case:?} on {path:?}: the refusal must be the documented sentence, \
                     got {}",
                    created.body
                );
                let gone = describe(&router, name).await;
                assert_eq!(
                    (gone.status, gone.error_code.as_str()),
                    (404, "NotFoundException"),
                    "case {case:?} on {path:?}: a refused create must create nothing, got {} \
                     {}",
                    gone.status, gone.body
                );
            }
        }
    }
}
