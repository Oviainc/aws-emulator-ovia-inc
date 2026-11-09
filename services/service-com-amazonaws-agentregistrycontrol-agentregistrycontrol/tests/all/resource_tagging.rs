// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the tagging slice of `agent-registry-control` — `TagResource` and
//! `ListTagsForResource` — driven over HTTP through the real router so every assertion
//! goes through routing, deserialization, the handler and serialization.
//!
//! The rule under test: a tagging operation's `resourceArn` is read against the ARN shapes
//! this service's model declares (`RegistryArn`, `RegistryRecordArn`), and the three
//! answers follow from that one reading — an ARN naming no resource this service could hold
//! is a `ValidationException` that names the member and carries the modelled `reason`, a
//! well-formed ARN whose resource is absent is a `ResourceNotFoundException`, and one that
//! resolves is tagged, with the tags written being exactly the tags a later read reports.
//!
//! Every test asserts over a TABLE of inputs, so it holds for a class of requests rather
//! than for the one ARN a recording happened to carry.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region in the test's SigV4 credential scope; every minted ARN carries it.
const REGION: &str = "us-west-2";

/// A signed-looking request, so the router resolves this service's signing name
/// (`agent-registry`) and the handlers read the region from the credential scope.
/// The signature itself is never checked: the test router runs with authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTAGENTREGISTRY/20260101/us-west-2/agent-registry/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// Longer than the service's own asynchronous-transition span, so a transition the clock
/// owes (a registry becoming `READY`) has certainly come due by the next call.
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// The fixed text `TagResource` and `ListTagsForResource` refuse an unreadable ARN with;
/// the offending value follows it.
const UNRECOGNIZED: &str = "Unrecognized Agent Registry resource ARN: ";

/// `ValidationExceptionReason`'s modelled default, which a field-level refusal carries.
/// Source: the model's `ValidationExceptionReason` enum, whose generated catalogue is
/// `VALIDATION_EXCEPTION_REASON_VALUES`.
const FIELD_VALIDATION_FAILED: &str = "FieldValidationFailed";

/// How many tags one resource may hold: `TagsMap`'s own `@length` maximum.
const MAX_TAGS: usize = 50;

/// One HTTP answer, reduced to what these tests assert on.
struct Reply {
    status: u16,
    body: Value,
    /// The wire error code (`x-amzn-errortype`), present only on a failure.
    error_code: Option<String>,
}

impl Reply {
    fn str_field(&self, name: &str) -> &str {
        self.body[name].as_str().unwrap_or_default()
    }

    fn message(&self) -> String {
        self.body["message"]
            .as_str()
            .or_else(|| self.body["Message"].as_str())
            .unwrap_or_default()
            .to_string()
    }
}

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_agentregistrycontrol_agentregistrycontrol::EmulatedService::new(
            engine,
        ),
    );
    router
}

async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Option<Value>) -> Reply {
    let payload = match &body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serializable body")),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://agent-registry-control.{REGION}.amazonaws.com{path}"
        ))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(payload)
        .expect("a valid request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.rsplit('#').next().unwrap_or(value).to_string());
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply {
        status,
        body,
        error_code,
    }
}

/// A client token of `ClientToken`'s shape (`@length 33..256`), derived from `seed` so each
/// create in a test uses its own.
fn token(seed: &str) -> String {
    format!("{seed}{}", "0".repeat(33_usize.saturating_sub(seed.len())))
}

/// A registry that has reached `READY`, and its ARN.
async fn ready_registry(router: &MultiServiceRouter, name: &str) -> String {
    let created = call(
        router,
        "POST",
        "/registries",
        Some(json!({ "name": name, "clientToken": token(name) })),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case {name:?}: the registry must be created, got {} {:?}",
        created.status, created.body
    );
    let arn = created.str_field("registryArn").to_string();
    tokio::time::sleep(PAST_THE_TRANSITION).await;
    arn
}

/// A record of `registry_arn`, and its ARN.
async fn record(router: &MultiServiceRouter, registry_arn: &str, name: &str) -> String {
    let registry_id = registry_arn.rsplit('/').next().unwrap_or_default();
    let created = call(
        router,
        "POST",
        &format!("/registries/{registry_id}/records"),
        Some(json!({
            "name": name,
            "recordType": "CUSTOM",
            "descriptors": { "custom": { "data": "{\"anything\":true}" } },
            "clientToken": token(name)
        })),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case {name:?}: the record must be created, got {} {:?}",
        created.status, created.body
    );
    created.str_field("recordArn").to_string()
}

async fn tag(router: &MultiServiceRouter, resource_arn: &str, tags: Value) -> Reply {
    call(
        router,
        "POST",
        &format!("/tags/{resource_arn}"),
        Some(json!({ "tags": tags })),
    )
    .await
}

async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> Reply {
    call(router, "GET", &format!("/tags/{resource_arn}"), None).await
}

async fn untag(router: &MultiServiceRouter, resource_arn: &str, keys: &[&str]) -> Reply {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={key}"))
        .collect::<Vec<_>>()
        .join("&");
    call(
        router,
        "DELETE",
        &format!("/tags/{resource_arn}?{query}"),
        None,
    )
    .await
}

/// The tag keys a resource reports, in key order.
async fn tag_keys(router: &MultiServiceRouter, resource_arn: &str) -> Vec<String> {
    let listed = list_tags(router, resource_arn).await;
    let mut keys: Vec<String> = listed.body["tags"]
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|(key, _)| key)
        .collect();
    keys.sort();
    keys
}

#[tokio::test]
async fn untagging_removes_the_keys_the_request_names_and_leaves_the_rest() {
    let router = router();
    let registry_arn = ready_registry(&router, "untag-registry").await;
    let record_arn = record(&router, &registry_arn, "untag-record").await;

    // Rows differ along the resource and along WHICH keys the request names: one of
    // several, two at once, a key the resource does not hold (which removes nothing), and
    // every key it holds.
    for (case, resource_arn) in [
        ("registry", registry_arn.as_str()),
        ("record", record_arn.as_str()),
    ] {
        let tagged = tag(
            &router,
            resource_arn,
            json!({ "a": "1", "b": "2", "c": "3", "d": "4" }),
        )
        .await;
        assert_eq!(
            tagged.status, 204,
            "case {case:?}: the tags must be written first, got {} {:?}",
            tagged.status, tagged.body
        );

        let steps: [(&str, Vec<&str>, Vec<&str>); 4] = [
            ("one of several", vec!["a"], vec!["b", "c", "d"]),
            ("two at once", vec!["b", "c"], vec!["d"]),
            ("a key the resource does not hold", vec!["zz"], vec!["d"]),
            ("every key it holds", vec!["d"], vec![]),
        ];
        for (step, keys, expected) in steps {
            let untagged = untag(&router, resource_arn, &keys).await;
            assert_eq!(
                untagged.status, 204,
                "case {case:?}/{step:?}: UntagResource answers the modelled 204, got {} {:?}",
                untagged.status, untagged.body
            );
            assert_eq!(
                untagged.body,
                Value::Null,
                "case {case:?}/{step:?}: a 204 carries no body, got {:?}",
                untagged.body
            );
            let held = tag_keys(&router, resource_arn).await;
            assert_eq!(
                held, expected,
                "case {case:?}/{step:?}: only the named keys are removed, got {held:?}"
            );
        }
    }
}

/// One step of the tagging table: the case name, the tags the request carries, and the
/// whole set the resource must report afterwards, in key order.
type TaggingStep = (&'static str, Value, Vec<(&'static str, &'static str)>);

#[tokio::test]
async fn the_tags_written_to_a_resource_are_the_tags_a_later_read_reports() {
    let router = router();
    let registry_arn = ready_registry(&router, "tagged-registry").await;
    let record_arn = record(&router, &registry_arn, "tagged-record").await;

    // Rows differ along the resource the ARN names (a registry, and a record nested under
    // one) and along the tag map: one entry, several, a value replaced on a key the
    // resource already holds, a key added beside the ones it holds, and an empty value
    // (`TagValue`'s `@length` is 0..256, so the empty string is a value).
    for (case, resource_arn) in [
        ("registry", registry_arn.as_str()),
        ("record", record_arn.as_str()),
    ] {
        let steps: [TaggingStep; 5] = [
            ("one tag", json!({ "team": "platform" }), vec![("team", "platform")]),
            (
                "several tags",
                json!({ "env": "prod", "owner": "agents" }),
                vec![("env", "prod"), ("owner", "agents"), ("team", "platform")],
            ),
            (
                "a value replaced",
                json!({ "team": "runtime" }),
                vec![("env", "prod"), ("owner", "agents"), ("team", "runtime")],
            ),
            (
                "an empty value",
                json!({ "retired": "" }),
                vec![
                    ("env", "prod"),
                    ("owner", "agents"),
                    ("retired", ""),
                    ("team", "runtime"),
                ],
            ),
            (
                "a key carrying the punctuation TagKey admits",
                json!({ "cost:center/01": "a._b" }),
                vec![
                    ("cost:center/01", "a._b"),
                    ("env", "prod"),
                    ("owner", "agents"),
                    ("retired", ""),
                    ("team", "runtime"),
                ],
            ),
        ];

        for (step, tags, expected) in steps {
            let tagged = tag(&router, resource_arn, tags.clone()).await;
            assert_eq!(
                tagged.status, 204,
                "case {case:?}/{step:?}: TagResource answers the modelled 204, got {} {:?}",
                tagged.status, tagged.body
            );
            assert_eq!(
                tagged.body,
                Value::Null,
                "case {case:?}/{step:?}: a 204 carries no body, got {:?}",
                tagged.body
            );

            // The round trip: the resource reports exactly the merged set, and nothing of
            // the request is lost or invented.
            let listed = list_tags(&router, resource_arn).await;
            assert_eq!(
                listed.status, 200,
                "case {case:?}/{step:?}: ListTagsForResource answers 200, got {} {:?}",
                listed.status, listed.body
            );
            let mut reported: Vec<(String, String)> = listed.body["tags"]
                .as_object()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|(key, value)| (key, value.as_str().unwrap_or_default().to_string()))
                .collect();
            reported.sort();
            let expected: Vec<(String, String)> = expected
                .into_iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect();
            assert_eq!(
                reported, expected,
                "case {case:?}/{step:?}: the resource reports the merged tag set, got \
                 {reported:?}"
            );
        }
    }

    // The two resources are tagged independently: a record's tags are not its registry's.
    let only_the_record = tag(&router, &record_arn, json!({ "nested": "yes" })).await;
    assert_eq!(
        only_the_record.status, 204,
        "case \"independent resources\": got {} {:?}",
        only_the_record.status, only_the_record.body
    );
    let registry_tags = list_tags(&router, &registry_arn).await;
    assert!(
        registry_tags.body["tags"].get("nested").is_none(),
        "case \"independent resources\": the registry must not carry the record's tag, got {:?}",
        registry_tags.body
    );
}

#[tokio::test]
async fn a_resource_with_no_tags_reports_an_empty_set() {
    let router = router();
    let registry_arn = ready_registry(&router, "untagged-registry").await;

    // `ResourceTagsMap`'s `@length` is 0..50, so "no tags" is an empty map and not an
    // absent member: a caller reading a resource it has not tagged gets a map it can
    // iterate.
    let listed = list_tags(&router, &registry_arn).await;
    assert_eq!(
        listed.status, 200,
        "case \"untagged registry\": got {} {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.body["tags"],
        json!({}),
        "case \"untagged registry\": the tag set is empty, not absent, got {:?}",
        listed.body
    );
}

#[tokio::test]
async fn an_arn_naming_no_agent_registry_resource_is_refused_with_the_member_and_the_reason() {
    let router = router();
    let registry_arn = ready_registry(&router, "refusal-registry").await;
    let registry_id = registry_arn
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();

    // Rows span the ways a value can fail to be the ARN of a resource this service holds.
    // The classes come from the model's two ARN shapes: `RegistryArn`
    // (`^arn:aws(-[^:]+)?:agent-registry:[a-z0-9-]+:[0-9]{12}:registry/[a-zA-Z0-9]{12,16}$`)
    // and `RegistryRecordArn` (that, continued with `/record/[a-zA-Z0-9]{12}`).
    let rows: Vec<(&str, String)> = vec![
        ("not an ARN at all", format!("registry/{registry_id}")),
        ("a bare registry id", registry_id.clone()),
        (
            "another vendor's ARN",
            format!("arn:aws:s3:::registry/{registry_id}"),
        ),
        (
            "a resource type this service does not hold",
            "arn:aws:agent-registry:us-west-2:000000000000:bogus/ZZZZ".to_string(),
        ),
        (
            "a registry id outside RegistryId's class",
            "arn:aws:agent-registry:us-west-2:000000000000:registry/short".to_string(),
        ),
        (
            "a record id outside RegistryRecordId's class",
            format!("arn:aws:agent-registry:us-west-2:000000000000:registry/{registry_id}/record/tooshort"),
        ),
        (
            "an account that is not twelve digits",
            format!("arn:aws:agent-registry:us-west-2:12345:registry/{registry_id}"),
        ),
        (
            "no region at all",
            format!("arn:aws:agent-registry::000000000000:registry/{registry_id}"),
        ),
        (
            "a segment past the record",
            format!(
                "arn:aws:agent-registry:us-west-2:000000000000:registry/{registry_id}/record/bbbbbbbbbbbb/version/1"
            ),
        ),
    ];

    for (case, resource_arn) in rows {
        for (operation, reply) in [
            ("TagResource", tag(&router, &resource_arn, json!({ "k": "v" })).await),
            ("ListTagsForResource", list_tags(&router, &resource_arn).await),
            ("UntagResource", untag(&router, &resource_arn, &["k"]).await),
        ] {
            assert_eq!(
                reply.status, 400,
                "case {case:?}/{operation}: an unreadable resourceArn is the modelled 400, \
                 got {} {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code.as_deref(),
                Some("ValidationException"),
                "case {case:?}/{operation}: the modelled error code is ValidationException, \
                 got {:?}",
                reply.error_code
            );
            assert_eq!(
                reply.message(),
                format!("{UNRECOGNIZED}{resource_arn}"),
                "case {case:?}/{operation}: the message names the offending ARN, got {:?}",
                reply.message()
            );
            assert_eq!(
                reply.str_field("reason"),
                FIELD_VALIDATION_FAILED,
                "case {case:?}/{operation}: the required reason member carries its modelled \
                 default, got {:?}",
                reply.body
            );
        }
    }
}

#[tokio::test]
async fn a_well_formed_arn_whose_resource_is_absent_is_a_not_found() {
    let router = router();
    let registry_arn = ready_registry(&router, "present-registry").await;

    // Rows: a registry no account holds, the same in another partition (`RegistryArn`
    // admits `aws(-[^:]+)?`, so this is READ and then not found, never refused as
    // unreadable), a longer-but-legal registry id (`RegistryId` is 12..16), and a record
    // the present registry does not hold.
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a registry no account holds",
            "arn:aws:agent-registry:us-west-2:000000000000:registry/zzzzzzzzzzzz".to_string(),
            "Registry with ARN arn:aws:agent-registry:us-west-2:000000000000:registry/zzzzzzzzzzzz not found.".to_string(),
        ),
        (
            "another partition",
            "arn:aws-cn:agent-registry:cn-north-1:000000000000:registry/zzzzzzzzzzzz".to_string(),
            "Registry with ARN arn:aws-cn:agent-registry:cn-north-1:000000000000:registry/zzzzzzzzzzzz not found.".to_string(),
        ),
        (
            "a sixteen-character registry id",
            "arn:aws:agent-registry:us-west-2:000000000000:registry/zzzzzzzzzzzzzzzz".to_string(),
            "Registry with ARN arn:aws:agent-registry:us-west-2:000000000000:registry/zzzzzzzzzzzzzzzz not found.".to_string(),
        ),
        (
            "a record the registry does not hold",
            format!("{registry_arn}/record/cccccccccccc"),
            "Registry record with ID cccccccccccc not found.".to_string(),
        ),
        (
            "a record of a registry that does not exist",
            format!(
                "arn:aws:agent-registry:us-west-2:000000000000:registry/{}/record/cccccccccccc",
                "zzzzzzzzzzzz"
            ),
            "Registry record with ID cccccccccccc not found.".to_string(),
        ),
    ];

    for (case, resource_arn, expected) in rows {
        for (operation, reply) in [
            ("TagResource", tag(&router, &resource_arn, json!({ "k": "v" })).await),
            ("ListTagsForResource", list_tags(&router, &resource_arn).await),
            ("UntagResource", untag(&router, &resource_arn, &["k"]).await),
        ] {
            assert_eq!(
                reply.status, 404,
                "case {case:?}/{operation}: an absent resource is the modelled 404, got {} {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error_code.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case:?}/{operation}: the modelled error code is \
                 ResourceNotFoundException, got {:?}",
                reply.error_code
            );
            assert_eq!(
                reply.message(),
                expected,
                "case {case:?}/{operation}: the message states which resource was not there, \
                 got {:?}",
                reply.message()
            );
        }
    }

    // The registry that IS there is unaffected by every refusal above.
    let present = list_tags(&router, &registry_arn).await;
    assert_eq!(
        present.status, 200,
        "case \"the present registry\": it must still be readable, got {:?}",
        present.body
    );
}

#[tokio::test]
async fn a_resource_holds_at_most_the_models_maximum_number_of_tags() {
    let router = router();
    let registry_arn = ready_registry(&router, "quota-registry").await;

    // `TagsMap`'s `@length` maximum is 50, so the bound is tested at it and one past it:
    // two requests that together take the resource exactly to 50 are accepted, the one
    // that would make 51 is refused, and replacing a key at the cap is still accepted
    // because it adds nothing.
    let first: Value = (0..30)
        .map(|index| (format!("key-{index:02}"), Value::from("v")))
        .collect::<serde_json::Map<String, Value>>()
        .into();
    let second: Value = (30..MAX_TAGS)
        .map(|index| (format!("key-{index:02}"), Value::from("v")))
        .collect::<serde_json::Map<String, Value>>()
        .into();

    for (case, tags) in [("first 30", first), ("up to the maximum", second)] {
        let reply = tag(&router, &registry_arn, tags).await;
        assert_eq!(
            reply.status, 204,
            "case {case:?}: a request inside the maximum is accepted, got {} {:?}",
            reply.status, reply.body
        );
    }
    let held = list_tags(&router, &registry_arn).await.body["tags"]
        .as_object()
        .map_or(0, serde_json::Map::len);
    assert_eq!(
        held, MAX_TAGS,
        "case \"up to the maximum\": the resource holds the maximum, got {held}"
    );

    let replaced = tag(&router, &registry_arn, json!({ "key-00": "w" })).await;
    assert_eq!(
        replaced.status, 204,
        "case \"replacing a key at the maximum\": it adds nothing, got {} {:?}",
        replaced.status, replaced.body
    );

    let past = tag(&router, &registry_arn, json!({ "key-50": "v" })).await;
    assert_eq!(
        past.status, 402,
        "case \"one past the maximum\": the modelled ServiceQuotaExceededException is a 402, \
         got {} {:?}",
        past.status, past.body
    );
    assert_eq!(
        past.error_code.as_deref(),
        Some("ServiceQuotaExceededException"),
        "case \"one past the maximum\": the modelled error code, got {:?}",
        past.error_code
    );
    let still_held = list_tags(&router, &registry_arn).await.body["tags"]
        .as_object()
        .map_or(0, serde_json::Map::len);
    assert_eq!(
        still_held, MAX_TAGS,
        "case \"one past the maximum\": the refused request wrote nothing, got {still_held}"
    );
}
