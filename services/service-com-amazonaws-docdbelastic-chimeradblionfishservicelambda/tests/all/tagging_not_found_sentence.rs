// SPDX-License-Identifier: CC0-1.0

//! How the ARN-ADDRESSED tagging surface words a `resourceArn` this emulator holds no
//! resource under.
//!
//! The rule these suites assert: **an operation that addresses its resource generically,
//! by the ARN of any resource class this service owns, refuses an ARN it holds nothing
//! under with 404 `ResourceNotFoundException` carrying the GENERIC fixed sentence
//! `Resource was not found`** — the whole message is fixed text — while the ARN the
//! request named travels in `resourceId` and the class its resource segment names travels
//! in `resourceType`. The three tag operations are exactly that surface: `TagResource`,
//! `UntagResource` and `ListTagsForResource` are all bound to `/tags/{resourceArn}` whose
//! only identifier is an `Arn` (`smithy_query(query="operation",
//! operation="TagResource,UntagResource,ListTagsForResource")`), so none of them is asked
//! for a resource of one named kind. An operation that IS — a cluster's own reader, a
//! snapshot's own reader — keeps its class's own sentence, which the contrast suite below
//! pins so the generic sentence stays scoped to the generic surface.
//!
//! The rule generalises over the operation, over the resource class the ARN names and
//! over the FORM of the identifier, so the table spans all three tag operations and, for
//! each, identifiers differing in which resource segment they spell, their region, their
//! account, the form and width of their resource id, and whether they are an ARN at all.
//! The recorded cases — one `TagResource` and one `UntagResource` against an ARN no
//! resource carries — are two rows of it.
//!
//! The second rule here is about the ENVELOPE rather than the sentence: every failure of
//! this service names its error shape in `x-amzn-errortype` in the QUALIFIED form its wire
//! carries — the shape name, the separator, then the locator of the namespace that declares
//! the shape — where the framework's restJson1 renderers write the bare name. It holds for
//! a modelled refusal an operation answered and for one the framework raised before any
//! handler ran, and a success names no shape at all.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeSet;
use std::sync::Arc;

/// The region the suites sign with.
const REGION: &str = "eu-west-2";

/// The canonical placeholder account every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// The sentence the ARN-addressed tagging surface answers for an ARN holding no
/// resource: recorded AWS behavior, the entire message with no variable part.
const TAGGED_RESOURCE_SENTENCE: &str = "Resource was not found";

/// The sentences an operation asked for a resource of ONE named class answers instead.
const CLUSTER_SENTENCE: &str = "Unable to find the cluster";
const SNAPSHOT_SENTENCE: &str = "Unable to find the snapshot";

/// The AWS resource type NAMES this service reports in `resourceType`.
///
/// Source: the modelled resource shapes — `smithy_query(query="shape",
/// shape="Cluster,ClusterSnapshot")` — whose own names are the type names.
const CLUSTER_TYPE: &str = "Cluster";
const SNAPSHOT_TYPE: &str = "ClusterSnapshot";

/// The header a restJson1 client resolves an error shape from.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// What this service's wire appends to an error shape's name in that header: the
/// separator, the service stack's locator root, and the namespace that declares the
/// shape — this service's own (`smithy_query(query="service_info")`: service shape
/// `com.amazonaws.docdbelastic#ChimeraDbLionfishServiceLambda`).
const ERROR_TYPE_LOCATOR: &str = ":http://internal.amazon.com/coral/com.amazonaws.docdbelastic/";

/// The ARN resource segments of those two classes.
const CLUSTER_SEGMENT: &str = "cluster";
const SNAPSHOT_SEGMENT: &str = "cluster-snapshot";

/// A SigV4 `Authorization` header for this service in [`REGION`].
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/docdb-elastic/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// One emulator: the shared engine plus a router carrying only this service.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_docdbelastic_chimeradblionfishservicelambda::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Percent-encode one value the way an AWS SDK encodes a single path segment or one
/// query-parameter value: an ARN carries both `:` and `/`, so nothing outside the
/// unreserved set survives raw.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Send one request and return its status, the `x-amzn-errortype` header the response
/// names its error shape in, and its parsed JSON body (`Value::Null` when the body is
/// empty).
async fn answer(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Option<String>, Value) {
    let payload = body.map(|b| b.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://docdb-elastic.{REGION}.amazonaws.com{path}"))
        .header("host", format!("docdb-elastic.{REGION}.amazonaws.com"))
        .header("authorization", authorization())
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get(ERROR_TYPE_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "{method} {path}: body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, error_type, parsed)
}

/// [`answer`] without the header, for a call whose envelope is asserted on its body alone.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let (status, _, parsed) = answer(router, method, path, body).await;
    (status, parsed)
}

/// The wire error code of an error response (`__type`, bare or namespaced).
fn error_code(body: &Value) -> String {
    let raw = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>");
    raw.rsplit('#').next().unwrap_or(raw).to_string()
}

/// The refusal's `message`, or the empty string.
fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A `CreateCluster` body with every required member and nothing else.
fn minimal_create(name: &str) -> Value {
    json!({
        "clusterName": name,
        "authType": "PLAIN_TEXT",
        "adminUserName": "admin",
        "adminUserPassword": "Secret123!",
        "shardCapacity": 2,
        "shardCount": 1,
    })
}

/// One tagging operation, as it addresses an ARN on the wire.
///
/// `tags` travels in the BODY of `TagResource` and `tagKeys` as repeated `@httpQuery`
/// parameters of `UntagResource`, so each row carries how many of them the call names:
/// the rule holds whatever the call would have written or removed.
struct TagCall {
    operation: &'static str,
    method: &'static str,
    /// The request path for an ARN, query string included.
    path: fn(&str) -> String,
    /// The request body for an ARN, when the operation carries one.
    body: fn() -> Option<Value>,
}

/// All three operations bound to `/tags/{resourceArn}`, each carrying a different number
/// of tags or tag keys.
fn tag_calls() -> Vec<TagCall> {
    vec![
        TagCall {
            operation: "TagResource",
            method: "POST",
            path: |arn| format!("/tags/{}", encode(arn)),
            body: || Some(json!({"tags": {"Project": "lionfish"}})),
        },
        TagCall {
            operation: "TagResource with several tags",
            method: "POST",
            path: |arn| format!("/tags/{}", encode(arn)),
            body: || {
                Some(json!({"tags": {"Project": "lionfish", "Tier": "gold", "Owner": ""}}))
            },
        },
        TagCall {
            operation: "UntagResource",
            method: "DELETE",
            path: |arn| format!("/tags/{}?tagKeys=Project", encode(arn)),
            body: || None,
        },
        TagCall {
            operation: "UntagResource naming several keys",
            method: "DELETE",
            path: |arn| format!("/tags/{}?tagKeys=Project&tagKeys=Tier", encode(arn)),
            body: || None,
        },
        TagCall {
            operation: "ListTagsForResource",
            method: "GET",
            path: |arn| format!("/tags/{}", encode(arn)),
            body: || None,
        },
    ]
}

/// One reader asked for a resource of ONE named class: the route the class's own
/// identifier travels in, and the sentence that class is missed with.
struct TypedReader {
    operation: &'static str,
    path: fn(&str) -> String,
    sentence: &'static str,
}

/// One identifier no resource carries, labelled by what makes it differ from the others,
/// with the resource type name its ARN segment names.
struct Identifier {
    case: &'static str,
    value: String,
    resource_type: &'static str,
}

/// Identifiers differing in resource segment, region, account, resource-id form and
/// width, and whether they are an ARN at all.
///
/// An identifier spelling no segment this service owns addresses its primary taggable
/// resource, the elastic cluster, so its reported type is [`CLUSTER_TYPE`].
fn identifiers() -> Vec<Identifier> {
    vec![
        Identifier {
            case: "a cluster ARN in the signing region",
            value: format!(
                "arn:aws:docdb-elastic:{REGION}:{ACCOUNT}:{CLUSTER_SEGMENT}/11111111-2222-3333-4444-555555555555"
            ),
            resource_type: CLUSTER_TYPE,
        },
        Identifier {
            case: "a cluster ARN in another region",
            value: format!(
                "arn:aws:docdb-elastic:us-west-2:{ACCOUNT}:{CLUSTER_SEGMENT}/00000000-0000-0000-0000-000000000000"
            ),
            resource_type: CLUSTER_TYPE,
        },
        Identifier {
            case: "a cluster ARN in another account",
            value: format!(
                "arn:aws:docdb-elastic:{REGION}:210987654321:{CLUSTER_SEGMENT}/ABCDEF01-2345-6789-abcd-ef0123456789"
            ),
            resource_type: CLUSTER_TYPE,
        },
        Identifier {
            case: "a snapshot ARN in the signing region",
            value: format!(
                "arn:aws:docdb-elastic:{REGION}:{ACCOUNT}:{SNAPSHOT_SEGMENT}/99999999-8888-7777-6666-555555555555"
            ),
            resource_type: SNAPSHOT_TYPE,
        },
        Identifier {
            case: "a snapshot ARN whose resource id is a long word",
            value: format!(
                "arn:aws:docdb-elastic:{REGION}:{ACCOUNT}:{SNAPSHOT_SEGMENT}/{}",
                "z".repeat(120)
            ),
            resource_type: SNAPSHOT_TYPE,
        },
        Identifier {
            case: "an identifier that is not an ARN at all",
            value: "not-an-arn-at-all".to_string(),
            resource_type: CLUSTER_TYPE,
        },
    ]
}

#[tokio::test]
async fn a_tagging_call_on_an_unheld_arn_states_the_generic_resource_sentence() {
    let router = emulator();
    // A tagged cluster DOES exist, so every row below refuses the ARN the request named
    // rather than an empty store.
    let (status, created) = call(
        &router,
        "POST",
        "/cluster",
        Some(minimal_create("present-cluster")),
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture cluster must be created, got {status} with {created}"
    );

    for tag_call in tag_calls() {
        for identifier in identifiers() {
            let case = format!("{} / {}", tag_call.operation, identifier.case);
            let (status, body) = call(
                &router,
                tag_call.method,
                &(tag_call.path)(&identifier.value),
                (tag_call.body)(),
            )
            .await;

            assert_eq!(
                status, 404,
                "case {case:?}: an ARN holding no resource is HTTP 404, got {status} with \
                 {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: expected a ResourceNotFoundException, got {body}"
            );
            assert_eq!(
                message(&body),
                TAGGED_RESOURCE_SENTENCE,
                "case {case:?}: expected the generic sentence {TAGGED_RESOURCE_SENTENCE:?}, \
                 got {:?}",
                message(&body)
            );
            // Fixed text: no part of the refused request comes back in it, and it names
            // no resource class.
            for fragment in [
                identifier.value.as_str(),
                REGION,
                ACCOUNT,
                "arn:aws:docdb-elastic",
                "cluster",
                "snapshot",
            ] {
                assert!(
                    !message(&body).contains(fragment),
                    "case {case:?}: the message must carry no part of the request and no \
                     resource class, found {fragment:?} in {:?}",
                    message(&body)
                );
            }
            assert_eq!(
                body["resourceId"].as_str(),
                Some(identifier.value.as_str()),
                "case {case:?}: the ARN belongs in resourceId, got {body}"
            );
            assert_eq!(
                body["resourceType"].as_str(),
                Some(identifier.resource_type),
                "case {case:?}: resourceType must be {:?}, got {body}",
                identifier.resource_type
            );
        }
    }
}

#[tokio::test]
async fn one_generic_sentence_serves_every_tagging_call_and_identifier() {
    // The defect this pins: a sentence chosen per resource class (or built from the
    // request) varies across these calls, where AWS's is one constant. So the sentence
    // is collected over every operation and every identifier and must come back as ONE
    // distinct value.
    let router = emulator();
    let mut sentences = BTreeSet::new();

    for tag_call in tag_calls() {
        for identifier in identifiers() {
            let case = format!("{} / {}", tag_call.operation, identifier.case);
            let (status, body) = call(
                &router,
                tag_call.method,
                &(tag_call.path)(&identifier.value),
                (tag_call.body)(),
            )
            .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status} with {body}"
            );
            sentences.insert(message(&body));
        }
    }

    assert_eq!(
        sentences,
        [TAGGED_RESOURCE_SENTENCE.to_string()].into_iter().collect(),
        "case the whole tagging surface: one fixed sentence must serve every call, got \
         {sentences:?}"
    );
}

#[tokio::test]
async fn an_operation_asked_for_one_named_class_keeps_its_own_sentence() {
    // The scope of the rule: the generic sentence belongs to the surface addressed by a
    // bare ARN. A reader asked for a resource of one named class was asked which class,
    // so it still answers that class's own sentence for the very same identifiers.
    let router = emulator();

    let readers = vec![
        TypedReader {
            operation: "GetCluster",
            path: |arn| format!("/cluster/{}", encode(arn)),
            sentence: CLUSTER_SENTENCE,
        },
        TypedReader {
            operation: "GetClusterSnapshot",
            path: |arn| format!("/cluster-snapshot/{}", encode(arn)),
            sentence: SNAPSHOT_SENTENCE,
        },
    ];

    for reader in readers {
        let expected = reader.sentence;
        for identifier in identifiers() {
            let case = format!("{} / {}", reader.operation, identifier.case);
            let (status, body) = call(&router, "GET", &(reader.path)(&identifier.value), None).await;
            assert_eq!(
                status, 404,
                "case {case:?}: an absent resource is HTTP 404, got {status} with {body}"
            );
            assert_eq!(
                message(&body),
                expected,
                "case {case:?}: expected the class sentence {expected:?}, got {:?}",
                message(&body)
            );
            assert_ne!(
                message(&body),
                TAGGED_RESOURCE_SENTENCE,
                "case {case:?}: a typed lookup must not take the generic sentence"
            );
        }
    }
}

#[tokio::test]
async fn every_failure_names_its_error_shape_in_the_qualified_form() {
    // The envelope rule, over everything that can answer a failure: the modelled refusal
    // each tag operation answers for an ARN it holds nothing under, a refusal the
    // framework raises before any handler runs, and a typed reader's own refusal. A
    // success names no error shape at all.
    let router = emulator();
    let unheld = format!(
        "arn:aws:docdb-elastic:{REGION}:{ACCOUNT}:{CLUSTER_SEGMENT}/00000000-0000-0000-0000-000000000000"
    );

    let (status, error_type, created) = answer(
        &router,
        "POST",
        "/cluster",
        Some(minimal_create("qualified-cluster")),
    )
    .await;
    assert_eq!(
        status, 200,
        "the cluster must be created, got {status} with {created}"
    );
    assert_eq!(
        error_type, None,
        "case \"a success\": a success names no error shape, got {error_type:?}"
    );

    for tag_call in tag_calls() {
        let case = format!("{} / an ARN no resource carries", tag_call.operation);
        let (status, error_type, body) = answer(
            &router,
            tag_call.method,
            &(tag_call.path)(&unheld),
            (tag_call.body)(),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} with {body}"
        );
        let expected = format!("ResourceNotFoundException{ERROR_TYPE_LOCATOR}");
        assert_eq!(
            error_type.as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected {expected:?} in {ERROR_TYPE_HEADER}, got {error_type:?}"
        );
        // The shape an SDK resolves is the text before the separator, and it is the shape
        // the body names in `__type`.
        let resolved = error_type
            .as_deref()
            .and_then(|value| value.split(':').next())
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            resolved,
            error_code(&body),
            "case {case:?}: the header and the body must name one shape, got {error_type:?} \
             and {body}"
        );
    }

    // A refusal the FRAMEWORK raises: `tags` is @required on TagResource
    // (`smithy_query(query="operation", operation="TagResource")`), so a body omitting it
    // is refused before the handler runs — and that refusal names its shape the same way.
    let (status, error_type, body) = answer(
        &router,
        "POST",
        &format!("/tags/{}", encode(&unheld)),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"an omitted required member\": expected HTTP 400, got {status} with {body}"
    );
    let named = error_type.unwrap_or_default();
    assert!(
        named.ends_with(ERROR_TYPE_LOCATOR),
        "case \"an omitted required member\": expected a shape qualified with \
         {ERROR_TYPE_LOCATOR:?}, got {named:?}"
    );

    // A typed reader's refusal: the envelope rule is the whole service's, not the tagging
    // surface's.
    let (status, error_type, body) = answer(
        &router,
        "GET",
        &format!("/cluster/{}", encode(&unheld)),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"GetCluster on an absent cluster\": expected HTTP 404, got {status} with {body}"
    );
    let expected = format!("ResourceNotFoundException{ERROR_TYPE_LOCATOR}");
    assert_eq!(
        error_type.as_deref(),
        Some(expected.as_str()),
        "case \"GetCluster on an absent cluster\": expected {expected:?}, got {error_type:?}"
    );
}

#[tokio::test]
async fn an_arn_this_service_does_hold_is_still_tagged_and_read_back() {
    // The positive control: the refusal above is about the ARN, not about the surface.
    // A resource that exists takes a tag write and reports it back through the reader.
    let router = emulator();
    let (status, created) = call(
        &router,
        "POST",
        "/cluster",
        Some(minimal_create("tagged-cluster")),
    )
    .await;
    assert_eq!(
        status, 200,
        "the cluster must be created, got {status} with {created}"
    );
    let arn = created["cluster"]["clusterArn"]
        .as_str()
        .expect("the created cluster carries its ARN")
        .to_string();

    let (status, body) = call(
        &router,
        "POST",
        &format!("/tags/{}", encode(&arn)),
        Some(json!({"tags": {"Project": "lionfish"}})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"tagging a held ARN\": expected HTTP 200, got {status} with {body}"
    );

    let (status, body) = call(&router, "GET", &format!("/tags/{}", encode(&arn)), None).await;
    assert_eq!(
        status, 200,
        "case \"reading a held ARN\": expected HTTP 200, got {status} with {body}"
    );
    assert_eq!(
        body["tags"]["Project"].as_str(),
        Some("lionfish"),
        "case \"reading a held ARN\": expected the written tag back, got {body}"
    );

    let (status, body) = call(
        &router,
        "DELETE",
        &format!("/tags/{}?tagKeys=Project", encode(&arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"untagging a held ARN\": expected HTTP 200, got {status} with {body}"
    );
    let (status, body) = call(&router, "GET", &format!("/tags/{}", encode(&arn)), None).await;
    assert_eq!(
        status, 200,
        "case \"reading after untag\": expected HTTP 200, got {status} with {body}"
    );
    assert!(
        body["tags"].get("Project").is_none(),
        "case \"reading after untag\": the removed key must be gone, got {body}"
    );
}
