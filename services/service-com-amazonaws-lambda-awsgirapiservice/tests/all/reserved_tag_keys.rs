// SPDX-License-Identifier: CC0-1.0

//! The tag keys Lambda stores a tag under, over every operation that stores one.
//!
//! The rule under test: a request that would store a tag whose key is empty, lies in the
//! `aws:` namespace AWS keeps for its own tags, or is named more than once in that one
//! request is refused with the modelled `InvalidParameterValueException` (HTTP 400) and
//! stores nothing — so a later `ListTags` or `GetFunction` reports exactly the tag set
//! the resource held before the call. The refusal states the three conditions and nothing
//! of the request: the sentence is the same whichever key and whichever operation failed.
//!
//! The rows differ along what the rule generalises over: which condition the key breaks
//! (empty, reserved, reserved in another letter case, the bare prefix), whether the bad
//! key travels alone or beside keys that are fine, and which operation carries it — the
//! tag writer (`TagResource`) and the creates that persist a `Tags` map of their own
//! (`CreateFunction`, `CreateEventSourceMapping`, `CreateCodeSigningConfig`,
//! `CreateCapacityProvider`). Rows that are
//! fine are in the same table, so the rule is shown to refuse these keys and no others.
//!
//! A duplicate key cannot be put on the wire through a JSON object (the parse keeps one
//! entry per name), so that clause of the rule is exercised where the keys are judged:
//! `state::tests::a_tag_key_is_storable_unless_it_is_empty_reserved_or_repeated`.

use std::collections::BTreeMap;
use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Map, Value};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// Where `TagResource` and `ListTags` are bound (`/2017-03-31/tags/{Resource}`).
const TAGS_URI: &str = "/2017-03-31/tags";

/// Where `CreateCodeSigningConfig` is bound.
const CODE_SIGNING_CONFIGS_URI: &str = "/2020-04-22/code-signing-configs";

/// Where `CreateCapacityProvider` is bound.
const CAPACITY_PROVIDERS_URI: &str = "/2025-11-30/capacity-providers";

/// Where `CreateEventSourceMapping` and `ListEventSourceMappings` are bound.
const MAPPINGS_URI: &str = "/2015-03-31/event-source-mappings";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in
/// `us-east-1`, which is what routes a request here and what region the ARNs carry.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names.
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
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
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
            .unwrap_or_else(|| String::from_utf8_lossy(&self.body).to_string())
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

/// One service mounted on its own engine, reached the way the production router reaches
/// it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// The tags of a row, as a JSON object: every key takes a value that is fine, so the only
/// thing a row varies is the KEY.
fn tags(keys: &[&str]) -> Value {
    let mut map = Map::new();
    for key in keys {
        map.insert((*key).to_string(), json!("a-value"));
    }
    Value::Object(map)
}

/// The tag set a resource holds, through `ListTags`.
async fn held_tags(case: &str, router: &MultiServiceRouter, arn: &str) -> BTreeMap<String, String> {
    let answer = call(router, "GET", &format!("{TAGS_URI}/{arn}"), &Value::Null).await;
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
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
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

/// One row: the keys the request carries, and whether Lambda stores a tag under them.
struct Case {
    name: &'static str,
    keys: &'static [&'static str],
    storable: bool,
}

/// The rows. The acceptable keys cover the punctuation the modelled `TagKey` pattern
/// admits (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, from
/// `smithy_query(query="shape", shape="TagKey")`), including a `:` away from the front,
/// which is what makes the reserved namespace a PREFIX rule rather than a ban on the
/// character.
const CASES: &[Case] = &[
    Case {
        name: "one ordinary key",
        keys: &["Project"],
        storable: true,
    },
    Case {
        name: "several ordinary keys",
        keys: &["Project", "Owner", "cost.centre"],
        storable: true,
    },
    Case {
        name: "a key carrying the punctuation the key pattern admits",
        keys: &["team/payments", "owner@example", "rev=2", "a-b_c"],
        storable: true,
    },
    Case {
        name: "a key that merely starts with the letters of the reserved prefix",
        keys: &["awsome"],
        storable: true,
    },
    Case {
        name: "a key carrying the reserved prefix further in",
        keys: &["cost:aws:share"],
        storable: true,
    },
    Case {
        name: "a key in the reserved namespace",
        keys: &["aws:cloudformation:stack-name"],
        storable: false,
    },
    Case {
        name: "a key in the reserved namespace written in another letter case",
        keys: &["AWS:CloudFormation:stack-name"],
        storable: false,
    },
    Case {
        name: "the reserved prefix alone",
        keys: &["aws:"],
        storable: false,
    },
    Case {
        name: "an empty key",
        keys: &[""],
        storable: false,
    },
    Case {
        name: "a reserved key beside keys that are fine",
        keys: &["Project", "aws:created-by", "Owner"],
        storable: false,
    },
    Case {
        name: "an empty key beside a key that is fine",
        keys: &["Project", ""],
        storable: false,
    },
];

/// Every writer, so each row is put to all of them.
const WRITERS: &[Writer] = &[
    Writer::TagResource,
    Writer::CreateFunction,
    Writer::CreateEventSourceMapping,
    Writer::CreateCodeSigningConfig,
    Writer::CreateCapacityProvider,
];

/// Drive one writer with the tags of a row, and answer what it said and the ARN of the
/// resource whose tag set the row must then be checked against.
async fn write_tags(
    router: &MultiServiceRouter,
    writer: Writer,
    resource: &str,
    tag_map: &Value,
) -> (Answer, String) {
    match writer {
        Writer::TagResource => {
            let arn = create_function(router, resource).await;
            let answer = call(
                router,
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
            // The mapping's event source: an Amazon SQS queue of this account and
            // region, which is what `CreateEventSourceMapping` admits as an
            // `EventSourceArn`.
            let function = format!("{resource}-fn");
            create_function(router, &function).await;
            let answer = call(
                router,
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

/// The status a writer answers when it accepts the request.
fn accepted_status(writer: Writer) -> u16 {
    match writer {
        // `TagResource`'s modelled success is 204 with an empty body.
        Writer::TagResource => 204,
        Writer::CreateFunction | Writer::CreateCodeSigningConfig => 201,
        Writer::CreateEventSourceMapping | Writer::CreateCapacityProvider => 202,
    }
}

#[tokio::test]
async fn a_tag_is_stored_only_under_a_key_lambda_stores_tags_under() {
    for writer in WRITERS {
        for (index, case) in CASES.iter().enumerate() {
            // One account per row, so nothing a neighbouring row created can be mistaken
            // for what this one stored.
            let engine = Arc::new(engine::InMemoryEngine::new());
            let router = router_on(&engine);
            let label = format!("{writer:?} / {}", case.name);
            let resource = format!("tagkeys-{index}");

            let (answer, arn) = write_tags(&router, *writer, &resource, &tags(case.keys)).await;

            if case.storable {
                assert_eq!(
                    answer.status,
                    accepted_status(*writer),
                    "case {label:?}: expected {} for {:?}, got {} {}",
                    accepted_status(*writer),
                    case.keys,
                    answer.status,
                    answer.text()
                );
                let held = held_tags(&label, &router, &arn).await;
                let expected: BTreeMap<String, String> = case
                    .keys
                    .iter()
                    .map(|key| ((*key).to_string(), "a-value".to_string()))
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
                "case {label:?}: expected 400 for {:?}, got {} {}",
                case.keys,
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

            // The refusal stored nothing: a create left no resource behind to carry tags,
            // and a tag write left the resource with the tag set it already had — none.
            // Each writer is asked through the reader that would report what it creates,
            // rather than through the tag ARN alone, because a refused create minted no
            // id to address.
            match writer {
                Writer::TagResource => {
                    let held = held_tags(&label, &router, &arn).await;
                    assert!(
                        held.is_empty(),
                        "case {label:?}: a refused tag write must store nothing, got {held:?}"
                    );
                }
                Writer::CreateFunction => {
                    let read = call(
                        &router,
                        "GET",
                        &format!("{FUNCTIONS_URI}/{resource}"),
                        &Value::Null,
                    )
                    .await;
                    assert_eq!(
                        read.status,
                        404,
                        "case {label:?}: a refused create must leave no function behind, got {} {}",
                        read.status,
                        read.text()
                    );
                }
                Writer::CreateEventSourceMapping => {
                    let listed = call(&router, "GET", MAPPINGS_URI, &Value::Null).await;
                    let mappings = listed
                        .json()
                        .get("EventSourceMappings")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    assert!(
                        mappings.is_empty(),
                        "case {label:?}: a refused create must leave no mapping behind, got {}",
                        listed.text()
                    );
                }
                Writer::CreateCodeSigningConfig => {
                    let listed = call(&router, "GET", CODE_SIGNING_CONFIGS_URI, &Value::Null).await;
                    let configs = listed
                        .json()
                        .get("CodeSigningConfigs")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    assert!(
                        configs.is_empty(),
                        "case {label:?}: a refused create must leave no configuration behind, \
                         got {}",
                        listed.text()
                    );
                }
                Writer::CreateCapacityProvider => {
                    let read = call(
                        &router,
                        "GET",
                        &format!("{CAPACITY_PROVIDERS_URI}/{resource}"),
                        &Value::Null,
                    )
                    .await;
                    assert_eq!(
                        read.status,
                        404,
                        "case {label:?}: a refused create must leave no provider behind, got {} {}",
                        read.status,
                        read.text()
                    );
                }
            }
        }
    }
}

/// A tag write that is refused leaves the tags the resource ALREADY held exactly as they
/// were: the request is refused whole rather than key by key.
#[tokio::test]
async fn a_refused_tag_write_leaves_the_stored_tags_alone() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let arn = create_function(&router, "tagkeys-merge").await;

    let first = call(
        &router,
        "POST",
        &format!("{TAGS_URI}/{arn}"),
        &json!({ "Tags": { "Project": "payments", "Owner": "team" } }),
    )
    .await;
    assert_eq!(
        first.status,
        204,
        "the first tag write must succeed, got {} {}",
        first.status,
        first.text()
    );

    // Each refused write names a key that would otherwise merge into the stored set, so a
    // guard applied per key rather than per request would show up as a partial write.
    for keys in [
        vec!["Stage", "aws:created-by"],
        vec!["Stage", ""],
        vec!["AWS:Stage"],
    ] {
        let refused = call(
            &router,
            "POST",
            &format!("{TAGS_URI}/{arn}"),
            &json!({ "Tags": tags(&keys) }),
        )
        .await;
        assert_eq!(
            refused.status,
            400,
            "case {keys:?}: expected 400, got {} {}",
            refused.status,
            refused.text()
        );
        let held = held_tags(&format!("{keys:?}"), &router, &arn).await;
        let expected: BTreeMap<String, String> = [
            ("Owner".to_string(), "team".to_string()),
            ("Project".to_string(), "payments".to_string()),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            held, expected,
            "case {keys:?}: a refused write must leave {expected:?} untouched, got {held:?}"
        );
    }
}
