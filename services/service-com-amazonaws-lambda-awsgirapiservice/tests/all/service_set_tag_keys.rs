// SPDX-License-Identifier: CC0-1.0

//! The OTHER half of the reserved tag-key rule: the tags AWS sets from inside itself.
//!
//! RULE: a tag key in the reserved `aws:` namespace is AWS's own — "Don't use the `aws:`
//! prefix in tag keys, because it is reserved for Amazon Web Services use" binds the
//! request a CLIENT makes (`reserved_tag_keys.rs` pins that half), while the AWS services
//! that provision a resource on a caller's behalf are what SET such a tag, through no
//! public API. In this emulator one service reaches another only over the in-process
//! callback, so a request that arrives on that path stores a reserved key, and every later
//! read of the resource reports it beside the client's own tags: `ListTags` on the ARN the
//! resource is addressed by answers with the whole set. The conditions that are not about
//! a caller — an empty key names nothing — refuse the request whichever path it arrived on.
//!
//! The rows differ along what the rule generalises over: the reserved NAMESPACE the key
//! sits in (the stack provenance a CloudFormation deployment stamps, and another service's
//! own namespace, which the issue did not cite), the case the prefix is spelled in, a key
//! that merely CONTAINS the prefix without being in the namespace, a key that is no
//! caller's to send on any path, the operation the key arrives on (`CreateFunction$Tags`,
//! `TagResource$Tags`, `CreateEventSourceMapping$Tags`, `CreateCodeSigningConfig$Tags`,
//! `CreateCapacityProvider$Tags`) and the KIND of Lambda resource tagged.
//!
//! Every case drives real HTTP through the production front door
//! (`MultiServiceRouter::dispatch`), so what is asserted is what a client sees; the
//! in-process path is the one the framework's callback transport stamps
//! (`smithy_server_runtime::CallbackRecursionDepth`), which is a request EXTENSION and
//! therefore cannot arrive from a socket.

use std::collections::BTreeMap;
use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Map, Value};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::{CallbackRecursionDepth, MultiServiceRouter};

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// Where `TagResource` and `ListTags` are bound (`/2017-03-31/tags/{Resource}`).
const TAGS_URI: &str = "/2017-03-31/tags";

/// Where `CreateCodeSigningConfig` is bound.
const CODE_SIGNING_CONFIGS_URI: &str = "/2020-04-22/code-signing-configs";

/// Where `CreateCapacityProvider` is bound.
const CAPACITY_PROVIDERS_URI: &str = "/2025-11-30/capacity-providers";

/// Where `CreateEventSourceMapping` is bound.
const MAPPINGS_URI: &str = "/2015-03-31/event-source-mappings";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`,
/// which is what routes a request here and which Region the ARNs carry.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The Region the scope above names.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries.
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// A well-formed capacity-provider operator role ARN.
const OPERATOR_ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-capacity";

/// Lambda's sentence for a request carrying a tag key it will not store. Written out here
/// rather than read from the crate, so a change to the wording fails this test.
const INVALID_TAGS_MESSAGE: &str = "One or more of your tags had a problem, please ensure \
     none of your tag names start with \"aws:\", none are empty, and there are no \
     duplicate tags";

/// The value every tag below carries: a row varies the KEY and nothing else.
const TAG_VALUE: &str = "a-value";

/// Which path a request arrives on, which is what decides whether a reserved tag key is
/// stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Path {
    /// A client's own request, arriving from a socket: it carries no in-process stamp.
    Caller,
    /// A request another emulated service made in-process while provisioning a resource,
    /// which the framework's callback transport stamps.
    InProcess,
}

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// The wire error code (`__type`, or the `x-amzn-errortype` header).
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                self.headers
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| self.text())
    }

    /// The refusal's `message`.
    fn message(&self) -> String {
        self.json()
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| self.text())
    }

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request on `path`.
///
/// The same request either way but for the stamp: an in-process call carries the
/// [`CallbackRecursionDepth`] extension the callback transport puts on it, which is the one
/// thing about a request a client cannot choose for itself.
async fn call(
    router: &MultiServiceRouter,
    path: Path,
    method: &str,
    uri: &str,
    body: &Value,
) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let mut request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json");
    if path == Path::InProcess {
        request = request.extension(CallbackRecursionDepth(1));
    }
    let response = router
        .dispatch(
            request
                .body(bytes::Bytes::from(payload))
                .expect("the test request must build"),
        )
        .await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// The tags of a row, as a JSON object.
fn tags(keys: &[&str]) -> Value {
    let mut map = Map::new();
    for key in keys {
        map.insert((*key).to_string(), json!(TAG_VALUE));
    }
    Value::Object(map)
}

/// The tag set a resource holds, read back through `ListTags` as a CLIENT reads it — the
/// reserved namespace is about WRITING a tag, so a client sees every tag the resource
/// carries.
async fn held_tags(case: &str, router: &MultiServiceRouter, arn: &str) -> BTreeMap<String, String> {
    let answer = call(
        router,
        Path::Caller,
        "GET",
        &format!("{TAGS_URI}/{arn}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: ListTags on {arn:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
    answer
        .json()
        .get("Tags")
        .and_then(Value::as_object)
        .map(|held| {
            held.iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// A deployment package: only its length and digest are ever reported.
fn package() -> Vec<u8> {
    (0..48u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect()
}

/// Create a function to tag, and answer its ARN.
async fn create_function(router: &MultiServiceRouter, name: &str) -> String {
    let answer = call(
        router,
        Path::Caller,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(package()) },
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// Every operation that stores a tag map, named by what it is asked to do with one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Writer {
    /// `TagResource` on a function that already exists.
    TagResource,
    /// `CreateFunction`, whose `Tags` are "A list of tags to apply to the function".
    CreateFunction,
    /// `CreateEventSourceMapping`, whose `Tags` are applied to the new mapping.
    CreateEventSourceMapping,
    /// `CreateCodeSigningConfig`, whose `Tags` are applied to the new configuration.
    CreateCodeSigningConfig,
    /// `CreateCapacityProvider`, whose `Tags` are applied to the new provider.
    CreateCapacityProvider,
}

/// Every writer, so each row is put to all of them.
const WRITERS: &[Writer] = &[
    Writer::TagResource,
    Writer::CreateFunction,
    Writer::CreateEventSourceMapping,
    Writer::CreateCodeSigningConfig,
    Writer::CreateCapacityProvider,
];

/// The status a writer answers when it accepts the request.
fn accepted_status(writer: Writer) -> u16 {
    match writer {
        // `TagResource`'s modelled success is 204 with an empty body.
        Writer::TagResource => 204,
        Writer::CreateFunction | Writer::CreateCodeSigningConfig => 201,
        Writer::CreateEventSourceMapping | Writer::CreateCapacityProvider => 202,
    }
}

/// One row: a tag key, and what it is about that key that the rule turns on.
struct Row {
    case: &'static str,
    key: &'static str,
    /// Whether the key lies in the namespace AWS keeps for itself, so a client's request
    /// may not write it and an in-process one may.
    reserved: bool,
    /// Whether the key is one NO request may store, whichever path it arrived on — the
    /// clauses of the rule that are not about a caller.
    never_storable: bool,
}

/// The rows the rule generalises over.
///
/// The three stack-provenance keys are the ones CloudFormation records on every resource it
/// provisions (`aws:cloudformation:stack-name`, `aws:cloudformation:stack-id` and
/// `aws:cloudformation:logical-id`, this emulator's
/// `service-com-amazonaws-cloudformation-cloudformation`
/// `state::provisioning::{STACK_NAME_TAG, STACK_ID_TAG, LOGICAL_ID_TAG}`); the Service
/// Catalog key is another service's own namespace, which the issue did not cite. The last
/// rows are the boundaries: the bare prefix is the namespace, the prefix in another case is
/// still the namespace, a key that merely contains `aws:` is not in it at all, and an empty
/// key is nobody's to store.
const ROWS: &[Row] = &[
    Row {
        case: "the stack a resource belongs to",
        key: "aws:cloudformation:stack-name",
        reserved: true,
        never_storable: false,
    },
    Row {
        case: "that stack's id",
        key: "aws:cloudformation:stack-id",
        reserved: true,
        never_storable: false,
    },
    Row {
        case: "the resource's logical id in the template",
        key: "aws:cloudformation:logical-id",
        reserved: true,
        never_storable: false,
    },
    Row {
        case: "another service's own namespace",
        key: "aws:servicecatalog:provisioningPrincipalArn",
        reserved: true,
        never_storable: false,
    },
    Row {
        case: "the bare reserved prefix as the whole key",
        key: "aws:",
        reserved: true,
        never_storable: false,
    },
    Row {
        case: "the reserved prefix in mixed case",
        key: "Aws:CloudFormation:Stack-Name",
        reserved: true,
        never_storable: false,
    },
    Row {
        case: "a key that merely contains the prefix",
        key: "cost:aws:share",
        reserved: false,
        never_storable: false,
    },
    Row {
        case: "a client's own key",
        key: "Project",
        reserved: false,
        never_storable: false,
    },
    Row {
        case: "an empty key, which names no tag on any path",
        key: "",
        reserved: false,
        never_storable: true,
    },
];

/// Drive one writer with the tags of a row on `path`, and answer what it said and the ARN
/// of the resource whose tag set the row must then be checked against.
async fn write_tags(
    router: &MultiServiceRouter,
    path: Path,
    writer: Writer,
    resource: &str,
    tag_map: &Value,
) -> (Answer, String) {
    match writer {
        Writer::TagResource => {
            let arn = create_function(router, resource).await;
            let answer = call(
                router,
                path,
                "POST",
                &format!("{TAGS_URI}/{arn}"),
                &json!({ "Tags": tag_map }),
            )
            .await;
            (answer, arn)
        }
        Writer::CreateFunction => {
            let answer = call(
                router,
                path,
                "POST",
                FUNCTIONS_URI,
                &json!({
                    "FunctionName": resource,
                    "Role": ROLE,
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Code": {
                        "ZipFile": base64::engine::general_purpose::STANDARD.encode(package()),
                    },
                    "Tags": tag_map,
                }),
            )
            .await;
            let arn = answer
                .json()
                .get("FunctionArn")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{resource}"));
            (answer, arn)
        }
        Writer::CreateEventSourceMapping => {
            // The mapping's event source: an Amazon SQS queue of this account and Region,
            // which is what `CreateEventSourceMapping` admits as an `EventSourceArn`.
            let function = format!("{resource}-fn");
            create_function(router, &function).await;
            let answer = call(
                router,
                path,
                "POST",
                MAPPINGS_URI,
                &json!({
                    "FunctionName": function,
                    "EventSourceArn": format!("arn:aws:sqs:{REGION}:{ACCOUNT}:{resource}"),
                    "Tags": tag_map,
                }),
            )
            .await;
            let arn = answer
                .json()
                .get("EventSourceMappingArn")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_default();
            (answer, arn)
        }
        Writer::CreateCodeSigningConfig => {
            let answer = call(
                router,
                path,
                "POST",
                CODE_SIGNING_CONFIGS_URI,
                &json!({
                    "AllowedPublishers": {
                        "SigningProfileVersionArns": [
                            format!(
                                "arn:aws:signer:{REGION}:{ACCOUNT}:/signing-profiles/\
                                 Profile0/00000000"
                            ),
                        ],
                    },
                    "Tags": tag_map,
                }),
            )
            .await;
            let arn = answer
                .json()
                .pointer("/CodeSigningConfig/CodeSigningConfigArn")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_default();
            (answer, arn)
        }
        Writer::CreateCapacityProvider => {
            let answer = call(
                router,
                path,
                "POST",
                CAPACITY_PROVIDERS_URI,
                &json!({
                    "CapacityProviderName": resource,
                    "VpcConfig": {
                        "SubnetIds": ["subnet-10000000"],
                        "SecurityGroupIds": ["sg-20000000"],
                    },
                    "PermissionsConfig": { "CapacityProviderOperatorRoleArn": OPERATOR_ROLE },
                    "Tags": tag_map,
                }),
            )
            .await;
            let arn = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:capacity-provider:{resource}");
            (answer, arn)
        }
    }
}

#[tokio::test]
async fn a_reserved_tag_key_is_refused_a_caller_and_stored_for_a_service() {
    for writer in WRITERS {
        for path in [Path::Caller, Path::InProcess] {
            for (index, row) in ROWS.iter().enumerate() {
                // One account per case, so nothing a neighbouring case created can be
                // mistaken for what this one stored.
                let engine = Arc::new(engine::InMemoryEngine::new());
                let router = router_on(&engine);
                let label = format!("{writer:?} / {path:?} / {}", row.case);
                let resource = format!("systemtags-{index}");

                let (answer, arn) =
                    write_tags(&router, path, *writer, &resource, &tags(&[row.key])).await;

                // The key is stored where it breaks no clause of the rule that holds for
                // this path: the reserved namespace binds a caller alone.
                let stored = !row.never_storable
                    && (!row.reserved || path == Path::InProcess);

                if stored {
                    assert_eq!(
                        answer.status,
                        accepted_status(*writer),
                        "case {label:?}: expected {} for key {:?}, got {} {}",
                        accepted_status(*writer),
                        row.key,
                        answer.status,
                        answer.text()
                    );
                    let held = held_tags(&label, &router, &arn).await;
                    let expected: BTreeMap<String, String> =
                        [(row.key.to_string(), TAG_VALUE.to_string())]
                            .into_iter()
                            .collect();
                    assert_eq!(
                        held, expected,
                        "case {label:?}: expected the resource to hold {expected:?}, got {held:?}"
                    );
                    continue;
                }

                assert_eq!(
                    answer.status,
                    400,
                    "case {label:?}: expected 400 for key {:?}, got {} {}",
                    row.key,
                    answer.status,
                    answer.text()
                );
                assert!(
                    answer
                        .error_code()
                        .contains("InvalidParameterValueException"),
                    "case {label:?}: expected InvalidParameterValueException, got {:?}",
                    answer.error_code()
                );
                assert_eq!(
                    answer.message(),
                    INVALID_TAGS_MESSAGE,
                    "case {label:?}: expected the reserved-tag sentence, got {:?}",
                    answer.message()
                );
            }
        }
    }
}

/// A reserved key an in-process request writes MERGES into the tag set a client already
/// gave the resource, and neither displaces the other: the two halves of the rule share one
/// tag map, which is what `ListTags` reports.
#[tokio::test]
async fn a_service_set_tag_merges_with_the_tags_a_caller_wrote() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let arn = create_function(&router, "systemtags-merge").await;

    let by_the_caller = call(
        &router,
        Path::Caller,
        "POST",
        &format!("{TAGS_URI}/{arn}"),
        &json!({ "Tags": { "Project": "payments", "Owner": "team" } }),
    )
    .await;
    assert_eq!(
        by_the_caller.status,
        204,
        "the caller's tag write must succeed, got {} {}",
        by_the_caller.status,
        by_the_caller.text()
    );

    let by_the_service = call(
        &router,
        Path::InProcess,
        "POST",
        &format!("{TAGS_URI}/{arn}"),
        &json!({
            "Tags": {
                "aws:cloudformation:stack-name": "tagged-functions",
                "aws:cloudformation:logical-id": "TaggedFunction",
            },
        }),
    )
    .await;
    assert_eq!(
        by_the_service.status,
        204,
        "the provisioning service's tag write must succeed, got {} {}",
        by_the_service.status,
        by_the_service.text()
    );

    let held = held_tags("a service tag beside a caller's", &router, &arn).await;
    let expected: BTreeMap<String, String> = [
        ("Owner", "team"),
        ("Project", "payments"),
        ("aws:cloudformation:logical-id", "TaggedFunction"),
        ("aws:cloudformation:stack-name", "tagged-functions"),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value.to_string()))
    .collect();
    assert_eq!(
        held, expected,
        "case \"a service tag beside a caller's\": expected {expected:?}, got {held:?}"
    );
}
